use std::io;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{encode, WireEncoding};

pub const MAX_GENERATED_MASTERS: u32 = 50_000;
pub const MAX_MASTER_TEXT_WIDTH: u16 = 64;
const MAX_ENCODED_MASTER_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasterCorpusSpec {
    pub total_records: u32,
    pub text_width: u16,
    pub seed: u64,
}

impl MasterCorpusSpec {
    pub fn validate(self) -> Result<Self, MasterGenerationError> {
        if !(1..=MAX_GENERATED_MASTERS).contains(&self.total_records) {
            return Err(MasterGenerationError::InvalidRecordCount);
        }
        if self.text_width > MAX_MASTER_TEXT_WIDTH {
            return Err(MasterGenerationError::InvalidTextWidth);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedMasterCorpus {
    pub records: u32,
    pub encoding: WireEncoding,
    pub encoded_bytes: u64,
    pub sha256: String,
    pub expected_semantic_sha256: String,
}

#[derive(Debug, Error)]
pub enum MasterGenerationError {
    #[error("master records must be between 1 and 50000")]
    InvalidRecordCount,
    #[error("master synthetic text width must not exceed 64")]
    InvalidTextWidth,
    #[error("encoded master corpus exceeded the 32 MiB production response limit")]
    EncodedBodyTooLarge,
    #[error("synthetic master generation failed")]
    Io(#[from] io::Error),
}

pub fn generate_master_corpus(
    spec: MasterCorpusSpec,
    encoding: WireEncoding,
) -> Result<(Vec<u8>, GeneratedMasterCorpus), MasterGenerationError> {
    let spec = spec.validate()?;
    let mut counter = CountingWriter { bytes: 0 };
    write_master_xml(&mut counter, spec, None)?;
    let encoded_bytes = predicted_encoded_bytes(counter.bytes, encoding)
        .ok_or(MasterGenerationError::EncodedBodyTooLarge)?;
    if encoded_bytes > MAX_ENCODED_MASTER_BYTES {
        return Err(MasterGenerationError::EncodedBodyTooLarge);
    }

    let mut utf8 = Vec::with_capacity(counter.bytes);
    let mut semantic_digest = Sha256::new();
    semantic_digest.update(b"bridge.tally.synthetic-master-semantics/1\0");
    write_master_xml(&mut utf8, spec, Some(&mut semantic_digest))?;
    debug_assert_eq!(utf8.len(), counter.bytes);
    let text = String::from_utf8(utf8).expect("synthetic master XML is ASCII-compatible UTF-8");
    let bytes = encode(&text, encoding);
    debug_assert_eq!(bytes.len(), encoded_bytes);

    let mut wire_digest = Sha256::new();
    wire_digest.update(b"bridge.tally.synthetic-master-wire/1\0");
    wire_digest.update(&bytes);
    Ok((
        bytes,
        GeneratedMasterCorpus {
            records: spec.total_records,
            encoding,
            encoded_bytes: encoded_bytes as u64,
            sha256: hex::encode(wire_digest.finalize()),
            expected_semantic_sha256: hex::encode(semantic_digest.finalize()),
        },
    ))
}

fn predicted_encoded_bytes(utf8_bytes: usize, encoding: WireEncoding) -> Option<usize> {
    match encoding {
        WireEncoding::Utf8 => Some(utf8_bytes),
        WireEncoding::Utf8Bom => utf8_bytes.checked_add(3),
        // Every generated code point is ASCII, so each UTF-8 byte becomes one
        // UTF-16 code unit plus a two-byte BOM.
        WireEncoding::Utf16Le | WireEncoding::Utf16Be => utf8_bytes.checked_mul(2)?.checked_add(2),
    }
}

fn write_master_xml<W: io::Write>(
    output: &mut W,
    spec: MasterCorpusSpec,
    mut semantic_digest: Option<&mut Sha256>,
) -> io::Result<()> {
    write!(
        output,
        "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COMPANYCONTEXT SCHEMA=\"bridge.tally.ledgers/1\" OBJECTTYPE=\"LEDGER\" NAME=\"BRIDGE SYNTHETIC BOOK\" GUID=\"00000000-0000-4000-8000-000000000001\" RECORDCOUNT=\"{}\" />",
        spec.total_records
    )?;
    let padding = "X".repeat(usize::from(spec.text_width));
    for record in 0..spec.total_records {
        let identity = synthetic_identity(spec.seed, u64::from(record));
        let guid = format!(
            "00000000-0000-4000-8{:03x}-{:012x}",
            record & 0x0fff,
            identity & 0x0000_ffff_ffff_ffff
        );
        let remote_id = format!("bridge-synthetic-master-{identity:016x}");
        let master_id = u64::from(record) + 1;
        let alter_id = master_id.saturating_mul(2);
        let name = format!("BRIDGE SYNTHETIC LEDGER {record:05}-{padding}");
        let parent = "BRIDGE SYNTHETIC GROUP";
        let opening_balance = if record % 2 == 0 {
            "-123.450"
        } else {
            "123.450"
        };
        write!(
            output,
            "<LEDGER NAME=\"{name}\" GUID=\"{guid}\" REMOTEID=\"{remote_id}\" MASTERID=\"{master_id}\" ALTERID=\"{alter_id}\"><PARENT>{parent}</PARENT><OPENINGBALANCE>{opening_balance}</OPENINGBALANCE></LEDGER>"
        )?;
        if let Some(digest) = semantic_digest.as_deref_mut() {
            semantic_field(digest, b"record_index", record.to_string().as_bytes());
            semantic_field(digest, b"name", name.as_bytes());
            semantic_field(digest, b"guid", guid.as_bytes());
            semantic_field(digest, b"remote_id", remote_id.as_bytes());
            semantic_field(digest, b"master_id", master_id.to_string().as_bytes());
            semantic_field(digest, b"alter_id", alter_id.to_string().as_bytes());
            semantic_field(digest, b"parent", parent.as_bytes());
            semantic_field(digest, b"opening_balance", opening_balance.as_bytes());
        }
    }
    output.write_all(b"</DATA></BODY></ENVELOPE>")
}

fn semantic_field(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u16).to_be_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn synthetic_identity(seed: u64, record: u64) -> u64 {
    let mut value = seed ^ record.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

struct CountingWriter {
    bytes: usize,
}

impl io::Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "master corpus too large"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    #[test]
    fn master_generation_is_deterministic_and_parser_valid() {
        let spec = MasterCorpusSpec {
            total_records: 1_000,
            text_width: 8,
            seed: 17,
        };
        let (first_bytes, first) = generate_master_corpus(spec, WireEncoding::Utf8).unwrap();
        let (second_bytes, second) = generate_master_corpus(spec, WireEncoding::Utf8).unwrap();
        assert_eq!(first_bytes, second_bytes);
        assert_eq!(first, second);
        let xml = decode(&first_bytes).unwrap();
        assert_eq!(xml.matches("<LEDGER ").count(), 1_000);
    }

    #[test]
    fn utf_encodings_preserve_master_semantics_and_respect_production_cap() {
        let spec = MasterCorpusSpec {
            total_records: 10_000,
            text_width: 8,
            seed: 19,
        };
        let mut semantic = None;
        for encoding in [
            WireEncoding::Utf8,
            WireEncoding::Utf8Bom,
            WireEncoding::Utf16Le,
            WireEncoding::Utf16Be,
        ] {
            let (bytes, generated) = generate_master_corpus(spec, encoding).unwrap();
            assert!(bytes.len() <= MAX_ENCODED_MASTER_BYTES);
            let xml = decode(&bytes).expect("decode generated masters");
            assert_eq!(xml.matches("<LEDGER ").count(), spec.total_records as usize);
            match &semantic {
                Some(expected) => assert_eq!(&generated.expected_semantic_sha256, expected),
                None => semantic = Some(generated.expected_semantic_sha256),
            }
        }
    }

    #[test]
    fn maximum_dimensions_are_preflighted_before_output() {
        assert!(MasterCorpusSpec {
            total_records: 0,
            text_width: 0,
            seed: 0,
        }
        .validate()
        .is_err());
        assert!(MasterCorpusSpec {
            total_records: MAX_GENERATED_MASTERS + 1,
            text_width: 0,
            seed: 0,
        }
        .validate()
        .is_err());
        assert!(MasterCorpusSpec {
            total_records: 1,
            text_width: MAX_MASTER_TEXT_WIDTH + 1,
            seed: 0,
        }
        .validate()
        .is_err());

        let maximum = MasterCorpusSpec {
            total_records: MAX_GENERATED_MASTERS,
            text_width: MAX_MASTER_TEXT_WIDTH,
            seed: 23,
        };
        let (bytes, generated) = generate_master_corpus(maximum, WireEncoding::Utf16Le).unwrap();
        assert_eq!(generated.records, MAX_GENERATED_MASTERS);
        assert_eq!(generated.encoded_bytes, bytes.len() as u64);
        assert!(bytes.len() <= MAX_ENCODED_MASTER_BYTES);
    }
}
