use std::{fmt::Write as _, io};

use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_GENERATED_RECORDS: u64 = 500_000;
pub const MAX_GENERATED_RECORDS_PER_WINDOW: u32 = 10_000;
pub const MAX_GENERATED_WINDOW_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES_PER_VOUCHER: u16 = 256;
const MAX_TEXT_WIDTH: u16 = 256;
const MAX_NESTING_DEPTH: u16 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoucherCorpusSpec {
    pub total_records: u64,
    pub records_per_window: u32,
    pub entries_per_voucher: u16,
    pub text_width: u16,
    pub nesting_depth: u16,
    pub seed: u64,
}

impl VoucherCorpusSpec {
    pub fn validate(self) -> Result<Self, VoucherGenerationError> {
        if !(1..=MAX_GENERATED_RECORDS).contains(&self.total_records) {
            return Err(VoucherGenerationError::InvalidTotalRecords);
        }
        if !(1..=MAX_GENERATED_RECORDS_PER_WINDOW).contains(&self.records_per_window) {
            return Err(VoucherGenerationError::InvalidRecordsPerWindow);
        }
        if self.entries_per_voucher > MAX_ENTRIES_PER_VOUCHER {
            return Err(VoucherGenerationError::InvalidEntriesPerVoucher);
        }
        if self.text_width > MAX_TEXT_WIDTH {
            return Err(VoucherGenerationError::InvalidTextWidth);
        }
        if self.nesting_depth > MAX_NESTING_DEPTH {
            return Err(VoucherGenerationError::InvalidNestingDepth);
        }
        Ok(self)
    }

    pub fn window_count(self) -> Result<u32, VoucherGenerationError> {
        let validated = self.validate()?;
        let divisor = u64::from(validated.records_per_window);
        let windows = validated.total_records.div_ceil(divisor);
        u32::try_from(windows).map_err(|_| VoucherGenerationError::InvalidTotalRecords)
    }

    pub fn window(self, index: u32) -> Result<VoucherWindowSpec, VoucherGenerationError> {
        let validated = self.validate()?;
        if index >= validated.window_count()? {
            return Err(VoucherGenerationError::InvalidWindowIndex);
        }
        let first_record = u64::from(index) * u64::from(validated.records_per_window);
        let remaining = validated.total_records - first_record;
        let record_count = remaining.min(u64::from(validated.records_per_window)) as u32;
        Ok(VoucherWindowSpec {
            window_index: index,
            first_record,
            record_count,
            entries_per_voucher: validated.entries_per_voucher,
            text_width: validated.text_width,
            nesting_depth: validated.nesting_depth,
            seed: validated.seed,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoucherWindowSpec {
    pub window_index: u32,
    pub first_record: u64,
    pub record_count: u32,
    pub entries_per_voucher: u16,
    pub text_width: u16,
    pub nesting_depth: u16,
    pub seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedWindow {
    pub window_index: u32,
    pub first_record: u64,
    pub record_count: u32,
    pub ledger_entry_count: u64,
    pub wire_bytes: u64,
    pub sha256: String,
    pub expected_semantic_sha256: String,
}

#[derive(Debug, Error)]
pub enum VoucherGenerationError {
    #[error("total records must be between 1 and 500000")]
    InvalidTotalRecords,
    #[error("records per window must be between 1 and 10000")]
    InvalidRecordsPerWindow,
    #[error("entries per voucher must not exceed 256")]
    InvalidEntriesPerVoucher,
    #[error("synthetic text width must not exceed 256")]
    InvalidTextWidth,
    #[error("synthetic nesting depth must not exceed 256")]
    InvalidNestingDepth,
    #[error("window index was outside the corpus")]
    InvalidWindowIndex,
    #[error("record range exceeded the supported integer range")]
    InvalidFirstRecord,
    #[error("generated window exceeded the 32 MiB encoded-body limit")]
    WindowTooLarge,
    #[error("synthetic output failed")]
    Io(#[from] io::Error),
}

struct DigestWriter<W> {
    inner: W,
    digest: Sha256,
    bytes: u64,
}

impl<W> DigestWriter<W> {
    fn new(inner: W) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"bridge.tally.synthetic-voucher-window/1\0");
        Self {
            inner,
            digest,
            bytes: 0,
        }
    }

    fn finish(self) -> (W, u64, String) {
        (self.inner, self.bytes, hex::encode(self.digest.finalize()))
    }
}

impl<W: io::Write> io::Write for DigestWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.digest.update(&bytes[..written]);
        self.bytes = self.bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct BoundedCounter {
    bytes: u64,
}

impl io::Write for BoundedCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next = self
            .bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "window too large"))?;
        if next > MAX_GENERATED_WINDOW_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "window too large",
            ));
        }
        self.bytes = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn generate_voucher_window<W: io::Write>(
    target: W,
    spec: VoucherWindowSpec,
) -> Result<(W, GeneratedWindow), VoucherGenerationError> {
    if spec.record_count == 0 || spec.record_count > MAX_GENERATED_RECORDS_PER_WINDOW {
        return Err(VoucherGenerationError::InvalidRecordsPerWindow);
    }
    if spec.entries_per_voucher > MAX_ENTRIES_PER_VOUCHER {
        return Err(VoucherGenerationError::InvalidEntriesPerVoucher);
    }
    if spec.text_width > MAX_TEXT_WIDTH {
        return Err(VoucherGenerationError::InvalidTextWidth);
    }
    if spec.nesting_depth > MAX_NESTING_DEPTH {
        return Err(VoucherGenerationError::InvalidNestingDepth);
    }
    spec.first_record
        .checked_add(u64::from(spec.record_count) - 1)
        .ok_or(VoucherGenerationError::InvalidFirstRecord)?;

    let mut preflight = BoundedCounter { bytes: 0 };
    if write_voucher_xml(&mut preflight, spec, None).is_err() {
        return Err(VoucherGenerationError::WindowTooLarge);
    }
    let preflight_bytes = preflight.bytes;

    let mut output = DigestWriter::new(target);
    let mut semantic_digest = Sha256::new();
    semantic_digest.update(b"bridge.tally.synthetic-voucher-semantics/1\0");
    write_voucher_xml(&mut output, spec, Some(&mut semantic_digest))?;
    io::Write::flush(&mut output)?;

    let (target, wire_bytes, sha256) = output.finish();
    debug_assert_eq!(wire_bytes, preflight_bytes);
    Ok((
        target,
        GeneratedWindow {
            window_index: spec.window_index,
            first_record: spec.first_record,
            record_count: spec.record_count,
            ledger_entry_count: u64::from(spec.record_count)
                .saturating_mul(u64::from(spec.entries_per_voucher)),
            wire_bytes,
            sha256,
            expected_semantic_sha256: hex::encode(semantic_digest.finalize()),
        },
    ))
}

fn write_voucher_xml<W: io::Write>(
    output: &mut W,
    spec: VoucherWindowSpec,
    mut semantic_digest: Option<&mut Sha256>,
) -> Result<(), io::Error> {
    io::Write::write_all(
        output,
        format!(
            "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS></HEADER><BODY><DATA><COMPANYCONTEXT SCHEMA=\"bridge.tally.vouchers/2\" OBJECTTYPE=\"VOUCHER\" NAME=\"BRIDGE SYNTHETIC BOOK\" GUID=\"00000000-0000-4000-8000-000000000001\" RECORDCOUNT=\"{}\" />",
            spec.record_count
        )
        .as_bytes(),
    )?;

    let padding = "X".repeat(usize::from(spec.text_width));
    for local_index in 0..u64::from(spec.record_count) {
        let record = spec.first_record + local_index;
        let identity = synthetic_identity(spec.seed, record);
        let remote_id = format!("bridge-synthetic-{identity:016x}");
        let guid = format!(
            "00000000-0000-4000-8{:03x}-{:012x}",
            spec.window_index & 0x0fff,
            identity & 0x0000_ffff_ffff_ffff
        );
        let voucher_number = format!("BRIDGE-{identity:016X}-{padding}");
        let mut voucher = String::with_capacity(
            320 + padding.len()
                + usize::from(spec.entries_per_voucher) * 180
                + usize::from(spec.nesting_depth) * 48,
        );
        write!(
            voucher,
            "<VOUCHER REMOTEID=\"{remote_id}\" GUID=\"{guid}\"><DATE>20260701</DATE><VOUCHERTYPENAME>Journal</VOUCHERTYPENAME><VOUCHERNUMBER>{voucher_number}</VOUCHERNUMBER><PARTYLEDGERNAME>BRIDGE SYNTHETIC PARTY</PARTYLEDGERNAME><ISCANCELLED>No</ISCANCELLED><ISOPTIONAL>No</ISOPTIONAL><LEDGERENTRYCOUNT>{}</LEDGERENTRYCOUNT>",
            spec.entries_per_voucher
        )
        .expect("writing to String cannot fail");
        voucher.push_str("<LEDGERENTRIES>");
        for _ in 0..spec.nesting_depth {
            voucher.push_str("<BRIDGESYNTHETICNEST>");
        }
        let mut entry_hashes = Vec::with_capacity(usize::from(spec.entries_per_voucher));
        for entry_index in 1..=spec.entries_per_voucher {
            let ledger_name = format!("BRIDGE SYNTHETIC LEDGER {entry_index:03}");
            let entry = format!(
                "<LEDGERENTRY><ENTRYINDEX>{entry_index}</ENTRYINDEX><LEDGERNAME>{ledger_name}</LEDGERNAME><AMOUNT>-1.00</AMOUNT><ISDEEMEDPOSITIVE>Yes</ISDEEMEDPOSITIVE></LEDGERENTRY>"
            );
            entry_hashes.push((
                entry_index,
                ledger_name,
                hex::encode(Sha256::digest(entry.as_bytes())),
            ));
            voucher.push_str(&entry);
        }
        for _ in 0..spec.nesting_depth {
            voucher.push_str("</BRIDGESYNTHETICNEST>");
        }
        voucher.push_str("</LEDGERENTRIES></VOUCHER>");
        if let Some(digest) = semantic_digest.as_deref_mut() {
            semantic_field(digest, b"record_index", record.to_string().as_bytes());
            semantic_field(digest, b"source_id", guid.as_bytes());
            semantic_field(digest, b"identity_kind", b"guid");
            semantic_field(digest, b"guid", guid.as_bytes());
            semantic_field(digest, b"remote_id", remote_id.as_bytes());
            semantic_field(digest, b"master_id", b"");
            semantic_field(digest, b"alter_id", b"");
            semantic_field(
                digest,
                b"raw_source_sha256",
                hex::encode(Sha256::digest(voucher.as_bytes())).as_bytes(),
            );
            semantic_field(digest, b"voucher_id", guid.as_bytes());
            semantic_field(digest, b"date", b"20260701");
            semantic_field(digest, b"voucher_type", b"Journal");
            semantic_field(digest, b"voucher_number", voucher_number.as_bytes());
            semantic_field(digest, b"party_ledger_name", b"BRIDGE SYNTHETIC PARTY");
            semantic_field(digest, b"cancelled", b"false");
            semantic_field(digest, b"optional", b"false");
            semantic_field(
                digest,
                b"ledger_entry_count",
                spec.entries_per_voucher.to_string().as_bytes(),
            );
            for (entry_index, ledger_name, raw_sha256) in &entry_hashes {
                semantic_field(digest, b"entry_index", entry_index.to_string().as_bytes());
                semantic_field(digest, b"ledger_name", ledger_name.as_bytes());
                semantic_field(digest, b"amount", b"-1.00");
                semantic_field(digest, b"is_deemed_positive", b"true");
                semantic_field(digest, b"entry_raw_source_sha256", raw_sha256.as_bytes());
            }
        }
        io::Write::write_all(output, voucher.as_bytes())?;
    }
    io::Write::write_all(output, b"</DATA></BODY></ENVELOPE>")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_cover_the_requested_corpus_exactly() {
        let spec = VoucherCorpusSpec {
            total_records: 2_501,
            records_per_window: 1_000,
            entries_per_voucher: 2,
            text_width: 16,
            nesting_depth: 0,
            seed: 7,
        };
        assert_eq!(spec.window_count().unwrap(), 3);
        let windows = (0..3)
            .map(|index| spec.window(index).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(windows[0].first_record, 0);
        assert_eq!(windows[1].first_record, 1_000);
        assert_eq!(windows[2].first_record, 2_000);
        assert_eq!(windows[2].record_count, 501);
        assert_eq!(
            windows
                .iter()
                .map(|window| window.record_count)
                .sum::<u32>(),
            2_501
        );
    }

    #[test]
    fn generation_is_deterministic_with_expected_xml_shape() {
        let spec = VoucherCorpusSpec {
            total_records: 2,
            records_per_window: 2,
            entries_per_voucher: 3,
            text_width: 8,
            nesting_depth: 0,
            seed: 42,
        };
        let (_, first) = generate_voucher_window(Vec::new(), spec.window(0).unwrap()).unwrap();
        let (bytes, second) = generate_voucher_window(Vec::new(), spec.window(0).unwrap()).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.record_count, 2);
        assert_eq!(first.ledger_entry_count, 6);
        assert_eq!(first.wire_bytes, bytes.len() as u64);
        let xml = String::from_utf8(bytes).unwrap();
        assert_eq!(xml.matches("<VOUCHER ").count(), 2);
        assert_eq!(xml.matches("<LEDGERENTRY>").count(), 6);
        assert!(!xml.contains("C:\\Users\\"));
        assert!(!xml.contains("/Users/"));
    }

    #[test]
    fn ci_smoke_window_has_a_versioned_golden_digest() {
        let spec = VoucherCorpusSpec {
            total_records: 50,
            records_per_window: 50,
            entries_per_voucher: 2,
            text_width: 16,
            nesting_depth: 0,
            seed: 7,
        };
        let (_, generated) = generate_voucher_window(Vec::new(), spec.window(0).unwrap()).unwrap();
        assert_eq!(generated.wire_bytes, 38_164);
        assert_eq!(
            generated.sha256,
            "fde2fda2ca5df0468ca95a1f7e7f184ee311ee2bcc43e7d771109031762ead2a"
        );

        let changed_seed = VoucherCorpusSpec { seed: 8, ..spec };
        let (_, changed) =
            generate_voucher_window(Vec::new(), changed_seed.window(0).unwrap()).unwrap();
        assert_ne!(generated.sha256, changed.sha256);
    }

    #[test]
    fn deep_characterization_uses_bounded_synthetic_wrappers() {
        let spec = VoucherCorpusSpec {
            total_records: 1,
            records_per_window: 1,
            entries_per_voucher: 2,
            text_width: 0,
            nesting_depth: MAX_NESTING_DEPTH,
            seed: 9,
        };
        let (bytes, generated) =
            generate_voucher_window(Vec::new(), spec.window(0).unwrap()).unwrap();
        let xml = String::from_utf8(bytes).unwrap();
        assert_eq!(xml.matches("<BRIDGESYNTHETICNEST>").count(), 256);
        assert_eq!(xml.matches("</BRIDGESYNTHETICNEST>").count(), 256);
        assert_eq!(generated.record_count, 1);
        assert_eq!(generated.ledger_entry_count, 2);
    }

    #[test]
    fn all_dimensions_are_bounded() {
        for invalid in [
            VoucherCorpusSpec {
                total_records: 0,
                records_per_window: 1,
                entries_per_voucher: 0,
                text_width: 0,
                nesting_depth: 0,
                seed: 0,
            },
            VoucherCorpusSpec {
                total_records: MAX_GENERATED_RECORDS + 1,
                records_per_window: 1,
                entries_per_voucher: 0,
                text_width: 0,
                nesting_depth: 0,
                seed: 0,
            },
            VoucherCorpusSpec {
                total_records: 1,
                records_per_window: 0,
                entries_per_voucher: 0,
                text_width: 0,
                nesting_depth: 0,
                seed: 0,
            },
            VoucherCorpusSpec {
                total_records: 1,
                records_per_window: 1,
                entries_per_voucher: MAX_ENTRIES_PER_VOUCHER + 1,
                text_width: 0,
                nesting_depth: 0,
                seed: 0,
            },
            VoucherCorpusSpec {
                total_records: 1,
                records_per_window: 1,
                entries_per_voucher: 0,
                text_width: MAX_TEXT_WIDTH + 1,
                nesting_depth: 0,
                seed: 0,
            },
            VoucherCorpusSpec {
                total_records: 1,
                records_per_window: 1,
                entries_per_voucher: 0,
                text_width: 0,
                nesting_depth: MAX_NESTING_DEPTH + 1,
                seed: 0,
            },
        ] {
            assert!(invalid.validate().is_err());
        }

        let oversized = VoucherWindowSpec {
            window_index: 0,
            first_record: 0,
            record_count: MAX_GENERATED_RECORDS_PER_WINDOW,
            entries_per_voucher: MAX_ENTRIES_PER_VOUCHER,
            text_width: MAX_TEXT_WIDTH,
            nesting_depth: MAX_NESTING_DEPTH,
            seed: 0,
        };
        let mut untouched = Vec::new();
        assert!(matches!(
            generate_voucher_window(&mut untouched, oversized),
            Err(VoucherGenerationError::WindowTooLarge)
        ));
        assert!(untouched.is_empty());

        let overflowing = VoucherWindowSpec {
            window_index: 0,
            first_record: u64::MAX,
            record_count: 2,
            entries_per_voucher: 0,
            text_width: 0,
            nesting_depth: 0,
            seed: 0,
        };
        assert!(matches!(
            generate_voucher_window(Vec::new(), overflowing),
            Err(VoucherGenerationError::InvalidFirstRecord)
        ));
    }
}
