use bridge_tally_protocol::{
    decode_tally_text_bytes_limited, StreamDecodedTallyText, TallyTextDecodeError,
    TallyTextEncoding, TallyTextStreamDecoder,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
enum WireEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

fn encode(text: &str, encoding: WireEncoding) -> Vec<u8> {
    match encoding {
        WireEncoding::Utf8 => text.as_bytes().to_vec(),
        WireEncoding::Utf8Bom => [vec![0xEF, 0xBB, 0xBF], text.as_bytes().to_vec()].concat(),
        WireEncoding::Utf16Le => {
            let mut bytes = vec![0xFF, 0xFE];
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes
        }
        WireEncoding::Utf16Be => {
            let mut bytes = vec![0xFE, 0xFF];
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            bytes
        }
    }
}

fn expected_encoding(encoding: WireEncoding) -> TallyTextEncoding {
    match encoding {
        WireEncoding::Utf8 => TallyTextEncoding::Utf8,
        WireEncoding::Utf8Bom => TallyTextEncoding::Utf8Bom,
        WireEncoding::Utf16Le => TallyTextEncoding::Utf16LeBom,
        WireEncoding::Utf16Be => TallyTextEncoding::Utf16BeBom,
    }
}

fn decode_chunks(
    chunks: impl IntoIterator<Item = Vec<u8>>,
    max_decoded_bytes: usize,
) -> Result<StreamDecodedTallyText, TallyTextDecodeError> {
    let mut decoder = TallyTextStreamDecoder::new(max_decoded_bytes);
    for chunk in chunks {
        decoder.push_chunk(&chunk)?;
    }
    decoder.finish()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn every_single_split_matches_one_shot_decoding_for_all_encodings() {
    let text = "<ENVELOPE>ASCII € 🧾 日本語</ENVELOPE>";
    for encoding in [
        WireEncoding::Utf8,
        WireEncoding::Utf8Bom,
        WireEncoding::Utf16Le,
        WireEncoding::Utf16Be,
    ] {
        let bytes = encode(text, encoding);
        let one_shot = decode_tally_text_bytes_limited(&bytes, bytes.len())
            .expect("one-shot reference decoding");
        for split in 0..=bytes.len() {
            let streamed = decode_chunks(
                [bytes[..split].to_vec(), bytes[split..].to_vec()],
                text.len(),
            )
            .unwrap_or_else(|error| panic!("split {split} failed: {error:?}"));
            assert_eq!(streamed.text, one_shot.text, "split {split}");
            assert_eq!(streamed.encoding, expected_encoding(encoding));
            assert_eq!(streamed.decoded_bytes, text.len());
            assert_eq!(streamed.decoded_sha256, sha256_hex(text.as_bytes()));
        }
    }
}

#[test]
fn one_byte_chunks_cross_boms_utf8_scalars_utf16_units_and_surrogate_pairs() {
    let text = "A€🧾Z";
    for encoding in [
        WireEncoding::Utf8,
        WireEncoding::Utf8Bom,
        WireEncoding::Utf16Le,
        WireEncoding::Utf16Be,
    ] {
        let bytes = encode(text, encoding);
        let chunks = bytes.iter().map(|byte| vec![*byte]).collect::<Vec<_>>();
        let decoded = decode_chunks(chunks, text.len()).expect("decode one-byte chunks");
        assert_eq!(decoded.text, text);
        assert_eq!(decoded.encoding, expected_encoding(encoding));
    }
}

#[test]
fn decoded_cap_is_inclusive_and_independent_of_wire_encoding() {
    let text = "A€🧾";
    assert_eq!(text.len(), 8);
    for encoding in [
        WireEncoding::Utf8,
        WireEncoding::Utf8Bom,
        WireEncoding::Utf16Le,
        WireEncoding::Utf16Be,
    ] {
        let bytes = encode(text, encoding);
        let exact = decode_chunks(
            bytes.iter().map(|byte| vec![*byte]).collect::<Vec<_>>(),
            text.len(),
        )
        .expect("exact decoded cap is inclusive");
        assert_eq!(exact.decoded_bytes, text.len());

        let error =
            decode_chunks([bytes], text.len() - 1).expect_err("decoded cap plus one must fail");
        assert_eq!(error, TallyTextDecodeError::TooLarge);
    }

    assert_eq!(
        decode_chunks([Vec::new()], 0)
            .expect("empty response at zero cap")
            .text,
        ""
    );
    assert_eq!(
        decode_chunks([b"x".to_vec()], 0).expect_err("non-empty response exceeds zero cap"),
        TallyTextDecodeError::TooLarge
    );
}

#[test]
fn invalid_and_truncated_utf8_never_finish_as_partial_text() {
    for bytes in [
        vec![0xE2, 0x82],
        vec![0xF0, 0x28, 0x8C, 0xBC],
        vec![0xEF],
        vec![0xEF, 0xBB],
    ] {
        assert_eq!(
            decode_chunks(bytes.iter().map(|byte| vec![*byte]), 64)
                .expect_err("invalid UTF-8 must fail"),
            TallyTextDecodeError::InvalidUtf8
        );
    }
}

#[test]
fn invalid_and_truncated_utf16_tails_are_encoding_specific() {
    let cases = [
        (vec![0xFF, 0xFE, 0x41], TallyTextDecodeError::InvalidUtf16Le),
        (
            vec![0xFF, 0xFE, 0x3D, 0xD8],
            TallyTextDecodeError::InvalidUtf16Le,
        ),
        (
            vec![0xFF, 0xFE, 0x00, 0xDC],
            TallyTextDecodeError::InvalidUtf16Le,
        ),
        (
            vec![0xFF, 0xFE, 0x3D, 0xD8, 0x41, 0x00],
            TallyTextDecodeError::InvalidUtf16Le,
        ),
        (vec![0xFE, 0xFF, 0x00], TallyTextDecodeError::InvalidUtf16Be),
        (
            vec![0xFE, 0xFF, 0xD8, 0x3D],
            TallyTextDecodeError::InvalidUtf16Be,
        ),
        (
            vec![0xFE, 0xFF, 0xDC, 0x00],
            TallyTextDecodeError::InvalidUtf16Be,
        ),
        (
            vec![0xFE, 0xFF, 0xD8, 0x3D, 0x00, 0x41],
            TallyTextDecodeError::InvalidUtf16Be,
        ),
    ];
    for (bytes, expected) in cases {
        assert_eq!(
            decode_chunks(bytes.iter().map(|byte| vec![*byte]), 64)
                .expect_err("invalid UTF-16 must fail"),
            expected
        );
    }
}

#[test]
fn decoded_digest_is_chunk_and_wire_encoding_independent() {
    let text = "<DATA>€🧾</DATA>";
    let expected = sha256_hex(text.as_bytes());
    let mut observed = Vec::new();
    for encoding in [
        WireEncoding::Utf8,
        WireEncoding::Utf8Bom,
        WireEncoding::Utf16Le,
        WireEncoding::Utf16Be,
    ] {
        let bytes = encode(text, encoding);
        let decoded = decode_chunks(
            bytes.chunks(3).map(<[u8]>::to_vec).collect::<Vec<_>>(),
            text.len(),
        )
        .expect("streamed digest evidence");
        assert_eq!(decoded.decoded_sha256, expected);
        assert_eq!(decoded.decoded_sha256, sha256_hex(decoded.text.as_bytes()));
        observed.push(decoded.decoded_sha256);
    }
    assert!(observed.iter().all(|digest| digest == &observed[0]));
}

#[test]
fn non_bom_utf8_prefixes_that_resemble_a_bom_remain_data() {
    let text = "ﻠ retained";
    let bytes = text.as_bytes();
    assert_eq!(&bytes[..2], &[0xEF, 0xBB]);
    let decoded = decode_chunks(bytes.iter().map(|byte| vec![*byte]), text.len())
        .expect("valid non-BOM UTF-8");
    assert_eq!(decoded.text, text);
    assert_eq!(decoded.encoding, TallyTextEncoding::Utf8);
}
