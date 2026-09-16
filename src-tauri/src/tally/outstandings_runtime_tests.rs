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
fn segment_plan_reserves_empty_partition_witnesses_inside_128_pairs() {
    let accepted = SegmentPlan::new(32, 252, CalibratedSegmentPolicy::for_test(252)).unwrap();
    assert_eq!(accepted.planned_primary_segment_pairs, 32);
    assert_eq!(accepted.reserved_empty_partition_witness_pairs, 96);
    assert_eq!(accepted.planned_segment_pairs, 128);
    assert!(accepted.is_admitted());

    let rejected = SegmentPlan::new(33, 252, CalibratedSegmentPolicy::for_test(252))
        .expect("large plan arithmetic remains representable");
    assert_eq!(rejected.planned_segment_pairs, 132);
    assert!(!rejected.is_admitted());
}

#[test]
fn zero_high_water_does_not_reserve_witness_pairs() {
    let empty = SegmentPlan::new(43, 0, CalibratedSegmentPolicy::for_test(252)).unwrap();
    assert_eq!(empty.planned_primary_segment_pairs, 0);
    assert_eq!(empty.reserved_empty_partition_witness_pairs, 0);
    assert_eq!(empty.planned_segment_pairs, 0);
    assert!(empty.is_admitted());
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
