//! Rule values as data, read from the vendored excerpt of the reference engine's rules table.
//!
//! Provenance: `rules/ay2026-27.s44ab.toml` holds the `[meta]` and `[s44ab]` tables of the
//! reference engine's `tae/rules/ay2026-27.toml`, byte for byte, under a five-line header. The
//! source file had sha256 [`SOURCE_SHA256`] when it was read at engine commit
//! [`SOURCE_COMMIT`]. The local parity example re-checks, against a live copy of the engine,
//! that the excerpt is still verbatim and that both files give the same values.
//!
//! [`VENDORED_SHA256`] is the vendored file's own hash; a unit test fails if the file changes
//! without that constant (and so without a reviewer seeing the provenance above) changing too.

use crate::error::{AuditError, Result};

pub const VENDORED: &str = include_str!("../rules/ay2026-27.s44ab.toml");
pub const VENDORED_SHA256: &str =
    "62d8026e4717bad9960086a34e05fd80c6101747bae31b9d0777e0f71a844e4e";
pub const SOURCE_PATH: &str = "tae/rules/ay2026-27.toml";
pub const SOURCE_SHA256: &str = "8a6ec80cd5d19da34392e93024dc9a43a98982b09fb457c662f627174acedf2d";
pub const SOURCE_COMMIT: &str = "c2f206beb870a8fdb0775f2d1c71aa1ccfccca64";

/// The values `cash_44ab` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    /// `[meta].version`, echoed into every result as `rules_version`.
    pub version: String,
    pub turnover_threshold_paise: i64,
    pub turnover_threshold_low_cash_paise: i64,
    pub cash_share_limit_bp: i64,
}

impl Rules {
    /// Parse a rules table (the vendored excerpt or the full source file).
    pub fn parse(text: &str) -> Result<Self> {
        let table: toml::Table =
            toml::from_str(text).map_err(|e| AuditError::Config(format!("rules TOML: {e}")))?;
        let section = |name: &str| {
            table
                .get(name)
                .and_then(toml::Value::as_table)
                .ok_or_else(|| AuditError::Config(format!("rules: no [{name}] table")))
        };
        let (meta, s44ab) = (section("meta")?, section("s44ab")?);
        let int = |key: &str| {
            s44ab
                .get(key)
                .and_then(toml::Value::as_integer)
                .ok_or_else(|| {
                    AuditError::Config(format!("rules: [s44ab].{key} is not an integer"))
                })
        };
        Ok(Self {
            version: meta
                .get("version")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| AuditError::Config("rules: [meta].version".to_string()))?
                .to_string(),
            turnover_threshold_paise: int("turnover_threshold_paise")?,
            turnover_threshold_low_cash_paise: int("turnover_threshold_low_cash_paise")?,
            cash_share_limit_bp: int("cash_share_limit_bp")?,
        })
    }

    /// The vendored AY 2026-27 values.
    pub fn vendored() -> Result<Self> {
        Self::parse(VENDORED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn vendored_rules_match_their_recorded_hash() {
        let actual = crate::canonical::hex(&Sha256::digest(VENDORED.as_bytes()));
        assert_eq!(
            actual, VENDORED_SHA256,
            "rules/ay2026-27.s44ab.toml changed; re-vendor it from {SOURCE_PATH} and update \
             VENDORED_SHA256, SOURCE_SHA256 and SOURCE_COMMIT together"
        );
    }

    #[test]
    fn vendored_rules_carry_the_s44ab_values() {
        let rules = Rules::vendored().unwrap();
        assert_eq!(rules.version, "2026-09-17.1");
        assert_eq!(rules.turnover_threshold_paise, 1_000_000_000);
        assert_eq!(rules.turnover_threshold_low_cash_paise, 10_000_000_000);
        assert_eq!(rules.cash_share_limit_bp, 500);
    }
}
