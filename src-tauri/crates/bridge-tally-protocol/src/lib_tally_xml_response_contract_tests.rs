use super::*;

#[test]
fn missing_charset_defers_to_expected_encoding_without_weakening_byte_checks() {
    const XML: &str = "<ENVELOPE />";

    let utf8 = decode_tally_xml_response_bytes_limited(
        XML.as_bytes(),
        "text/xml",
        ExpectedTallyTextEncoding::Utf8,
        1024,
    )
    .expect("bare XML content type accepts expected UTF-8 bytes");
    assert_eq!(utf8.text, XML);
    assert_eq!(utf8.encoding, TallyTextEncoding::Utf8);

    let utf16_bytes = encode_tally_xml_request_utf16le(XML);
    let utf16 = decode_tally_xml_response_bytes_limited(
        &utf16_bytes,
        "text/xml",
        ExpectedTallyTextEncoding::Utf16Le,
        1024,
    )
    .expect("bare XML content type accepts expected UTF-16LE bytes");
    assert_eq!(utf16.text, XML);
    assert_eq!(utf16.encoding, TallyTextEncoding::Utf16LeBom);

    assert_eq!(
        decode_tally_xml_response_bytes_limited(
            XML.as_bytes(),
            "text/xml; charset=utf-16",
            ExpectedTallyTextEncoding::Utf8,
            1024,
        ),
        Err(TallyTextDecodeError::DeclaredEncodingMismatch),
    );
    assert_eq!(
        decode_tally_xml_response_bytes_limited(
            &utf16_bytes,
            "text/xml",
            ExpectedTallyTextEncoding::Utf8,
            1024,
        ),
        Err(TallyTextDecodeError::ObservedEncodingMismatch),
    );
}
