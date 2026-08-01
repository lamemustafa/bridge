use bridge_tally_protocol::outstandings::{AlterIdRange, CompleteSegment};
use std::num::NonZeroU64;
use std::time::Duration;

const TARGET_RESPONSE_BYTES: usize = 28 * 1024 * 1024;
const TARGET_READ_MILLIS: u128 = 15_000;
// Guide §2.5a: stop before another segment when comparable requests show a
// material rising trend near the immutable 20-second transport deadline.
const TREND_SAMPLE_COUNT: usize = 3;
const COMPARABLE_PERCENT: usize = 25;
const TREND_HISTORY_LIMIT: usize = 32;
pub(crate) const MAX_SEGMENT_PAIRS_PER_SCAN: u64 = 128;
#[cfg(feature = "live-calibration-harness")]
const BILLWISE_LAB_EXIT_WIDTH: u64 = 252;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SegmentPerformance {
    alter_id_width: u64,
    rows: usize,
    encoded_bytes: usize,
    max_read_millis: u128,
}

/// Evidence that an initial AlterID span has been calibrated on an ordered,
/// bill-bearing corpus. No production constructor exists until that evidence
/// has been collected and owner-reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CalibratedSegmentPolicy {
    initial_width: NonZeroU64,
}

impl CalibratedSegmentPolicy {
    #[cfg(test)]
    pub(crate) fn for_test(initial_width: u64) -> Self {
        Self {
            initial_width: NonZeroU64::new(initial_width).expect("test segment width is non-zero"),
        }
    }

    #[cfg(feature = "live-calibration-harness")]
    pub(crate) fn for_billwise_lab_exit_check() -> Self {
        Self {
            initial_width: NonZeroU64::new(BILLWISE_LAB_EXIT_WIDTH)
                .expect("owner-approved exit width is non-zero"),
        }
    }

    pub(crate) fn initial_width(self) -> u64 {
        self.initial_width.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SegmentPlan {
    pub(crate) date_partitions: usize,
    pub(crate) alter_id_high_water: u64,
    pub(crate) initial_width: u64,
    pub(crate) planned_segment_pairs: u64,
}

impl SegmentPlan {
    pub(crate) fn new(
        date_partitions: usize,
        alter_id_high_water: u64,
        policy: CalibratedSegmentPolicy,
    ) -> Result<Self, bridge_tally_protocol::outstandings::OutstandingsError> {
        let initial_width = policy.initial_width();
        let ranges_per_partition = alter_id_high_water / initial_width
            + u64::from(!alter_id_high_water.is_multiple_of(initial_width));
        let date_partition_count = u64::try_from(date_partitions).map_err(|_| {
            bridge_tally_protocol::outstandings::OutstandingsError::ArithmeticOverflow
        })?;
        let planned_segment_pairs = date_partition_count
            .checked_mul(ranges_per_partition)
            .ok_or(bridge_tally_protocol::outstandings::OutstandingsError::ArithmeticOverflow)?;
        Ok(Self {
            date_partitions,
            alter_id_high_water,
            initial_width,
            planned_segment_pairs,
        })
    }

    pub(crate) fn is_admitted(self) -> bool {
        self.planned_segment_pairs <= MAX_SEGMENT_PAIRS_PER_SCAN
    }

    pub(crate) fn admitted_budget(self) -> Option<SegmentPairBudget> {
        self.is_admitted().then_some(SegmentPairBudget { spent: 0 })
    }
}

#[derive(Debug)]
pub(crate) struct SegmentPairBudget {
    spent: u64,
}

impl SegmentPairBudget {
    pub(crate) fn admit_next(&mut self) -> bool {
        let Some(next) = self.spent.checked_add(1) else {
            return false;
        };
        if next > MAX_SEGMENT_PAIRS_PER_SCAN {
            return false;
        }
        self.spent = next;
        true
    }
}

#[derive(Debug)]
pub(crate) struct SegmentTrendGuard {
    observations: Vec<SegmentPerformance>,
    next_width: u64,
}

impl SegmentTrendGuard {
    pub(crate) fn new(policy: CalibratedSegmentPolicy) -> Self {
        Self {
            observations: Vec::new(),
            next_width: policy.initial_width.get(),
        }
    }
    pub(crate) fn next_range(
        &self,
        exclusive_start: u64,
        high_water: u64,
    ) -> Result<Option<AlterIdRange>, bridge_tally_protocol::outstandings::OutstandingsError> {
        if exclusive_start >= high_water {
            return Ok(None);
        }
        let inclusive_end = exclusive_start
            .saturating_add(self.next_width)
            .min(high_water);
        AlterIdRange::new(exclusive_start, inclusive_end).map(Some)
    }

    pub(crate) fn observe_complete_segment(
        &mut self,
        segment: &CompleteSegment,
        max_read_elapsed: Duration,
    ) -> bool {
        let current = SegmentPerformance {
            alter_id_width: segment.alter_id_range().width(),
            rows: segment.vouchers().len(),
            encoded_bytes: segment.encoded_bytes(),
            max_read_millis: max_read_elapsed.as_millis(),
        };
        let should_stop = self.observe_performance(current);
        self.shrink_after_comparable_observations(current);
        should_stop
    }

    fn shrink_after_comparable_observations(&mut self, current: SegmentPerformance) {
        let comparable = self
            .observations
            .iter()
            .rev()
            .filter(|previous| segments_are_comparable(previous, &current))
            .take(TREND_SAMPLE_COUNT)
            .copied()
            .collect::<Vec<_>>();
        if comparable.len() < TREND_SAMPLE_COUNT {
            return;
        }
        let observed_width = comparable
            .iter()
            .map(|sample| sample.alter_id_width)
            .min()
            .unwrap_or(current.alter_id_width);
        let worst_millis = comparable
            .iter()
            .map(|sample| sample.max_read_millis)
            .max()
            .unwrap_or(0);
        let worst_bytes = comparable
            .iter()
            .map(|sample| sample.encoded_bytes)
            .max()
            .unwrap_or(0);
        let time_bound = if worst_millis == 0 {
            observed_width
        } else {
            u64::try_from(
                u128::from(observed_width)
                    .saturating_mul(TARGET_READ_MILLIS)
                    .checked_div(worst_millis)
                    .unwrap_or(1),
            )
            .unwrap_or(u64::MAX)
            .max(1)
        };
        let byte_bound = if worst_bytes == 0 {
            observed_width
        } else {
            observed_width
                .saturating_mul(u64::try_from(TARGET_RESPONSE_BYTES).unwrap_or(u64::MAX))
                .checked_div(u64::try_from(worst_bytes).unwrap_or(u64::MAX))
                .unwrap_or(1)
                .max(1)
        };
        self.next_width = self
            .next_width
            .min(observed_width)
            .min(time_bound)
            .min(byte_bound)
            .max(1);
    }

    fn observe_performance(&mut self, current: SegmentPerformance) -> bool {
        let mut comparable = self
            .observations
            .iter()
            .rev()
            .filter(|previous| segments_are_comparable(previous, &current))
            .take(TREND_SAMPLE_COUNT - 1)
            .copied()
            .collect::<Vec<_>>();
        comparable.reverse();
        comparable.push(current);
        self.observations.push(current);
        if self.observations.len() > TREND_HISTORY_LIMIT {
            self.observations.remove(0);
        }

        if comparable.len() < TREND_SAMPLE_COUNT {
            return false;
        }
        let elapsed = comparable
            .iter()
            .map(|item| item.max_read_millis)
            .collect::<Vec<_>>();
        elapsed.windows(2).all(|pair| pair[0] < pair[1])
            && elapsed[elapsed.len() - 1] >= TARGET_READ_MILLIS
            && elapsed[elapsed.len() - 1].saturating_sub(elapsed[0]) >= 1_000
    }
}

fn segments_are_comparable(left: &SegmentPerformance, right: &SegmentPerformance) -> bool {
    within_percent_u64(
        left.alter_id_width,
        right.alter_id_width,
        COMPARABLE_PERCENT,
    ) && within_percent(left.rows, right.rows, COMPARABLE_PERCENT)
        && within_percent(left.encoded_bytes, right.encoded_bytes, COMPARABLE_PERCENT)
}

fn within_percent_u64(left: u64, right: u64, percent: usize) -> bool {
    let maximum = left.max(right).max(1);
    left.abs_diff(right).saturating_mul(100)
        <= maximum.saturating_mul(u64::try_from(percent).unwrap_or(u64::MAX))
}

fn within_percent(left: usize, right: usize, percent: usize) -> bool {
    let maximum = left.max(right).max(1);
    left.abs_diff(right).saturating_mul(100) <= maximum.saturating_mul(percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrated_ranges_are_contiguous_without_overlap() {
        let guard = SegmentTrendGuard::new(CalibratedSegmentPolicy::for_test(64));
        assert_eq!(
            guard.next_range(0, 150).unwrap().unwrap(),
            AlterIdRange::new(0, 64).unwrap()
        );
        assert_eq!(
            guard.next_range(64, 150).unwrap().unwrap(),
            AlterIdRange::new(64, 128).unwrap()
        );
        assert_eq!(
            guard.next_range(128, 150).unwrap().unwrap(),
            AlterIdRange::new(128, 150).unwrap()
        );
        assert!(guard.next_range(150, 150).unwrap().is_none());
    }

    #[test]
    fn a_full_high_water_width_tiles_the_budget_axis_once() {
        let guard = SegmentTrendGuard::new(CalibratedSegmentPolicy::for_test(252));
        let only = guard.next_range(0, 252).unwrap().unwrap();
        assert_eq!(only, AlterIdRange::new(0, 252).unwrap());
        assert!(guard
            .next_range(only.inclusive_end(), 252)
            .unwrap()
            .is_none());
    }

    #[cfg(feature = "live-calibration-harness")]
    #[test]
    fn live_exit_width_is_fixed_to_the_billwise_lab_high_water() {
        assert_eq!(
            CalibratedSegmentPolicy::for_billwise_lab_exit_check().initial_width(),
            252
        );
    }

    #[test]
    fn segment_plan_admits_128_pairs_and_rejects_larger_scans() {
        let accepted = SegmentPlan::new(128, 252, CalibratedSegmentPolicy::for_test(252)).unwrap();
        assert_eq!(accepted.planned_segment_pairs, 128);
        assert!(accepted.is_admitted());

        let rejected = SegmentPlan::new(24, 101_601, CalibratedSegmentPolicy::for_test(252))
            .expect("large plan arithmetic remains representable");
        assert_eq!(rejected.planned_segment_pairs, 9_696);
        assert!(!rejected.is_admitted());
    }

    #[test]
    fn actual_pair_budget_stays_bounded_if_runtime_shrinks_the_width() {
        let plan = SegmentPlan::new(1, 1, CalibratedSegmentPolicy::for_test(1)).unwrap();
        let mut budget = plan.admitted_budget().expect("one pair is admitted");
        for _ in 0..MAX_SEGMENT_PAIRS_PER_SCAN {
            assert!(budget.admit_next());
        }
        assert!(!budget.admit_next());
        assert!(!budget.admit_next());
    }

    #[test]
    fn one_sample_cannot_tune_and_three_comparable_samples_can_only_shrink() {
        let mut guard = SegmentTrendGuard::new(CalibratedSegmentPolicy::for_test(100));
        guard.observe_performance(performance(100, 100, 20 * 1024 * 1024, 18_000));
        guard.shrink_after_comparable_observations(performance(100, 100, 20 * 1024 * 1024, 18_000));
        assert_eq!(guard.next_range(100, 1_000).unwrap().unwrap().width(), 100);

        for sample in [
            performance(100, 100, 20 * 1024 * 1024, 17_000),
            performance(100, 100, 20 * 1024 * 1024, 18_000),
        ] {
            guard.observe_performance(sample);
        }
        guard.shrink_after_comparable_observations(performance(100, 100, 20 * 1024 * 1024, 18_000));
        let shrunk = guard.next_range(200, 1_000).unwrap().unwrap().width();
        assert!(shrunk < 100);

        for _ in 0..3 {
            guard.observe_performance(performance(shrunk, 100, 1, 1));
        }
        guard.shrink_after_comparable_observations(performance(shrunk, 100, 1, 1));
        assert_eq!(
            guard.next_range(300, 1_000).unwrap().unwrap().width(),
            shrunk
        );
    }

    #[test]
    fn trend_logic_uses_comparable_shapes_and_ignores_small_jitter() {
        let mut guard = SegmentTrendGuard::new(CalibratedSegmentPolicy::for_test(100));
        for (index, current) in [
            performance(100, 100, 8 * 1024 * 1024, 12_000),
            performance(100, 100, 8 * 1024 * 1024, 14_000),
            performance(100, 100, 8 * 1024 * 1024, 15_100),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(guard.observe_performance(current), index == 2);
        }

        let mut jitter = SegmentTrendGuard::new(CalibratedSegmentPolicy::for_test(100));
        for current in [
            performance(100, 100, 8 * 1024 * 1024, 14_900),
            performance(100, 100, 8 * 1024 * 1024, 15_000),
            performance(100, 100, 8 * 1024 * 1024, 15_500),
        ] {
            assert!(!jitter.observe_performance(current));
        }
    }

    fn performance(
        alter_id_width: u64,
        rows: usize,
        encoded_bytes: usize,
        max_read_millis: u128,
    ) -> SegmentPerformance {
        SegmentPerformance {
            alter_id_width,
            rows,
            encoded_bytes,
            max_read_millis,
        }
    }
}
