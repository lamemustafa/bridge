use std::{
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
    thread,
    time::Duration,
    time::Instant,
};

use bridge_tally_protocol::{
    decode_tally_text_bytes_limited, parse_voucher_source_records_with_evidence,
    ParsedSourceIdentityKind, ParsedSourceRecord, TallyVoucher,
};
use bridge_tally_qualification::{
    read_qualification_body, window_manifest_sha256, BoundedBodyRead, CorpusEvidence,
    EnvironmentEvidence, MemoryEvidence, QualificationReceipt, SampleEvidence, Scenario,
    WindowEvidence, RESPONSE_LIMIT_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tally_protocol_simulator::generate_voucher_window;

const MAX_CHILD_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_WORKER_INPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerInput {
    files: Vec<PathBuf>,
    windows: Vec<WindowEvidence>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerResult {
    elapsed_ns: u64,
    parsed_records: u64,
    parsed_ledger_entries: u64,
    output_sha256: String,
    verified_manifest_sha256: String,
    memory: MemoryEvidence,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("bridge_tally_qualification_failed:{code}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), &'static str> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("run") => {
            let scenario = args
                .next()
                .as_deref()
                .and_then(Scenario::parse)
                .ok_or("invalid_scenario")?;
            let output = args.next().map(PathBuf::from).ok_or("missing_output")?;
            let seed = args
                .next()
                .as_deref()
                .unwrap_or("1")
                .parse::<u64>()
                .map_err(|_| "invalid_seed")?;
            let sample_count = args
                .next()
                .as_deref()
                .unwrap_or("3")
                .parse::<u16>()
                .map_err(|_| "invalid_sample_count")?;
            if !(1..=25).contains(&sample_count) {
                return Err("invalid_sample_count");
            }
            if args.next().is_some() {
                return Err("unexpected_argument");
            }
            controller(scenario, output, seed, sample_count)
        }
        Some("worker") => {
            let input = args
                .next()
                .map(PathBuf::from)
                .ok_or("missing_worker_input")?;
            if args.next().is_some() {
                return Err("unexpected_argument");
            }
            worker(input)
        }
        _ => Err("usage_run_scenario_output_seed_samples"),
    }
}

fn controller(
    scenario: Scenario,
    output: PathBuf,
    seed: u64,
    sample_count: u16,
) -> Result<(), &'static str> {
    let environment = EnvironmentEvidence::current().map_err(|_| "invalid_environment")?;
    let spec = scenario.corpus(seed);
    spec.validate().map_err(|_| "invalid_corpus")?;
    let temp = tempfile::tempdir().map_err(|_| "tempdir_failed")?;
    let mut files = Vec::new();
    let mut windows = Vec::new();
    for index in 0..spec.window_count().map_err(|_| "invalid_corpus")? {
        let path = temp.path().join(format!("window-{index:06}.xml"));
        let file = File::create(&path).map_err(|_| "corpus_create_failed")?;
        let (_, generated) =
            generate_voucher_window(file, spec.window(index).map_err(|_| "invalid_corpus")?)
                .map_err(|_| "corpus_generation_failed")?;
        if generated.wire_bytes > RESPONSE_LIMIT_BYTES {
            return Err("generated_window_exceeded_response_limit");
        }
        files.push(path);
        windows.push(generated.into());
    }
    let corpus = CorpusEvidence::from_windows(scenario, spec, windows.clone())
        .map_err(|_| "invalid_corpus_evidence")?;
    let worker_input = WorkerInput { files, windows };
    let input_path = temp.path().join("worker-input.json");
    let input_bytes = serde_json::to_vec(&worker_input).map_err(|_| "worker_input_failed")?;
    if input_bytes.len() > MAX_WORKER_INPUT_BYTES {
        return Err("worker_input_too_large");
    }
    fs::write(&input_path, input_bytes).map_err(|_| "worker_input_failed")?;

    let executable = std::env::current_exe().map_err(|_| "executable_unavailable")?;
    let mut samples = Vec::with_capacity(usize::from(sample_count));
    for ordinal in 0..sample_count {
        let child = run_worker(&executable, &input_path, scenario.worker_timeout())?;
        if child.stdout.len() > MAX_CHILD_OUTPUT_BYTES
            || child.stderr.len() > MAX_CHILD_OUTPUT_BYTES
        {
            return Err("worker_failed");
        }
        if !child.status.success() {
            return Err(classify_worker_failure(&child.stderr));
        }
        let result: WorkerResult =
            serde_json::from_slice(&child.stdout).map_err(|_| "worker_output_invalid")?;
        if result.verified_manifest_sha256 != corpus.manifest_sha256 {
            return Err("worker_manifest_mismatch");
        }
        samples.push(SampleEvidence {
            ordinal,
            elapsed_ns: result.elapsed_ns,
            parsed_records: result.parsed_records,
            parsed_ledger_entries: result.parsed_ledger_entries,
            output_sha256: result.output_sha256,
            outcome: "passed".to_owned(),
            memory: result.memory,
        });
    }

    let receipt =
        QualificationReceipt::build(environment, corpus, samples).map_err(|_| "receipt_invalid")?;
    let bytes = receipt
        .to_pretty_json()
        .map_err(|_| "receipt_serialization_failed")?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut target =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| "receipt_create_failed")?;
    target
        .write_all(&bytes)
        .map_err(|_| "receipt_write_failed")?;
    target.flush().map_err(|_| "receipt_write_failed")?;
    target
        .as_file()
        .sync_all()
        .map_err(|_| "receipt_write_failed")?;
    target
        .persist(output)
        .map_err(|_| "receipt_replace_failed")?;
    Ok(())
}

fn classify_worker_failure(stderr: &[u8]) -> &'static str {
    let Ok(message) = std::str::from_utf8(stderr) else {
        return "worker_failed";
    };
    let Some(code) = message
        .lines()
        .find_map(|line| line.strip_prefix("bridge_tally_qualification_failed:"))
    else {
        return "worker_failed";
    };
    match code {
        "worker_input_read_failed" => "worker_input_read_failed",
        "worker_input_too_large" => "worker_input_too_large",
        "worker_input_invalid" => "worker_input_invalid",
        "window_read_failed" => "window_read_failed",
        "window_size_limit" => "window_size_limit",
        "window_manifest_mismatch" => "window_manifest_mismatch",
        "window_decode_failed" => "window_decode_failed",
        "window_parse_failed" => "window_parse_failed",
        "parsed_record_count_mismatch" => "parsed_record_count_mismatch",
        "parsed_count_overflow" => "parsed_count_overflow",
        "parsed_entry_count_mismatch" => "parsed_entry_count_mismatch",
        "parsed_semantics_mismatch" => "parsed_semantics_mismatch",
        "elapsed_overflow" => "elapsed_overflow",
        "zero_elapsed" => "zero_elapsed",
        "worker_output_invalid" => "worker_output_invalid",
        "worker_output_too_large" => "worker_output_too_large",
        "worker_output_failed" => "worker_output_failed",
        _ => "worker_failed",
    }
}

fn run_worker(
    executable: &std::path::Path,
    input_path: &std::path::Path,
    timeout: Duration,
) -> Result<std::process::Output, &'static str> {
    let mut child = Command::new(executable)
        .arg("worker")
        .arg(input_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "worker_spawn_failed")?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("worker_timeout_invalid")?;
    loop {
        if child
            .try_wait()
            .map_err(|_| "worker_wait_failed")?
            .is_some()
        {
            return child.wait_with_output().map_err(|_| "worker_output_failed");
        }
        if Instant::now() >= deadline {
            if child.kill().is_err()
                && child
                    .try_wait()
                    .map_err(|_| "worker_wait_failed")?
                    .is_none()
            {
                return Err("worker_termination_failed");
            }
            child.wait().map_err(|_| "worker_wait_failed")?;
            return Err("worker_timeout");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn worker(input_path: PathBuf) -> Result<(), &'static str> {
    let mut input_bytes = Vec::with_capacity(MAX_WORKER_INPUT_BYTES + 1);
    File::open(input_path)
        .map_err(|_| "worker_input_read_failed")?
        .take((MAX_WORKER_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut input_bytes)
        .map_err(|_| "worker_input_read_failed")?;
    if input_bytes.len() > MAX_WORKER_INPUT_BYTES {
        return Err("worker_input_too_large");
    }
    let input: WorkerInput =
        serde_json::from_slice(&input_bytes).map_err(|_| "worker_input_invalid")?;
    if input.files.len() != input.windows.len() || input.files.is_empty() {
        return Err("worker_input_invalid");
    }
    let baseline_peak = peak_resident_bytes();
    let started = Instant::now();
    let mut parsed_records = 0_u64;
    let mut parsed_ledger_entries = 0_u64;
    let verified_manifest_sha256 =
        window_manifest_sha256(&input.windows).map_err(|_| "worker_input_invalid")?;
    let mut output_digest = Sha256::new();
    output_digest.update(b"bridge.tally.synthetic-corpus-semantics/1\0");

    for (path, expected) in input.files.iter().zip(&input.windows) {
        let file = File::open(path).map_err(|_| "window_read_failed")?;
        let bytes = match read_qualification_body(file, Some(expected.wire_bytes))
            .map_err(|_| "window_read_failed")?
        {
            BoundedBodyRead::Accepted(bytes) => bytes,
            BoundedBodyRead::SizeLimit => return Err("window_size_limit"),
        };
        let mut payload_digest = Sha256::new();
        payload_digest.update(b"bridge.tally.synthetic-voucher-window/1\0");
        payload_digest.update(&bytes);
        if bytes.len() as u64 != expected.wire_bytes
            || bytes.len() as u64 > RESPONSE_LIMIT_BYTES
            || hex::encode(payload_digest.finalize()) != expected.sha256
        {
            return Err("window_manifest_mismatch");
        }
        let decoded = decode_tally_text_bytes_limited(&bytes, RESPONSE_LIMIT_BYTES as usize)
            .map_err(|_| "window_decode_failed")?;
        let parsed = parse_voucher_source_records_with_evidence(&decoded.text)
            .map_err(|_| "window_parse_failed")?;
        if parsed.records.len() != expected.records as usize {
            return Err("parsed_record_count_mismatch");
        }
        let mut window_semantics = Sha256::new();
        window_semantics.update(b"bridge.tally.synthetic-voucher-semantics/1\0");
        let mut window_entries = 0_u64;
        for (local_index, record) in parsed.records.iter().enumerate() {
            parsed_ledger_entries = parsed_ledger_entries
                .checked_add(record.record.ledger_entries.len() as u64)
                .ok_or("parsed_count_overflow")?;
            window_entries = window_entries
                .checked_add(record.record.ledger_entries.len() as u64)
                .ok_or("parsed_count_overflow")?;
            update_semantic_digest(
                &mut window_semantics,
                expected.first_record + local_index as u64,
                record,
            );
        }
        if window_entries != expected.ledger_entries {
            return Err("parsed_entry_count_mismatch");
        }
        let window_semantic_sha256 = hex::encode(window_semantics.finalize());
        if window_semantic_sha256 != expected.expected_semantic_sha256 {
            return Err("parsed_semantics_mismatch");
        }
        output_digest.update(window_semantic_sha256.as_bytes());
        parsed_records = parsed_records
            .checked_add(parsed.records.len() as u64)
            .ok_or("parsed_count_overflow")?;
    }
    let elapsed_ns = u64::try_from(started.elapsed().as_nanos()).map_err(|_| "elapsed_overflow")?;
    if elapsed_ns == 0 {
        return Err("zero_elapsed");
    }
    let final_peak = peak_resident_bytes();
    let memory = memory_evidence(baseline_peak, final_peak);
    let result = WorkerResult {
        elapsed_ns,
        parsed_records,
        parsed_ledger_entries,
        output_sha256: hex::encode(output_digest.finalize()),
        verified_manifest_sha256,
        memory,
    };
    let bytes = serde_json::to_vec(&result).map_err(|_| "worker_output_invalid")?;
    if bytes.len() > MAX_CHILD_OUTPUT_BYTES {
        return Err("worker_output_too_large");
    }
    std::io::stdout()
        .write_all(&bytes)
        .map_err(|_| "worker_output_failed")?;
    Ok(())
}

fn update_semantic_digest(
    digest: &mut Sha256,
    record_index: u64,
    source: &ParsedSourceRecord<TallyVoucher>,
) {
    semantic_field(digest, b"record_index", record_index.to_string().as_bytes());
    semantic_optional(digest, b"source_id", source.source_id.as_deref());
    semantic_field(
        digest,
        b"identity_kind",
        match source.identity_kind {
            Some(ParsedSourceIdentityKind::Guid) => b"guid",
            Some(ParsedSourceIdentityKind::RemoteId) => b"remote_id",
            Some(ParsedSourceIdentityKind::MasterId) => b"master_id",
            None => b"",
        },
    );
    semantic_optional(digest, b"guid", source.identities.guid.as_deref());
    semantic_optional(digest, b"remote_id", source.identities.remote_id.as_deref());
    semantic_optional(digest, b"master_id", source.identities.master_id.as_deref());
    semantic_optional(digest, b"alter_id", source.alter_id.as_deref());
    semantic_field(
        digest,
        b"raw_source_sha256",
        source.raw_source_sha256.as_bytes(),
    );
    semantic_optional(digest, b"voucher_id", source.record.id.as_deref());
    semantic_optional(digest, b"date", source.record.date.as_deref());
    semantic_optional(
        digest,
        b"voucher_type",
        source.record.voucher_type.as_deref(),
    );
    semantic_optional(
        digest,
        b"voucher_number",
        source.record.voucher_number.as_deref(),
    );
    semantic_optional(
        digest,
        b"party_ledger_name",
        source.record.party_ledger_name.as_deref(),
    );
    semantic_optional_bool(digest, b"cancelled", source.record.cancelled);
    semantic_optional_bool(digest, b"optional", source.record.optional);
    let ledger_count = source
        .record
        .ledger_entry_count
        .map(|value| value.to_string());
    semantic_optional(digest, b"ledger_entry_count", ledger_count.as_deref());
    for entry in &source.record.ledger_entries {
        semantic_field(
            digest,
            b"entry_index",
            entry.entry_index.to_string().as_bytes(),
        );
        semantic_field(digest, b"ledger_name", entry.ledger_name.as_bytes());
        semantic_field(digest, b"amount", entry.amount.as_bytes());
        semantic_field(
            digest,
            b"is_deemed_positive",
            if entry.is_deemed_positive {
                b"true"
            } else {
                b"false"
            },
        );
        semantic_field(
            digest,
            b"entry_raw_source_sha256",
            entry.raw_source_sha256.as_bytes(),
        );
    }
}

fn semantic_optional(digest: &mut Sha256, label: &[u8], value: Option<&str>) {
    semantic_field(digest, label, value.unwrap_or_default().as_bytes());
}

fn semantic_optional_bool(digest: &mut Sha256, label: &[u8], value: Option<bool>) {
    semantic_field(
        digest,
        label,
        match value {
            Some(true) => b"true",
            Some(false) => b"false",
            None => b"",
        },
    );
}

fn semantic_field(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u16).to_be_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn memory_evidence(
    baseline: Option<(u64, &'static str)>,
    peak: Option<(u64, &'static str)>,
) -> MemoryEvidence {
    match (baseline, peak) {
        (Some((baseline, method)), Some((peak, final_method)))
            if method == final_method && peak >= baseline =>
        {
            MemoryEvidence {
                lifetime_peak_resident_bytes: Some(peak),
                baseline_lifetime_peak_resident_bytes: Some(baseline),
                lifetime_peak_delta_bytes: Some(peak - baseline),
                method: Some(method.to_owned()),
                unavailable_reason: None,
            }
        }
        _ => MemoryEvidence::unavailable("platform_peak_resident_measurement_unavailable"),
    }
}

#[cfg(windows)]
fn peak_resident_bytes() -> Option<(u64, &'static str)> {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::GetCurrentProcess,
    };
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    (ok != 0).then_some((
        counters.PeakWorkingSetSize as u64,
        "windows_get_process_memory_info_peak_working_set_bytes",
    ))
}

#[cfg(unix)]
fn peak_resident_bytes() -> Option<(u64, &'static str)> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let value = unsafe { usage.assume_init() }.ru_maxrss;
    let value = u64::try_from(value).ok()?.checked_mul(1024)?;
    #[cfg(target_os = "macos")]
    return Some((value, "macos_getrusage_ru_maxrss_kib_normalized_to_bytes"));
    #[cfg(not(target_os = "macos"))]
    Some((value, "unix_getrusage_ru_maxrss_kib_normalized_to_bytes"))
}

#[cfg(not(any(unix, windows)))]
fn peak_resident_bytes() -> Option<(u64, &'static str)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_smoke_generated_window_is_consumable_by_reviewed_parser() {
        let spec = Scenario::CiSmoke.corpus(7);
        let (bytes, generated) =
            generate_voucher_window(Vec::new(), spec.window(0).unwrap()).unwrap();
        let decoded =
            decode_tally_text_bytes_limited(&bytes, RESPONSE_LIMIT_BYTES as usize).unwrap();
        let parsed = parse_voucher_source_records_with_evidence(&decoded.text).unwrap();
        assert_eq!(parsed.records.len(), generated.record_count as usize);
        assert_eq!(
            parsed
                .records
                .iter()
                .map(|record| record.record.ledger_entries.len() as u64)
                .sum::<u64>(),
            generated.ledger_entry_count
        );
    }

    #[test]
    fn large_voucher_alias_is_schema_valid_and_consumable() {
        assert_eq!(
            Scenario::parse("deep-voucher"),
            Some(Scenario::LargeVoucher)
        );
        assert_eq!(
            Scenario::parse("large-voucher"),
            Some(Scenario::LargeVoucher)
        );
        let spec = Scenario::LargeVoucher.corpus(7);
        assert_eq!(spec.nesting_depth, 0);
        let (bytes, generated) =
            generate_voucher_window(Vec::new(), spec.window(0).unwrap()).unwrap();
        let decoded =
            decode_tally_text_bytes_limited(&bytes, RESPONSE_LIMIT_BYTES as usize).unwrap();
        let parsed = parse_voucher_source_records_with_evidence(&decoded.text).unwrap();
        assert_eq!(parsed.records.len(), 1);
        assert_eq!(generated.ledger_entry_count, 256);
        assert_eq!(parsed.records[0].record.ledger_entries.len(), 256);
    }

    #[test]
    fn worker_failure_classification_is_allowlisted() {
        assert_eq!(
            classify_worker_failure(b"bridge_tally_qualification_failed:window_parse_failed\n"),
            "window_parse_failed"
        );
        assert_eq!(
            classify_worker_failure(b"bridge_tally_qualification_failed:secret-value\n"),
            "worker_failed"
        );
        assert_eq!(classify_worker_failure(&[0xff]), "worker_failed");
    }

    #[test]
    fn maximum_window_manifest_fits_the_dedicated_worker_input_cap() {
        let files = (0..500)
            .map(|index| {
                PathBuf::from(format!(
                    "synthetic-qualification-temporary-root/window-{index:06}.xml"
                ))
            })
            .collect::<Vec<_>>();
        let windows = (0..500)
            .map(|ordinal| WindowEvidence {
                ordinal,
                first_record: u64::from(ordinal) * 1_000,
                records: 1_000,
                ledger_entries: 2_000,
                wire_bytes: 750_000,
                sha256: "a".repeat(64),
                expected_semantic_sha256: "b".repeat(64),
            })
            .collect::<Vec<_>>();
        let encoded = serde_json::to_vec(&WorkerInput { files, windows }).unwrap();
        assert!(encoded.len() > MAX_CHILD_OUTPUT_BYTES);
        assert!(encoded.len() <= MAX_WORKER_INPUT_BYTES);
    }
}
