use bridge_tally_protocol::trial_balance::TrialBalance;
use rust_xlsxwriter::XlsxError;

#[derive(Debug)]
pub struct TrialBalanceWorkbookSource {
    pub company: String,
    pub from_yyyymmdd: String,
    pub to_yyyymmdd: String,
    pub source_bytes: usize,
    pub trial_balance: TrialBalance,
}

#[derive(Debug, serde::Serialize)]
pub struct TrialBalanceExportSummary {
    pub path: String,
    pub company: String,
    pub from_yyyymmdd: String,
    pub to_yyyymmdd: String,
    pub ledger_count: usize,
    pub opening_difference: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TrialBalanceXlsxError {
    #[error("Bridge could not build the Trial Balance workbook: {0}")]
    Workbook(#[from] XlsxError),
    #[error("Bridge could not read a Trial Balance date for the spreadsheet ({0})")]
    InvalidDate(String),
    #[error("Bridge could not represent a Trial Balance amount in Excel ({0})")]
    InvalidAmount(String),
    #[error("Bridge could not reconcile the Trial Balance controls exactly")]
    ControlMismatch,
}
