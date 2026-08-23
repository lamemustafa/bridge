use serde::Deserialize;

// The generic envelope scaffold (`Envelope`/`Header`/`Body`/`Data`) and the
// bare-text `Value` leaf live in `crate::outstandings_shared` because
// `CompanyBookExtent` parsing -- needed by both the native and voucher-scan
// read paths -- uses them too. Re-exported here (rather than duplicated) so
// this module's own scan-only collections keep the same names they always
// had.
pub(super) use crate::outstandings_shared::{Envelope, Header, Value};

#[derive(Deserialize)]
pub(super) struct RawLedgerMaster {
    #[serde(rename = "@NAME", default)]
    pub(super) attribute_name: Option<String>,
    #[serde(rename = "GUID", default)]
    pub(super) guid: Option<Value>,
    #[serde(rename = "ISBILLWISEON", default)]
    pub(super) bill_wise_on: Option<Value>,
    #[serde(rename = "OPENINGBALANCE", default)]
    pub(super) opening_balance: Option<Value>,
}

#[derive(Default, Deserialize)]
pub(super) struct LedgerCollection {
    #[serde(rename = "LEDGER", default)]
    pub(super) ledgers: Vec<RawLedgerMaster>,
}

#[derive(Default, Deserialize)]
pub(super) struct VoucherCollection {
    #[serde(rename = "VOUCHER", default)]
    pub(super) vouchers: Vec<RawVoucher>,
}

#[derive(Default, Deserialize)]
pub(super) struct WitnessVoucherCollection {
    #[serde(rename = "VOUCHER", default)]
    pub(super) vouchers: Vec<RawWitnessVoucher>,
}

#[derive(Deserialize)]
pub(super) struct RawWitnessVoucher {
    #[serde(rename = "GUID")]
    pub(super) guid: String,
    #[serde(rename = "ALTERID")]
    pub(super) alter_id: Value,
    #[serde(rename = "DATE")]
    pub(super) date: Value,
}

#[derive(Deserialize)]
pub(super) struct RawVoucher {
    #[serde(rename = "GUID")]
    pub(super) guid: String,
    #[serde(rename = "MASTERID")]
    pub(super) master_id: Value,
    #[serde(rename = "ALTERID")]
    pub(super) alter_id: Value,
    #[serde(rename = "DATE")]
    pub(super) date: Value,
    #[serde(rename = "VOUCHERTYPENAME")]
    pub(super) voucher_type: String,
    #[serde(rename = "VOUCHERNUMBER", default)]
    pub(super) voucher_number: Option<String>,
    #[serde(rename = "PARTYLEDGERNAME", default)]
    pub(super) party_ledger_name: Option<Value>,
    #[serde(rename = "ISCANCELLED")]
    pub(super) cancelled: Value,
    #[serde(rename = "ISDELETED")]
    pub(super) deleted: Value,
    #[serde(rename = "ISOPTIONAL", default)]
    pub(super) optional: Option<Value>,
    #[serde(rename = "ALLLEDGERENTRIES.LIST", default)]
    pub(super) ledger_entries: Vec<RawLedgerEntry>,
}

#[derive(Deserialize)]
pub(super) struct RawLedgerEntry {
    #[serde(rename = "LEDGERNAME", default)]
    pub(super) ledger_name: Option<Value>,
    #[serde(rename = "BILLALLOCATIONS.LIST", default)]
    pub(super) bill_allocations: Vec<RawBillAllocation>,
}

#[derive(Deserialize)]
pub(super) struct RawBillAllocation {
    // BILLID and BILLCREATIONDATE are deliberately not modeled: the scan's
    // bill identity is the Tally ledger plus NAME, and neither field changes
    // ageing or reconciliation. Carrying unused identifiers would create an
    // alternate, unverified key without improving either contract.
    #[serde(rename = "NAME", default)]
    pub(super) name: Option<Value>,
    #[serde(rename = "BILLTYPE", default)]
    pub(super) bill_type: Option<Value>,
    #[serde(rename = "AMOUNT", default)]
    pub(super) amount: Option<Value>,
    /// Tally's authoritative date for the bill itself, which can differ from
    /// the enclosing voucher's date. The wildcard fetch already returns it.
    #[serde(rename = "BILLDATE", default)]
    pub(super) bill_date: Option<Value>,
    #[serde(rename = "BILLCREDITPERIOD", default)]
    pub(super) bill_credit_period: Option<Value>,
}
