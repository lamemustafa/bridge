use bridge_tally_primitives::TallyDate;
use bridge_tally_protocol::{
    decode_tally_xml_response_bytes_limited, native_outstandings::parse_native_bill_rows,
    ExpectedTallyTextEncoding, TallyTextDecodeError, TallyTextEncoding,
};
use sha2::{Digest, Sha256};

const LED_ASCII: &[u8] = include_bytes!("fixtures/encoding/led-ascii.bin");
const LED_UTF16: &[u8] = include_bytes!("fixtures/encoding/led-utf16.bin");
const BILLS_ASCII: &[u8] = include_bytes!("fixtures/encoding/bills-ascii.bin");
const BILLS_UTF16: &[u8] = include_bytes!("fixtures/encoding/bills-utf16.bin");
const UTF8_CONTENT_TYPE: &str = "text/xml; charset=utf-8";
const UTF16_CONTENT_TYPE: &str = "text/xml; charset=utf-16";
const PARTY: &str = "BVL एप्सिलॉन ट्रेडर्स";

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn bomless_utf16_response_cannot_be_decoded_under_a_utf8_contract() {
    assert_eq!(&LED_UTF16[..4], &[0x3c, 0x00, 0x45, 0x00]);
    assert_eq!(
        decode_tally_xml_response_bytes_limited(
            LED_UTF16,
            UTF8_CONTENT_TYPE,
            ExpectedTallyTextEncoding::Utf8,
            LED_UTF16.len(),
        )
        .expect_err("BOM-less UTF-16 must not become NUL-interleaved UTF-8 XML"),
        TallyTextDecodeError::ObservedEncodingMismatch,
    );
}

#[test]
fn captured_bomless_utf16_response_decodes_exactly() {
    let decoded = decode_tally_xml_response_bytes_limited(
        LED_UTF16,
        UTF16_CONTENT_TYPE,
        ExpectedTallyTextEncoding::Utf16Le,
        LED_UTF16.len(),
    )
    .expect("captured UTF-16LE ledger response must decode");

    assert_eq!(decoded.encoding, TallyTextEncoding::Utf16Le);
    assert_eq!(
        sha256_hex(decoded.text.as_bytes()),
        "6efea658fe5bf34455a9ca91c5395aa9c767a7558803cf7969a0e6b6187d8093",
    );
    assert!(decoded.text.contains(&format!("<LEDGER NAME=\"{PARTY}\"")));
    assert!(decoded.text.contains(&format!("<NAME>{PARTY}</NAME>")));
}

#[test]
fn response_content_type_must_agree_with_the_expected_encoding() {
    assert_eq!(
        decode_tally_xml_response_bytes_limited(
            LED_UTF16,
            UTF8_CONTENT_TYPE,
            ExpectedTallyTextEncoding::Utf16Le,
            LED_UTF16.len(),
        )
        .expect_err("UTF-8 declaration must contradict UTF-16 expectation"),
        TallyTextDecodeError::DeclaredEncodingMismatch,
    );
}

#[test]
fn missing_ambiguous_or_non_xml_content_types_fail_closed() {
    for content_type in [
        "text/xml",
        "application/xml; charset=utf-16",
        "text/xml; charset=utf-16; charset=utf-16",
        "text/xml; charset=utf-16; boundary=unexpected",
    ] {
        assert_eq!(
            decode_tally_xml_response_bytes_limited(
                LED_UTF16,
                content_type,
                ExpectedTallyTextEncoding::Utf16Le,
                LED_UTF16.len(),
            )
            .expect_err("ambiguous response declaration must fail"),
            TallyTextDecodeError::UnsupportedContentType,
        );
    }
}

#[test]
fn captured_bills_round_trip_the_exact_non_ascii_party_through_decode_and_parse() {
    let decoded = decode_tally_xml_response_bytes_limited(
        BILLS_UTF16,
        UTF16_CONTENT_TYPE,
        ExpectedTallyTextEncoding::Utf16Le,
        BILLS_UTF16.len(),
    )
    .expect("captured UTF-16LE bills response must decode");
    assert_eq!(
        sha256_hex(decoded.text.as_bytes()),
        "fd151544c5e06726eb778f01f33dad76b3750923d07548cdab293ded86837178",
    );

    let books_from = TallyDate::parse("20260401").unwrap();
    let as_of = TallyDate::parse("20260801").unwrap();
    let rows = parse_native_bill_rows(&decoded.text, &books_from, &as_of)
        .expect("decoded captured bills must pass the tolerant native parser");
    assert_eq!(
        rows.iter()
            .find(|row| row.reference == "UNICODE-REC")
            .expect("captured Unicode bill row")
            .party,
        PARTY,
    );
}

#[test]
fn ascii_capture_has_the_measured_sixteen_question_mark_substitutions() {
    let ascii = decode_tally_xml_response_bytes_limited(
        BILLS_ASCII,
        UTF8_CONTENT_TYPE,
        ExpectedTallyTextEncoding::Utf8,
        BILLS_ASCII.len(),
    )
    .expect("captured UTF-8 bills response must decode")
    .text;
    let utf16 = decode_tally_xml_response_bytes_limited(
        BILLS_UTF16,
        UTF16_CONTENT_TYPE,
        ExpectedTallyTextEncoding::Utf16Le,
        BILLS_UTF16.len(),
    )
    .expect("captured UTF-16LE bills response must decode")
    .text;

    let ascii_chars = ascii.chars().collect::<Vec<_>>();
    let utf16_chars = utf16.chars().collect::<Vec<_>>();
    assert_eq!(ascii_chars.len(), utf16_chars.len());
    let differences = ascii_chars
        .iter()
        .zip(&utf16_chars)
        .enumerate()
        .filter(|(_, (ascii, utf16))| ascii != utf16)
        .map(|(offset, (ascii, utf16))| (offset, *ascii, *utf16))
        .collect::<Vec<_>>();
    assert_eq!(differences.len(), 16);
    assert!(differences.iter().all(|(_, ascii, _)| *ascii == '?'));
    assert_eq!(
        differences
            .iter()
            .map(|(_, _, intended)| intended)
            .collect::<String>(),
        PARTY
            .chars()
            .filter(|character| !character.is_ascii())
            .collect::<String>(),
    );

    assert_eq!(LED_ASCII.len(), 7_114);
    assert_eq!(LED_UTF16.len(), 14_228);
}
