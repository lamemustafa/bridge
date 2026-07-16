use std::cmp::Ordering;

/// Exact signed decimal accumulator for already-validated `ExactDecimal`
/// lexemes. It never converts through floating point and treats scale-only
/// differences and negative zero as numerically equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExactDecimalAccumulator {
    negative: bool,
    digits: String,
    scale: usize,
}

impl Default for ExactDecimalAccumulator {
    fn default() -> Self {
        Self {
            negative: false,
            digits: "0".to_string(),
            scale: 0,
        }
    }
}

impl ExactDecimalAccumulator {
    pub(crate) fn add(&mut self, value: &str) {
        let (negative, digits, scale) = decimal_parts(value);
        self.combine(negative, digits, scale);
    }

    pub(crate) fn subtract(&mut self, value: &str) {
        let (negative, digits, scale) = decimal_parts(value);
        let zero = digits.bytes().all(|digit| digit == b'0');
        self.combine(if zero { false } else { !negative }, digits, scale);
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.digits.bytes().all(|digit| digit == b'0')
    }

    pub(crate) fn is_negative_nonzero(&self) -> bool {
        self.negative && !self.is_zero()
    }

    pub(crate) fn equals(&self, value: &str) -> bool {
        let mut difference = self.clone();
        difference.subtract(value);
        difference.is_zero()
    }

    fn combine(&mut self, negative: bool, digits: String, scale: usize) {
        let target_scale = self.scale.max(scale);
        let left = aligned_digits(&self.digits, target_scale - self.scale);
        let right = aligned_digits(&digits, target_scale - scale);
        let (negative, digits) = if self.negative == negative {
            (self.negative, add_unsigned(&left, &right))
        } else {
            match compare_unsigned(&left, &right) {
                Ordering::Greater => (self.negative, subtract_unsigned(&left, &right)),
                Ordering::Less => (negative, subtract_unsigned(&right, &left)),
                Ordering::Equal => (false, "0".to_string()),
            }
        };
        self.negative = negative;
        self.digits = digits;
        self.scale = target_scale;
        self.normalise();
    }

    fn normalise(&mut self) {
        self.digits = self.digits.trim_start_matches('0').to_string();
        while self.scale > 0 && self.digits.ends_with('0') {
            self.digits.pop();
            self.scale -= 1;
        }
        if self.digits.is_empty() {
            self.digits.push('0');
            self.negative = false;
            self.scale = 0;
        }
    }
}

pub(crate) fn numeric_equal(left: &str, right: &str) -> bool {
    let mut difference = ExactDecimalAccumulator::default();
    difference.add(left);
    difference.subtract(right);
    difference.is_zero()
}

pub(crate) fn is_negative_nonzero(value: &str) -> bool {
    let mut parsed = ExactDecimalAccumulator::default();
    parsed.add(value);
    parsed.is_negative_nonzero()
}

pub(crate) fn magnitude_cmp(left: &str, right: &str) -> Ordering {
    let (_, left_digits, left_scale) = decimal_parts(left);
    let (_, right_digits, right_scale) = decimal_parts(right);
    let target_scale = left_scale.max(right_scale);
    compare_unsigned(
        &aligned_digits(&left_digits, target_scale - left_scale),
        &aligned_digits(&right_digits, target_scale - right_scale),
    )
}

pub(crate) fn same_nonzero_sign(left: &str, right: &str) -> bool {
    if numeric_equal(left, "0") || numeric_equal(right, "0") {
        return false;
    }
    is_negative_nonzero(left) == is_negative_nonzero(right)
}

fn decimal_parts(value: &str) -> (bool, String, usize) {
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |unsigned| (true, unsigned));
    let (whole, fractional) = value.split_once('.').unwrap_or((value, ""));
    (negative, format!("{whole}{fractional}"), fractional.len())
}

fn aligned_digits(digits: &str, zeros: usize) -> String {
    let mut aligned = String::with_capacity(digits.len() + zeros);
    aligned.push_str(digits);
    aligned.extend(std::iter::repeat_n('0', zeros));
    aligned
}

fn compare_unsigned(left: &str, right: &str) -> Ordering {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn add_unsigned(left: &str, right: &str) -> String {
    let mut carry = 0_u8;
    let mut result = Vec::new();
    let mut left = left.bytes().rev();
    let mut right = right.bytes().rev();
    loop {
        let left_digit = left.next().map(|byte| byte - b'0');
        let right_digit = right.next().map(|byte| byte - b'0');
        if left_digit.is_none() && right_digit.is_none() && carry == 0 {
            break;
        }
        let sum = left_digit.unwrap_or(0) + right_digit.unwrap_or(0) + carry;
        result.push(b'0' + sum % 10);
        carry = sum / 10;
    }
    result.reverse();
    String::from_utf8(result).expect("decimal digits are ASCII")
}

fn subtract_unsigned(larger: &str, smaller: &str) -> String {
    let mut borrow = 0_i8;
    let mut result = Vec::new();
    let mut smaller = smaller.bytes().rev();
    for byte in larger.bytes().rev() {
        let mut digit = (byte - b'0') as i8 - borrow;
        let subtract = smaller.next().map_or(0, |value| (value - b'0') as i8);
        if digit < subtract {
            digit += 10;
            borrow = 1;
        } else {
            borrow = 0;
        }
        result.push(b'0' + (digit - subtract) as u8);
    }
    while result.len() > 1 && result.last() == Some(&b'0') {
        result.pop();
    }
    result.reverse();
    String::from_utf8(result).expect("decimal digits are ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_accumulator_handles_scale_sign_large_values_and_negative_zero() {
        let mut value = ExactDecimalAccumulator::default();
        value.add("999999999999999999999.0010");
        value.add("-999999999999999999999.001");
        assert!(value.is_zero());
        assert!(numeric_equal("-0.000", "0"));
        assert!(numeric_equal("1.2300", "1.23"));
        assert!(is_negative_nonzero("-0.001"));
        assert!(!is_negative_nonzero("-0.000"));
        assert_eq!(magnitude_cmp("-10.00", "9.999"), Ordering::Greater);
        assert!(same_nonzero_sign("-10", "-1.0"));
    }
}
