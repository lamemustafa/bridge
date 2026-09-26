//! Every composite here is read from a committed capture, never typed.
use super::is_currency_composite;

/// A native Trial Balance of the synthetic several-currency lab book
/// (licensed 7.1), whose amount fields hold composites.
const FOREX_TRIAL_BALANCE: &[u8] =
    include_bytes!("../tests/fixtures/trial_balance_currency_forex_live.utf16le.xml");

fn forex_trial_balance() -> String {
    String::from_utf16(
        &FOREX_TRIAL_BALANCE
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

/// Every amount-field value in the capture that holds an ` @ `.
fn captured_composites() -> Vec<String> {
    let capture = forex_trial_balance();
    ["TBALOPENING", "DEBITTOTALS", "CREDITTOTALS", "TBALCLOSING"]
        .iter()
        .flat_map(|tag| {
            capture
                .split(&format!("<{tag} "))
                .skip(1)
                .filter_map(|tail| {
                    let text = &tail[tail.find('>')? + 1..tail.find(&format!("</{tag}>"))?];
                    text.contains(" @ ").then(|| text.to_string())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn every_captured_composite_is_classified() {
    let composites = captured_composites();
    assert_eq!(composites.len(), 11, "{composites:?}");
    for composite in &composites {
        assert!(is_currency_composite(composite), "{composite}");
    }
}

#[test]
fn the_captured_empty_rate_composite_is_classified() {
    let empty_rate = captured_composites()
        .into_iter()
        .find(|value| value.contains(" /$"))
        .expect("the captured empty-rate composite");
    assert!(is_currency_composite(&empty_rate), "{empty_rate}");
}

#[test]
fn a_damaged_composite_is_not_one() {
    let empty_rate = captured_composites()
        .into_iter()
        .find(|value| value.contains(" /$"))
        .unwrap();
    let rated = captured_composites()
        .into_iter()
        .find(|value| !value.contains(" /$"))
        .expect("a captured composite with a rate");
    let cut = &empty_rate[..empty_rate.find(" = ").unwrap()];
    for damaged in [
        cut.to_string(),
        format!("{empty_rate} @ $ 1/$"),
        format!("{rated} = I\u{20b9} 1.00"),
        empty_rate.replace("0.00", "\u{0660}.00"),
        empty_rate.replacen("$ ", "", 1),
        rated.replacen(".00", ".", 1),
        rated.replacen(" @ ", " @", 1),
    ] {
        assert!(!is_currency_composite(&damaged), "{damaged}");
    }
}

#[test]
fn a_plain_amount_is_not_a_composite() {
    for plain in ["-4250.00", "4250.00", "0.00", "", "$ 100.00", "-I\u{20b9} 8600.00"] {
        assert!(!is_currency_composite(plain), "{plain:?}");
    }
}

/// A `vouchers` read of the same book (#674): the party entry, its bill
/// allocation and the sales entry each hold a composite.
#[test]
fn every_composite_in_the_captured_voucher_is_classified() {
    let bytes: &[u8] =
        include_bytes!("../tests/fixtures/agent/vouchers-forex-composite-20260915.utf16le.xml");
    let capture = String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let amounts: Vec<&str> = capture
        .split("<AMOUNT")
        .skip(1)
        .filter_map(|tail| Some(&tail[tail.find('>')? + 1..tail.find("</AMOUNT>")?]))
        .collect();
    assert_eq!(amounts.len(), 3, "{amounts:?}");
    for amount in amounts {
        assert!(is_currency_composite(amount), "{amount}");
    }
}
