use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct GstDraftRequest {
    pub company: String,
    pub financial_year: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GstReturnDraft {
    pub company: String,
    pub financial_year: String,
    pub gstr1: Gstr1Draft,
    pub gstr3b: Gstr3bDraft,
    pub missing_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Gstr1Draft {
    pub b2b_invoice_count: u32,
    pub b2c_invoice_count: u32,
    pub credit_debit_note_count: u32,
    pub hsn_summary_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Gstr3bDraft {
    pub outward_taxable_value: String,
    pub integrated_tax: String,
    pub central_tax: String,
    pub state_tax: String,
    pub cess: String,
}

impl GstReturnDraft {
    pub fn empty(request: GstDraftRequest) -> Self {
        Self {
            company: request.company,
            financial_year: request.financial_year,
            gstr1: Gstr1Draft {
                b2b_invoice_count: 0,
                b2c_invoice_count: 0,
                credit_debit_note_count: 0,
                hsn_summary_count: 0,
            },
            gstr3b: Gstr3bDraft {
                outward_taxable_value: "0.00".to_string(),
                integrated_tax: "0.00".to_string(),
                central_tax: "0.00".to_string(),
                state_tax: "0.00".to_string(),
                cess: "0.00".to_string(),
            },
            missing_fields: vec![
                "GST draft calculation is not enabled in this build.".to_string(),
                "Sales voucher extraction and tax ledger mapping are pending.".to_string(),
            ],
        }
    }
}
