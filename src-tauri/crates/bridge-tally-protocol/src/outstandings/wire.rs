use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct Envelope<T> {
    #[serde(rename = "HEADER")]
    pub(super) header: Header,
    #[serde(rename = "BODY")]
    pub(super) body: Body<T>,
}

#[derive(Deserialize)]
pub(super) struct Header {
    #[serde(rename = "STATUS")]
    pub(super) status: String,
}

#[derive(Deserialize)]
pub(super) struct Body<T> {
    #[serde(rename = "DATA")]
    pub(super) data: Data<T>,
}

#[derive(Deserialize)]
pub(super) struct Data<T> {
    #[serde(rename = "COLLECTION")]
    pub(super) collection: T,
}

#[derive(Default, Deserialize)]
pub(super) struct Value {
    #[serde(rename = "$text", default)]
    pub(super) text: String,
}

#[derive(Deserialize)]
pub(super) struct CompanyCollection {
    #[serde(rename = "COMPANY", default)]
    pub(super) companies: Vec<RawCompany>,
}

#[derive(Deserialize)]
pub(super) struct RawCompany {
    #[serde(rename = "@NAME")]
    pub(super) attribute_name: String,
    #[serde(rename = "NAME")]
    pub(super) name: Value,
    #[serde(rename = "GUID")]
    pub(super) guid: Value,
    #[serde(rename = "BOOKSFROM")]
    pub(super) books_from: Value,
    #[serde(rename = "LASTVOUCHERDATE")]
    pub(super) last_voucher_date: Value,
    #[serde(rename = "ALTVCHID", default)]
    pub(super) alter_voucher_id: Option<Value>,
}

#[derive(Deserialize)]
pub(super) struct RawLedgerMaster {
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
}
