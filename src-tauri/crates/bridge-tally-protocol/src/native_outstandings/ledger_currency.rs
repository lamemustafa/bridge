//! A ledger's own currency against the book's base currency (bridge#551).
//!
//! A foreign-currency ledger's bills arrive from Tally's Bills reports as plain
//! amounts, and a zero foreign balance closes as a plain `0.00`
//! (`LEDGER_CURRENCY_CAPTURE_PROVENANCE.md`, `FOREX_LEDGER_CAPTURE_PROVENANCE.md`).
//! The shape of an amount therefore cannot tell a foreign ledger from a base
//! one. Its `CURRENCYNAME` can: every row measured carries the NAME of the
//! Currency master it is kept in (`I₹`, `Rs.`, `$`), and the base master's NAME
//! differs by book, so it is read from the same book, never assumed.

/// A ledger kept in a currency other than the base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignCurrencyLedger {
    pub ledger: String,
    /// Its `CURRENCYNAME`, verbatim.
    pub currency: String,
}

/// The ledgers of one read, classified against the base master's NAME.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerCurrencies {
    /// Ledgers in a currency other than the base, in read order.
    pub foreign: Vec<ForeignCurrencyLedger>,
    /// With exactly one Currency master: ledgers whose `CURRENCYNAME` was
    /// absent or empty, taken as the base because no other currency exists.
    /// Always 0 with several masters, where such a ledger refuses.
    pub unobserved: usize,
}

/// Why a read's ledgers cannot be classified. Fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerCurrencyRefusal {
    /// Several Currency masters, and this ledger's `CURRENCYNAME` was absent or
    /// empty: it could be kept in any of them.
    Unobserved { ledger: String },
    /// No ledger is in the base currency, or, with one master, a ledger names a
    /// currency that is not it. The base NAME read and the ledgers read do not
    /// describe one book, so nothing is classified.
    BaseUnmatched { ledger: Option<String> },
}

impl LedgerCurrencyRefusal {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unobserved { .. } => "ledger_currency_unobserved",
            Self::BaseUnmatched { .. } => "ledger_currency_base_unmatched",
        }
    }
}

/// Classify every ledger of one read by its `CURRENCYNAME` against the base
/// master's NAME, read from the same book. Pass the **full** ledger set, before
/// any group or name filter: a filtered set made only of foreign ledgers would
/// otherwise refuse as base-unmatched.
///
/// - One Currency master: no ledger can be kept in another currency, since
///   Tally assigns a ledger a currency from its masters (inferred, not
///   measured: no single-master book with a foreign ledger has been
///   attempted). An absent or empty `CURRENCYNAME` is taken as the base and
///   counted in `unobserved`; a present value other than the base NAME
///   refuses.
/// - Several masters: an absent or empty `CURRENCYNAME` refuses; a value other
///   than the base NAME is foreign; and at least one ledger must be in the base.
/// - No master, or an empty base NAME: refuses.
///
/// The comparison is exact, codepoint for codepoint.
pub fn classify_ledger_currencies<'a>(
    base_name: &str,
    currency_count: usize,
    ledgers: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Result<LedgerCurrencies, LedgerCurrencyRefusal> {
    if currency_count == 0 || base_name.is_empty() {
        return Err(LedgerCurrencyRefusal::BaseUnmatched { ledger: None });
    }
    let single = currency_count == 1;
    let mut classified = LedgerCurrencies::default();
    let mut base_seen = false;
    for (ledger, currency) in ledgers {
        match currency.filter(|currency| !currency.is_empty()) {
            None if single => {
                classified.unobserved += 1;
                base_seen = true;
            }
            None => {
                return Err(LedgerCurrencyRefusal::Unobserved {
                    ledger: ledger.to_string(),
                })
            }
            Some(currency) if currency == base_name => base_seen = true,
            Some(_) if single => {
                return Err(LedgerCurrencyRefusal::BaseUnmatched {
                    ledger: Some(ledger.to_string()),
                })
            }
            Some(currency) => classified.foreign.push(ForeignCurrencyLedger {
                ledger: ledger.to_string(),
                currency: currency.to_string(),
            }),
        }
    }
    if !base_seen && !single {
        return Err(LedgerCurrencyRefusal::BaseUnmatched { ledger: None });
    }
    Ok(classified)
}

#[cfg(test)]
#[path = "ledger_currency_tests.rs"]
mod tests;
