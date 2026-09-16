//! A fail-closed stop carrying a stable category.

use std::fmt;

/// The category is the contract: tests assert on it, so a refusal cannot be
/// satisfied by some *other* refusal firing first. The message is for the
/// operator and may change freely.
///
/// Messages never carry the PDF password. They may carry a row number or a
/// column name, and callers that forward a refusal off the machine forward the
/// category and row number only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub category: &'static str,
    pub message: String,
    /// The 1-based statement row the refusal is about, when there is one.
    pub row: Option<usize>,
}

impl Refusal {
    pub fn new(category: &'static str, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
            row: None,
        }
    }

    pub fn at_row(category: &'static str, row: usize, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
            row: Some(row),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.category, self.message)
    }
}

impl std::error::Error for Refusal {}
