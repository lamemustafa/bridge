//! A pre-flight volume bound for windowed voucher reads.
//!
//! The rule is protocol reference §11c. In short: Tally builds a whole response
//! before Bridge can refuse it, and a client that gives up does not stop Tally
//! (§11b.2). Bridge's response cap and per-leg deadline are enforced on the
//! response, client-side, so they protect Bridge and never the gateway. A bound
//! that protects Tally has to be applied *before* a request is sent.
//!
//! So a windowed voucher read first establishes how many vouchers the window
//! holds, multiplies that by what one voucher costs in the request's shape, and
//! divides the window by date so that no single request is predicted to exceed
//! [`WINDOW_READ_BUDGET_BYTES`]. A day that alone exceeds the budget is refused
//! by name rather than sent, because a date window cannot divide a day.
//!
//! Every estimate here is a planning figure, not a bound (§12a.8). The transport
//! cap stays the final safeguard; this only stops Bridge from *asking* for a
//! response it already expects to be too large.
use super::*;
use chrono::NaiveDate;

/// Predicted encoded bytes one windowed read may carry.
///
/// Half the transport response cap. The prediction is an estimate, and the
/// half left over is what absorbs the error in it before the cap is reached.
/// Encoded means on the wire: Bridge reads in UTF-16LE (§1.2), which costs
/// exactly twice a UTF-8 measurement of the same response.
pub(super) const WINDOW_READ_BUDGET_BYTES: u64 =
    bridge_tally_transport::XML_RESPONSE_MAX_BYTES as u64 / 2;

/// Encoded bytes per voucher of the census row (`GUID,ALTERID,DATE`).
///
/// §11a, §11b and §12a.8 each measured a minimal voucher `FETCH` at about
/// 1.2 KB per row in UTF-8 — Tally emits a fixed envelope whatever narrow fields
/// are named — which is about 2.4 KB on Bridge's UTF-16 wire. This leaves room
/// above that rather than assuming it.
const CENSUS_WIRE_BYTES_PER_VOUCHER: u64 = 4 * 1024;

/// The most census requests one window read may spend. Each covers a fixed
/// AlterID span, so this caps the book size a census can describe; above it the
/// volume is unestimated and the read is refused.
const MAX_CENSUS_READS: u64 = 32;

/// The most planned data reads one window read may spend before it is refused
/// as needing a narrower window. The same ceiling the outstandings scanner uses
/// for its own segment pairs.
pub(super) const MAX_PLANNED_READS: usize = 128;

/// How many places across the window the per-voucher cost is sampled from.
///
/// One place is not enough: on the heaviest book measured, a one-day probe gave
/// 28 KB per voucher while the whole year averaged 35 KB (§11c). Sampling the
/// low, middle and high thirds of the window's AlterIDs and planning at the
/// heaviest of them is still an estimate, but not one taken from a single spot.
const CALIBRATION_SPANS: usize = 3;

/// A measured bytes-per-voucher figure is a mean over the vouchers sampled, and
/// the vouchers not sampled can be heavier, so a measured figure is planned at
/// one and a half times itself. This margin is deliberate and is the only thing
/// standing between a sample and a book heavier than it.
const MEASURED_HEADROOM_NUMERATOR: u64 = 3;
const MEASURED_HEADROOM_DENOMINATOR: u64 = 2;

/// The request shapes this bound plans for, each with the per-voucher cost to
/// assume when nothing about the book has been measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VoucherReadShape {
    /// `render_import_verification_read`: named ledger-entry fields.
    ImportVerification,
    /// `render_agent_movement_vouchers`: named ledger-entry fields.
    Movement,
    /// `render_agent_vouchers`: the `ALLLEDGERENTRIES.*` entry wildcard.
    EntryWildcard,
}

impl VoucherReadShape {
    /// Encoded bytes per voucher to assume before anything has been measured.
    ///
    /// Set above the heaviest figure measured for each shape (§11c): named entry
    /// fields averaged 35 KB per voucher in UTF-8 over a whole year of an
    /// inventory-heavy trading book, and the entry wildcard ran about 4.6 times
    /// the named fields on the same book. Doubled for the UTF-16 wire, then
    /// given room: 96 KiB is 48 KiB of UTF-8, and 384 KiB is 192 KiB. An
    /// accounting-only book costs a fraction of this, which is why a measured
    /// figure replaces it when one is available.
    pub(super) const fn default_wire_bytes_per_voucher(self) -> u64 {
        match self {
            Self::ImportVerification | Self::Movement => 96 * 1024,
            Self::EntryWildcard => 384 * 1024,
        }
    }

    /// Whether a sub-window Tally cannot serve is divided and retried (#485),
    /// rather than failing the read. Only the verification read did this before
    /// the pre-flight bound existed, and it still alone does: retrying a read
    /// that timed out queues more work behind a gateway still building the
    /// abandoned response, so it is not a behaviour to extend by default.
    const fn splits_on_oversize(self) -> bool {
        matches!(self, Self::ImportVerification)
    }

    /// The refusal for a single day that still cannot be served after splitting.
    const fn day_not_readable_code(self) -> &'static str {
        match self {
            Self::ImportVerification => "verification_window_day_not_readable",
            Self::Movement | Self::EntryWildcard => "voucher_window_day_not_readable",
        }
    }

    pub(super) fn render(
        self,
        company: &str,
        from: &str,
        to: &str,
        sample: Option<AlterIdSpan>,
    ) -> Result<String, String> {
        // An unsampled read goes through the shape's own renderer, so a window
        // the bound leaves whole is sent exactly as it was before the bound.
        match (self, sample) {
            (Self::ImportVerification, None) => Ok(
                super::agent_import::render_import_verification_read(company, from, to),
            ),
            (Self::ImportVerification, Some(_)) => Ok(
                super::agent_import::render_import_verification_sample(company, from, to, sample),
            ),
            (Self::Movement, None) => render_agent_movement_vouchers(company, from, to),
            (Self::Movement, Some(_)) => {
                render_agent_movement_vouchers_sample(company, from, to, sample)
            }
            (Self::EntryWildcard, None) => render_agent_vouchers(company, from, to, None),
            (Self::EntryWildcard, Some(_)) => {
                render_agent_vouchers_sample(company, from, to, sample)
            }
        }
    }
}

/// An AlterID range `(after, through]`, used to bound a request by construction:
/// with distinct AlterIDs it cannot return more than `through - after` rows,
/// however the vouchers are spread over dates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AlterIdSpan {
    pub(super) after: u64,
    pub(super) through: u64,
}

impl AlterIdSpan {
    /// The clause appended to a `$Date` window formula.
    pub(super) fn filter(self) -> String {
        format!(
            " AND $AlterID &gt; {} AND $AlterID &lt;= {}",
            self.after, self.through
        )
    }
}

/// One contiguous date range of a planned window read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlannedRead {
    pub(super) from: NaiveDate,
    pub(super) to: NaiveDate,
    /// Vouchers the census counted in this range.
    pub(super) vouchers: u64,
}

/// Why a window cannot be planned under the budget. Each is refused by name,
/// before any data read is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlanRefusal {
    /// One day alone is predicted over the budget. A date window cannot divide
    /// a day, so no plan exists.
    DayOverBudget { day: NaiveDate, vouchers: u64 },
    /// The window divides, but into more reads than one call may spend.
    TooManyReads { reads: usize },
}

impl PlanRefusal {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::DayOverBudget { .. } => "voucher_window_day_over_budget",
            Self::TooManyReads { .. } => "voucher_window_too_many_reads",
        }
    }
}

/// Neither the high-water mark nor a census could describe the window, so no
/// prediction exists. Refused rather than sent: an unestimated read is exactly
/// the unbounded request this module exists to stop.
pub(super) const VOLUME_UNESTIMATED: &str = "voucher_window_volume_unestimated";

/// Vouchers one read may carry at `bytes_per_voucher` under `budget`.
fn vouchers_per_read(budget: u64, bytes_per_voucher: u64) -> u64 {
    budget / bytes_per_voucher.max(1)
}

/// A measured bytes-per-voucher figure with its planning headroom applied.
pub(super) fn with_headroom(measured: u64) -> u64 {
    measured
        .saturating_mul(MEASURED_HEADROOM_NUMERATOR)
        .div_ceil(MEASURED_HEADROOM_DENOMINATOR)
}

/// Divide `[from, to]` into contiguous date ranges, each predicted within
/// `budget` at `bytes_per_voucher`.
///
/// The ranges tile the window exactly — no gap, no overlap — so reading every
/// one observes the same vouchers one undivided read would have. A day without
/// vouchers joins whichever range it falls in; it costs nothing to read.
///
/// Greedy in date order: each range extends until the next day would push it
/// over the budget. A window predicted within budget as a whole is returned as
/// a single range equal to the window, which is how a small window keeps its
/// original, undivided request.
///
/// `days` must hold only days inside the window; the census guarantees that.
pub(super) fn plan_window_reads(
    from: NaiveDate,
    to: NaiveDate,
    days: &BTreeMap<NaiveDate, u64>,
    bytes_per_voucher: u64,
    budget: u64,
    max_reads: usize,
) -> Result<Vec<PlannedRead>, PlanRefusal> {
    let capacity = vouchers_per_read(budget, bytes_per_voucher);
    let mut reads = Vec::new();
    let mut start = from;
    let mut carried = 0_u64;
    for (&day, &vouchers) in days.range(from..=to) {
        if vouchers == 0 {
            continue;
        }
        if vouchers > capacity {
            return Err(PlanRefusal::DayOverBudget { day, vouchers });
        }
        if carried.saturating_add(vouchers) > capacity {
            // `carried > 0` here, because `vouchers <= capacity`, so an earlier
            // day in `[start, day)` holds vouchers and `day > start`.
            let end = day.pred_opt().expect("a later day has a predecessor");
            reads.push(PlannedRead {
                from: start,
                to: end,
                vouchers: carried,
            });
            start = day;
            carried = 0;
        }
        carried = carried.saturating_add(vouchers);
    }
    reads.push(PlannedRead {
        from: start,
        to,
        vouchers: carried,
    });
    if reads.len() > max_reads {
        return Err(PlanRefusal::TooManyReads { reads: reads.len() });
    }
    Ok(reads)
}

/// What the window holds, as far as the pre-flight could establish.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum VolumeEstimate {
    /// Nothing the book could hold makes this window exceed the budget: the
    /// high-water mark, or the census total, times the shape's conservative
    /// per-voucher cost is within it. Read undivided.
    WithinBudget,
    /// Counted per day, with the AlterIDs of the window's vouchers.
    Census(WindowCensus),
}

/// A per-day count of the vouchers in a window, with their AlterIDs.
///
/// This is the whole interface the planner needs from an estimator. Bridge's
/// own census produces it; a caller that already holds a lighter witness read
/// of the same window (`GUID`, `ALTERID` and `DATE` per voucher) can build one
/// with [`WindowCensus::from_rows`] and pass it as
/// [`WindowPlanSource::Counted`], and no census is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WindowCensus {
    days: BTreeMap<NaiveDate, u64>,
    /// Ascending AlterIDs of every voucher counted.
    alter_ids: Vec<u64>,
}

impl WindowCensus {
    pub(super) fn from_rows(rows: impl IntoIterator<Item = (NaiveDate, u64)>) -> Self {
        let mut days = BTreeMap::<NaiveDate, u64>::new();
        let mut alter_ids = Vec::new();
        for (day, alter_id) in rows {
            *days.entry(day).or_default() += 1;
            alter_ids.push(alter_id);
        }
        alter_ids.sort_unstable();
        Self { days, alter_ids }
    }

    pub(super) fn total(&self) -> u64 {
        self.days.values().sum()
    }

    /// Every counted day falls inside `[from, to]`. A count describing days
    /// outside the window is not a count of this window.
    fn within(&self, from: NaiveDate, to: NaiveDate) -> bool {
        self.days.keys().all(|day| *day >= from && *day <= to)
    }
}

/// The AlterID spans `(0, high_water]` is censused in, each narrow enough to be
/// bounded by construction under the budget, or `None` when the book needs more
/// census reads than one call may spend.
pub(super) fn census_spans(high_water: u64, budget: u64) -> Option<Vec<AlterIdSpan>> {
    let width = vouchers_per_read(budget, CENSUS_WIRE_BYTES_PER_VOUCHER).max(1);
    if high_water.div_ceil(width) > MAX_CENSUS_READS {
        return None;
    }
    let mut spans = Vec::new();
    let mut after = 0;
    while after < high_water {
        let through = after.saturating_add(width).min(high_water);
        spans.push(AlterIdSpan { after, through });
        after = through;
    }
    Some(spans)
}

/// Spans selecting at most `sample` of the window's vouchers in total, drawn
/// from the top of each third of their AlterIDs so the sample is not taken from
/// one spot.
///
/// Each span is bounded by construction — it cannot return more rows than it
/// selects — so the sample is safe to send at the conservative default before
/// anything about the book is known. Spans never overlap.
pub(super) fn calibration_spans(alter_ids: &[u64], sample: u64) -> Vec<AlterIdSpan> {
    let count = alter_ids.len();
    let sample = usize::try_from(sample).unwrap_or(usize::MAX).min(count);
    if sample == 0 {
        return Vec::new();
    }
    let before = |index: usize| {
        if index == 0 {
            alter_ids[0].saturating_sub(1)
        } else {
            alter_ids[index - 1]
        }
    };
    if sample == count {
        return vec![AlterIdSpan {
            after: before(0),
            through: alter_ids[count - 1],
        }];
    }
    // Fewer places than vouchers sampled, so every span selects at least one
    // and the spans together never select more than `sample`.
    let places = CALIBRATION_SPANS.min(sample);
    let per_span = sample / places;
    let mut spans = Vec::new();
    let mut next_free = 0;
    for place in 1..=places {
        // Inclusive index of the last voucher in this part of the window.
        let last = (count * place).div_ceil(places) - 1;
        let first = (last + 1).saturating_sub(per_span).max(next_free);
        if first > last {
            continue;
        }
        spans.push(AlterIdSpan {
            after: before(first),
            through: alter_ids[last],
        });
        next_free = last + 1;
    }
    spans
}

/// Where a window read's date ranges come from.
pub(super) enum WindowPlanSource {
    /// Estimate the window's volume and plan it. `known_high_water` is a voucher
    /// high-water mark the caller has just read itself, which saves reading it
    /// again.
    Estimate { known_high_water: Option<u64> },
    /// Plan from a count the caller already holds for exactly this window, and
    /// send no census. See [`WindowCensus`]. No production caller holds such a
    /// count yet; this is the interface one would use, proven by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    Counted(WindowCensus),
    /// Read exactly these ranges again, as a corroborating second read of a
    /// window already planned and read once.
    Replay(Vec<(String, String)>),
}

/// The per-call limits a window read plans under.
#[derive(Clone, Copy, Debug)]
pub(super) struct WindowReadLimits {
    pub(super) budget_bytes: u64,
    pub(super) default_bytes_per_voucher: u64,
}

impl WindowReadLimits {
    pub(super) const fn for_shape(shape: VoucherReadShape) -> Self {
        Self {
            budget_bytes: WINDOW_READ_BUDGET_BYTES,
            default_bytes_per_voucher: shape.default_wire_bytes_per_voucher(),
        }
    }
}

pub(super) struct WindowReadOutcome<T> {
    pub(super) rows: Vec<T>,
    /// The data reads alone, folded in date order.
    pub(super) evidence: Evidence,
    /// The pre-flight reads (high water, census, calibration sample), when any
    /// were sent. Kept apart so that a proof committing to the data read does
    /// not silently start committing to a planning estimate as well.
    pub(super) preflight_evidence: Option<Evidence>,
    /// The date ranges actually read, in order.
    pub(super) reads: Vec<(String, String)>,
    /// The voucher high-water mark the plan was bounded by, when one was used,
    /// so a follow-up read of an adjacent window need not read it again.
    pub(super) high_water: Option<u64>,
}

impl<T> WindowReadOutcome<T> {
    /// Every read this window cost, pre-flight first, for a caller that
    /// accounts for all of them together.
    pub(super) fn all_evidence(&self) -> Evidence {
        match &self.preflight_evidence {
            Some(preflight) => combine_evidence(preflight.clone(), self.evidence.clone()),
            None => self.evidence.clone(),
        }
    }
}

fn fold_evidence(target: &mut Option<Evidence>, next: Evidence) {
    *target = Some(match target.take() {
        Some(current) => combine_evidence(current, next),
        None => next,
    });
}

fn parse_day(value: &str) -> Result<NaiveDate, ToolFailure> {
    NaiveDate::parse_from_str(value, "%Y%m%d")
        .map_err(|_| ToolFailure::from("invalid_date_range".to_string()))
}

fn stamp(day: NaiveDate) -> String {
    day.format("%Y%m%d").to_string()
}

/// A plan as a stack whose pops come out in date order.
fn stack_of(plan: &[PlannedRead]) -> Vec<(String, String)> {
    plan.iter()
        .rev()
        .map(|read| (stamp(read.from), stamp(read.to)))
        .collect()
}

fn with_preflight(failure: ToolFailure, preflight: &Option<Evidence>) -> ToolFailure {
    match preflight {
        Some(evidence) => failure.with_prior_evidence(evidence.clone()),
        None => failure,
    }
}

impl Server {
    /// Read a voucher window of `shape`, divided so that no request is
    /// predicted over the budget (protocol reference §11c).
    ///
    /// `parse` turns one response into rows. Rows from every part are returned
    /// together in date order, so a caller sees what one undivided read would
    /// have produced and must validate the union, not each part.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn read_voucher_window<T, P>(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        from: &str,
        to: &str,
        shape: VoucherReadShape,
        source: WindowPlanSource,
        limits: WindowReadLimits,
        mut parse: P,
    ) -> Result<WindowReadOutcome<T>, ToolFailure>
    where
        P: FnMut(&str) -> Result<Vec<T>, String>,
    {
        let first = parse_day(from)?;
        let last = parse_day(to)?;
        if first > last {
            return Err("invalid_date_range".to_string().into());
        }
        let mut preflight = None;
        let mut high_water = None;
        let mut census = None;
        let mut bytes_per_voucher = limits.default_bytes_per_voucher;
        // A stack whose pops are ascending and whose ranges always tile the
        // part of the window not yet read, so the rows arrive in date order.
        let mut pending: Vec<(String, String)> = match source {
            WindowPlanSource::Replay(ranges) => {
                for (start, end) in &ranges {
                    if parse_day(start)? > parse_day(end)? {
                        return Err("invalid_date_range".to_string().into());
                    }
                }
                ranges.into_iter().rev().collect()
            }
            source => {
                let estimate = match source {
                    WindowPlanSource::Counted(counted) => {
                        if !counted.within(first, last) {
                            return Err("window_not_honoured".to_string().into());
                        }
                        within_budget_or_census(counted, limits)
                    }
                    WindowPlanSource::Estimate { known_high_water } => {
                        let (estimate, mark) = self
                            .estimate_window_volume(
                                identity,
                                company,
                                (from, to),
                                known_high_water,
                                limits,
                                &mut preflight,
                            )
                            .await
                            .map_err(|failure| with_preflight(failure, &preflight))?;
                        high_water = Some(mark);
                        estimate
                    }
                    WindowPlanSource::Replay(_) => unreachable!("handled above"),
                };
                match estimate {
                    VolumeEstimate::WithinBudget => vec![(from.to_string(), to.to_string())],
                    VolumeEstimate::Census(counted) => {
                        if let Some(measured) = self
                            .measure_bytes_per_voucher(
                                identity,
                                company,
                                (from, to),
                                shape,
                                &counted,
                                limits,
                                &mut parse,
                                &mut preflight,
                            )
                            .await
                            .map_err(|failure| with_preflight(failure, &preflight))?
                        {
                            bytes_per_voucher = with_headroom(measured);
                        }
                        let plan = plan_window_reads(
                            first,
                            last,
                            &counted.days,
                            bytes_per_voucher,
                            limits.budget_bytes,
                            MAX_PLANNED_READS,
                        )
                        .map_err(|refusal| {
                            with_preflight(refusal.code().to_string().into(), &preflight)
                        })?;
                        census = Some(counted);
                        stack_of(&plan)
                    }
                }
            }
        };
        let mut rows = Vec::new();
        let mut evidence: Option<Evidence> = None;
        let mut reads = Vec::new();
        // #485: the smallest span already known to be unservable on THIS call,
        // so a sibling of the same size is split without spending a deadline.
        let mut smallest_failed_days: Option<i64> = None;
        let outcome: Result<(), ToolFailure> = async {
            while let Some((start, end)) = pending.pop() {
                if shape.splits_on_oversize()
                    && must_split_before_reading(
                        window_span_days(&start, &end),
                        smallest_failed_days,
                    )
                {
                    // Known too big. Split without spending a deadline to confirm it.
                    if let Some((left, right)) = split_verification_window(&start, &end) {
                        pending.push(right);
                        pending.push(left);
                        continue;
                    }
                }
                let request = shape.render(company, &start, &end, None)?;
                match self.post_read(identity, request).await {
                    Ok((xml, read_evidence)) => {
                        let part = parse(&xml).map_err(|code| {
                            ToolFailure::from(code).with_prior_evidence(read_evidence.clone())
                        })?;
                        // Keep measuring as the window is read: a range heavier
                        // than the sample raises the figure the rest is planned at.
                        // Only ever raised, so a light range never loosens the plan.
                        if let (Some(counted), Some(measured)) = (
                            census.as_ref(),
                            measured_bytes_per_voucher(&read_evidence, part.len()),
                        ) {
                            let planned = with_headroom(measured);
                            let end_day = parse_day(&end)?;
                            if planned > bytes_per_voucher && end_day < last {
                                bytes_per_voucher = planned;
                                let rest = end_day
                                    .succ_opt()
                                    .expect("end precedes the window's last day");
                                let allowance = MAX_PLANNED_READS.saturating_sub(reads.len() + 1);
                                let plan = plan_window_reads(
                                    rest,
                                    last,
                                    &counted.days,
                                    bytes_per_voucher,
                                    limits.budget_bytes,
                                    allowance,
                                )
                                .map_err(|refusal| ToolFailure::from(refusal.code().to_string()))?;
                                // `pending` tiles exactly the days after `end`, so
                                // replacing it with a plan of those days loses none.
                                pending = stack_of(&plan);
                            }
                        }
                        rows.extend(part);
                        fold_evidence(&mut evidence, read_evidence);
                        reads.push((start, end));
                    }
                    Err(failure) if shape.splits_on_oversize() && window_is_too_large(&failure) => {
                        // Record the span so sibling branches do not pay a deadline to
                        // learn the same thing. `min` because a later, smaller failure
                        // is the tighter bound.
                        if let Some(span) = window_span_days(&start, &end) {
                            smallest_failed_days =
                                Some(smallest_failed_days.map_or(span, |known| known.min(span)));
                        }
                        // A single day that still cannot be served is not something
                        // splitting can fix, and returning the days that did work
                        // would be a read over an incomplete window. Refuse.
                        let (left, right) =
                            split_verification_window(&start, &end).ok_or_else(|| {
                                ToolFailure::from(shape.day_not_readable_code().to_string())
                            })?;
                        pending.push(right);
                        pending.push(left);
                    }
                    Err(failure) => return Err(failure),
                }
            }
            Ok(())
        }
        .await;
        if let Err(failure) = outcome {
            // A failure owns every read this window cost before it: the
            // pre-flight, then each part already read, in that order.
            let prior = match (preflight, evidence) {
                (Some(estimate), Some(data)) => Some(combine_evidence(estimate, data)),
                (estimate, data) => estimate.or(data),
            };
            return Err(match prior {
                Some(prior) => failure.with_prior_evidence(prior),
                None => failure,
            });
        }
        // Unreachable while every plan holds at least one range; kept total
        // rather than panicking on a future change to that.
        let evidence =
            evidence.unwrap_or_else(|| super::agent_import::local_evidence("voucher_window_empty"));
        Ok(WindowReadOutcome {
            rows,
            evidence,
            preflight_evidence: preflight,
            reads,
            high_water,
        })
    }

    /// Establish what `window` holds, cheapest first: the company's voucher
    /// high-water mark bounds every window of the book at once; only when that
    /// bound is not enough is the window itself censused.
    async fn estimate_window_volume(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        window: (&str, &str),
        known_high_water: Option<u64>,
        limits: WindowReadLimits,
        preflight: &mut Option<Evidence>,
    ) -> Result<(VolumeEstimate, u64), ToolFailure> {
        let high_water = match known_high_water {
            Some(value) => value,
            None => {
                let (xml, evidence) = self
                    .post_read(identity, render_agent_company_high_water(company))
                    .await?;
                fold_evidence(preflight, evidence);
                voucher_high_water(&xml, identity.company_guid())
                    .ok_or_else(|| ToolFailure::from(VOLUME_UNESTIMATED.to_string()))?
            }
        };
        // Every voucher carries a distinct AlterID no greater than the high-water
        // mark (§10), so the book — and therefore any window of it — holds at
        // most `high_water` vouchers.
        if high_water.saturating_mul(limits.default_bytes_per_voucher) <= limits.budget_bytes {
            return Ok((VolumeEstimate::WithinBudget, high_water));
        }
        let spans = census_spans(high_water, limits.budget_bytes)
            .ok_or_else(|| ToolFailure::from(VOLUME_UNESTIMATED.to_string()))?;
        let mut counted = Vec::new();
        for span in spans {
            let request = render_agent_voucher_census(company, window.0, window.1, span)?;
            let (xml, evidence) = match self.post_read(identity, request).await {
                Ok(read) => read,
                // A census Tally cannot serve leaves the window undescribed, and
                // an undescribed window is refused, never read blind.
                Err(failure) if window_is_too_large(&failure) => {
                    return Err(ToolFailure::from(VOLUME_UNESTIMATED.to_string()));
                }
                Err(failure) => return Err(failure),
            };
            fold_evidence(preflight, evidence);
            counted.extend(
                parse_voucher_census(&xml, window, span)
                    .map_err(|_| ToolFailure::from(VOLUME_UNESTIMATED.to_string()))?,
            );
        }
        Ok((
            within_budget_or_census(WindowCensus::from_rows(counted), limits),
            high_water,
        ))
    }

    /// Read a bounded sample of the window in the real shape and return the
    /// heaviest bytes per voucher among its spans, or `None` when nothing could
    /// be measured — in which case the caller keeps the conservative default.
    #[allow(clippy::too_many_arguments)]
    async fn measure_bytes_per_voucher<T, P>(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        window: (&str, &str),
        shape: VoucherReadShape,
        census: &WindowCensus,
        limits: WindowReadLimits,
        parse: &mut P,
        preflight: &mut Option<Evidence>,
    ) -> Result<Option<u64>, ToolFailure>
    where
        P: FnMut(&str) -> Result<Vec<T>, String>,
    {
        let sample =
            vouchers_per_read(limits.budget_bytes, limits.default_bytes_per_voucher).max(1);
        let mut heaviest = None;
        for span in calibration_spans(&census.alter_ids, sample) {
            let request = shape.render(company, window.0, window.1, Some(span))?;
            let (xml, evidence) = self.post_read(identity, request).await?;
            let measured = parse(&xml)
                .ok()
                .and_then(|rows| measured_bytes_per_voucher(&evidence, rows.len()));
            fold_evidence(preflight, evidence);
            heaviest = heaviest.max(measured);
        }
        Ok(heaviest)
    }
}

fn within_budget_or_census(census: WindowCensus, limits: WindowReadLimits) -> VolumeEstimate {
    if census
        .total()
        .saturating_mul(limits.default_bytes_per_voucher)
        <= limits.budget_bytes
    {
        VolumeEstimate::WithinBudget
    } else {
        VolumeEstimate::Census(census)
    }
}

/// Bytes per voucher of one paired read. `post_read` reports both accepted
/// bodies, so one response is half of it.
fn measured_bytes_per_voucher(evidence: &Evidence, rows: usize) -> Option<u64> {
    let rows = u64::try_from(rows).ok().filter(|rows| *rows > 0)?;
    let body = u64::try_from(evidence.bytes / 2).ok()?;
    Some(body.div_ceil(rows))
}

fn window_is_too_large(failure: &ToolFailure) -> bool {
    is_window_too_large_code(&failure.code)
}

/// Inclusive span of a `YYYYMMDD` window in days, or `None` if either bound is
/// unparseable. A one-day window spans 1.
pub(super) fn window_span_days(from: &str, to: &str) -> Option<i64> {
    let parse = |value: &str| chrono::NaiveDate::parse_from_str(value, "%Y%m%d").ok();
    Some((parse(to)? - parse(from)?).num_days() + 1)
}

/// Whether a window is already known to be unservable at this size, so it can be
/// split without spending a deadline to confirm it.
///
/// Only ever an optimisation. Answering `false` wrongly costs one extra read;
/// answering `true` wrongly costs one extra split, and a split never narrows the
/// window — both halves stay in the queue. Neither answer can change the set of
/// vouchers observed, which is why an unparseable span (`None`) simply falls
/// through to reading rather than being treated as a failure.
pub(super) fn must_split_before_reading(span: Option<i64>, smallest_failed: Option<i64>) -> bool {
    match (span, smallest_failed) {
        (Some(span), Some(failed)) => span >= failed && span > 1,
        _ => false,
    }
}

/// Split an inclusive `YYYYMMDD` window into two inclusive halves that exactly
/// partition it, or `None` when it is already a single day.
///
/// Exactness is the whole point. The verification read filters on
/// `$Date >= from AND $Date <= to`, so `[from, mid]` and `[mid + 1, to]` cover
/// every date the undivided window covered, once each. A gap would silently drop
/// vouchers from an attribution check and an overlap would double-count them —
/// either turns a read that got smaller into a verification that got weaker.
pub(super) fn split_verification_window(
    from: &str,
    to: &str,
) -> Option<((String, String), (String, String))> {
    let parse = |value: &str| chrono::NaiveDate::parse_from_str(value, "%Y%m%d").ok();
    let (start, end) = (parse(from)?, parse(to)?);
    if start >= end {
        return None;
    }
    let mid = start + chrono::Duration::days((end - start).num_days() / 2);
    // `mid` equals `start` on a two-day window, which is correct and still shrinks
    // both halves. It cannot reach `end`, because the window spans at least one day
    // and `floor(n / 2) < n` for every `n >= 1` — so the right half is always a
    // proper subset and the loop in `read_voucher_window` terminates. Asserted
    // rather than clamped: a clamp here would be unreachable today and would
    // silently absorb a future change to the midpoint that broke termination.
    debug_assert!(mid < end, "midpoint must shrink both halves");
    let stamp = |date: chrono::NaiveDate| date.format("%Y%m%d").to_string();
    Some((
        (stamp(start), stamp(mid)),
        (stamp(mid + chrono::Duration::days(1)), stamp(end)),
    ))
}

/// The voucher high-water mark, with a company that has never held a voucher
/// read as zero (Tally omits `ALTVCHID` for it; see `pre_import_mark_refusal`).
/// `None` when the mark could not be observed at all.
fn voucher_high_water(xml: &str, company_guid: &str) -> Option<u64> {
    match parse_company_high_water(xml, company_guid) {
        Ok(value) => value["altvchid"].as_u64(),
        Err(code) if code == VOUCHER_CHECKPOINT_NOT_OBSERVED => Some(0),
        Err(_) => None,
    }
}

/// Parse a census response into `(date, AlterID)` per voucher.
///
/// Counts `VOUCHER` start elements inside `BODY/DATA/COLLECTION` only, and
/// takes only the two direct child fields it needs: §12.7 records an empty
/// response whose `CMPINFO` carries a bare `<VOUCHER>0</VOUCHER>`, which a
/// document-wide count reads as a row. Every row must fall inside the window
/// and the span it was asked for, or the census is not describing what was
/// asked and is refused.
pub(super) fn parse_voucher_census(
    xml: &str,
    window: (&str, &str),
    span: AlterIdSpan,
) -> Result<Vec<(NaiveDate, u64)>, String> {
    use quick_xml::events::Event;
    validate_agent_envelope(xml)?;
    let invalid = || "agent_read_protocol_invalid".to_string();
    let first = NaiveDate::parse_from_str(window.0, "%Y%m%d").map_err(|_| invalid())?;
    let last = NaiveDate::parse_from_str(window.1, "%Y%m%d").map_err(|_| invalid())?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut scope = NativeCollectionScope::default();
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut tag = String::new();
    let mut rows = Vec::new();
    let wanted = |name: &str| matches!(name, "DATE" | "ALTERID");
    loop {
        match reader.read_event() {
            Ok(event @ (Event::Start(_) | Event::Empty(_))) => {
                let empty = matches!(event, Event::Empty(_));
                let (Event::Start(event) | Event::Empty(event)) = event else {
                    unreachable!("matched as Start or Empty above")
                };
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if name == "VOUCHER" && scope.collection() {
                    if empty {
                        return Err(invalid());
                    }
                    current = Some(BTreeMap::new());
                }
                // `row` is true directly inside a VOUCHER, before this child is
                // pushed, so only the voucher's own DATE and ALTERID are claimed.
                if scope.row("VOUCHER") && wanted(&name) {
                    claim_agent_scalar(current.as_mut().ok_or_else(invalid)?, &name)?;
                }
                scope.start(name.clone());
                if empty {
                    scope.end(&name)?;
                    tag.clear();
                } else {
                    tag = name;
                }
            }
            Ok(Event::Text(text)) => {
                if let Some(row) = current
                    .as_mut()
                    .filter(|_| scope.field("VOUCHER") && wanted(&tag))
                {
                    append_agent_text(row, &tag, decoded_agent_text(text)?);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(row) = current
                    .as_mut()
                    .filter(|_| scope.field("VOUCHER") && wanted(&tag))
                {
                    append_agent_text(row, &tag, decoded_agent_reference(reference)?);
                }
            }
            Ok(Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                scope.end(&end)?;
                if end == "VOUCHER" && scope.collection() {
                    let row = current.take().ok_or_else(invalid)?;
                    let day = row
                        .get("DATE")
                        .and_then(|value| NaiveDate::parse_from_str(value.trim(), "%Y%m%d").ok())
                        .ok_or_else(invalid)?;
                    let alter_id = row
                        .get("ALTERID")
                        .and_then(|value| value.trim().parse::<u64>().ok())
                        .ok_or_else(invalid)?;
                    if day < first
                        || day > last
                        || alter_id <= span.after
                        || alter_id > span.through
                    {
                        return Err("window_not_honoured".to_string());
                    }
                    rows.push((day, alter_id));
                }
                tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(_) => return Err(invalid()),
            _ => {}
        }
    }
    scope.finish()?;
    Ok(rows)
}

#[cfg(test)]
#[path = "agent_voucher_window_tests.rs"]
mod tests;
