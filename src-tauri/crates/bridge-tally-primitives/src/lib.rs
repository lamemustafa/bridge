//! Portable Tally value types shared by the read protocol and the domain core.
//!
//! These are pure primitives: an exact decimal, a validated Tally date, and the
//! shared error. They live below `bridge-tally-core` deliberately.
//! `bridge-tally-core` also carries delivery/destination capability
//! (`AxalTallyGateway`, `DestinationAdapter`), and the sealed read-only
//! `bridge-tally-live-read` controller is forbidden from reaching that surface
//! by `scripts/check-tally-live-read-boundary.mjs`. Keeping the value types
//! here lets `bridge-tally-protocol` use them without dragging write capability
//! into the read path.

use serde::{Deserialize, Deserializer, Serialize};

pub mod exact_arithmetic;

/// Maximum accepted `ExactDecimal` lexeme length.
pub const MAX_EXACT_DECIMAL_BYTES: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum TallyError {
    #[error("Tally is not reachable")]
    Unreachable,
    #[error("Tally returned an invalid protocol response ({code})")]
    Protocol { code: String },
    #[error("Tally data failed validation ({code})")]
    InvalidData { code: String },
    #[error("Capability is unavailable ({code})")]
    Unsupported { code: String },
    #[error("Tally read response exceeded the bounded limit ({scope:?})")]
    ReadResponseTooLarge { scope: ReadResponseScope },
    #[error("Operation was cancelled")]
    Cancelled,
    #[error("The outcome of the write could not be proven")]
    OutcomeUnknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadResponseScope {
    VoucherWindow,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ExactDecimal(String);

impl ExactDecimal {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let body = bytes.strip_prefix(b"-").unwrap_or(bytes);
        let mut sections = body.split(|byte| *byte == b'.');
        let whole = sections.next().unwrap_or_default();
        let fractional = sections.next();
        let valid = bytes.len() <= MAX_EXACT_DECIMAL_BYTES
            && !whole.is_empty()
            && whole.iter().all(u8::is_ascii_digit)
            && fractional
                .is_none_or(|part| !part.is_empty() && part.iter().all(u8::is_ascii_digit))
            && sections.next().is_none();
        if !valid {
            return Err(TallyError::InvalidData {
                code: "invalid_exact_decimal".to_string(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn zero() -> Self {
        Self("0".to_string())
    }

    pub fn checked_add(&self, other: &Self) -> Result<Self, TallyError> {
        let mut total = exact_arithmetic::ExactDecimalAccumulator::default();
        total.add(self.as_str());
        total.add(other.as_str());
        Self::parse(total.canonical_string())
    }

    pub fn checked_subtract(&self, other: &Self) -> Result<Self, TallyError> {
        let mut total = exact_arithmetic::ExactDecimalAccumulator::default();
        total.add(self.as_str());
        total.subtract(other.as_str());
        Self::parse(total.canonical_string())
    }

    pub fn is_zero(&self) -> bool {
        exact_arithmetic::numeric_equal(self.as_str(), "0")
    }

    pub fn is_negative(&self) -> bool {
        exact_arithmetic::is_negative_nonzero(self.as_str())
    }

    pub fn abs(&self) -> Result<Self, TallyError> {
        if self.is_negative() {
            Self::parse(self.as_str().trim_start_matches('-').to_string())
        } else {
            Ok(self.clone())
        }
    }

    pub fn cmp_magnitude(&self, other: &Self) -> std::cmp::Ordering {
        exact_arithmetic::magnitude_cmp(self.as_str(), other.as_str())
    }
}

impl<'de> Deserialize<'de> for ExactDecimal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// A validated Gregorian calendar date in Tally's canonical YYYYMMDD form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct TallyDate(String);

impl TallyDate {
    pub fn parse(value: impl Into<String>) -> Result<Self, TallyError> {
        let value = value.into();
        if !is_valid_yyyymmdd(&value) {
            return Err(invalid_data("invalid_tally_date"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn next_day(&self) -> Result<Self, TallyError> {
        let year = self.0[0..4]
            .parse::<u32>()
            .map_err(|_| invalid_data("invalid_tally_date"))?;
        let month = self.0[4..6]
            .parse::<u32>()
            .map_err(|_| invalid_data("invalid_tally_date"))?;
        let day = self.0[6..8]
            .parse::<u32>()
            .map_err(|_| invalid_data("invalid_tally_date"))?;
        let month_days =
            gregorian_month_days(year, month).ok_or_else(|| invalid_data("invalid_tally_date"))?;
        let (next_year, next_month, next_day) = if day < month_days {
            (year, month, day + 1)
        } else if month < 12 {
            (year, month + 1, 1)
        } else {
            (
                year.checked_add(1)
                    .filter(|value| *value <= 9999)
                    .ok_or_else(|| invalid_data("tally_date_overflow"))?,
                1,
                1,
            )
        };
        Self::parse(format!("{next_year:04}{next_month:02}{next_day:02}"))
    }

    pub fn previous_day(&self) -> Result<Self, TallyError> {
        let year = self.0[0..4]
            .parse::<u32>()
            .map_err(|_| invalid_data("invalid_tally_date"))?;
        let month = self.0[4..6]
            .parse::<u32>()
            .map_err(|_| invalid_data("invalid_tally_date"))?;
        let day = self.0[6..8]
            .parse::<u32>()
            .map_err(|_| invalid_data("invalid_tally_date"))?;
        let (previous_year, previous_month, previous_day) = if day > 1 {
            (year, month, day - 1)
        } else if month > 1 {
            let previous_month = month - 1;
            let previous_day = gregorian_month_days(year, previous_month)
                .ok_or_else(|| invalid_data("invalid_tally_date"))?;
            (year, previous_month, previous_day)
        } else {
            let previous_year = year
                .checked_sub(1)
                .filter(|value| *value >= 1)
                .ok_or_else(|| invalid_data("tally_date_underflow"))?;
            (previous_year, 12, 31)
        };
        Self::parse(format!(
            "{previous_year:04}{previous_month:02}{previous_day:02}"
        ))
    }
}

impl<'de> Deserialize<'de> for TallyDate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}
fn is_valid_yyyymmdd(value: &str) -> bool {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let year = value[0..4].parse::<u32>().unwrap_or_default();
    let month = value[4..6].parse::<u32>().unwrap_or_default();
    let day = value[6..8].parse::<u32>().unwrap_or_default();
    let Some(days_in_month) = gregorian_month_days(year, month) else {
        return false;
    };
    year != 0 && (1..=days_in_month).contains(&day)
}
fn gregorian_month_days(year: u32, month: u32) -> Option<u32> {
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 if leap => Some(29),
        2 => Some(28),
        _ => None,
    }
}

fn invalid_data(code: &'static str) -> TallyError {
    TallyError::InvalidData {
        code: code.to_string(),
    }
}
