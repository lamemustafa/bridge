use bridge_tally_primitives::ExactDecimal;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialBalanceLedger {
    pub name: String,
    pub parent: Option<String>,
    pub guid: String,
    pub master_id: u64,
    pub alter_id: u64,
    pub opening: ExactDecimal,
    pub closing: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialBalance {
    pub rows: Vec<TrialBalanceLedger>,
    /// A synthetic control row equal to `-sum(opening)`. Tally renders this as
    /// "Difference in opening balances" but does not emit it on the wire.
    pub opening_difference: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrialBalanceError {
    TallyReportedFailure,
    InvalidResponse(&'static str),
    Arithmetic,
}

impl std::fmt::Display for TrialBalanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TallyReportedFailure => {
                formatter.write_str("Tally reported that the trial-balance read failed")
            }
            Self::InvalidResponse(code) => {
                write!(
                    formatter,
                    "Tally returned an invalid trial-balance response ({code})"
                )
            }
            Self::Arithmetic => {
                formatter.write_str("Trial-balance arithmetic could not be proven exactly")
            }
        }
    }
}

impl std::error::Error for TrialBalanceError {}
