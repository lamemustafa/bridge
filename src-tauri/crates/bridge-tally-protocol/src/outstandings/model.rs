use std::{fmt, sync::Arc};

use bridge_tally_core::{ExactDecimal, TallyDate};
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

    pub fn joined_with(self, adjacent: Self) -> Result<Self, OutstandingsError> {
        if !self.is_adjacent_to(adjacent) {
            return Err(OutstandingsError::InvalidAlterIdRange);
        }
        Self::new(self.exclusive_start, adjacent.inclusive_end)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillAllocation {
    pub name: Option<String>,
    pub bill_type: String,
    pub amount: MoneyValue,
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
    pub ledger_entries: Vec<LedgerEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteSegment {
    pub(crate) reporting_window: DateWindow,
    pub(crate) alter_id_range: AlterIdRange,
    pub(crate) vouchers: Vec<Voucher>,
    pub(crate) encoded_bytes: usize,
}

/// A paired, byte-stable zero-row response that is not yet evidence of
/// absence. Only an adjacent wider read can promote it to a complete segment.
#[derive(Debug, PartialEq, Eq)]
pub struct EmptySegmentCandidate {
    pub(crate) reporting_window: DateWindow,
    pub(crate) alter_id_range: AlterIdRange,
    pub(crate) encoded_bytes: usize,
}

impl EmptySegmentCandidate {
    pub fn reporting_window(&self) -> &DateWindow {
        &self.reporting_window
    }

    pub fn alter_id_range(&self) -> AlterIdRange {
        self.alter_id_range
    }

    pub fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct CorroboratedEmptySegments {
    pub(crate) empty: CompleteSegment,
    pub(crate) adjacent: CompleteSegment,
}

impl CorroboratedEmptySegments {
    pub fn into_segments(self) -> [CompleteSegment; 2] {
        [self.empty, self.adjacent]
    }
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
    Empty(EmptySegmentCandidate),
    Partial(PartialScan),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EmptySegmentCorroboration {
    Complete(CorroboratedEmptySegments),
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
pub struct PartyOutstanding {
    pub party: String,
    pub receivable: ExactDecimal,
    pub payable: ExactDecimal,
    pub outstanding_total: ExactDecimal,
    pub oldest_bill_age_days: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutstandingsReport {
    pub company_name: String,
    pub as_of_yyyymmdd: String,
    pub receivable_total: ExactDecimal,
    pub payable_total: ExactDecimal,
    pub ageing: AgeingBuckets,
    pub top_parties: Vec<PartyOutstanding>,
    pub source_voucher_count: usize,
    pub source_bytes: usize,
}
