use std::{collections::BTreeMap, fmt, sync::Arc};

use bridge_tally_primitives::{ExactDecimal, TallyDate};
use serde::Serialize;

use crate::xml_read_profiles::ValidatedCompanyName;

const MAX_NARROW_DATE_WINDOW_DAYS: usize = 31;

/// Compatibility rule for Tally period boundaries (guide §2.7 / I12). The
/// day 1/2/31 rule is observed only in Educational mode; unknown and licensed
/// modes rely on returned-span verification at the completeness boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateBoundaryProfile {
    EducationRestricted,
    ModeAgnostic,
}

impl DateBoundaryProfile {
    fn accepts_boundary(self, date: &TallyDate) -> bool {
        match self {
            Self::EducationRestricted => matches!(&date.as_str()[6..8], "01" | "02" | "31"),
            Self::ModeAgnostic => true,
        }
    }

    fn accepts_partition_end(self, date: &TallyDate) -> bool {
        match self {
            Self::EducationRestricted => matches!(&date.as_str()[6..8], "01" | "31"),
            Self::ModeAgnostic => true,
        }
    }

    /// The greatest date at or before `limit` that this profile accepts as a
    /// window boundary.
    ///
    /// A reporting window must not run past the as-of date. `LastVoucherDate`
    /// can be later than today when the book contains a future-dated voucher,
    /// and a window ending there makes `compute_outstandings` reject the whole
    /// read (as-of may not precede the window end) instead of simply excluding
    /// future activity. Clamping needs the profile because an Education
    /// boundary is only legal on day 01, 02 or 31.
    pub fn latest_boundary_at_or_before(self, limit: &TallyDate) -> Option<TallyDate> {
        match self {
            Self::ModeAgnostic => Some(limit.clone()),
            // An INEXACT clamp would silently shrink the scanned period while
            // the report still carries the caller's as-of date: on Education
            // with as-of on day 15, the cutoff would fall back to day 02 and
            // every posting from the 3rd onward would vanish from a report
            // labelled "as of the 15th". A wrong number under a confident label
            // is worse than no number, so accept the boundary only when it lands
            // exactly on the requested date and let the caller fail closed
            // otherwise.
            Self::EducationRestricted => self.accepts_boundary(limit).then(|| limit.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutstandingsError {
    InvalidDateWindow,
    InvalidAlterIdRange,
    InvalidCompanyIdentity,
    CompanyIdentityMismatch,
    InvalidResponse(&'static str),
    InvalidAmount,
    ArithmeticOverflow,
}

impl fmt::Display for OutstandingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidDateWindow => "outstandings date window is invalid",
            Self::InvalidAlterIdRange => "outstandings AlterID range is invalid",
            Self::InvalidCompanyIdentity => "outstandings company identity is invalid",
            Self::CompanyIdentityMismatch => "Tally returned a different company identity",
            Self::InvalidResponse(code) => code,
            Self::InvalidAmount => "Tally returned an invalid amount",
            Self::ArithmeticOverflow => "outstandings arithmetic exceeded the exact-decimal bound",
        })
    }
}

impl std::error::Error for OutstandingsError {}

/// The complete report period. It cannot be rendered directly as a segment
/// request; callers must first partition it into `NarrowDateWindow` values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateWindow {
    from: TallyDate,
    to: TallyDate,
    boundary_profile: DateBoundaryProfile,
}

impl DateWindow {
    pub fn parse(
        boundary_profile: DateBoundaryProfile,
        from: impl Into<String>,
        to: impl Into<String>,
    ) -> Result<Self, OutstandingsError> {
        let from =
            TallyDate::parse(from.into()).map_err(|_| OutstandingsError::InvalidDateWindow)?;
        let to = TallyDate::parse(to.into()).map_err(|_| OutstandingsError::InvalidDateWindow)?;
        if from > to
            || !boundary_profile.accepts_boundary(&from)
            || !boundary_profile.accepts_boundary(&to)
        {
            return Err(OutstandingsError::InvalidDateWindow);
        }
        Ok(Self {
            from,
            to,
            boundary_profile,
        })
    }

    pub fn from(&self) -> &TallyDate {
        &self.from
    }

    pub fn to(&self) -> &TallyDate {
        &self.to
    }

    /// Partition the report period into non-overlapping windows of at most 31
    /// calendar days. Educational profiles retain the verified day 1/2/31
    /// boundaries; other profiles permit ordinary calendar boundaries and
    /// rely on returned-span verification. Every next window starts on the
    /// day after the previous window ends.
    pub fn narrow_partitions(&self) -> Result<Vec<NarrowDateWindow>, OutstandingsError> {
        let mut partitions = Vec::new();
        let mut start = self.from.clone();
        loop {
            let mut cursor = start.clone();
            let mut last_splittable_end = (cursor == self.to).then(|| cursor.clone());
            for _ in 1..MAX_NARROW_DATE_WINDOW_DAYS {
                if cursor == self.to {
                    break;
                }
                let next = cursor
                    .next_day()
                    .map_err(|_| OutstandingsError::InvalidDateWindow)?;
                if next > self.to {
                    break;
                }
                cursor = next;
                if cursor == self.to {
                    last_splittable_end = Some(cursor.clone());
                    break;
                }
                if self.boundary_profile.accepts_partition_end(&cursor) {
                    last_splittable_end = Some(cursor.clone());
                }
            }
            let end = last_splittable_end.ok_or(OutstandingsError::InvalidDateWindow)?;
            let window = DateWindow {
                from: start.clone(),
                to: end.clone(),
                boundary_profile: self.boundary_profile,
            };
            partitions.push(NarrowDateWindow::try_from(window)?);
            if end == self.to {
                return Ok(partitions);
            }
            start = end
                .next_day()
                .map_err(|_| OutstandingsError::InvalidDateWindow)?;
            if !self.boundary_profile.accepts_boundary(&start) {
                return Err(OutstandingsError::InvalidDateWindow);
            }
        }
    }
}

/// A date predicate narrow enough to be admitted to the wildcard segment
/// request. It is constructible only from a validated report partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarrowDateWindow(DateWindow);

impl NarrowDateWindow {
    pub fn from(&self) -> &TallyDate {
        self.0.from()
    }

    pub fn to(&self) -> &TallyDate {
        self.0.to()
    }

    pub fn as_date_window(&self) -> &DateWindow {
        &self.0
    }

    pub fn into_date_window(self) -> DateWindow {
        self.0
    }
}

/// One or two independently requested date windows that jointly re-observe an
/// empty primary partition without exceeding the universal 31-day wire cap.
///
/// Every covered day is observed through a window *different* from the primary
/// window. Re-reading the identical primary window would reproduce its
/// false-empty route and is therefore not corroboration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrictlyWiderDateCover {
    primary: NarrowDateWindow,
    slices: Vec<NarrowDateWindow>,
}

impl StrictlyWiderDateCover {
    pub fn for_primary(primary: &NarrowDateWindow) -> Option<Self> {
        let candidates = witness_candidates(primary);

        if let Some(slice) = candidates
            .iter()
            .find(|slice| slice_covers_primary(slice, primary))
        {
            return Some(Self {
                primary: primary.clone(),
                slices: vec![slice.clone()],
            });
        }

        for (index, left) in candidates.iter().enumerate() {
            for right in candidates.iter().skip(index + 1) {
                if slices_cover_primary(left, right, primary) {
                    return Some(Self {
                        primary: primary.clone(),
                        slices: vec![left.clone(), right.clone()],
                    });
                }
            }
        }
        None
    }

    pub fn primary(&self) -> &NarrowDateWindow {
        &self.primary
    }

    pub fn slices(&self) -> &[NarrowDateWindow] {
        &self.slices
    }
}

fn witness_candidates(primary: &NarrowDateWindow) -> Vec<NarrowDateWindow> {
    let profile = primary.0.boundary_profile;
    let mut starts = Vec::new();
    let mut cursor = primary.from().clone();
    starts.push(cursor.clone());
    for _ in 1..MAX_NARROW_DATE_WINDOW_DAYS {
        let Ok(previous) = cursor.previous_day() else {
            break;
        };
        cursor = previous;
        starts.push(cursor.clone());
    }
    let mut cursor = primary.from().clone();
    while cursor < *primary.to() {
        let Ok(next) = cursor.next_day() else {
            break;
        };
        cursor = next;
        starts.push(cursor.clone());
    }

    let mut candidates = Vec::new();
    for start in starts {
        let mut end = start.clone();
        for _ in 0..MAX_NARROW_DATE_WINDOW_DAYS {
            if let Ok(window) = DateWindow::parse(profile, start.as_str(), end.as_str()) {
                if let Ok(window) = NarrowDateWindow::try_from(window) {
                    if window != *primary
                        && window.to() >= primary.from()
                        && window.from() <= primary.to()
                    {
                        candidates.push(window);
                    }
                }
            }
            let Ok(next) = end.next_day() else {
                break;
            };
            end = next;
        }
    }
    candidates.sort_by(|left, right| {
        left.from()
            .cmp(right.from())
            .then_with(|| left.to().cmp(right.to()))
    });
    candidates.dedup();
    candidates
}

fn slice_covers_primary(slice: &NarrowDateWindow, primary: &NarrowDateWindow) -> bool {
    slice.from() <= primary.from()
        && slice.to() >= primary.to()
        && slice != primary
        && (slice.from() < primary.from() || slice.to() > primary.to())
}

fn slices_cover_primary(
    left: &NarrowDateWindow,
    right: &NarrowDateWindow,
    primary: &NarrowDateWindow,
) -> bool {
    if left == primary || right == primary {
        return false;
    }
    let mut cursor = primary.from().clone();
    loop {
        if !((left.from() <= &cursor && &cursor <= left.to())
            || (right.from() <= &cursor && &cursor <= right.to()))
        {
            return false;
        }
        if cursor == *primary.to() {
            break;
        }
        let Ok(next) = cursor.next_day() else {
            return false;
        };
        cursor = next;
    }
    left.from() < primary.from()
        || left.to() > primary.to()
        || right.from() < primary.from()
        || right.to() > primary.to()
}

impl TryFrom<DateWindow> for NarrowDateWindow {
    type Error = OutstandingsError;

    fn try_from(window: DateWindow) -> Result<Self, Self::Error> {
        let mut cursor = window.from.clone();
        let mut days = 1_usize;
        while cursor < window.to {
            cursor = cursor
                .next_day()
                .map_err(|_| OutstandingsError::InvalidDateWindow)?;
            days += 1;
            if days > MAX_NARROW_DATE_WINDOW_DAYS {
                return Err(OutstandingsError::InvalidDateWindow);
            }
        }
        Ok(Self(window))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VoucherAlterId(u64);

impl VoucherAlterId {
    pub fn parse(value: &str) -> Result<Self, OutstandingsError> {
        let value = value
            .trim()
            .parse::<u64>()
            .map_err(|_| OutstandingsError::InvalidResponse("voucher_alter_id_invalid"))?;
        if value == 0 {
            return Err(OutstandingsError::InvalidResponse(
                "voucher_alter_id_invalid",
            ));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoucherAlterIdHighWater(u64);

impl VoucherAlterIdHighWater {
    pub fn parse(value: &str) -> Result<Self, OutstandingsError> {
        let value = value
            .trim()
            .parse::<u64>()
            .map_err(|_| OutstandingsError::InvalidResponse("company_altvchid_invalid"))?;
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

/// A server-side partition expressed as `$AlterID > start AND $AlterID <= end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlterIdRange {
    exclusive_start: u64,
    inclusive_end: u64,
}

impl AlterIdRange {
    pub fn new(exclusive_start: u64, inclusive_end: u64) -> Result<Self, OutstandingsError> {
        if exclusive_start >= inclusive_end {
            return Err(OutstandingsError::InvalidAlterIdRange);
        }
        Ok(Self {
            exclusive_start,
            inclusive_end,
        })
    }

    pub fn exclusive_start(self) -> u64 {
        self.exclusive_start
    }

    pub fn inclusive_end(self) -> u64 {
        self.inclusive_end
    }

    pub fn width(self) -> u64 {
        self.inclusive_end - self.exclusive_start
    }

    pub fn contains(self, alter_id: VoucherAlterId) -> bool {
        alter_id.get() > self.exclusive_start && alter_id.get() <= self.inclusive_end
    }

    pub fn is_adjacent_to(self, next: Self) -> bool {
        self.inclusive_end == next.exclusive_start
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PinnedCompany {
    name: ValidatedCompanyName,
    guid: Arc<str>,
}

impl PinnedCompany {
    pub(crate) fn verified(
        name: ValidatedCompanyName,
        guid: String,
    ) -> Result<Self, OutstandingsError> {
        if guid.trim() != guid
            || guid.is_empty()
            || guid.len() > 255
            || guid.chars().any(char::is_control)
        {
            return Err(OutstandingsError::InvalidCompanyIdentity);
        }
        Ok(Self {
            name,
            guid: Arc::from(guid),
        })
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn guid(&self) -> &str {
        &self.guid
    }
}

impl fmt::Debug for PinnedCompany {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PinnedCompany([verified identity])")
    }
}

/// Whether the book carries bill-wise OPENING balances on ledger masters.
///
/// Those bills exist without any voucher, so a voucher-only scan cannot see
/// them and must not claim complete outstandings when they are present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerOpeningCoverage {
    /// GUID-to-name identity from the coverage read. Equality is consumed by
    /// the runtime's paired-read check, so a rename cannot hide behind an
    /// unchanged ledger count.
    ledger_identities: BTreeMap<String, String>,
    bill_wise_openings: usize,
}

impl LedgerOpeningCoverage {
    pub(crate) fn new(
        ledger_identities: BTreeMap<String, String>,
        bill_wise_openings: usize,
    ) -> Self {
        Self {
            ledger_identities,
            bill_wise_openings,
        }
    }
    pub fn ledgers_seen(&self) -> usize {
        self.ledger_identities.len()
    }
    pub fn bill_wise_openings(&self) -> usize {
        self.bill_wise_openings
    }
    /// True when a voucher-only scan can still be complete.
    pub fn is_fully_covered_by_vouchers(&self) -> bool {
        self.bill_wise_openings == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyBookExtent {
    company: PinnedCompany,
    books_from: TallyDate,
    last_voucher_date: TallyDate,
    voucher_alter_id_high_water: Option<VoucherAlterIdHighWater>,
}

impl CompanyBookExtent {
    pub(crate) fn new(
        company: PinnedCompany,
        books_from: TallyDate,
        last_voucher_date: TallyDate,
        voucher_alter_id_high_water: Option<VoucherAlterIdHighWater>,
    ) -> Self {
        Self {
            company,
            books_from,
            last_voucher_date,
            voucher_alter_id_high_water,
        }
    }

    pub fn company(&self) -> &PinnedCompany {
        &self.company
    }
    pub fn books_from(&self) -> &TallyDate {
        &self.books_from
    }
    pub fn last_voucher_date(&self) -> &TallyDate {
        &self.last_voucher_date
    }
    pub fn voucher_alter_id_high_water(&self) -> Option<VoucherAlterIdHighWater> {
        self.voucher_alter_id_high_water
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoneyValue {
    Exact(ExactDecimal),
    Absent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BillReferenceKind {
    NewRef,
    AgstRef,
    Advance,
    OnAccount,
}

impl BillReferenceKind {
    pub(crate) fn parse(value: &str) -> Result<Self, OutstandingsError> {
        match value.trim() {
            "New Ref" => Ok(Self::NewRef),
            "Agst Ref" => Ok(Self::AgstRef),
            "Advance" => Ok(Self::Advance),
            "On Account" => Ok(Self::OnAccount),
            _ => Err(OutstandingsError::InvalidResponse(
                "bill_reference_kind_unknown",
            )),
        }
    }

    pub(crate) fn requires_named_reference(self) -> bool {
        !matches!(self, Self::OnAccount)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillAllocation {
    pub name: Option<String>,
    pub bill_type: BillReferenceKind,
    pub amount: MoneyValue,
    /// Tally's own date for the bill. Ageing must run from this when present:
    /// a bill's date can differ from the date of the voucher that opened it,
    /// and using the voucher date then puts the balance in the wrong bucket.
    pub bill_date: Option<TallyDate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    pub ledger_name: String,
    pub bill_allocations: Vec<BillAllocation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voucher {
    pub guid: String,
    pub master_id: String,
    pub alter_id: VoucherAlterId,
    pub date: TallyDate,
    pub voucher_type: String,
    pub voucher_number: Option<String>,
    pub party_ledger_name: Option<String>,
    pub cancelled: bool,
    pub deleted: bool,
    /// Optional vouchers are non-posting in Tally and must never reach
    /// ordinary-book totals. See `compute_outstandings`.
    pub optional: bool,
    pub ledger_entries: Vec<LedgerEntry>,
}

/// The only row shape admitted by the empty-date corroboration profile.
/// Keeping it distinct from `Voucher` prevents a three-field witness response
/// from ever reaching bill computation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WitnessVoucher {
    pub guid: String,
    pub alter_id: VoucherAlterId,
    pub date: TallyDate,
}

/// A byte-stable paired response from the date-only witness profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteWitnessPair {
    pub(crate) window: DateWindow,
    pub(crate) vouchers: Vec<WitnessVoucher>,
}

impl CompleteWitnessPair {
    pub fn window(&self) -> &DateWindow {
        &self.window
    }

    pub fn vouchers(&self) -> &[WitnessVoucher] {
        &self.vouchers
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum WitnessPairVerification {
    Complete(CompleteWitnessPair),
    Partial(PartialScan),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteSegment {
    pub(crate) reporting_window: DateWindow,
    pub(crate) alter_id_range: AlterIdRange,
    pub(crate) vouchers: Vec<Voucher>,
    pub(crate) encoded_bytes: usize,
}

impl CompleteSegment {
    pub fn reporting_window(&self) -> &DateWindow {
        &self.reporting_window
    }
    pub fn alter_id_range(&self) -> AlterIdRange {
        self.alter_id_range
    }
    pub fn vouchers(&self) -> &[Voucher] {
        &self.vouchers
    }
    pub fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }
}

/// Optional I5 evidence for a complete empty date partition. Construction
/// requires that partition's full AlterID tiling plus a paired, non-empty read
/// of a strictly wider date window with no row dated inside it.
#[derive(Debug, PartialEq, Eq)]
pub struct EmptyDateWindowWitness {
    pub(crate) empty_window: DateWindow,
    pub(crate) wider_window: DateWindow,
    pub(crate) observed_row_count: usize,
}

/// Records which non-empty primary partition established liveness immediately
/// before a particular empty partition's cover. This is retained in the typed
/// completion evidence rather than reconstructed from logs later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyPartitionControlProvenance {
    control_window: DateWindow,
    expected_row: WitnessVoucher,
    vouched_cover_slices: Vec<NarrowDateWindow>,
}

impl EmptyPartitionControlProvenance {
    pub(crate) fn new(
        control_window: DateWindow,
        expected_row: WitnessVoucher,
        vouched_cover_slices: Vec<NarrowDateWindow>,
    ) -> Self {
        Self {
            control_window,
            expected_row,
            vouched_cover_slices,
        }
    }

    pub fn control_window(&self) -> &DateWindow {
        &self.control_window
    }

    pub fn expected_row(&self) -> &WitnessVoucher {
        &self.expected_row
    }

    pub fn vouched_cover_slices(&self) -> &[NarrowDateWindow] {
        &self.vouched_cover_slices
    }
}

/// Mandatory I5 corroboration for a zero-row primary date partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyPartitionWitness {
    empty_window: DateWindow,
    control_provenance: EmptyPartitionControlProvenance,
}

impl EmptyPartitionWitness {
    pub(crate) fn new(
        empty_window: DateWindow,
        control_provenance: EmptyPartitionControlProvenance,
    ) -> Self {
        Self {
            empty_window,
            control_provenance,
        }
    }

    pub fn empty_window(&self) -> &DateWindow {
        &self.empty_window
    }

    pub fn control_provenance(&self) -> &EmptyPartitionControlProvenance {
        &self.control_provenance
    }
}

/// A partition that may enter final assembly. A zero-row `CompleteScan` has
/// no public path across this boundary unless its capped, date-shifted witness
/// and control provenance are present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorroboratedDatePartition {
    NonEmpty(CompleteScan),
    Empty {
        scan: CompleteScan,
        witness: EmptyPartitionWitness,
    },
    /// `ALTVCHID = 0` is the separately proven genuinely-empty-book case.
    /// It carries no same-profile control because none can exist.
    EmptyBook(CompleteScan),
}

impl CorroboratedDatePartition {
    pub fn non_empty(scan: CompleteScan) -> Result<Self, PartialScan> {
        if scan.vouchers.is_empty() {
            return Err(PartialScan::new("empty_date_partition_uncorroborated"));
        }
        Ok(Self::NonEmpty(scan))
    }

    pub(crate) fn empty(
        scan: CompleteScan,
        witness: EmptyPartitionWitness,
    ) -> Result<Self, PartialScan> {
        if !scan.vouchers.is_empty() || scan.reporting_window != witness.empty_window {
            return Err(PartialScan::new("empty_date_witness_scope_mismatch"));
        }
        Ok(Self::Empty { scan, witness })
    }

    pub fn empty_book(scan: CompleteScan) -> Result<Self, PartialScan> {
        if scan.voucher_alter_id_high_water.get() != 0 || !scan.vouchers.is_empty() {
            return Err(PartialScan::new("empty_book_partition_invalid"));
        }
        Ok(Self::EmptyBook(scan))
    }

    pub(crate) fn scan(&self) -> &CompleteScan {
        match self {
            Self::NonEmpty(scan) | Self::Empty { scan, .. } | Self::EmptyBook(scan) => scan,
        }
    }

    pub fn empty_witness(&self) -> Option<&EmptyPartitionWitness> {
        match self {
            Self::NonEmpty(_) | Self::EmptyBook(_) => None,
            Self::Empty { witness, .. } => Some(witness),
        }
    }
}

impl EmptyDateWindowWitness {
    pub fn empty_window(&self) -> &DateWindow {
        &self.empty_window
    }

    pub fn wider_window(&self) -> &DateWindow {
        &self.wider_window
    }

    pub fn observed_row_count(&self) -> usize {
        self.observed_row_count
    }
}

/// A scan proven complete from paired reads and contiguous segment coverage.
///
/// A partial scan is a different type and cannot cross this boundary:
///
/// ```compile_fail
/// use bridge_tally_protocol::outstandings::{CompleteScan, PartialScan};
/// fn consume(_: &CompleteScan) {}
/// let partial: PartialScan = todo!();
/// consume(&partial);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteScan {
    pub(crate) company: PinnedCompany,
    pub(crate) reporting_window: DateWindow,
    pub(crate) voucher_alter_id_high_water: VoucherAlterIdHighWater,
    pub(crate) vouchers: Vec<Voucher>,
    pub(crate) encoded_bytes: usize,
    /// Provenance for date partitions that were empty in the primary profile
    /// and therefore required an independently shaped witness request.
    pub(crate) empty_partition_witnesses: Vec<EmptyPartitionWitness>,
}

impl CompleteScan {
    pub fn company(&self) -> &PinnedCompany {
        &self.company
    }
    pub fn window(&self) -> &DateWindow {
        &self.reporting_window
    }
    pub fn voucher_alter_id_high_water(&self) -> VoucherAlterIdHighWater {
        self.voucher_alter_id_high_water
    }
    pub fn vouchers(&self) -> &[Voucher] {
        &self.vouchers
    }
    pub fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }
    pub fn empty_partition_witnesses(&self) -> &[EmptyPartitionWitness] {
        &self.empty_partition_witnesses
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartialScan {
    pub reason_code: String,
}

impl PartialScan {
    pub fn new(reason_code: impl Into<String>) -> Self {
        Self {
            reason_code: reason_code.into(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SegmentVerification {
    Complete(CompleteSegment),
    Partial(PartialScan),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EmptyDateWindowVerification {
    Complete(EmptyDateWindowWitness),
    Partial(PartialScan),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanResult {
    Complete(CompleteScan),
    Partial(PartialScan),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgeingBuckets {
    pub days_0_30: ExactDecimal,
    pub days_31_60: ExactDecimal,
    pub days_61_90: ExactDecimal,
    pub days_90_plus: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgeingBillCounts {
    pub days_0_30: usize,
    pub days_31_60: usize,
    pub days_61_90: usize,
    pub days_90_plus: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartyOutstanding {
    pub party: String,
    pub receivable: ExactDecimal,
    pub payable: ExactDecimal,
    pub outstanding_total: ExactDecimal,
    /// `None` means this party's open exposure is entirely On Account, which
    /// has no bill reference and therefore no truthful bill age.
    pub oldest_bill_age_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutstandingsReport {
    pub company_name: String,
    pub as_of_yyyymmdd: String,
    pub receivable_total: ExactDecimal,
    pub payable_total: ExactDecimal,
    /// At least one observed receivable On Account allocation is included in
    /// `receivable_total` but cannot be assigned a truthful bill age.
    pub has_unaged_receivable: bool,
    pub ageing: AgeingBuckets,
    pub open_receivable_bill_count: usize,
    pub ageing_bill_counts: AgeingBillCounts,
    pub top_parties: Vec<PartyOutstanding>,
    pub source_voucher_count: usize,
    pub source_bytes: usize,
}
