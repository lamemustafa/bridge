//! One error type for the whole slice. A consumer-rule refusal carries the rule's code exactly
//! as the reference engine's `ReadRefused` names it (`C3-content-hash`, `C8-window`, ...), so a
//! test can assert the rule that fired rather than matching a message.

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    /// The read does not satisfy a tally-read-v1 consumer rule. `code` names the rule.
    #[error("{code}: {detail}")]
    Refused { code: &'static str, detail: String },
    /// A part's bytes are not the XML (or JSON) the book needs: malformed markup, an amount
    /// or date that does not parse, a GUID that is missing. The reference engine raises here
    /// too (a parse error or a `ValueError`); neither side guesses.
    #[error("{part}: {detail}")]
    Parse { part: String, detail: String },
    /// `Book::population` refuses while any voucher's status is unknown.
    #[error("{0} vouchers have unknown status; refusing to form the books population")]
    UnknownVoucherStatus(usize),
    /// The engagement or rules configuration is missing a key or has the wrong type.
    #[error("config: {0}")]
    Config(String),
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

impl AuditError {
    pub(crate) fn refused(code: &'static str, detail: impl Into<String>) -> Self {
        Self::Refused {
            code,
            detail: detail.into(),
        }
    }

    pub(crate) fn parse(part: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Parse {
            part: part.into(),
            detail: detail.into(),
        }
    }

    /// The consumer-rule code, when this is a refusal.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Refused { code, .. } => Some(code),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, AuditError>;
