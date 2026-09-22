use std::fmt;

use bridge_tally_primitives::{ExactDecimal, TallyDate};

use crate::outstandings_shared::OutstandingsReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeOutstandingsError {
    /// A two-digit display date could not be resolved to a valid calendar
    /// date, or its lexeme did not match the observed `D-MMM-YY` shape.
    InvalidDate(&'static str),
    InvalidAmount,
    /// A ledger's `CLOSINGBALANCE` was a foreign-currency display expression
    /// rather than the base-currency decimal this read requires.
    ForeignCurrencyLedgerBalance {
        ledger_name: String,
    },
    /// Tally's response did not match the documented grammar. The code
    /// identifies which structural rule was violated.
    InvalidResponse(&'static str),
    ArithmeticOverflow,
    /// The response carried a `<STATUS>` element. Both native response
    /// shapes used here (the flat Bills Receivable/Payable report and the
    /// Ledger collection) only ever carry `STATUS` on failure — the flat
    /// report's verification is INVERTED (no `STATUS` at all is success),
    /// and the ledger collection's `STATUS` must read `1`.
    TallyReportedFailure,
}

impl fmt::Display for NativeOutstandingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDate(code) => {
                write!(formatter, "native outstandings date invalid ({code})")
            }
            Self::InvalidAmount => formatter.write_str("Tally returned an invalid native amount"),
            Self::ForeignCurrencyLedgerBalance { ledger_name } => write!(
                formatter,
                "Tally reported a foreign-currency closing balance for ledger {ledger_name}"
            ),
            Self::InvalidResponse(code) => {
                write!(formatter, "native outstandings response invalid ({code})")
            }
            Self::ArithmeticOverflow => formatter
                .write_str("native outstandings arithmetic exceeded the exact-decimal bound"),
            Self::TallyReportedFailure => {
                formatter.write_str("Tally reported failure for the native outstandings request")
            }
        }
    }
}

impl std::error::Error for NativeOutstandingsError {}

/// Which of a bill's two dates ageing is measured from.
///
/// `DueDate` is the verified default (TALLY_PROTOCOL_REFERENCE ground truth
/// captured 2026-08-07): Tally's own `BILLOVERDUE` counter ages from
/// `BILLDUE`, not `BILLDATE`, whenever a bill carries a credit period that
/// makes the two differ. `BillDate` remains selectable for callers that want
/// it explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgeingAnchor {
    BillDate,
    DueDate,
}

/// One outstanding bill row from the flat Bills Receivable/Payable report.
///
/// `tally_overdue_days` is Tally's own `BILLOVERDUE` counter, measured
/// against the requested `SVTODATE`. Tally leaves it empty when the counter
/// is not applicable, including a future-due bill. It is retained only as an
/// independent cross-check against Bridge's own ageing computation and must
/// never be used as ageing's source of truth (it is not recomputed for an
/// as-of date other than the one that was requested).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBillRow {
    pub party: String,
    pub reference: String,
    pub bill_date: TallyDate,
    pub due_date: TallyDate,
    pub closing_balance: ExactDecimal,
    pub tally_overdue_days: Option<i64>,
}

/// One ledger master row from the `List of Ledgers` collection snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerSnapshotEntry {
    pub name: String,
    pub parent: Option<String>,
    /// `None` means Tally returned an empty `<CLOSINGBALANCE>` element. It is
    /// distinct from an established numeric zero so each consumer can retain
    /// its own documented treatment of that observed wire shape.
    pub closing_balance: Option<ExactDecimal>,
    pub opening_balance: ExactDecimal,
    pub bill_wise_on: bool,
}

/// A party's unallocated residual: the gap between the ledger's own
/// `CLOSINGBALANCE` and the sum of everything the Bills Receivable/Payable
/// reports show as open bills for that party. Because the native reports
/// only ever list named bills, a non-zero residual is exactly the party's
/// on-account exposure — money the ledger balance carries with no bill
/// reference at all, and therefore no truthful bill age.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartyResidual {
    pub party: String,
    pub amount: ExactDecimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeOutstandingsResult {
    pub report: OutstandingsReport,
    pub residuals: Vec<PartyResidual>,
    /// Sum of the absolute magnitude of every party residual: the total
    /// unallocated (on-account) exposure the bill-level reports cannot see.
    pub residual_total: ExactDecimal,
    /// The outcome of independently comparing Tally's `BILLOVERDUE` values
    /// with Bridge's due-date ageing. It is never used as ageing's source of
    /// truth, but a refused as-of date is materially different from scattered
    /// source-data disagreement and must reach the operator distinctly.
    pub overdue_crosscheck: NativeOverdueCrosscheck,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeOverdueCrosscheck {
    Honored,
    Inconsistent,
    RefusedAsOf {
        tally_as_of: TallyDate,
    },
    /// No bill row can corroborate the requested date, while the separately
    /// read ledger snapshot carries an unallocated balance whose period could
    /// have moved. The report must remain partial rather than claiming the
    /// requested as-of date was honored.
    UnconfirmedAsOfWithoutBillReferences,
    /// The response carried no positive overdue counter that can identify
    /// Tally's effective date. This includes no bill rows, empty counters,
    /// and zero-only counters; none can establish that Tally honored the
    /// requested as-of date.
    UnconfirmedAsOfWithoutEffectiveDateEvidence,
}

/// What Tally reports about a company's currencies. `symbol`, `mailing_name`,
/// `decimal_places` and `is_inr` describe the BASE currency once it is
/// identified ([`Self::base_determined`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CompanyCurrency {
    pub symbol: String,
    pub mailing_name: String,
    /// How many currency masters the company defines. With exactly one, it is
    /// the base currency. With several, the Currency masters alone do not say
    /// which is the base (row order and `RESERVEDNAME` do not either), and
    /// guessing would put a wrong currency symbol in front of a real balance:
    /// the company's own `CURRENCYNAME` must identify it
    /// ([`Self::with_company_currency_name`]).
    pub currency_count: usize,
    /// The base currency's display precision reported by Tally. Consumers
    /// must carry this to their rendering boundary rather than silently
    /// assuming paise precision.
    pub decimal_places: u8,
    /// The identified base currency's mailing name is INR (`INR` or `Indian
    /// Rupees`). False while the base is undetermined.
    pub is_inr: bool,
    /// Whether this read has identified which master is the base currency.
    #[serde(skip)]
    pub base_determined: bool,
    /// Every master as read, in response order.
    #[serde(skip)]
    pub masters: Vec<CurrencyMaster>,
}

/// One Currency master as the currency read returns it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CurrencyMaster {
    /// `NAME`: the symbol ledgers carry (for example `I₹` or `$`).
    pub name: String,
    /// `ORIGINALNAME`: on the books measured, the value the company's
    /// `CURRENCYNAME` carries when this master is its base currency (for
    /// example `₹` for a master named `I₹`).
    pub original_name: Option<String>,
    pub mailing_name: String,
    pub decimal_places: u8,
}

fn mailing_name_is_inr(mailing_name: &str) -> bool {
    // "Rs." is shared by several currencies, so only the observed Indian
    // mailing identity is authoritative enough to put ₹ before real money.
    mailing_name.eq_ignore_ascii_case("Indian Rupees") || mailing_name.eq_ignore_ascii_case("INR")
}

impl CompanyCurrency {
    /// The read of a company's Currency masters. With exactly one master, that
    /// master is the base currency. With several, the base stays undetermined
    /// until [`Self::with_company_currency_name`] identifies it.
    pub fn from_masters(masters: Vec<CurrencyMaster>) -> Self {
        let currency_count = masters.len();
        let base = (currency_count == 1).then(|| masters[0].clone());
        let mut currency = Self {
            symbol: String::new(),
            mailing_name: String::new(),
            currency_count,
            decimal_places: 0,
            is_inr: false,
            base_determined: false,
            masters,
        };
        if let Some(base) = base {
            currency.set_base(&base);
        }
        currency
    }

    fn set_base(&mut self, base: &CurrencyMaster) {
        self.symbol = base.name.clone();
        self.mailing_name = base.mailing_name.clone();
        self.decimal_places = base.decimal_places;
        self.is_inr = mailing_name_is_inr(&base.mailing_name);
        self.base_determined = true;
    }

    /// Identifies the base currency among several masters. On the three books
    /// measured (licensed TallyPrime 7.1, 22 Sep 2026; protocol reference
    /// §9.10a.2), the company's `CURRENCYNAME` matched exactly one master's
    /// `ORIGINALNAME` and never its `NAME`: company `₹`, masters `I₹`/`₹` and
    /// `$`/`$`. That is a measured rule, not a documented contract. The base
    /// is the unique master whose `ORIGINALNAME` equals it character for
    /// character. With no match or several, it stays undetermined, which
    /// refuses. A single-master read is unchanged.
    pub fn with_company_currency_name(mut self, company_currency_name: &str) -> Self {
        // An empty value names nothing, even if some master's ORIGINALNAME is
        // also empty.
        if self.currency_count < 2 || company_currency_name.is_empty() {
            return self;
        }
        let matching: Vec<CurrencyMaster> = self
            .masters
            .iter()
            .filter(|master| master.original_name.as_deref() == Some(company_currency_name))
            .cloned()
            .collect();
        if let [base] = matching.as_slice() {
            self.set_base(base);
        }
        self
    }

    /// Whether this read may label the company's figures as INR, or the
    /// reason it may not.
    pub fn inr_admission(&self) -> Result<(), &'static str> {
        match (self.currency_count, self.base_determined, self.is_inr) {
            (0, _, _) => Err("company_currency_probe_failed"),
            (_, false, _) => Err("company_base_currency_undetermined"),
            (_, true, false) => Err("company_base_currency_not_inr"),
            (_, true, true) => Ok(()),
        }
    }
}
