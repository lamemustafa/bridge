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

/// The most census requests one window read may spend. A book whose voucher
/// mark needs more AlterID spans than this to be counted is refused as
/// [`BOOK_TOO_LARGE`] before any census is sent: at 8,192 rows a span, a mark
/// above about 2.1 million. That is a product limit of the bounded read, not
/// an estimate; see [`census_spans`].
pub(super) const MAX_CENSUS_READS: usize = 256;

/// The most planned data reads one window read may spend before it is refused
/// as needing a narrower window. The same ceiling the outstandings scanner uses
/// for its own segment pairs.
pub(super) const MAX_PLANNED_READS: usize = 128;

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
/// heavy for one read; in a census it bounds each census request by
/// construction, since with distinct AlterIDs it cannot return more than
/// `through - after` rows (see [`census_spans`]).
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

/// A part returned vouchers that are not the ones its plan counted for it: a row
/// outside the part's dates or AlterID span, or a population that differs from
/// the census for that part. Either Tally did not honour the part's filter or the
/// book changed after it was counted; both refuse, because a union of parts is
/// only the window's vouchers if every part holds exactly its own.
pub(super) const PART_NOT_ADMITTED: &str = "voucher_window_part_not_admitted";

/// A replay of a divided read arrived without the witness of the read it
/// repeats. A replay of AlterID-limited parts reads only up to the first read's
/// ceilings, so without that read's marks it cannot see a voucher created above
/// them, and its responses can match the first read's exactly while both miss it.
pub(super) const REPLAY_UNWITNESSED: &str = "voucher_window_replay_unwitnessed";

/// The book's voucher mark needs more census spans than one read may spend.
/// Narrowing the window does not help — a census walks the book's AlterIDs
/// whatever the window — so this is a product limit, refused by name before any
/// census is sent.
pub(super) const BOOK_TOO_LARGE: &str = "voucher_window_book_too_large";

/// A company's two change marks (§10): `ALTVCHID` and `ALTMSTID`.
///
/// A divided read is bracketed on both. Measured live on TallyPrime 7.1 Silver
/// (2026-09-21): creating, altering, cancelling, re-dating and deleting a voucher
/// each advanced the voucher mark, but renaming a ledger advanced only the master
/// mark — while every voucher export naming that ledger changed. A read
/// bracketed on the voucher mark alone returned `complete` across a rename made
/// between two of its parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CompanyMarks {
    pub(super) vouchers: u64,
    pub(super) masters: u64,
}

/// What a divided read observed that a replay of it must still hold: the marks
/// it opened and closed on (equal, or it would have been refused), and the
/// census its parts were admitted against. A replay reads no census of its own;
/// it is admitted against this one and closes against these marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WindowWitness {
    pub(super) marks: CompanyMarks,
    pub(super) census: Option<WindowCensus>,
}

/// A measured per-voucher cost may replace the shape's default, but never falls
/// below half of it.
///
/// This floor is what makes the one margin a guarantee rather than a hope. A
/// part planned within the budget at a figure no lower than half the default
/// holds at most `2 × budget / default` vouchers; if none of them is heavier
/// than the default — which is set above the heaviest cost measured for the
/// shape (§11c.1) — the part is at most twice the budget, which is the cap.
/// Without it, one light first part (bank receipts at a tenth of an inventory
/// voucher's cost) would plan the next part at ten times its safe size.
/// The cost is more, smaller parts on a light book, never a refusal.
pub(super) fn planning_figure(default_bytes_per_voucher: u64, measured: u64) -> u64 {
    measured.max(default_bytes_per_voucher.div_ceil(MEASURED_FLOOR_DIVISOR))
}

/// See [`planning_figure`].
const MEASURED_FLOOR_DIVISOR: u64 = 2;

/// What a census does with a read Tally did not serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CensusFailure {
    /// The response was over the transport cap, or the read timed out. Every
    /// census is bounded before it is sent, so neither is divided or retried:
    /// an oversized census means the per-row figure was wrong, and a timed-out
    /// one may still be building on the gateway. The window is refused as
    /// unestimated.
    Refuse,
    /// Any other failure is the caller's, unchanged.
    Propagate,
}

pub(super) fn census_failure(code: &str) -> CensusFailure {
    if code == "response_size_limit_exceeded" || is_window_too_large_code(code) {
        CensusFailure::Refuse
    } else {
        CensusFailure::Propagate
    }
}

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
    /// The GUID counted with each AlterID, when the census carried one. Bridge's
    /// own census always does; a count built from `(day, AlterID)` alone does not,
    /// and is then admitted on AlterIDs only.
    guids: BTreeMap<u64, String>,
}

/// One voucher as a census counted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CensusRow {
    pub(super) day: NaiveDate,
    pub(super) alter_id: u64,
    pub(super) guid: String,
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
        Self {
            days,
            guids: BTreeMap::new(),
        }
    }

    /// A census as Bridge reads it, with each voucher's GUID. Refused when two
    /// rows claim one AlterID: AlterIDs are distinct within a company (§10), so
    /// such a count does not describe one state of the book.
    pub(super) fn from_census_rows(rows: impl IntoIterator<Item = CensusRow>) -> Option<Self> {
        let mut guids = BTreeMap::new();
        let mut days = Vec::new();
        for row in rows {
            if guids
                .insert(row.alter_id, row.guid.trim().to_ascii_lowercase())
                .is_some()
            {
                return None;
            }
            days.push((row.day, row.alter_id));
        }
        let mut census = Self::from_rows(days);
        census.guids = guids;
        Some(census)
    }

    /// The vouchers counted in `[from, to]` and inside `span`, if any, as
    /// AlterID to GUID (when counted).
    fn population(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        span: Option<AlterIdSpan>,
    ) -> BTreeMap<u64, Option<&str>> {
        self.days
            .range(from..=to)
            .flat_map(|(_, ids)| ids.iter().copied())
            .filter(|id| span.is_none_or(|span| span.holds(*id)))
            .map(|id| (id, self.guids.get(&id).map(String::as_str)))
            .collect()
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
    /// Estimate the window's volume and plan it. `known_marks` are the company's
    /// marks the caller has just read itself, which saves reading them again;
    /// they open the bracket exactly as a mark read here would.
    Estimate { known_marks: Option<CompanyMarks> },
    /// Plan from a count the caller already holds for exactly this window, and
    /// send no census. See [`WindowCensus`]. No high-water mark is read, so a
    /// divided read is not bracketed: the caller answers for the count being
    /// current. No production caller holds such a count yet; this is the
    /// interface one would use, proven by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    Counted(WindowCensus),
    /// Read exactly these parts again, as a corroborating second read of a
    /// window already planned and read once. A replay of a divided read must
    /// carry that read's [`WindowWitness`]: its parts are admitted against the
    /// witness's census and it closes against the witness's marks, so a voucher
    /// created above the first read's ceilings refuses the replay instead of
    /// being missed by both reads alike. Without one it is refused unread.
    Replay {
        parts: Vec<WindowPart>,
        witness: Option<WindowWitness>,
    },
}

impl WindowPlanSource {
    /// The corroborating replay of a read: exactly its parts, with its witness.
    pub(super) fn replay_of(parts: Vec<WindowPart>, witness: Option<WindowWitness>) -> Self {
        Self::Replay { parts, witness }
    }
}

/// What one window read cost Tally, for a caller that must later send the same
/// window as one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WindowServed {
    pub(super) divided: bool,
    /// Tally refused one of the read's data requests as too large or timed out.
    pub(super) refused_a_part: bool,
    /// One copy of every data response, summed: `post_read` reports both bodies
    /// of a paired read, so half of its evidence bytes.
    pub(super) data_bytes: u64,
}

impl WindowServed {
    pub(super) fn of(reads: &[WindowPart], data: &Evidence, refused_a_part: bool) -> Self {
        Self {
            divided: is_divided(reads),
            refused_a_part,
            data_bytes: u64::try_from(data.bytes / 2).unwrap_or(u64::MAX),
        }
    }

    /// Whether one request of the whole window is within the budget, on what the
    /// read measured rather than on the pre-flight's prediction: an undivided
    /// read was that request, and a divided read's parts together were the
    /// window's whole response — unless Tally refused one of the read's
    /// requests as too large or timed out. A request that timed out has no
    /// size to sum, and one Tally refused may be the whole window itself, so
    /// such a read never admits the whole request.
    pub(super) fn fits_one_request(self) -> bool {
        !self.refused_a_part && (!self.divided || self.data_bytes <= WINDOW_READ_BUDGET_BYTES)
    }
}

/// The per-call limits a window read plans under.
#[derive(Clone, Copy, Debug)]
pub(super) struct WindowReadLimits {
    pub(super) budget_bytes: u64,
    pub(super) default_bytes_per_voucher: u64,
    /// The most data requests one read may dispatch. [`MAX_PLANNED_READS`] in
    /// production.
    pub(super) max_reads: usize,
}

impl WindowReadLimits {
    pub(super) const fn for_shape(shape: VoucherReadShape) -> Self {
        Self {
            budget_bytes: WINDOW_READ_BUDGET_BYTES,
            default_bytes_per_voucher: shape.default_wire_bytes_per_voucher(),
            max_reads: MAX_PLANNED_READS,
        }
    }

    /// Rows one census request may be asked for. A census is bounded by
    /// construction — an AlterID span of this width cannot return more rows —
    /// so it is sized against the whole transport cap (twice the budget), not
    /// the half-budget that absorbs the error in an estimated data part. Its
    /// only uncertainty is the per-row figure: 4 KiB is assumed against 2.42 to
    /// 2.72 KB measured live (§11c.5), so a full census is about 22 MB.
    pub(super) fn census_capacity(self) -> u64 {
        vouchers_per_read(
            self.budget_bytes.saturating_mul(2),
            CENSUS_WIRE_BYTES_PER_VOUCHER,
        )
        .max(1)
    }
}

/// The identity of one voucher row as a window read admits it. Every read shape
/// Bridge sends fetches `DATE`, `GUID`, `ALTERID` and `MASTERID`.
pub(super) trait WindowRow {
    fn window_date(&self) -> Option<&str>;
    fn window_alter_id(&self) -> Option<u64>;
    fn window_guid(&self) -> Option<&str>;
    /// `Err` when a master ID is present but not a number.
    fn window_master_id(&self) -> Result<Option<u64>, String>;
}

impl WindowRow for Value {
    fn window_date(&self) -> Option<&str> {
        self["date"].as_str()
    }
    fn window_alter_id(&self) -> Option<u64> {
        self["alter_id"].as_u64()
    }
    fn window_guid(&self) -> Option<&str> {
        self["guid"].as_str()
    }
    fn window_master_id(&self) -> Result<Option<u64>, String> {
        master_id_of(match &self["master_id"] {
            Value::String(value) => Some(value.as_str()),
            Value::Number(value) => return Ok(value.as_u64()),
            _ => None,
        })
    }
}

/// A master ID as Tally renders it, padded with a leading space.
pub(super) fn master_id_of(value: Option<&str>) -> Result<Option<u64>, String> {
    value
        .map(|value| {
            value
                .trim()
                .parse::<u64>()
                .map_err(|_| "voucher_master_id_invalid".to_string())
        })
        .transpose()
}

pub(super) struct WindowReadOutcome<T> {
    pub(super) rows: Vec<T>,
    /// The data reads alone, folded in order.
    pub(super) evidence: Evidence,
    /// The opening pre-flight reads (the marks and the census), when any were
    /// sent. Kept apart so that a proof committing to the data read does not
    /// silently start committing to a planning estimate.
    pub(super) preflight_evidence: Option<Evidence>,
    /// The closing bracket, read after the last data part. Kept apart from the
    /// opening reads so that every fold of this read's evidence can keep the
    /// order the requests were sent in.
    pub(super) closing_evidence: Option<Evidence>,
    /// The parts actually read, in order.
    pub(super) reads: Vec<WindowPart>,
    /// What a replay of this read must carry. `None` only when no marks were
    /// read, which is the caller-counted source alone.
    pub(super) witness: Option<WindowWitness>,
    /// Whether Tally refused a data request of this read as too large or timed
    /// out, and the read went on in smaller parts. The parts' sizes then say
    /// nothing about the request Tally refused.
    pub(super) refused_a_part: bool,
}

impl<T> WindowReadOutcome<T> {
    /// Every read this window cost, in the order it was sent: the opening
    /// pre-flight, the data parts, then the closing bracket.
    pub(super) fn all_evidence(&self) -> Evidence {
        chronological(
            &self.preflight_evidence,
            &Some(self.evidence.clone()),
            &self.closing_evidence,
        )
        .unwrap_or_else(|| self.evidence.clone())
    }
}

/// Fold opening, data and closing evidence in the order the requests were sent.
fn chronological(
    opening: &Option<Evidence>,
    data: &Option<Evidence>,
    closing: &Option<Evidence>,
) -> Option<Evidence> {
    [opening, data, closing]
        .into_iter()
        .flatten()
        .cloned()
        .reduce(combine_evidence)
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

/// A failure carrying every read made before it, in the order sent.
fn with_prior(
    failure: ToolFailure,
    preflight: &Option<Evidence>,
    data: &Option<Evidence>,
) -> ToolFailure {
    with_prior_closed(failure, preflight, data, &None)
}

/// [`with_prior`] for a failure at or after the closing bracket.
fn with_prior_closed(
    failure: ToolFailure,
    preflight: &Option<Evidence>,
    data: &Option<Evidence>,
    closing: &Option<Evidence>,
) -> ToolFailure {
    match chronological(preflight, data, closing) {
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
        /// The marks the census was bounded by, when they were read.
        marks: Option<CompanyMarks>,
    },
}

impl Server {
    /// Read a voucher window of `shape`, divided so that no request is
    /// predicted over the budget (protocol reference §11c).
    ///
    /// `parse` turns one response into rows. Rows from every part are returned
    /// together in order, so a caller sees what one undivided read would have
    /// produced.
    ///
    /// A divided read is admitted part by part and as a whole:
    ///
    /// - every row of a part must lie in that part's dates and AlterID span, and
    ///   when the window was counted, a part's vouchers must be exactly the ones
    ///   the census counted for it ([`PART_NOT_ADMITTED`]);
    /// - GUIDs and master IDs must be unique across the union of parts, not
    ///   only within each response;
    /// - it is bracketed: both company marks are read again after the last part,
    ///   and marks that moved refuse the read. Parts read at different moments
    ///   describe one state of the book only if nothing changed between them,
    ///   and a day read in AlterID spans covers only the AlterIDs that existed
    ///   when it was counted.
    ///
    /// Every data request counts against the read allowance when it is
    /// dispatched, including a part divided after Tally could not serve it.
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
        T: WindowRow,
        P: FnMut(&str) -> Result<Vec<T>, String>,
    {
        let first = parse_day(from)?;
        let last = parse_day(to)?;
        if first > last {
            return Err("invalid_date_range".to_string().into());
        }
        let mut preflight = None;
        let mut opening: Option<CompanyMarks> = None;
        let mut census = None;
        let mut ceiling = 0;
        let mut bytes_per_voucher = limits.default_bytes_per_voucher;
        // A stack whose pops are in order and whose parts always tile the part
        // of the window not yet read, so the rows arrive in order.
        // A replay reads exactly the parts it was given: it never re-plans from
        // a measurement, which could merge parts back into a request the first
        // read already found Tally could not serve.
        let replaying = matches!(source, WindowPlanSource::Replay { .. });
        let mut pending: Vec<WindowPart> = match source {
            WindowPlanSource::Replay { parts, witness } => {
                for part in &parts {
                    let (part_from, part_to) = (parse_day(&part.from)?, parse_day(&part.to)?);
                    if part_from > part_to {
                        return Err("invalid_date_range".to_string().into());
                    }
                    if part_from < first || part_to > last {
                        return Err("window_not_honoured".to_string().into());
                    }
                }
                match witness {
                    Some(witness) => {
                        opening = Some(witness.marks);
                        ceiling = witness.marks.vouchers.max(
                            witness
                                .census
                                .as_ref()
                                .map_or(0, WindowCensus::max_alter_id),
                        );
                        census = witness.census;
                    }
                    None if is_divided(&parts) => {
                        return Err(REPLAY_UNWITNESSED.to_string().into());
                    }
                    None => {}
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
                    WindowPlanSource::Estimate { known_marks } => self
                        .estimate_window_volume(
                            identity,
                            company,
                            (first, last),
                            known_marks,
                            limits,
                            &mut preflight,
                            &mut opening,
                        )
                        .await
                        .map_err(|failure| with_prior(failure, &preflight, &None))?,
                    WindowPlanSource::Replay { .. } => unreachable!("handled above"),
                };
                match estimate {
                    Preflight::Whole => vec![WindowPart {
                        from: from.to_string(),
                        to: to.to_string(),
                        span: None,
                    }],
                    Preflight::Counted {
                        census: counted,
                        marks,
                    } => {
                        ceiling = marks
                            .map_or(0, |marks| marks.vouchers)
                            .max(counted.max_alter_id());
                        let plan = plan_window_reads(
                            first,
                            last,
                            &counted,
                            None,
                            ceiling,
                            bytes_per_voucher,
                            limits.budget_bytes,
                            limits.max_reads,
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
        let mut dispatched = 0_usize;
        let mut refused_a_part = false;
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
                // The allowance is spent when a request is dispatched, not when a
                // plan is made: a part divided after Tally could not serve it, or
                // split because a sibling could not be served, costs a request
                // no plan counted.
                if dispatched >= limits.max_reads {
                    return Err(PlanRefusal::TooManyReads {
                        reads: dispatched + 1 + pending.len(),
                    }
                    .code()
                    .to_string()
                    .into());
                }
                dispatched += 1;
                let request =
                    voucher_window_part_read(shape, company, &part.from, &part.to, part.span)?;
                match self.post_read(identity, request).await {
                    Ok((xml, read_evidence)) => {
                        let parsed = parse(&xml);
                        let observed = measured_bytes_per_voucher(
                            &read_evidence,
                            parsed.as_ref().map_or(0, Vec::len),
                        );
                        // Account for this part before anything below can refuse.
                        fold_evidence(&mut evidence, read_evidence);
                        let parsed = parsed.map_err(ToolFailure::from)?;
                        admit_part(&part, (first, last), &parsed, census.as_ref())?;
                        rows.extend(parsed);
                        reads.push(part.clone());
                        // Plan the rest at the book's own measured cost: the
                        // first measurement replaces the default, and a later,
                        // heavier part raises it. A lighter part never lowers
                        // it again, so one light part cannot loosen the plan.
                        if replaying {
                            continue;
                        }
                        let (Some(counted), Some(observed)) = (census.as_ref(), observed) else {
                            continue;
                        };
                        let next = if measured {
                            bytes_per_voucher.max(observed)
                        } else {
                            planning_figure(limits.default_bytes_per_voucher, observed)
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
                        let allowance = limits.max_reads.saturating_sub(dispatched);
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
                        refused_a_part = true;
                        // The failed attempt was a request; keep what it observed.
                        if let Some(attempt) = failure.evidence.clone() {
                            fold_evidence(&mut evidence, *attempt);
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
        // The union of parts must hold each voucher once. Each response is
        // admitted on its own by its parser; only here can a voucher returned by
        // two parts — re-dated between them, or served by a filter Tally did not
        // honour — be seen.
        //
        // Import verification admits its own union under its own refusal code
        // (`ImportReadSource::admit`), which is kept rather than preempted here.
        if shape != VoucherReadShape::ImportVerification {
            admit_union(&rows).map_err(|code| with_prior(code.into(), &preflight, &evidence))?;
        }
        // Close the bracket on a divided read, whether it was planned divided,
        // divided after Tally could not serve a part, or replayed. One undivided
        // request is one observation, exactly as before the bound.
        let mut closing = None;
        if let (true, Some(opened)) = (is_divided(&reads), opening) {
            let closed = self
                .read_marks(identity, company, &mut closing)
                .await
                .map_err(|failure| with_prior_closed(failure, &preflight, &evidence, &closing))?;
            if closed != opened {
                return Err(with_prior_closed(
                    WINDOW_CHANGED_DURING_READ.to_string().into(),
                    &preflight,
                    &evidence,
                    &closing,
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
            closing_evidence: closing,
            reads,
            witness: opening.map(|marks| WindowWitness { marks, census }),
            refused_a_part,
        })
    }

    async fn read_marks(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        evidence: &mut Option<Evidence>,
    ) -> Result<CompanyMarks, ToolFailure> {
        let (xml, read) = self
            .post_read(identity, company_high_water_read(company))
            .await?;
        fold_evidence(evidence, read);
        Ok(company_marks(&xml, identity.company_guid())?)
    }

    /// Establish what `window` holds, cheapest first: the company's voucher
    /// high-water mark bounds every window of the book at once; only when that
    /// bound is not enough is the window itself counted.
    #[allow(clippy::too_many_arguments)]
    async fn estimate_window_volume(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        (first, last): (NaiveDate, NaiveDate),
        known_marks: Option<CompanyMarks>,
        limits: WindowReadLimits,
        preflight: &mut Option<Evidence>,
        observed_marks: &mut Option<CompanyMarks>,
    ) -> Result<Preflight, ToolFailure> {
        let marks = match known_marks {
            Some(marks) => marks,
            None => self.read_marks(identity, company, preflight).await?,
        };
        *observed_marks = Some(marks);
        let high_water = marks.vouchers;
        // Every voucher carries a distinct AlterID no greater than the high-water
        // mark (§10), so the book — and therefore any window of it — holds at
        // most `high_water` vouchers.
        if high_water.saturating_mul(limits.default_bytes_per_voucher) <= limits.budget_bytes {
            return Ok(Preflight::Whole);
        }
        let rows = self
            .census_window(
                identity,
                company,
                (first, last),
                high_water,
                limits,
                preflight,
            )
            .await?;
        let census = WindowCensus::from_census_rows(rows)
            .ok_or_else(|| ToolFailure::from(VOLUME_UNESTIMATED.to_string()))?;
        Ok(whole_or_counted(census, Some(marks), limits))
    }

    /// Count the window's vouchers per day, every census request bounded
    /// before it is sent. See [`census_spans`].
    async fn census_window(
        &self,
        identity: &VerifiedCompanyIdentity,
        company: &str,
        (first, last): (NaiveDate, NaiveDate),
        high_water: u64,
        limits: WindowReadLimits,
        preflight: &mut Option<Evidence>,
    ) -> Result<Vec<CensusRow>, ToolFailure> {
        let unestimated = || ToolFailure::from(VOLUME_UNESTIMATED.to_string());
        let spans = census_spans(high_water, limits.census_capacity())
            .map_err(|code| ToolFailure::from(code.to_string()))?;
        let (from, to) = (stamp(first), stamp(last));
        let mut rows = Vec::new();
        for span in spans {
            let request = voucher_census_read(company, &from, &to, span)?;
            let (xml, evidence) = match self.post_read(identity, request).await {
                Ok(read) => read,
                Err(failure) if census_failure(&failure.code) == CensusFailure::Refuse => {
                    // A census bounded by construction that Tally still could not
                    // serve is not divided further or retried: the gateway may
                    // still be building it. Keep what the attempt observed, and
                    // which of the two it was.
                    let mut refused = unestimated();
                    refused.cause = census_refusal_cause(&failure.code);
                    refused.evidence = failure.evidence;
                    return Err(refused);
                }
                Err(failure) => return Err(failure),
            };
            fold_evidence(preflight, evidence);
            rows.extend(
                parse_voucher_census(&xml, (&from, &to), span).map_err(|code| {
                    let mut refused = unestimated();
                    refused.cause = census_refusal_cause(&code);
                    refused
                })?,
            );
        }
        Ok(rows)
    }
}

/// Why a census refused its window as unestimated, as a data-free cause beside
/// [`VOLUME_UNESTIMATED`].
fn census_refusal_cause(code: &str) -> Option<&'static str> {
    match code {
        "window_not_honoured" => Some("census_window_not_honoured"),
        "agent_read_protocol_invalid" => Some("census_protocol_invalid"),
        "response_size_limit_exceeded" => Some("census_response_too_large"),
        _ if is_window_too_large_code(code) => Some("census_deadline_exceeded"),
        _ => None,
    }
}

/// Whether a set of parts is a divided read: more than one request, or a part
/// limited to an AlterID span.
fn is_divided(parts: &[WindowPart]) -> bool {
    parts.len() > 1 || parts.iter().any(|part| part.span.is_some())
}

/// The census requests for a book whose voucher mark is `high_water`, each with
/// a cardinality bound fixed before it is sent (§11c.3).
///
/// A date census alone is bounded only by the whole book: a date range can
/// hold every voucher the book has. So a book whose mark fits one census is
/// counted in one date census of the window, bounded by the mark itself; any
/// larger book is counted in AlterID spans of `capacity` across `(0, mark]`,
/// each also narrowed to the window's dates, and each bounded by construction
/// because AlterIDs are distinct. The spans are produced one at a time, and a
/// mark needing more of them than [`MAX_CENSUS_READS`] is refused before any is.
///
/// The cost is a census count proportional to the book, not the window: a
/// mark of 250,000 is 31 census requests however short the window. That is a
/// cost in elapsed time, not in gateway safety: every request stays bounded,
/// so a caller that abandons the read leaves at most one bounded request in
/// Tally. A request that returns only a count per range would avoid the walk,
/// but no such shape is qualified live.
pub(super) fn census_spans(high_water: u64, capacity: u64) -> Result<CensusSpans, &'static str> {
    let capacity = capacity.max(1);
    let requests = high_water.div_ceil(capacity).max(1);
    if requests > MAX_CENSUS_READS as u64 {
        return Err(BOOK_TOO_LARGE);
    }
    Ok(CensusSpans {
        whole: high_water <= capacity,
        after: 0,
        high_water,
        capacity,
        done: false,
    })
}

/// See [`census_spans`].
pub(super) struct CensusSpans {
    whole: bool,
    after: u64,
    high_water: u64,
    capacity: u64,
    done: bool,
}

impl Iterator for CensusSpans {
    type Item = Option<AlterIdSpan>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if self.whole {
            self.done = true;
            return Some(None);
        }
        let through = self
            .after
            .saturating_add(self.capacity)
            .min(self.high_water);
        let span = AlterIdSpan {
            after: self.after,
            through,
        };
        self.after = through;
        self.done = through >= self.high_water;
        Some(Some(span))
    }
}

/// Admit one part's rows against the part and, when the window was counted,
/// against the census. See [`PART_NOT_ADMITTED`].
fn admit_part<T: WindowRow>(
    part: &WindowPart,
    (first, last): (NaiveDate, NaiveDate),
    rows: &[T],
    census: Option<&WindowCensus>,
) -> Result<(), ToolFailure> {
    let whole_window = part.span.is_none() && census.is_none() && {
        parse_day(&part.from)? == first && parse_day(&part.to)? == last
    };
    // An undivided read is admitted by its caller against the window, exactly
    // as before the bound; nothing here narrows it further.
    if whole_window {
        return Ok(());
    }
    let refused = || ToolFailure::from(PART_NOT_ADMITTED.to_string());
    let (from, to) = (parse_day(&part.from)?, parse_day(&part.to)?);
    let mut observed = BTreeMap::new();
    for row in rows {
        let day = row
            .window_date()
            .and_then(|value| NaiveDate::parse_from_str(value.trim(), "%Y%m%d").ok())
            .ok_or_else(refused)?;
        if day < from || day > to {
            return Err(refused());
        }
        let alter_id = row.window_alter_id();
        if let Some(span) = part.span {
            if !alter_id.is_some_and(|id| span.holds(id)) {
                return Err(refused());
            }
        }
        if census.is_some() {
            let alter_id = alter_id.ok_or_else(refused)?;
            let guid = row
                .window_guid()
                .map(|guid| guid.trim().to_ascii_lowercase());
            if observed.insert(alter_id, guid).is_some() {
                return Err(refused());
            }
        }
    }
    let Some(census) = census else {
        return Ok(());
    };
    let expected = census.population(from, to, part.span);
    let matches = expected.len() == observed.len()
        && expected.iter().all(|(id, counted)| {
            observed.get(id).is_some_and(|guid| match counted {
                Some(counted) => guid.as_deref() == Some(*counted),
                None => true,
            })
        });
    if matches {
        Ok(())
    } else {
        Err(refused())
    }
}

/// GUIDs and master IDs must be unique across every part of a window.
fn admit_union<T: WindowRow>(rows: &[T]) -> Result<(), String> {
    let mut identities = VoucherSourceIdentities::default();
    for row in rows {
        identities.admit(row.window_guid(), row.window_master_id()?)?;
    }
    Ok(())
}

fn whole_or_counted(
    census: WindowCensus,
    marks: Option<CompanyMarks>,
    limits: WindowReadLimits,
) -> Preflight {
    if census
        .total()
        .saturating_mul(limits.default_bytes_per_voucher)
        <= limits.budget_bytes
    {
        Preflight::Whole
    } else {
        Preflight::Counted { census, marks }
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

/// The company's marks, with a company that has never held a voucher read as a
/// voucher mark of zero (Tally omits `ALTVCHID` for it; see
/// `pre_import_mark_refusal`). A response the marks cannot be read from is
/// refused with the parser's own code — never read as an empty book, and never
/// folded into a planning or bracket outcome that would hide which it was.
fn company_marks(xml: &str, company_guid: &str) -> Result<CompanyMarks, String> {
    parse_company_marks(xml, company_guid)
        .map(|(vouchers, masters)| CompanyMarks { vouchers, masters })
}

/// Parse a census response into a [`CensusRow`] per voucher.
///
/// Counts `VOUCHER` start elements inside `BODY/DATA/COLLECTION` only, and
/// takes only the two direct child fields it needs: §12.7 records an empty
/// response whose `CMPINFO` carries a bare `<VOUCHER>0</VOUCHER>`, which a
/// document-wide count reads as a row. Every row must fall inside the window
/// and the span it was asked for, if any, and carry its GUID, or the census is
/// not describing what was asked and is refused.
pub(super) fn parse_voucher_census(
    xml: &str,
    window: (&str, &str),
    span: Option<AlterIdSpan>,
) -> Result<Vec<CensusRow>, String> {
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
    let wanted = |name: &str| matches!(name, "DATE" | "ALTERID" | "GUID");
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
                // pushed, so only the voucher's own DATE, ALTERID and GUID are
                // claimed.
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
                        // AlterID 0 is below every span's exclusive lower bound,
                        // so a day divided by AlterID could never read it. No
                        // such voucher has been observed; refuse rather than
                        // plan around one.
                        .filter(|alter_id| *alter_id > 0)
                        .ok_or_else(invalid)?;
                    let guid = row
                        .get("GUID")
                        .map(|guid| guid.trim().to_string())
                        .filter(|guid| !guid.is_empty())
                        .ok_or_else(invalid)?;
                    if day < first || day > last || span.is_some_and(|span| !span.holds(alter_id)) {
                        return Err("window_not_honoured".to_string());
                    }
                    rows.push(CensusRow {
                        day,
                        alter_id,
                        guid,
                    });
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
