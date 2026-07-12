pub fn is_valid_gstin(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.len() == 15
        && bytes[0..2].iter().all(u8::is_ascii_digit)
        && bytes[2..7].iter().all(u8::is_ascii_alphabetic)
        && bytes[7..11].iter().all(u8::is_ascii_digit)
        && bytes[11].is_ascii_alphabetic()
        && bytes[12].is_ascii_alphanumeric()
        && bytes[13] == b'Z'
        && bytes[14].is_ascii_alphanumeric()
}

pub fn is_valid_pan(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.len() == 10
        && bytes[0..5].iter().all(u8::is_ascii_alphabetic)
        && bytes[5..9].iter().all(u8::is_ascii_digit)
        && bytes[9].is_ascii_alphabetic()
}

pub fn voucher_balances(debits: i64, credits: i64) -> bool {
    debits == credits
}

#[cfg(test)]
mod tests {
    use super::{is_valid_gstin, is_valid_pan, voucher_balances};

    #[test]
    fn validates_gstin_shape() {
        assert!(is_valid_gstin("27ABCDE1234F1Z5"));
        assert!(!is_valid_gstin("ABCDE1234F"));
    }

    #[test]
    fn validates_pan_shape() {
        assert!(is_valid_pan("ABCDE1234F"));
        assert!(!is_valid_pan("27ABCDE1234F1Z5"));
    }

    #[test]
    fn validates_balanced_voucher() {
        assert!(voucher_balances(10_000, 10_000));
        assert!(!voucher_balances(10_000, 9_999));
    }
}
