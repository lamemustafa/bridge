//! A pre-flight volume bound for windowed voucher reads.
//!
//! The rule is protocol reference §11c. In short: Tally builds a whole response
//! before Bridge can refuse it, and a client that gives up does not stop Tally
//! (§11b.2). Bridge's response cap and per-leg deadline are enforced on the
//! response, client-side, so they protect Bridge and never the gateway. A bound
//! that protects Tally has to be applied *before* a request is sent.
//!
//! So a windowed voucher read first establishes how many vouchers the window
//! holds, day by day, and divides it so that no single request is predicted to
//! exceed [`WINDOW_READ_BUDGET_BYTES`] at the book's own measured cost per
//! voucher. A day too heavy for one read is divided further, by AlterID ranges
//! within that day; only a single voucher predicted over the budget is refused.
//!
//! Every estimate here is a planning figure, not a bound (§12a.8). The transport
//! cap stays the final safeguard; this only stops Bridge from *asking* for a
//! response it already expects to be too large.
use super::*;
use chrono::NaiveDate;

/// Predicted encoded bytes one windowed read may carry.
///
/// Half the transport response cap, and the **only** margin in the plan: a
/// measured per-voucher cost is used as measured, not inflated again. The half
/// left over absorbs the error in the measurement before the cap is reached.
/// On the heaviest book measured, a one-day probe understated the year's mean
/// by a factor of 1.25 (§11c.1); this margin is sized for that with room, not
/// for a book whose vouchers differ in weight by more than 2x from the parts
/// already read (§11c.4). Encoded means on the wire: Bridge reads in UTF-16LE
/// (§1.2), which costs exactly twice a UTF-8 measurement of the same response.
pub(super) const WINDOW_READ_BUDGET_BYTES: u64 =
    bridge_tally_transport::XML_RESPONSE_MAX_BYTES as u64 / 2;

/// Encoded bytes per voucher of the census row (`GUID,ALTERID,DATE`).
///
/// §11a, §11b and §12a.8 each measured a minimal voucher `FETCH` at about
/// 1.2 KB per row in UTF-8 — Tally emits a fixed envelope whatever narrow fields
/// are named — which is about 2.4 KB on Bridge's UTF-16 wire. This leaves room
/// above that rather than assuming it.
const CENSUS_WIRE_BYTES_PER_VOUCHER: u64 = 4 * 1024;

/// The most census requests one window read may spend, planned and reactive
/// together. Above it the volume is unestimated and the read is refused.
const MAX_CENSUS_READS: usize = 64;

/// The most planned data reads one window read may spend before it is refused
/// as needing a narrower window. The same ceiling the outstandings scanner uses
/// for its own segment pairs.
pub(super) const MAX_PLANNED_READS: usize = 128;

/// How much denser than its average a census assumes the next date range to
/// be, once a first range has been counted. Census rows are light, so an
/// underestimate costs a light response larger than planned, and a census
/// response the transport refuses is halved by date.
const CENSUS_DENSITY_FACTOR: u64 = 2;

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
    /// inventory-heavy trading book, and the entry wildcard ran about 128 KB on
    /// the same book. Doubled for the UTF-16 wire, then given room: 96 KiB is
    /// 48 KiB of UTF-8, and 384 KiB is 192 KiB. It sizes only the first data
    /// part of a divided window; every part read after it is planned at the
    /// book's own measured cost.
    pub(super) const fn default_wire_bytes_per_voucher(self) -> u64 {
        match self {
            Self::ImportVerification | Self::Movement => 96 * 1024,
            Self::EntryWildcard => 384 * 1024,
        }
    }

    /// Whether a part Tally cannot serve is divided and retried (#485), rather
    /// than failing the read. Only the verification read did this before the
    /// pre-flight bound existed, and it still alone does: retrying a read that
    /// timed out queues more work behind a gateway still building the abandoned
    /// response, so it is not a behaviour to extend by default.
    const fn splits_on_oversize(self) -> bool {
        matches!(self, Self::ImportVerification)
    }

    /// The refusal for a part that still cannot be served after dividing it.
    const fn day_not_readable_code(self) -> &'static str {
        match self {
            Self::ImportVerification => "verification_window_day_not_readable",
            Self::Movement | Self::EntryWildcard => "voucher_window_day_not_readable",
        }
    }

    /// The request for one part. A part without a span goes through the
    /// shape's own renderer, so a window the bound leaves whole is sent exactly
    /// as it was before the bound existed.
    pub(super) fn render(
        self,
        company: &str,
        from: &str,
        to: &str,
        span: Option<AlterIdSpan>,
    ) -> Result<String, String> {
        match (self, span) {
            (Self::ImportVerification, None) => Ok(
                super::agent_import::render_import_verification_read(company, from, to),
            ),
            (Self::ImportVerification, Some(_)) => Ok(
                super::agent_import::render_import_verification_in_span(company, from, to, span),
            ),
            (Self::Movement, None) => render_agent_movement_vouchers(company, from, to),
            (Self::Movement, Some(_)) => {
                render_agent_movement_vouchers_in_span(company, from, to, span)
            }
            (Self::EntryWildcard, None) => render_agent_vouchers(company, from, to, None),
            (Self::EntryWildcard, Some(_)) => {
                render_agent_vouchers_in_span(company, from, to, span)
            }
        }
    }
}

/// An AlterID range `(after, through]`. Within one day it divides a day too
/// heavy for one read; in a census of one day it bounds the census by
/// construction, since with distinct AlterIDs it cannot return more than
/// `through - after` rows.
///
/// Dividing a day by spans predicts each span's size from the census, so that
/// prediction is only as good as the census is current: a voucher created or
/// altered after it was counted takes an AlterID above the mark the spans were
/// planned to. The spans still cover `(0, ceiling]`, and the closing
/// high-water bracket in `read_voucher_window` refuses a read during which the
/// mark moved, rather than returning one that may have missed such a voucher.
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

    fn holds(self, alter_id: u64) -> bool {
        alter_id > self.after && alter_id <= self.through
    }
}

/// One request of a divided window: a date range, and for a part of one day, the
/// AlterID span of that day it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WindowPart {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) span: Option<AlterIdSpan>,
}

/// One planned read, with the vouchers the census counted in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlannedRead {
    pub(super) from: NaiveDate,
    pub(super) to: NaiveDate,
    pub(super) span: Option<AlterIdSpan>,
    pub(super) vouchers: u64,
}

impl PlannedRead {
    fn part(&self) -> WindowPart {
        WindowPart {
            from: stamp(self.from),
            to: stamp(self.to),
            span: self.span,
        }
    }
}

/// Why a window cannot be planned under the budget. Each is refused by name,
/// before any further data read is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlanRefusal {
    /// A single voucher is predicted over the budget, so no division of any day
    /// can bring a read within it.
    VoucherOverBudget { day: NaiveDate },
    /// The window divides, but into more reads than one call may spend.
    TooManyReads { reads: usize },
}

impl PlanRefusal {
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::VoucherOverBudget { .. } => "voucher_window_part_over_budget",
            Self::TooManyReads { .. } => "voucher_window_too_many_reads",
        }
    }
}

/// Neither the high-water mark nor a census could describe the window, so no
/// prediction exists. Refused rather than sent: an unestimated read is exactly
/// the unbounded request this module exists to stop.
pub(super) const VOLUME_UNESTIMATED: &str = "voucher_window_volume_unestimated";

/// The voucher high-water mark moved while a divided window was being read, so
/// its parts may not describe one state of the book. See `read_voucher_window`.
pub(super) const WINDOW_CHANGED_DURING_READ: &str = "voucher_window_changed_during_read";

/// Vouchers one read may carry at `bytes_per_voucher` under `budget`.
fn vouchers_per_read(budget: u64, bytes_per_voucher: u64) -> u64 {
    budget / bytes_per_voucher.max(1)
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
    /// Ascending AlterIDs of the vouchers counted on each day.
    days: BTreeMap<NaiveDate, Vec<u64>>,
}

impl WindowCensus {
    pub(super) fn from_rows(rows: impl IntoIterator<Item = (NaiveDate, u64)>) -> Self {
        let mut days = BTreeMap::<NaiveDate, Vec<u64>>::new();
        for (day, alter_id) in rows {
            days.entry(day).or_default().push(alter_id);
        }
        for ids in days.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }
        Self { days }
    }

    pub(super) fn total(&self) -> u64 {
        self.days.values().map(|ids| ids.len() as u64).sum()
    }

    fn max_alter_id(&self) -> u64 {
        self.days
            .values()
            .filter_map(|ids| ids.last().copied())
            .max()
            .unwrap_or(0)
    }

    /// Every counted day falls inside `[from, to]`. A count describing days
    /// outside the window is not a count of this window.
    fn within(&self, from: NaiveDate, to: NaiveDate) -> bool {
        self.days.keys().all(|day| *day >= from && *day <= to)
    }

    /// The day's counted AlterIDs inside `span`, or all of them.
    fn ids_on(&self, day: NaiveDate, span: Option<AlterIdSpan>) -> Vec<u64> {
        self.days
            .get(&day)
            .map(|ids| {
                ids.iter()
                    .copied()
                    .filter(|id| span.is_none_or(|span| span.holds(*id)))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Divide `[from, to]` into reads, each predicted within `budget` at
/// `bytes_per_voucher`.
///
/// Days are packed greedily, in order, into contiguous date ranges. A day with
/// more vouchers than one read may carry is read alone, in AlterID spans of that
/// day that together cover `(0, ceiling]` — every AlterID the book can hold
/// while the high-water mark stays at `ceiling`. A day without vouchers joins
/// whichever range it falls in. The reads tile the window exactly, so reading
/// every one observes the same vouchers one undivided read would.
///
/// `resume_after` continues a day already partly read: on `from`, only
/// AlterIDs above it remain, and they are read in spans starting there.
///
/// A window predicted within budget as a whole is returned as a single read
/// equal to the window, which is how a small window keeps its original request.
#[allow(clippy::too_many_arguments)]
pub(super) fn plan_window_reads(
    from: NaiveDate,
    to: NaiveDate,
    census: &WindowCensus,
    resume_after: Option<u64>,
    ceiling: u64,
    bytes_per_voucher: u64,
    budget: u64,
    max_reads: usize,
) -> Result<Vec<PlannedRead>, PlanRefusal> {
    let capacity = vouchers_per_read(budget, bytes_per_voucher);
    let mut reads = Vec::new();
    let mut start = from;
    let mut carried = 0_u64;
    for (&day, ids) in census.days.range(from..=to) {
        let resumed = if day == from { resume_after } else { None };
        let ids = match resumed {
            Some(after) => ids.iter().copied().filter(|id| *id > after).collect(),
            None => ids.clone(),
        };
        let vouchers = ids.len() as u64;
        if vouchers == 0 && resumed.is_none() {
            continue;
        }
        if vouchers > 0 && capacity == 0 {
            return Err(PlanRefusal::VoucherOverBudget { day });
        }
        if vouchers > capacity || resumed.is_some() {
            if day > start {
                let end = day.pred_opt().expect("a later day has a predecessor");
                reads.push(PlannedRead {
                    from: start,
                    to: end,
                    span: None,
                    vouchers: carried,
                });
            }
            day_spans(
                day,
                &ids,
                resumed.unwrap_or(0),
                ceiling,
                capacity,
                &mut reads,
            );
            start = match day.succ_opt() {
                Some(next) => next,
                None => return finish(reads, max_reads),
            };
            carried = 0;
            if day >= to {
                return finish(reads, max_reads);
            }
            continue;
        }
        if carried.saturating_add(vouchers) > capacity {
            // `carried > 0` here, because `vouchers <= capacity`, so an earlier
            // day in `[start, day)` holds vouchers and `day > start`.
            let end = day.pred_opt().expect("a later day has a predecessor");
            reads.push(PlannedRead {
                from: start,
                to: end,
                span: None,
                vouchers: carried,
            });
            start = day;
            carried = 0;
        }
        carried = carried.saturating_add(vouchers);
    }
    if start <= to {
        reads.push(PlannedRead {
            from: start,
            to,
            span: None,
            vouchers: carried,
        });
    }
    finish(reads, max_reads)
}

fn finish(reads: Vec<PlannedRead>, max_reads: usize) -> Result<Vec<PlannedRead>, PlanRefusal> {
    if reads.len() > max_reads {
        return Err(PlanRefusal::TooManyReads { reads: reads.len() });
    }
    Ok(reads)
}

/// Spans of one day, each holding at most `capacity` of its counted vouchers,
/// that together cover `(after, ceiling]`.
fn day_spans(
    day: NaiveDate,
    ids: &[u64],
    after: u64,
    ceiling: u64,
    capacity: u64,
    reads: &mut Vec<PlannedRead>,
) {
    let capacity = usize::try_from(capacity.max(1)).unwrap_or(usize::MAX);
    let chunks = ids.chunks(capacity).collect::<Vec<_>>();
    let mut lower = after;
    for (index, chunk) in chunks.iter().enumerate() {
        let through = if index + 1 == chunks.len() {
            ceiling.max(*chunk.last().expect("chunks are non-empty"))
        } else {
            *chunk.last().expect("chunks are non-empty")
        };
        reads.push(PlannedRead {
            from: day,
            to: day,
            span: Some(AlterIdSpan {
                after: lower,
                through,
            }),
            vouchers: chunk.len() as u64,
        });
        lower = through;
    }
    if chunks.is_empty() && ceiling > after {
        reads.push(PlannedRead {
            from: day,
            to: day,
            span: Some(AlterIdSpan {
                after,
                through: ceiling,
            }),
            vouchers: 0,
        });
    }
}

/// Divide a part Tally could not serve (#485): a date range in halves by date,
/// a single day in halves of its counted AlterIDs. `None` when neither is
/// possible — one day with fewer than two counted vouchers.
fn halve_part(
    part: &WindowPart,
    census: Option<&WindowCensus>,
    ceiling: u64,
) -> Option<(WindowPart, WindowPart)> {
    if part.span.is_none() {
        if let Some(((left_from, left_to), (right_from, right_to))) =
            split_verification_window(&part.from, &part.to)
        {
            return Some((
                WindowPart {
                    from: left_from,
                    to: left_to,
                    span: None,
                },
                WindowPart {
                    from: right_from,
                    to: right_to,
                    span: None,
                },
            ));
        }
    }
    let day = NaiveDate::parse_from_str(&part.from, "%Y%m%d").ok()?;
    let ids = census?.ids_on(day, part.span);
    if ids.len() < 2 || part.from != part.to {
        return None;
    }
    let outer = part.span.unwrap_or(AlterIdSpan {
        after: 0,
        through: ceiling.max(*ids.last()?),
    });
    let mid = ids[ids.len() / 2 - 1];
    let side = |span| WindowPart {
        from: part.from.clone(),
        to: part.to.clone(),
        span: Some(span),
    };
    Some((
        side(AlterIdSpan {
            after: outer.after,
            through: mid,
        }),
        side(AlterIdSpan {
            after: mid,
            through: outer.through,
        }),
    ))
}

/// Where a window read's parts come from.
pub(super) enum WindowPlanSource {
    /// Estimate the window's volume and plan it. `known_high_water` is a voucher
    /// high-water mark the caller has just read itself, which saves reading it
    /// again.
    Estimate { known_high_water: Option<u64> },
    /// Plan from a count the caller already holds for exactly this window, and
    /// send no census. See [`WindowCensus`]. No high-water mark is read, so a
    /// divided read is not bracketed: the caller answers for the count being
    /// current. No production caller holds such a count yet; this is the
    /// interface one would use, proven by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    Counted(WindowCensus),
    /// Read exactly these parts again, as a corroborating second read of a
    /// window already planned and read once.
    Replay(Vec<WindowPart>),
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

    fn census_capacity(self) -> u64 {
        vouchers_per_read(self.budget_bytes, CENSUS_WIRE_BYTES_PER_VOUCHER).max(1)
    }
}

pub(super) struct WindowReadOutcome<T> {
    pub(super) rows: Vec<T>,
    /// The data reads alone, folded in order.
    pub(super) evidence: Evidence,
    /// The pre-flight reads (high water, census, the closing high-water
    /// bracket), when any were sent. Kept apart so that a proof committing to
    /// the data read does not silently start committing to a planning estimate.
    pub(super) preflight_evidence: Option<Evidence>,
    /// The parts actually read, in order.
    pub(super) reads: Vec<WindowPart>,
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

/// A plan as a stack whose pops come out in order.
fn stack_of(plan: &[PlannedRead]) -> Vec<WindowPart> {
    plan.iter().rev().map(PlannedRead::part).collect()
}

/// A failure carrying every read made before it: pre-flight first, then data.
fn with_prior(
    failure: ToolFailure,
    preflight: &Option<Evidence>,
    data: &Option<Evidence>,
) -> ToolFailure {
    let prior = match (preflight.clone(), data.clone()) {
        (Some(estimate), Some(read)) => Some(combine_evidence(estimate, read)),
        (estimate, read) => estimate.or(read),
    };
    match prior {
        Some(prior) => failure.with_prior_evidence(prior),
        None => failure,
    }
}

/// What the pre-flight established before any data read.
enum Preflight {
    /// Read undivided: nothing the book or the window holds can exceed the budget.
    Whole,
    /// Counted per day; plan it.
    Counted {
        census: WindowCensus,
        /// The mark the census was bounded by, when one was read, which the
        /// closing bracket compares against.
        high_water: Option<u64>,
    },
}

impl Server {
    /// Read a voucher window of `shape`, divided so that no request is
    /// predicted over the budget (protocol reference §11c).
    ///
    /// `parse` turns one response into rows. Rows from every part are returned
    /// together in order, so a caller sees what one undivided read would have
    /// produced and must validate the union, not each part.
    ///
    /// A divided read is bracketed: the voucher high-water mark is read again
    /// after the last part, and a mark that moved refuses the read. Parts read
    /// at different moments describe one state of the book only if nothing was
    /// created or altered between them, and a day read in AlterID spans covers
    /// only the AlterIDs that existed when it was counted.
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
        let mut ceiling = 0;
        let mut bytes_per_voucher = limits.default_bytes_per_voucher;
        // A stack whose pops are in order and whose parts always tile the part
        // of the window not yet read, so the rows arrive in order.
        let mut pending: Vec<WindowPart> = match source {
            WindowPlanSource::Replay(parts) => {
                for part in &parts {
                    if parse_day(&part.from)? > parse_day(&part.to)? {
                        return Err("invalid_date_range".to_string().into());
                    }
                }
                parts.into_iter().rev().collect()
            }
            source => {
                let estimate = match source {
                    WindowPlanSource::Counted(counted) => {
                        if !counted.within(first, last) {
                            return Err("window_not_honoured".to_string().into());
                        }
                        whole_or_counted(counted, None, limits)
                    }
                    WindowPlanSource::Estimate { known_high_water } => self
                        .estimate_window_volume(
                            identity,
                            company,
                            (first, last),
                            known_high_water,
                            limits,
                            &mut preflight,
                            &mut high_water,
                        )
                        .await
                        .map_err(|failure| with_prior(failure, &preflight, &None))?,
                    WindowPlanSource::Replay(_) => unreachable!("handled above"),
                };
                match estimate {
                    Preflight::Whole => vec![WindowPart {
                        from: from.to_string(),
                        to: to.to_string(),
                        span: None,
                    }],
                    Preflight::Counted {
                        census: counted,
                        high_water: mark,
                    } => {
                        ceiling = mark.unwrap_or(0).max(counted.max_alter_id());
                        let plan = plan_window_reads(
                            first,
                            last,
                            &counted,
                            None,
                            ceiling,
                            bytes_per_voucher,
                            limits.budget_bytes,
                            MAX_PLANNED_READS,
                        )
                        .map_err(|refusal| {
                            with_prior(refusal.code().to_string().into(), &preflight, &None)
                        })?;
                        census = Some(counted);
                        stack_of(&plan)
                    }
                }
            }
        };
        let mut rows = Vec::new();
        let mut evidence: Option<Evidence> = None;
        let mut reads: Vec<WindowPart> = Vec::new();
        let mut measured = false;
        // #485: the smallest span already known to be unservable on THIS call,
        // so a sibling of the same size is split without spending a deadline.
        let mut smallest_failed_days: Option<i64> = None;
        let outcome: Result<(), ToolFailure> = async {
            while let Some(part) = pending.pop() {
                if shape.splits_on_oversize()
                    && part.span.is_none()
                    && must_split_before_reading(
                        window_span_days(&part.from, &part.to),
                        smallest_failed_days,
                    )
                {
                    // Known too big. Split without spending a deadline to confirm it.
                    if let Some((left, right)) = halve_part(&part, census.as_ref(), ceiling) {
                        pending.push(right);
                        pending.push(left);
                        continue;
                    }
                }
                let request = shape.render(company, &part.from, &part.to, part.span)?;
                match self.post_read(identity, request).await {
                    Ok((xml, read_evidence)) => {
                        let parsed = parse(&xml);
                        let observed = measured_bytes_per_voucher(
                            &read_evidence,
                            parsed.as_ref().map_or(0, Vec::len),
                        );
                        // Account for this part before anything below can refuse.
                        fold_evidence(&mut evidence, read_evidence);
                        rows.extend(parsed.map_err(ToolFailure::from)?);
                        reads.push(part.clone());
                        // Plan the rest at the book's own measured cost: the
                        // first measurement replaces the default, and a later,
                        // heavier part raises it. A lighter part never lowers
                        // it again, so one light part cannot loosen the plan.
                        let (Some(counted), Some(observed)) = (census.as_ref(), observed) else {
                            continue;
                        };
                        let next = if measured {
                            bytes_per_voucher.max(observed)
                        } else {
                            observed
                        };
                        measured = true;
                        if next == bytes_per_voucher {
                            continue;
                        }
                        bytes_per_voucher = next;
                        let end_day = parse_day(&part.to)?;
                        // The rest of the window starts after this part: inside
                        // the same day when this was a span short of the ceiling.
                        let (rest, resume_after) = match part.span {
                            Some(span) if span.through < ceiling => (end_day, Some(span.through)),
                            _ => match end_day.succ_opt() {
                                Some(next_day) => (next_day, None),
                                None => continue,
                            },
                        };
                        if rest > last {
                            continue;
                        }
                        let allowance = MAX_PLANNED_READS.saturating_sub(reads.len());
                        let plan = plan_window_reads(
                            rest,
                            last,
                            counted,
                            resume_after,
                            ceiling,
                            bytes_per_voucher,
                            limits.budget_bytes,
                            allowance,
                        )
                        .map_err(|refusal| ToolFailure::from(refusal.code().to_string()))?;
                        // `pending` tiles exactly what follows this part, so
                        // replacing it with a plan of the same stretch loses none.
                        pending = stack_of(&plan);
                    }
                    Err(failure) if shape.splits_on_oversize() && window_is_too_large(&failure) => {
                        // Record the span so sibling branches do not pay a
                        // deadline to learn the same thing. `min` because a
                        // later, smaller failure is the tighter bound.
                        if part.span.is_none() {
                            if let Some(span) = window_span_days(&part.from, &part.to) {
                                smallest_failed_days = Some(
                                    smallest_failed_days.map_or(span, |known| known.min(span)),
                                );
                            }
                        }
                        // A part that cannot be divided further is not something
                        // splitting can fix, and returning the parts that did work
                        // would be a read over an incomplete window. Refuse.
                        let (left, right) = halve_part(&part, census.as_ref(), ceiling)
                            .ok_or_else(|| {
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
            return Err(with_prior(failure, &preflight, &evidence));
        }
        // Close the bracket on a divided read. Only a divided read needs it: one
        // undivided request is one observation, exactly as before the bound.
        let divided = reads.len() > 1 || reads.iter().any(|part| part.span.is_some());
        if let (true, Some(opening)) = (divided, census_mark(&census, high_water)) {
            let closing = self
                .read_high_water(identity, company, &mut preflight)
                .await
                .map_err(|failure| with_prior(failure, &preflight, &evidence))?;
            if closing != Some(opening) {
                return Err(with_prior(
                    WINDOW_CHANGED_DURING_READ.to_string().into(),
                    &preflight,
                    &evidence,
                ));
            }
        }
        // Unreachable while every plan holds at least one part; kept total
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

    async fn read_high_water(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        preflight: &mut Option<Evidence>,
    ) -> Result<Option<u64>, ToolFailure> {
        let (xml, evidence) = self
            .post_read(identity, render_agent_company_high_water(company))
            .await?;
        fold_evidence(preflight, evidence);
        Ok(voucher_high_water(&xml, identity.company_guid()))
    }

    /// Establish what `window` holds, cheapest first: the company's voucher
    /// high-water mark bounds every window of the book at once; only when that
    /// bound is not enough is the window itself counted, by date first.
    #[allow(clippy::too_many_arguments)]
    async fn estimate_window_volume(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        (first, last): (NaiveDate, NaiveDate),
        known_high_water: Option<u64>,
        limits: WindowReadLimits,
        preflight: &mut Option<Evidence>,
        observed_high_water: &mut Option<u64>,
    ) -> Result<Preflight, ToolFailure> {
        let high_water = match known_high_water {
            Some(value) => value,
            None => self
                .read_high_water(identity, company, preflight)
                .await?
                .ok_or_else(|| ToolFailure::from(VOLUME_UNESTIMATED.to_string()))?,
        };
        *observed_high_water = Some(high_water);
        // Every voucher carries a distinct AlterID no greater than the high-water
        // mark (§10), so the book — and therefore any window of it — holds at
        // most `high_water` vouchers.
        if high_water.saturating_mul(limits.default_bytes_per_voucher) <= limits.budget_bytes {
            return Ok(Preflight::Whole);
        }
        let book_days = NaiveDate::parse_from_str(identity.books_from_yyyymmdd(), "%Y%m%d")
            .ok()
            .map_or(1, |books_from| (last - books_from).num_days() + 1)
            .max(1);
        let prior_density = high_water.div_ceil(u64::try_from(book_days).unwrap_or(1));
        let rows = self
            .census_window(
                identity,
                company,
                (first, last),
                high_water,
                prior_density,
                limits,
                preflight,
            )
            .await?;
        Ok(whole_or_counted(
            WindowCensus::from_rows(rows),
            Some(high_water),
            limits,
        ))
    }

    /// Count the window's vouchers per day, narrowing by date first.
    ///
    /// Each census covers a date range sized so its expected rows fit one read:
    /// at first from the book's average density (high-water mark over the days
    /// since the books began), then from the density the census has observed so
    /// far, times [`CENSUS_DENSITY_FACTOR`]. A short window on an ordinary book
    /// is therefore one census. A census the transport refuses as too large is
    /// halved by date; a single day that is still too large is counted in
    /// AlterID spans of the whole book, which bounds each by construction. A
    /// census that times out is not retried: the gateway may still be scanning.
    #[allow(clippy::too_many_arguments)]
    async fn census_window(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        (first, last): (NaiveDate, NaiveDate),
        high_water: u64,
        prior_density: u64,
        limits: WindowReadLimits,
        preflight: &mut Option<Evidence>,
    ) -> Result<Vec<(NaiveDate, u64)>, ToolFailure> {
        let capacity = limits.census_capacity();
        let unestimated = || ToolFailure::from(VOLUME_UNESTIMATED.to_string());
        let mut rows = Vec::new();
        let mut spent = 0_usize;
        let mut density = prior_density.max(1);
        let (mut counted_days, mut counted_rows) = (0_u64, 0_u64);
        // Pending census requests, in order. A span is only ever used for one day.
        let mut pending: Vec<(NaiveDate, NaiveDate, Option<AlterIdSpan>)> = Vec::new();
        let mut cursor = Some(first);
        loop {
            let (start, end, span) = match pending.pop() {
                Some(next) => next,
                None => {
                    let Some(start) = cursor.filter(|day| *day <= last) else {
                        break;
                    };
                    let days = (capacity / density).max(1);
                    let end = start
                        .checked_add_days(chrono::Days::new(days - 1))
                        .map_or(last, |end| end.min(last));
                    cursor = end.succ_opt();
                    (start, end, None)
                }
            };
            spent += 1;
            if spent > MAX_CENSUS_READS {
                return Err(unestimated());
            }
            let request = render_agent_voucher_census(company, &stamp(start), &stamp(end), span)?;
            let (xml, evidence) = match self.post_read(identity, request).await {
                Ok(read) => read,
                Err(failure) if failure.code == "response_size_limit_exceeded" => {
                    if let Some(((left_from, left_to), (right_from, right_to))) =
                        split_verification_window(&stamp(start), &stamp(end))
                    {
                        pending.push((parse_day(&right_from)?, parse_day(&right_to)?, None));
                        pending.push((parse_day(&left_from)?, parse_day(&left_to)?, None));
                    } else if span.is_none() {
                        // One day too dense for a date census: count it in
                        // AlterID spans, each bounded by construction.
                        let mut through = high_water;
                        while through > 0 {
                            let after = through.saturating_sub(capacity);
                            pending.push((start, end, Some(AlterIdSpan { after, through })));
                            through = after;
                        }
                    } else {
                        return Err(unestimated());
                    }
                    continue;
                }
                Err(failure) if is_window_too_large_code(&failure.code) => {
                    return Err(unestimated());
                }
                Err(failure) => return Err(failure),
            };
            fold_evidence(preflight, evidence);
            let counted = parse_voucher_census(&xml, (&stamp(start), &stamp(end)), span)
                .map_err(|_| unestimated())?;
            if span.is_none() {
                counted_days += u64::try_from((end - start).num_days() + 1).unwrap_or(1);
                counted_rows += counted.len() as u64;
                density = counted_rows
                    .div_ceil(counted_days.max(1))
                    .saturating_mul(CENSUS_DENSITY_FACTOR)
                    .max(1);
            }
            rows.extend(counted);
        }
        Ok(rows)
    }
}

fn census_mark(census: &Option<WindowCensus>, high_water: Option<u64>) -> Option<u64> {
    census.as_ref().and(high_water)
}

fn whole_or_counted(
    census: WindowCensus,
    high_water: Option<u64>,
    limits: WindowReadLimits,
) -> Preflight {
    if census
        .total()
        .saturating_mul(limits.default_bytes_per_voucher)
        <= limits.budget_bytes
    {
        Preflight::Whole
    } else {
        Preflight::Counted { census, high_water }
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
/// and the span it was asked for, if any,, or the census is not describing what was
/// asked and is refused.
pub(super) fn parse_voucher_census(
    xml: &str,
    window: (&str, &str),
    span: Option<AlterIdSpan>,
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
                    if day < first || day > last || span.is_some_and(|span| !span.holds(alter_id)) {
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
