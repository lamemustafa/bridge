//! Tally's composite amount: a foreign-currency amount, its rate and the base
//! amount, written into one field when a foreign amount is entered on a ledger
//! whose own currency is the base.
//!
//! Captured forms (licensed TallyPrime 7.1, a synthetic several-currency book):
//! `-$ 100.00 @ I₹ 86/$  = -I₹ 8600.00` on a voucher's entries and bill
//! allocation (`fixtures/agent/vouchers-forex-composite-20260915`), and with an
//! empty rate `$ 0.00 @ I₹ /$  = I₹ 0.00` in a Trial Balance
//! (`fixtures/trial_balance_currency_forex_live`).
//!
//! Only the shape is classified here. No value is read from a composite, so
//! nothing downstream can mistake one for an amount; a caller that finds a
//! composite sets the row aside or refuses it.
//!
//! Strict by design: a composite cut short, doubled, or with a non-ASCII digit
//! in an amount is not one, and falls through to the caller's own amount
//! parse, which refuses it. A currency symbol is any run without whitespace,
//! an ASCII digit or `-@=/`, so a symbol is not checked against a list.

/// Whether `text` is exactly one composite: `<amount> @ <rate> = <amount>`,
/// each amount an optional `-`, a currency symbol, one space and an ASCII
/// decimal, and the rate `<base symbol> <decimal>/<foreign symbol>` or, empty,
/// `<base symbol> /<foreign symbol>`.
///
/// The symbols must agree with each other, as every captured composite does:
/// the rate is quoted in the base amount's symbol per the foreign amount's,
/// and the two are different currencies. A string of the right shape whose
/// symbols disagree is not a composite, so its caller refuses it.
///
/// Signs are not compared. A ledger balance can hold a foreign amount and a
/// base amount of opposite signs (bought at one rate, sold at another); a
/// caller for which the signs must agree checks that itself.
pub fn is_currency_composite(text: &str) -> bool {
    let Some((foreign, rest)) = split_once_exact(text, " @ ") else {
        return false;
    };
    let Some((rate, base)) = split_once_exact(rest, " = ") else {
        return false;
    };
    let (Some(foreign), Some(rate), Some(base)) = (
        symbol_amount(foreign),
        rate_symbols(rate.trim_end_matches(' ')),
        symbol_amount(base),
    ) else {
        return false;
    };
    rate.base == base.symbol && rate.per == foreign.symbol && foreign.symbol != base.symbol
}

fn split_once_exact<'a>(text: &'a str, separator: &str) -> Option<(&'a str, &'a str)> {
    let (left, right) = text.split_once(separator)?;
    (!right.contains(separator)).then_some((left, right))
}

struct SymbolAmount<'a> {
    symbol: &'a str,
}

fn symbol_amount(text: &str) -> Option<SymbolAmount<'_>> {
    let text = text.strip_prefix('-').unwrap_or(text);
    let (symbol, amount) = text.split_once(' ')?;
    (is_symbol(symbol) && is_ascii_decimal(amount)).then_some(SymbolAmount { symbol })
}

/// The rate's two symbols: the base it is quoted in, and the foreign unit.
struct RateSymbols<'a> {
    base: &'a str,
    per: &'a str,
}

fn rate_symbols(text: &str) -> Option<RateSymbols<'_>> {
    let (base, rest) = text.split_once(' ')?;
    let (number, per) = rest.split_once('/')?;
    (is_symbol(base) && (number.is_empty() || is_ascii_decimal(number)) && is_symbol(per))
        .then_some(RateSymbols { base, per })
}

fn is_symbol(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| {
            !c.is_whitespace() && !c.is_ascii_digit() && !matches!(c, '-' | '@' | '=' | '/')
        })
}

fn is_ascii_decimal(text: &str) -> bool {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && (!fraction.is_empty() || !text.ends_with('.'))
}

#[cfg(test)]
#[path = "currency_composite_tests.rs"]
mod tests;
