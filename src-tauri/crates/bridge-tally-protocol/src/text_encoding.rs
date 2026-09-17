//! Byte-level Tally text encoding: request encoding, response content-type
//! validation, and bounded, strict decoding of UTF-8 and UTF-16 response bodies.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TallyTextEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16LeBom,
    Utf16BeBom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedTallyTextEncoding {
    Utf8,
    Utf16Le,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTallyText {
    pub text: String,
    pub encoding: TallyTextEncoding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TallyTextDecodeError {
    TooLarge,
    InvalidUtf8,
    InvalidUtf16Le,
    InvalidUtf16Be,
    UnsupportedContentType,
    DeclaredEncodingMismatch,
    ObservedEncodingMismatch,
}

/// Encodes one Tally XML request as a BOM-prefixed UTF-16LE HTTP entity.
/// The caller must pair these bytes with `text/xml; charset=utf-16`.
pub fn encode_tally_xml_request_utf16le(xml: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(xml.len().saturating_mul(2).saturating_add(2));
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

/// Incremental, decoded-size-bounded Tally text decoder.
///
/// The decoder retains the decoded UTF-8 text because the current protocol
/// parsers consume `&str`, but it never retains the complete encoded body. Its
/// boundary state is limited to a possible BOM, an incomplete UTF-8 scalar,
/// or one incomplete UTF-16 code unit/surrogate pair.
pub struct TallyTextStreamDecoder {
    max_decoded_bytes: usize,
    expected_encoding: Option<ExpectedTallyTextEncoding>,
    prefix: Vec<u8>,
    mode: Option<TallyTextStreamMode>,
    text: String,
    decoded_sha256: Sha256,
}

enum TallyTextStreamMode {
    Utf8 {
        encoding: TallyTextEncoding,
        pending: Vec<u8>,
    },
    Utf16 {
        encoding: TallyTextEncoding,
        little_endian: bool,
        pending_byte: Option<u8>,
        pending_high_surrogate: Option<u16>,
    },
}

/// Completed incremental decoding evidence. The digest covers the decoded
/// text re-encoded as UTF-8, with any wire BOM removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamDecodedTallyText {
    pub text: String,
    pub encoding: TallyTextEncoding,
    pub decoded_bytes: usize,
    pub decoded_sha256: String,
}

impl TallyTextStreamDecoder {
    pub fn new(max_decoded_bytes: usize) -> Self {
        Self {
            max_decoded_bytes,
            expected_encoding: None,
            prefix: Vec::with_capacity(3),
            mode: None,
            text: String::new(),
            decoded_sha256: Sha256::new(),
        }
    }

    pub fn new_with_expected_encoding(
        max_decoded_bytes: usize,
        expected_encoding: ExpectedTallyTextEncoding,
    ) -> Self {
        Self {
            expected_encoding: Some(expected_encoding),
            ..Self::new(max_decoded_bytes)
        }
    }

    /// Consumes one encoded response chunk. An error makes the partial decoder
    /// unsuitable for evidence; callers must discard it and must not interpret
    /// the retained prefix as a partial response.
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Result<(), TallyTextDecodeError> {
        let mut offset = 0;
        while self.mode.is_none() && offset < chunk.len() {
            self.prefix.push(chunk[offset]);
            offset += 1;
            let decision = match self.expected_encoding {
                Some(expected) => expected_tally_text_prefix_decision(&self.prefix, expected),
                None => tally_text_prefix_decision(&self.prefix),
            };
            match decision {
                TallyTextPrefixDecision::NeedMore => {}
                TallyTextPrefixDecision::Utf8Bom => {
                    self.prefix.clear();
                    self.mode = Some(TallyTextStreamMode::Utf8 {
                        encoding: TallyTextEncoding::Utf8Bom,
                        pending: Vec::with_capacity(3),
                    });
                }
                TallyTextPrefixDecision::Utf16LeBom => {
                    self.prefix.clear();
                    self.mode = Some(TallyTextStreamMode::Utf16 {
                        encoding: TallyTextEncoding::Utf16LeBom,
                        little_endian: true,
                        pending_byte: None,
                        pending_high_surrogate: None,
                    });
                }
                TallyTextPrefixDecision::Utf16LeWithoutBom => {
                    self.mode = Some(TallyTextStreamMode::Utf16 {
                        encoding: TallyTextEncoding::Utf16Le,
                        little_endian: true,
                        pending_byte: None,
                        pending_high_surrogate: None,
                    });
                }
                TallyTextPrefixDecision::Utf16BeBom => {
                    self.prefix.clear();
                    self.mode = Some(TallyTextStreamMode::Utf16 {
                        encoding: TallyTextEncoding::Utf16BeBom,
                        little_endian: false,
                        pending_byte: None,
                        pending_high_surrogate: None,
                    });
                }
                TallyTextPrefixDecision::Utf8WithoutBom => {
                    self.mode = Some(TallyTextStreamMode::Utf8 {
                        encoding: TallyTextEncoding::Utf8,
                        pending: Vec::with_capacity(3),
                    });
                }
                TallyTextPrefixDecision::EncodingMismatch => {
                    return Err(TallyTextDecodeError::ObservedEncodingMismatch);
                }
            }
        }

        if self.mode.is_none() {
            return Ok(());
        }
        if !self.prefix.is_empty() {
            let prefix = std::mem::take(&mut self.prefix);
            self.process_selected(&prefix)?;
        }
        self.process_selected(&chunk[offset..])
    }

    pub fn finish(mut self) -> Result<StreamDecodedTallyText, TallyTextDecodeError> {
        if self.mode.is_none() {
            self.mode = Some(match self.expected_encoding {
                Some(ExpectedTallyTextEncoding::Utf16Le) => TallyTextStreamMode::Utf16 {
                    encoding: TallyTextEncoding::Utf16Le,
                    little_endian: true,
                    pending_byte: None,
                    pending_high_surrogate: None,
                },
                Some(ExpectedTallyTextEncoding::Utf8) | None => TallyTextStreamMode::Utf8 {
                    encoding: TallyTextEncoding::Utf8,
                    pending: Vec::with_capacity(3),
                },
            });
            let prefix = std::mem::take(&mut self.prefix);
            self.process_selected(&prefix)?;
        }
        let encoding = match self.mode.as_ref().expect("stream mode is selected") {
            TallyTextStreamMode::Utf8 { encoding, pending } => {
                if !pending.is_empty() {
                    return Err(TallyTextDecodeError::InvalidUtf8);
                }
                *encoding
            }
            TallyTextStreamMode::Utf16 {
                encoding,
                pending_byte,
                pending_high_surrogate,
                ..
            } => {
                if pending_byte.is_some() || pending_high_surrogate.is_some() {
                    return Err(invalid_utf16(*encoding));
                }
                *encoding
            }
        };
        // A BOM-less UTF-16LE XML entity is byte-valid UTF-8 but decodes to
        // interleaved NULs. Literal NUL is never legal XML data, so reject it
        // under an explicit UTF-8 contract instead of guessing an encoding.
        if self.expected_encoding == Some(ExpectedTallyTextEncoding::Utf8)
            && self.text.contains('\0')
        {
            return Err(TallyTextDecodeError::ObservedEncodingMismatch);
        }
        let decoded_bytes = self.text.len();
        let decoded_sha256 = encode_sha256(self.decoded_sha256.finalize());
        Ok(StreamDecodedTallyText {
            text: self.text,
            encoding,
            decoded_bytes,
            decoded_sha256,
        })
    }

    fn process_selected(&mut self, bytes: &[u8]) -> Result<(), TallyTextDecodeError> {
        match self.mode.as_mut().expect("stream mode is selected") {
            TallyTextStreamMode::Utf8 { pending, .. } => process_utf8_chunk(
                pending,
                bytes,
                self.max_decoded_bytes,
                &mut self.text,
                &mut self.decoded_sha256,
            ),
            TallyTextStreamMode::Utf16 {
                encoding,
                little_endian,
                pending_byte,
                pending_high_surrogate,
            } => process_utf16_chunk(
                *encoding,
                *little_endian,
                pending_byte,
                pending_high_surrogate,
                bytes,
                self.max_decoded_bytes,
                &mut self.text,
                &mut self.decoded_sha256,
            ),
        }
    }
}

enum TallyTextPrefixDecision {
    NeedMore,
    Utf8Bom,
    Utf16LeWithoutBom,
    Utf16LeBom,
    Utf16BeBom,
    Utf8WithoutBom,
    EncodingMismatch,
}

fn tally_text_prefix_decision(prefix: &[u8]) -> TallyTextPrefixDecision {
    const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
    const UTF16_LE_BOM: &[u8] = &[0xFF, 0xFE];
    const UTF16_BE_BOM: &[u8] = &[0xFE, 0xFF];
    if prefix == UTF8_BOM {
        TallyTextPrefixDecision::Utf8Bom
    } else if prefix == UTF16_LE_BOM {
        TallyTextPrefixDecision::Utf16LeBom
    } else if prefix == UTF16_BE_BOM {
        TallyTextPrefixDecision::Utf16BeBom
    } else if UTF8_BOM.starts_with(prefix)
        || UTF16_LE_BOM.starts_with(prefix)
        || UTF16_BE_BOM.starts_with(prefix)
    {
        TallyTextPrefixDecision::NeedMore
    } else {
        TallyTextPrefixDecision::Utf8WithoutBom
    }
}

fn expected_tally_text_prefix_decision(
    prefix: &[u8],
    expected: ExpectedTallyTextEncoding,
) -> TallyTextPrefixDecision {
    match (expected, tally_text_prefix_decision(prefix)) {
        (_, TallyTextPrefixDecision::NeedMore) => TallyTextPrefixDecision::NeedMore,
        (ExpectedTallyTextEncoding::Utf8, TallyTextPrefixDecision::Utf8Bom) => {
            TallyTextPrefixDecision::Utf8Bom
        }
        (ExpectedTallyTextEncoding::Utf8, TallyTextPrefixDecision::Utf8WithoutBom) => {
            TallyTextPrefixDecision::Utf8WithoutBom
        }
        (ExpectedTallyTextEncoding::Utf16Le, TallyTextPrefixDecision::Utf16LeBom) => {
            TallyTextPrefixDecision::Utf16LeBom
        }
        (ExpectedTallyTextEncoding::Utf16Le, TallyTextPrefixDecision::Utf8WithoutBom) => {
            TallyTextPrefixDecision::Utf16LeWithoutBom
        }
        _ => TallyTextPrefixDecision::EncodingMismatch,
    }
}

fn process_utf8_chunk(
    pending: &mut Vec<u8>,
    mut bytes: &[u8],
    max_decoded_bytes: usize,
    text: &mut String,
    digest: &mut Sha256,
) -> Result<(), TallyTextDecodeError> {
    while !pending.is_empty() && !bytes.is_empty() {
        pending.push(bytes[0]);
        bytes = &bytes[1..];
        match std::str::from_utf8(pending) {
            Ok(decoded) => {
                append_decoded(decoded, max_decoded_bytes, text, digest)?;
                pending.clear();
            }
            Err(error) if error.error_len().is_some() => {
                return Err(TallyTextDecodeError::InvalidUtf8);
            }
            Err(_) => {}
        }
    }
    if bytes.is_empty() {
        return Ok(());
    }
    match std::str::from_utf8(bytes) {
        Ok(decoded) => append_decoded(decoded, max_decoded_bytes, text, digest),
        Err(error) if error.error_len().is_some() => Err(TallyTextDecodeError::InvalidUtf8),
        Err(error) => {
            let valid = &bytes[..error.valid_up_to()];
            let tail = &bytes[error.valid_up_to()..];
            let decoded =
                std::str::from_utf8(valid).map_err(|_| TallyTextDecodeError::InvalidUtf8)?;
            append_decoded(decoded, max_decoded_bytes, text, digest)?;
            if tail.len() > 3 {
                return Err(TallyTextDecodeError::InvalidUtf8);
            }
            pending.extend_from_slice(tail);
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_utf16_chunk(
    encoding: TallyTextEncoding,
    little_endian: bool,
    pending_byte: &mut Option<u8>,
    pending_high_surrogate: &mut Option<u16>,
    bytes: &[u8],
    max_decoded_bytes: usize,
    text: &mut String,
    digest: &mut Sha256,
) -> Result<(), TallyTextDecodeError> {
    let mut offset = 0;
    if let Some(first) = pending_byte.take() {
        let Some(second) = bytes.first().copied() else {
            *pending_byte = Some(first);
            return Ok(());
        };
        process_utf16_unit(
            encoding,
            if little_endian {
                u16::from_le_bytes([first, second])
            } else {
                u16::from_be_bytes([first, second])
            },
            pending_high_surrogate,
            max_decoded_bytes,
            text,
            digest,
        )?;
        offset = 1;
    }
    while offset + 1 < bytes.len() {
        let pair = [bytes[offset], bytes[offset + 1]];
        let unit = if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        };
        process_utf16_unit(
            encoding,
            unit,
            pending_high_surrogate,
            max_decoded_bytes,
            text,
            digest,
        )?;
        offset += 2;
    }
    if offset < bytes.len() {
        *pending_byte = Some(bytes[offset]);
    }
    Ok(())
}

fn process_utf16_unit(
    encoding: TallyTextEncoding,
    unit: u16,
    pending_high_surrogate: &mut Option<u16>,
    max_decoded_bytes: usize,
    text: &mut String,
    digest: &mut Sha256,
) -> Result<(), TallyTextDecodeError> {
    if (0xD800..=0xDBFF).contains(&unit) {
        if pending_high_surrogate.replace(unit).is_some() {
            return Err(invalid_utf16(encoding));
        }
        return Ok(());
    }
    let scalar = if (0xDC00..=0xDFFF).contains(&unit) {
        let high = pending_high_surrogate
            .take()
            .ok_or_else(|| invalid_utf16(encoding))?;
        0x1_0000 + (((u32::from(high) - 0xD800) << 10) | (u32::from(unit) - 0xDC00))
    } else {
        if pending_high_surrogate.take().is_some() {
            return Err(invalid_utf16(encoding));
        }
        u32::from(unit)
    };
    let character = char::from_u32(scalar).ok_or_else(|| invalid_utf16(encoding))?;
    let mut encoded = [0_u8; 4];
    append_decoded(
        character.encode_utf8(&mut encoded),
        max_decoded_bytes,
        text,
        digest,
    )
}

fn append_decoded(
    decoded: &str,
    max_decoded_bytes: usize,
    text: &mut String,
    digest: &mut Sha256,
) -> Result<(), TallyTextDecodeError> {
    if text.len().saturating_add(decoded.len()) > max_decoded_bytes {
        return Err(TallyTextDecodeError::TooLarge);
    }
    text.push_str(decoded);
    digest.update(decoded.as_bytes());
    Ok(())
}

fn invalid_utf16(encoding: TallyTextEncoding) -> TallyTextDecodeError {
    match encoding {
        TallyTextEncoding::Utf16Le | TallyTextEncoding::Utf16LeBom => {
            TallyTextDecodeError::InvalidUtf16Le
        }
        TallyTextEncoding::Utf16BeBom => TallyTextDecodeError::InvalidUtf16Be,
        TallyTextEncoding::Utf8 | TallyTextEncoding::Utf8Bom => TallyTextDecodeError::InvalidUtf8,
    }
}

pub fn validate_tally_xml_response_content_type(
    content_type: &str,
    expected: ExpectedTallyTextEncoding,
) -> Result<(), TallyTextDecodeError> {
    if content_type.chars().any(char::is_control) {
        return Err(TallyTextDecodeError::UnsupportedContentType);
    }
    let mut parts = content_type.split(';');
    let media_type = parts.next().unwrap_or_default().trim();
    if !media_type.eq_ignore_ascii_case("text/xml") {
        return Err(TallyTextDecodeError::UnsupportedContentType);
    }
    let mut declared = None;
    for parameter in parts {
        let Some((name, value)) = parameter.trim().split_once('=') else {
            return Err(TallyTextDecodeError::UnsupportedContentType);
        };
        if !name.trim().eq_ignore_ascii_case("charset") || declared.is_some() {
            return Err(TallyTextDecodeError::UnsupportedContentType);
        }
        declared = Some(value.trim());
    }
    let Some(declared) = declared else {
        return Ok(());
    };
    let declared = match declared {
        value if value.eq_ignore_ascii_case("utf-8") => ExpectedTallyTextEncoding::Utf8,
        value if value.eq_ignore_ascii_case("utf-16") => ExpectedTallyTextEncoding::Utf16Le,
        _ => return Err(TallyTextDecodeError::UnsupportedContentType),
    };
    if declared != expected {
        return Err(TallyTextDecodeError::DeclaredEncodingMismatch);
    }
    Ok(())
}

pub fn decode_tally_xml_response_bytes_limited(
    bytes: impl AsRef<[u8]>,
    content_type: &str,
    expected: ExpectedTallyTextEncoding,
    max_bytes: usize,
) -> Result<DecodedTallyText, TallyTextDecodeError> {
    let bytes = bytes.as_ref();
    if bytes.len() > max_bytes {
        return Err(TallyTextDecodeError::TooLarge);
    }
    validate_tally_xml_response_content_type(content_type, expected)?;
    let mut decoder = TallyTextStreamDecoder::new_with_expected_encoding(max_bytes, expected);
    decoder.push_chunk(bytes)?;
    let decoded = decoder.finish()?;
    Ok(DecodedTallyText {
        text: decoded.text,
        encoding: decoded.encoding,
    })
}

fn encode_sha256(bytes: impl AsRef<[u8]>) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in bytes.as_ref() {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

pub fn decode_tally_text_bytes_limited(
    bytes: impl AsRef<[u8]>,
    max_bytes: usize,
) -> Result<DecodedTallyText, TallyTextDecodeError> {
    let bytes = bytes.as_ref();
    if bytes.len() > max_bytes {
        return Err(TallyTextDecodeError::TooLarge);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(payload.to_vec())
            .map(|text| DecodedTallyText {
                text,
                encoding: TallyTextEncoding::Utf8Bom,
            })
            .map_err(|_| TallyTextDecodeError::InvalidUtf8);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if payload.len() % 2 != 0 {
            return Err(TallyTextDecodeError::InvalidUtf16Le);
        }
        let units = payload
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map(|text| DecodedTallyText {
                text,
                encoding: TallyTextEncoding::Utf16LeBom,
            })
            .map_err(|_| TallyTextDecodeError::InvalidUtf16Le);
    }
    if let Some(payload) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        if payload.len() % 2 != 0 {
            return Err(TallyTextDecodeError::InvalidUtf16Be);
        }
        let units = payload
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units)
            .map(|text| DecodedTallyText {
                text,
                encoding: TallyTextEncoding::Utf16BeBom,
            })
            .map_err(|_| TallyTextDecodeError::InvalidUtf16Be);
    }
    String::from_utf8(bytes.to_vec())
        .map(|text| DecodedTallyText {
            text,
            encoding: TallyTextEncoding::Utf8,
        })
        .map_err(|_| TallyTextDecodeError::InvalidUtf8)
}

pub fn decode_xml_bytes(bytes: impl AsRef<[u8]>) -> anyhow::Result<String> {
    decode_xml_bytes_limited(bytes, usize::MAX)
}

pub fn decode_xml_bytes_limited(
    bytes: impl AsRef<[u8]>,
    max_bytes: usize,
) -> anyhow::Result<String> {
    match decode_tally_text_bytes_limited(bytes, max_bytes) {
        Ok(decoded) => Ok(decoded.text),
        Err(TallyTextDecodeError::TooLarge) => {
            anyhow::bail!("Tally response exceeded the {max_bytes}-byte limit")
        }
        Err(TallyTextDecodeError::InvalidUtf8) => {
            anyhow::bail!("Tally returned an invalid UTF-8 XML response")
        }
        Err(TallyTextDecodeError::InvalidUtf16Le) => {
            anyhow::bail!("Tally returned an invalid UTF-16LE XML response")
        }
        Err(TallyTextDecodeError::InvalidUtf16Be) => {
            anyhow::bail!("Tally returned an invalid UTF-16BE XML response")
        }
        Err(
            TallyTextDecodeError::UnsupportedContentType
            | TallyTextDecodeError::DeclaredEncodingMismatch
            | TallyTextDecodeError::ObservedEncodingMismatch,
        ) => anyhow::bail!("Tally response encoding contract was invalid"),
    }
}

#[cfg(test)]
#[path = "text_encoding_tests.rs"]
mod tests;
