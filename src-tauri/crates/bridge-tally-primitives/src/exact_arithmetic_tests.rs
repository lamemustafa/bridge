use super::*;

#[test]
pub fn exact_accumulator_handles_scale_sign_large_values_and_negative_zero() {
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
