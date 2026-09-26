//! Tally's composite amount: a foreign-currency amount, its rate and the base
//! amount, written into one field when a foreign amount is entered on a ledger
//! whose own currency is the base.
//!
//! Captured forms (licensed TallyPrime 7.1, a synthetic several-currency book):
//! `-$ 100.00 @ I₹ 86/$  = -I₹ 8600.00` on a voucher's entries and bill
//! allocation (`fixtures/agent/vouchers-forex-composite-20260915`), and with an
//! empty rate `$ 0.00 @ I₹ /$  = I₹ 0.00` in a Trial Balance
//! (`fixtures/trial_balance_currency_forex_live`). Only the shape is classified here. No value
//! is read from a composite, so nothing downstream can mistake one for an
//! amount; a caller that finds a composite sets the row aside or refuses it.
//!
//! Strict by design: a composite cut short, doubled, or with a non-ASCII digit
//! is not one, and falls through to the caller's own amount parse, which
//! refuses it.

/// Whether `text` is exactly one composite: `<amount> @ <rate> = <amount>`,
/// each amount an optional `-`, a currency symbol, one space and an ASCII
/// decimal, and the rate `<symbol> <decimal>/<symbol>` or, empty,
/// `<symbol> /<symbol>`.
pub fn is_currency_composite(text: &str) -> bool {
    let Some((foreign, rest)) = split_once_exact(text, " @ ") else {
        return false;
    };
    let Some((rate, base)) = split_once_exact(rest, " = ") else {
        return false;
    };
    is_symbol_amount(foreign) && is_rate(rate.trim_end_matches(' ')) && is_symbol_amount(base)
}

fn split_once_exact<'a>(text: &'a str, separator: &str) -> Option<(&'a str, &'a str)> {
    let (left, right) = text.split_once(separator)?;
    (!right.contains(separator)).then_some((left, right))
}

fn is_symbol_amount(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    match text.split_once(' ') {
        Some((symbol, amount)) => is_symbol(symbol) && is_ascii_decimal(amount),
        None => false,
    }
}

fn is_rate(text: &str) -> bool {
    let Some((symbol, rest)) = text.split_once(' ') else {
        return false;
    };
    let Some((number, per)) = rest.split_once('/') else {
        return false;
    };
    is_symbol(symbol) && (number.is_empty() || is_ascii_decimal(number)) && is_symbol(per)
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
