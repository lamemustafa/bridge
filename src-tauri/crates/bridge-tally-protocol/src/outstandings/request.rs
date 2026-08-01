use super::{AlterIdRange, NarrowDateWindow, PinnedCompany};

#[derive(Clone, Copy)]
enum CollectionName {
    VoucherOutstandingsV1,
    VoucherEmptyPartitionWitnessV1,
    CompanyBookExtentV1,
    LedgerOpeningCoverageV1,
}

impl CollectionName {
    const fn as_str(self) -> &'static str {
        match self {
            Self::VoucherOutstandingsV1 => "BridgeVoucherOutstandingsV1",
            Self::VoucherEmptyPartitionWitnessV1 => "BridgeVoucherEmptyPartitionWitnessV1",
            Self::CompanyBookExtentV1 => "BridgeCompanyBookExtentV1",
            Self::LedgerOpeningCoverageV1 => "BridgeLedgerOpeningCoverageV1",
        }
    }
}

#[derive(Clone, Copy)]
enum ObjectType {
    Voucher,
    Company,
    Ledger,
}

impl ObjectType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Voucher => "Voucher",
            Self::Company => "Company",
            Self::Ledger => "Ledger",
        }
    }
}

#[derive(Clone, Copy)]
enum FilterName {
    OutstandingsPartitionV1,
    EmptyPartitionWitnessDateV1,
}

impl FilterName {
    const fn as_str(self) -> &'static str {
        match self {
            Self::OutstandingsPartitionV1 => "BridgeOutstandingsPartitionV1",
            Self::EmptyPartitionWitnessDateV1 => "BridgeEmptyPartitionWitnessDateV1",
        }
    }
}

#[derive(Clone, Copy)]
enum VoucherFetchField {
    Guid,
    MasterId,
    AlterId,
    Date,
    VoucherTypeName,
    VoucherNumber,
    PartyLedgerName,
    IsCancelled,
    IsDeleted,
    IsOptional,
    /// The single wildcard exception verified for bill-level outstandings.
    /// This closed variant cannot be reused for another collection or path.
    AllLedgerEntriesWildcard,
}

impl VoucherFetchField {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Guid => "GUID",
            Self::MasterId => "MASTERID",
            Self::AlterId => "ALTERID",
            Self::Date => "DATE",
            Self::VoucherTypeName => "VOUCHERTYPENAME",
            Self::VoucherNumber => "VOUCHERNUMBER",
            Self::PartyLedgerName => "PARTYLEDGERNAME",
            Self::IsCancelled => "ISCANCELLED",
            Self::IsDeleted => "ISDELETED",
            Self::IsOptional => "ISOPTIONAL",
            Self::AllLedgerEntriesWildcard => "ALLLEDGERENTRIES.*",
        }
    }
}

#[derive(Clone, Copy)]
enum CompanyFetchField {
    Name,
    Guid,
    BooksFrom,
    LastVoucherDate,
    AlterVoucherId,
}

impl CompanyFetchField {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Guid => "GUID",
            Self::BooksFrom => "BooksFrom",
            Self::LastVoucherDate => "LastVoucherDate",
            Self::AlterVoucherId => "ALTVCHID",
        }
    }
}

#[derive(Clone, Copy)]
enum LedgerFetchField {
    Guid,
    Name,
    IsBillWiseOn,
    OpeningBalance,
}

impl LedgerFetchField {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Guid => "GUID",
            Self::Name => "Name",
            Self::IsBillWiseOn => "ISBILLWISEON",
            Self::OpeningBalance => "OPENINGBALANCE",
        }
    }
}

struct LedgerCollectionDefinition {
    name: CollectionName,
    object_type: ObjectType,
    fetch: &'static [LedgerFetchField],
}

/// Bill-wise OPENING balances live on the ledger master, not in any voucher.
/// A voucher-only scan cannot see them, so the scan must at least detect them.
const LEDGER_OPENING_DEFINITION: LedgerCollectionDefinition = LedgerCollectionDefinition {
    name: CollectionName::LedgerOpeningCoverageV1,
    object_type: ObjectType::Ledger,
    fetch: &[
        LedgerFetchField::Guid,
        LedgerFetchField::Name,
        LedgerFetchField::IsBillWiseOn,
        LedgerFetchField::OpeningBalance,
    ],
};

/// Closed definitions have no function or compute-expression slot. A caller
/// cannot spell `$$NumItems`, name the enclosing collection, or attach a
/// per-row constant lookup.
struct VoucherCollectionDefinition {
    name: CollectionName,
    object_type: ObjectType,
    fetch: &'static [VoucherFetchField],
    filter: FilterName,
}

struct CompanyCollectionDefinition {
    name: CollectionName,
    object_type: ObjectType,
    fetch: &'static [CompanyFetchField],
}

const OUTSTANDINGS_DEFINITION: VoucherCollectionDefinition = VoucherCollectionDefinition {
    name: CollectionName::VoucherOutstandingsV1,
    object_type: ObjectType::Voucher,
    fetch: &[
        VoucherFetchField::Guid,
        VoucherFetchField::MasterId,
        VoucherFetchField::AlterId,
        VoucherFetchField::Date,
        VoucherFetchField::VoucherTypeName,
        VoucherFetchField::VoucherNumber,
        VoucherFetchField::PartyLedgerName,
        VoucherFetchField::IsCancelled,
        VoucherFetchField::IsDeleted,
        VoucherFetchField::IsOptional,
        VoucherFetchField::AllLedgerEntriesWildcard,
    ],
    filter: FilterName::OutstandingsPartitionV1,
};

/// The I5 corroboration profile deliberately fetches only row identity and
/// date. It has no wildcard exception, AlterID predicate, or computed value.
const EMPTY_PARTITION_WITNESS_DEFINITION: VoucherCollectionDefinition =
    VoucherCollectionDefinition {
        name: CollectionName::VoucherEmptyPartitionWitnessV1,
        object_type: ObjectType::Voucher,
        fetch: &[
            VoucherFetchField::Guid,
            VoucherFetchField::AlterId,
            VoucherFetchField::Date,
        ],
        filter: FilterName::EmptyPartitionWitnessDateV1,
    };

const COMPANY_EXTENT_DEFINITION: CompanyCollectionDefinition = CompanyCollectionDefinition {
    name: CollectionName::CompanyBookExtentV1,
    object_type: ObjectType::Company,
    fetch: &[
        CompanyFetchField::Name,
        CompanyFetchField::Guid,
        CompanyFetchField::BooksFrom,
        CompanyFetchField::LastVoucherDate,
        CompanyFetchField::AlterVoucherId,
    ],
};

/// Wire request admitted to the outstandings-specific transport cap. Only the
/// closed profile builder in this module can construct it.
///
/// ```compile_fail
/// use bridge_tally_protocol::outstandings::VoucherOutstandingsRequestXml;
/// let _ = VoucherOutstandingsRequestXml("<ENVELOPE/>".to_string());
/// ```
///
/// A broad reporting period cannot cross the sealed request boundary:
///
/// ```compile_fail
/// use bridge_tally_protocol::outstandings::{
///     voucher_outstandings_request, AlterIdRange, DateBoundaryProfile, DateWindow,
///     PinnedCompany,
/// };
/// let company: PinnedCompany = todo!();
/// let reporting_period = DateWindow::parse(
///     DateBoundaryProfile::ModeAgnostic,
///     "20240401",
///     "20260401",
/// ).unwrap();
/// voucher_outstandings_request(
///     &company,
///     &reporting_period,
///     AlterIdRange::new(0, 1).unwrap(),
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoucherOutstandingsRequestXml(String);

impl VoucherOutstandingsRequestXml {
    pub fn into_xml(self) -> String {
        self.0
    }
}

/// Ordinary-cap XML for the distinct I5 date witness profile. It is opaque for
/// the same reason as the wildcard outstandings request, but never reaches the
/// wildcard profile's 40 MiB transport exception.
///
/// ```compile_fail
/// use bridge_tally_protocol::outstandings::VoucherEmptyPartitionWitnessRequestXml;
/// let _ = VoucherEmptyPartitionWitnessRequestXml("<ENVELOPE/>".to_string());
/// ```
///
/// ```compile_fail
/// use bridge_tally_protocol::outstandings::{
///     voucher_empty_partition_witness_request, DateBoundaryProfile, DateWindow, PinnedCompany,
/// };
/// let company: PinnedCompany = todo!();
/// let broad = DateWindow::parse(DateBoundaryProfile::ModeAgnostic, "20240101", "20250101").unwrap();
/// voucher_empty_partition_witness_request(&company, &broad);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoucherEmptyPartitionWitnessRequestXml(String);

impl VoucherEmptyPartitionWitnessRequestXml {
    pub fn into_xml(self) -> String {
        self.0
    }
}

pub fn voucher_outstandings_request(
    company: &PinnedCompany,
    segment_window: &NarrowDateWindow,
    alter_id_range: AlterIdRange,
) -> VoucherOutstandingsRequestXml {
    VoucherOutstandingsRequestXml(render_outstandings(
        company.name(),
        segment_window.from().as_str(),
        segment_window.to().as_str(),
        alter_id_range.exclusive_start(),
        alter_id_range.inclusive_end(),
    ))
}

pub fn voucher_empty_partition_witness_request(
    company: &PinnedCompany,
    window: &NarrowDateWindow,
) -> VoucherEmptyPartitionWitnessRequestXml {
    VoucherEmptyPartitionWitnessRequestXml(render_empty_partition_witness(
        company.name(),
        window.from().as_str(),
        window.to().as_str(),
    ))
}

pub(crate) fn render_outstandings_vouchers(
    company: &PinnedCompany,
    segment_window: &NarrowDateWindow,
    alter_id_range: AlterIdRange,
) -> String {
    voucher_outstandings_request(company, segment_window, alter_id_range).into_xml()
}

pub(crate) fn render_outstandings_template(
    company: &str,
    from: &str,
    to: &str,
    exclusive_start: u64,
    inclusive_end: u64,
) -> String {
    render_outstandings(company, from, to, exclusive_start, inclusive_end)
}

pub(crate) fn render_empty_partition_witness_template(
    company: &str,
    from: &str,
    to: &str,
) -> String {
    render_empty_partition_witness(company, from, to)
}

fn render_outstandings(
    company: &str,
    from: &str,
    to: &str,
    exclusive_start: u64,
    inclusive_end: u64,
) -> String {
    format!(
        r#"<ENVELOPE>
  <HEADER>
    <VERSION>1</VERSION>
    <TALLYREQUEST>Export</TALLYREQUEST>
    <TYPE>Collection</TYPE>
    <ID>{collection}</ID>
  </HEADER>
  <BODY>
    <DESC>
      <STATICVARIABLES>
        <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
        <SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY>
        <SVFROMDATE TYPE="Date">{from}</SVFROMDATE>
        <SVTODATE TYPE="Date">{to}</SVTODATE>
      </STATICVARIABLES>
      <TDL>
        <TDLMESSAGE>
          <SYSTEM TYPE="Formulae" NAME="{filter}">$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate AND $AlterID &gt; {exclusive_start} AND $AlterID &lt;= {inclusive_end}</SYSTEM>
          <COLLECTION NAME="{collection}" ISMODIFY="No">
            <TYPE>{object_type}</TYPE>
            <FETCH>{fetch}</FETCH>
            <FILTERS>{filter}</FILTERS>
          </COLLECTION>
        </TDLMESSAGE>
      </TDL>
    </DESC>
  </BODY>
</ENVELOPE>"#,
        collection = OUTSTANDINGS_DEFINITION.name.as_str(),
        company = xml_escape(company),
        from = from,
        to = to,
        exclusive_start = exclusive_start,
        inclusive_end = inclusive_end,
        filter = OUTSTANDINGS_DEFINITION.filter.as_str(),
        object_type = OUTSTANDINGS_DEFINITION.object_type.as_str(),
        fetch = render_voucher_fetch(OUTSTANDINGS_DEFINITION.fetch),
    )
}

fn render_empty_partition_witness(company: &str, from: &str, to: &str) -> String {
    format!(
        r#"<ENVELOPE>
  <HEADER>
    <VERSION>1</VERSION>
    <TALLYREQUEST>Export</TALLYREQUEST>
    <TYPE>Collection</TYPE>
    <ID>{collection}</ID>
  </HEADER>
  <BODY>
    <DESC>
      <STATICVARIABLES>
        <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
        <SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY>
        <SVFROMDATE TYPE="Date">{from}</SVFROMDATE>
        <SVTODATE TYPE="Date">{to}</SVTODATE>
      </STATICVARIABLES>
      <TDL>
        <TDLMESSAGE>
          <SYSTEM TYPE="Formulae" NAME="{filter}">$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate</SYSTEM>
          <COLLECTION NAME="{collection}" ISMODIFY="No">
            <TYPE>{object_type}</TYPE>
            <FETCH>{fetch}</FETCH>
            <FILTERS>{filter}</FILTERS>
          </COLLECTION>
        </TDLMESSAGE>
      </TDL>
    </DESC>
  </BODY>
</ENVELOPE>"#,
        collection = EMPTY_PARTITION_WITNESS_DEFINITION.name.as_str(),
        company = xml_escape(company),
        from = from,
        to = to,
        filter = EMPTY_PARTITION_WITNESS_DEFINITION.filter.as_str(),
        object_type = EMPTY_PARTITION_WITNESS_DEFINITION.object_type.as_str(),
        fetch = render_voucher_fetch(EMPTY_PARTITION_WITNESS_DEFINITION.fetch),
    )
}

pub(crate) fn render_company_book_extent(company: &str) -> String {
    format!(
        r#"<ENVELOPE>
  <HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>{collection}</ID></HEADER>
  <BODY><DESC>
    <STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES>
    <TDL><TDLMESSAGE><COLLECTION NAME="{collection}" ISMODIFY="No"><TYPE>{object_type}</TYPE><FETCH>{fetch}</FETCH></COLLECTION></TDLMESSAGE></TDL>
  </DESC></BODY>
</ENVELOPE>"#,
        collection = COMPANY_EXTENT_DEFINITION.name.as_str(),
        company = xml_escape(company),
        object_type = COMPANY_EXTENT_DEFINITION.object_type.as_str(),
        fetch = render_company_fetch(COMPANY_EXTENT_DEFINITION.fetch),
    )
}

pub(crate) fn render_ledger_opening_coverage(company: &str) -> String {
    format!(
        r#"<ENVELOPE>
  <HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>{collection}</ID></HEADER>
  <BODY><DESC>
    <STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{company}</SVCURRENTCOMPANY></STATICVARIABLES>
    <TDL><TDLMESSAGE><COLLECTION NAME="{collection}" ISMODIFY="No"><TYPE>{object_type}</TYPE><FETCH>{fetch}</FETCH></COLLECTION></TDLMESSAGE></TDL>
  </DESC></BODY>
</ENVELOPE>"#,
        collection = LEDGER_OPENING_DEFINITION.name.as_str(),
        company = xml_escape(company),
        object_type = LEDGER_OPENING_DEFINITION.object_type.as_str(),
        fetch = LEDGER_OPENING_DEFINITION
            .fetch
            .iter()
            .map(|field| field.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn render_voucher_fetch(fields: &[VoucherFetchField]) -> String {
    fields
        .iter()
        .map(|field| field.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_company_fetch(fields: &[CompanyFetchField]) -> String {
    fields
        .iter()
        .map(|field| field.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_definition_has_only_the_verified_outstandings_wildcard() {
        let xml =
            render_outstandings_template("Synthetic & Company", "20260401", "20260402", 400, 800);
        assert!(!xml.contains("$$NumItems"));
        assert!(!xml.contains("<COMPUTE>"));
        assert!(!xml.contains("ALLLEDGERENTRIES.BILLALLOCATIONS"));
        assert!(!xml.contains("BILLALLOCATIONS.BILLTYPE"));
        assert_eq!(xml.matches("ALLLEDGERENTRIES.*").count(), 1);
        assert!(xml.contains("Synthetic &amp; Company"));
        assert!(xml.contains("$AlterID &gt; 400 AND $AlterID &lt;= 800"));
        assert!(render_company_book_extent("Synthetic").contains("ALTVCHID"));
    }

    #[test]
    fn empty_partition_witness_is_date_only_and_uses_no_wildcard_exception_shape() {
        let xml =
            render_empty_partition_witness_template("Synthetic & Company", "20260401", "20260402");
        assert!(xml.contains("<ID>BridgeVoucherEmptyPartitionWitnessV1</ID>"));
        assert!(xml.contains("<FETCH>GUID, ALTERID, DATE</FETCH>"));
        assert!(xml.contains("$Date &gt;= ##SVFromDate AND $Date &lt;= ##SVToDate"));
        assert!(xml.contains("Synthetic &amp; Company"));
        assert!(!xml.contains("ALLLEDGERENTRIES.*"));
        assert!(!xml.contains("$AlterID"));
        assert!(!xml.contains("<COMPUTE>"));
        assert!(!xml.contains("$$NumItems"));
    }
}
