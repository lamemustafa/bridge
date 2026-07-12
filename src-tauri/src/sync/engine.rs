#[derive(Debug, Clone)]
pub struct SyncPlan {
    pub company: String,
    pub include_ledgers: bool,
    pub include_vouchers: bool,
}

impl SyncPlan {
    pub fn gst_foundation(company: impl Into<String>) -> Self {
        Self {
            company: company.into(),
            include_ledgers: true,
            include_vouchers: true,
        }
    }
}
