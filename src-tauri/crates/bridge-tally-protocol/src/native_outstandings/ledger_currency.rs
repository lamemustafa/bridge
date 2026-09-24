//! A ledger's own currency against the book's base currency (bridge#551;
//! TALLY_PROTOCOL_REFERENCE §8.2d).
//!
//! A foreign-currency ledger's bills arrive from Tally's Bills reports as plain
//! amounts, and a zero foreign balance closes as a plain `0.00`. The shape of
//! an amount therefore cannot tell a foreign ledger from a base one. Its
//! `CURRENCYNAME` can: every row measured carries the NAME of the Currency
//! master it is kept in (`I₹`, `₹`, `Rs.`, `$`), and the base master's NAME
//! differs by book, so it is read from the same book, never assumed.

use super::model::NativeOutstandingsError;
use super::model::{identify_base_master, CompanyCurrency, CurrencyMaster};
use super::wire::parse_currency_masters;

/// The NAME of a book's base Currency master, held only where the base is
/// known. With several masters, `CompanyCurrency::symbol` is merely the first
/// master read (on the captured FOREX book, `$`), so this type is built only
/// from a book with exactly one master. Among several masters the base is
/// identified by [`CurrencyMasters::identify_base`]: the company's own
/// `CURRENCYNAME` is the base master's ORIGINALNAME (`₹` on the captured FOREX
/// book), not the NAME its ledgers carry (`I₹`), so the base NAME is found
/// through that master and never taken from the company field directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseCurrencyName {
    name: String,
    single_master: bool,
}

impl BaseCurrencyName {
    /// The one master of a book that defines exactly one; `None` otherwise, or
    /// when its NAME is empty.
    pub fn of_single_master(currency: &CompanyCurrency) -> Option<Self> {
        (currency.currency_count == 1 && !currency.symbol.trim().is_empty()).then(|| Self {
            name: currency.symbol.clone(),
            single_master: true,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// A base among several masters, for tests that classify without a
    /// currency read ([`CurrencyMasters::identify_base`] is the production
    /// constructor).
    #[cfg(test)]
    pub(crate) fn among_several_for_tests(name: &str) -> Self {
        Self {
            name: name.to_string(),
            single_master: false,
        }
    }
}

/// A book's Currency masters, as the currency read returns them, in read
/// order (bridge#551). The base is identified from them
/// ([`Self::identify_base`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrencyMasters {
    masters: Vec<CurrencyMaster>,
}

/// A book's base Currency master, identified by
/// [`CurrencyMasters::identify_base`]; the only way to build one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentifiedBaseCurrency {
    base: BaseCurrencyName,
    decimal_places: u8,
    is_inr: bool,
}

/// Parses a currency read into its masters (bridge#551).
pub fn parse_currency_master_list(xml: &str) -> Result<CurrencyMasters, NativeOutstandingsError> {
    Ok(CurrencyMasters {
        masters: parse_currency_masters(xml)?,
    })
}

impl CurrencyMasters {
    pub fn count(&self) -> usize {
        self.masters.len()
    }

    /// Whether two reads returned the same masters in the same order, by
    /// `NAME`, `MAILINGNAME` and `DECIMALPLACES`; `ORIGINALNAME`, which only
    /// one of them fetches, is not compared.
    pub fn same_masters_as(&self, other: &Self) -> bool {
        let key = |masters: &Self| {
            masters
                .masters
                .iter()
                .map(|master| {
                    (
                        master.name.clone(),
                        master.mailing_name.clone(),
                        master.decimal_places,
                    )
                })
                .collect::<Vec<_>>()
        };
        key(self) == key(other)
    }

    /// The base master: the only one, or, among several, the unique master
    /// whose `ORIGINALNAME` equals the company's own `CURRENCYNAME`
    /// (TALLY_PROTOCOL_REFERENCE §9.10a.2). `None` when none is identified, or
    /// when the identified master's NAME is blank, since ledgers are
    /// classified against that NAME.
    pub fn identify_base(
        &self,
        company_currency_name: Option<&str>,
    ) -> Option<IdentifiedBaseCurrency> {
        let master = identify_base_master(&self.masters, company_currency_name)?;
        (!master.name.trim().is_empty()).then(|| IdentifiedBaseCurrency {
            base: BaseCurrencyName {
                name: master.name.clone(),
                single_master: self.masters.len() == 1,
            },
            decimal_places: master.decimal_places,
            is_inr: master.is_inr(),
        })
    }
}

impl IdentifiedBaseCurrency {
    pub fn base(&self) -> &BaseCurrencyName {
        &self.base
    }

    pub fn decimal_places(&self) -> u8 {
        self.decimal_places
    }

    /// The INR rule, by the base master's mailing name alone
    /// ([`CurrencyMaster::is_inr`]).
    pub fn is_inr(&self) -> bool {
        self.is_inr
    }
}

/// A ledger kept in a currency other than the base.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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
    /// absent or empty, taken as the base on the inference that a book with
    /// one master can hold no ledger in another currency. Always 0 with several
    /// masters, where such a ledger refuses.
    pub unobserved: usize,
}

/// Why a read's ledgers cannot be classified. Fails closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerCurrencyRefusal {
    /// Several Currency masters, and this ledger's `CURRENCYNAME` was absent or
    /// empty: it could be kept in any of them.
    Unobserved { ledger: String },
    /// With one master, a ledger names a currency that is not it; with
    /// several, no ledger is in the base. Either way the base NAME and the
    /// ledgers read disagree, so nothing is classified. This does not detect a
    /// wrong base that some ledgers happen to carry: that is why the base is a
    /// [`BaseCurrencyName`], not a string.
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
/// - One Currency master: an absent or empty `CURRENCYNAME` is taken as the
///   base and counted in `unobserved`. That rests on the inference, not a
///   measurement, that a book with one master can hold no ledger in another
///   currency, since Tally assigns a ledger its currency from the masters. A
///   present value other than the base NAME refuses.
/// - Several masters: an absent or empty `CURRENCYNAME` refuses; a value other
///   than the base NAME is foreign; and at least one ledger must be in the base.
///
/// The comparison is exact, codepoint for codepoint; emptiness is judged on the
/// trimmed value.
pub fn classify_ledger_currencies<'a>(
    base: &BaseCurrencyName,
    ledgers: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Result<LedgerCurrencies, LedgerCurrencyRefusal> {
    let mut classified = LedgerCurrencies::default();
    let mut base_seen = false;
    for (ledger, currency) in ledgers {
        match currency.filter(|currency| !currency.trim().is_empty()) {
            None if base.single_master => classified.unobserved += 1,
            None => {
                return Err(LedgerCurrencyRefusal::Unobserved {
                    ledger: ledger.to_string(),
                })
            }
            Some(currency) if currency == base.name => base_seen = true,
            Some(_) if base.single_master => {
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
    if !base_seen && !base.single_master {
        return Err(LedgerCurrencyRefusal::BaseUnmatched { ledger: None });
    }
    Ok(classified)
}

#[cfg(test)]
#[path = "ledger_currency_tests.rs"]
mod tests;
