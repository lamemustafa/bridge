use chrono::NaiveDate;

pub fn validate_company_name(value: &str) -> Result<(), String> {
    normalize_company_name(value).map(|_| ())
}

pub(crate) fn is_valid_company_number(value: &str) -> bool {
    (1..=16).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn normalize_company_name(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("Select a Tally company before requesting company data".to_string());
    }
    if trimmed.len() > 255 || trimmed.chars().any(char::is_control) {
        return Err(
            "Tally company name must be at most 255 bytes and contain no control characters"
                .to_string(),
        );
    }
    Ok(trimmed.to_string())
}

pub fn normalize_company_guid(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 256
        || !trimmed.is_ascii()
        || trimmed.chars().any(char::is_control)
    {
        return Err(
            "Tally company GUID must be printable ASCII, at most 256 bytes, and contain no control characters"
                .to_string(),
        );
    }
    Ok(trimmed.to_string())
}

pub fn validate_date_range(from: &str, to: &str) -> Result<(), String> {
    fn parse(label: &str, value: &str) -> Result<NaiveDate, String> {
        if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!("{label} must use the YYYYMMDD format"));
        }
        NaiveDate::parse_from_str(value, "%Y%m%d")
            .map_err(|_| format!("{label} must be a valid calendar date"))
    }

    let from_date = parse("From date", from)?;
    let to_date = parse("To date", to)?;
    if from_date > to_date {
        return Err("From date must be on or before To date".to_string());
    }
    Ok(())
}

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
#[path = "validators_tests.rs"]
mod tests;
