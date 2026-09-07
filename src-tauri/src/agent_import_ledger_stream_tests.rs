//! Local journal tests; no authored Tally wire responses.
use super::*;
use std::cell::Cell;
use std::io::{BufReader, Cursor, Read};
use std::rc::Rc;

fn batch(id: &str, narration: &str) -> ImportLedgerLine {
    serde_json::from_value(json!({
        "batch_id":id,"company_guid":"synthetic-guid","company":null,
        "txn_ids":["txn"],"date_from":"20260901","date_to":"20260901",
        "sha256":"a".repeat(64),"built_at":"2026-09-07T00:00:00Z","status":"built",
        "pre_import_mark":{"kind":"company_high_water","value":1,"master_value":1},
        "vouchers":[{"bridge_txn_id":"txn","date":"20260901","voucher_type":"Journal",
            "narration":narration,"reference":null,"voucher_number":null,"entries":[
                {"ledger":"Cash","amount":"1","side":"Dr"},
                {"ledger":"Synthetic Ledger","amount":"1","side":"Cr"}]}]
    }))
    .unwrap()
}

fn record(value: &impl Serialize) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    bytes
}

/// Generates one physical record at a time and forbids whole-stream Read APIs.
struct Records<I> {
    records: I,
    current: Vec<u8>,
    offset: usize,
    consumed: Rc<Cell<usize>>,
    largest_record: Rc<Cell<usize>>,
}

impl<I: Iterator<Item = Vec<u8>>> Read for Records<I> {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        panic!("journal admission must consume bounded records through BufRead")
    }
}

impl<I: Iterator<Item = Vec<u8>>> BufRead for Records<I> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.offset == self.current.len() {
            self.current = self.records.next().unwrap_or_default();
            self.offset = 0;
            self.largest_record
                .set(self.largest_record.get().max(self.current.len()));
        }
        Ok(&self.current[self.offset..])
    }
    fn consume(&mut self, amount: usize) {
        self.offset += amount;
        self.consumed.set(self.consumed.get() + amount);
    }
}

#[test]
fn streaming_admission_retains_only_target_payload_and_latest_physical_generation() {
    let target = batch("target", &"target narration ".repeat(50_000));
    let mut updated = batch("target", "latest payload");
    updated.status = "posted_verified".into();
    let large_unrelated = batch("unrelated", &"unrelated narration ".repeat(2_000));
    let full_records = 1000;
    let status_records = 20_000;
    let mut physical = 0;
    let rows = std::iter::from_fn(|| {
        let bytes = match physical {
            0 => record(&target),
            i if i <= full_records => record(&large_unrelated),
            i if i == full_records + 1 => record(&updated),
            i if i <= full_records + 1 + status_records => record(&StatusRecord::from(&updated)),
            i if i == full_records + 2 + status_records => record(&large_unrelated),
            _ => return None,
        };
        physical += 1;
        Some(bytes)
    });
    let consumed = Rc::new(Cell::new(0));
    let largest_record = Rc::new(Cell::new(0));
    let reader = Records {
        records: rows,
        current: Vec::new(),
        offset: 0,
        consumed: consumed.clone(),
        largest_record: largest_record.clone(),
    };
    let selected = read_snapshot(reader, Some("target")).unwrap().unwrap();
    assert!(consumed.get() > MAX_RECORD_BYTES);
    assert!(largest_record.get() < 1_000_000);
    assert_eq!(
        selected.batch.vouchers[0].narration.as_deref(),
        Some("latest payload")
    );
    assert_eq!(selected.batch.status, "posted_verified");
    assert!(selected.generation == VerificationGeneration(full_records + 1 + status_records));
}

#[test]
fn streaming_admission_checks_corruption_after_target_and_unrelated_status_bindings() {
    let target = batch("target", "target");
    let unrelated = batch("unrelated", "other");
    let prefix = [record(&target), record(&unrelated)].concat();
    for ending in [
        b"{\"batch_id\":".to_vec(),
        b"\n".to_vec(),
        record(
            &json!({"record_type":"verification_status","batch_id":"missing",
            "batch_sha256":unrelated.sha256,"status":"posted_verified"}),
        ),
        record(
            &json!({"record_type":"verification_status","batch_id":"unrelated",
            "batch_sha256":"wrong","status":"posted_verified"}),
        ),
        record(&json!({"record_type":"unknown"})),
    ] {
        let bytes = [prefix.clone(), ending].concat();
        for wanted in [None, Some("target"), Some("missing")] {
            assert_eq!(
                read_snapshot(Cursor::new(&bytes), wanted).err(),
                Some("import_ledger_invalid".into())
            );
        }
    }
    let invalid_utf8 = [prefix.clone(), vec![0xff, b'\n']].concat();
    assert_eq!(
        read_snapshot(Cursor::new(invalid_utf8), Some("target")).err(),
        Some("import_ledger_unavailable".into())
    );
    // A complete JSON object is still an incomplete JSONL append without LF.
    let final_record = serde_json::to_vec(&target).unwrap();
    assert_eq!(
        read_snapshot(Cursor::new(final_record), Some("target")).err(),
        Some("import_ledger_invalid".into())
    );
    assert!(read_snapshot(Cursor::new(Vec::<u8>::new()), None)
        .unwrap()
        .is_none());
    assert!(read_snapshot(Cursor::new(record(&target)), Some("target"))
        .unwrap()
        .is_some());
}

#[test]
fn oversized_record_stops_at_the_line_bound_without_reading_the_remaining_stream() {
    let mut reader = BufReader::with_capacity(8192, std::io::repeat(b' '));
    let mut line = Vec::new();
    assert_eq!(
        read_record(&mut reader, &mut line).err(),
        Some("import_ledger_record_too_large".into())
    );
    assert_eq!(line.len(), MAX_RECORD_BYTES);
    assert!(line.capacity() <= MAX_RECORD_BYTES);
    // The next chunk stays unread; there is no unbounded read-to-end fallback.
    assert_eq!(reader.buffer().len(), 8192);
}

#[test]
fn dispatch_admission_survives_verification_and_legacy_full_records() {
    let initial = batch("dispatch-test", "local journal test");
    let mut bytes = record(&initial);
    bytes.extend(record(&StatusRecord::dispatch(&initial)));
    let mut verified = initial.clone();
    verified.status = "verification_incomplete".into();
    bytes.extend(record(&StatusRecord::from(&verified)));
    // Legacy full records must not erase a previous attempt either.
    bytes.extend(record(&verified));
    let snapshot = read_snapshot(Cursor::new(bytes.clone()), Some("dispatch-test"))
        .unwrap()
        .unwrap();
    assert!(snapshot.dispatched);
    assert_eq!(snapshot.batch.status, "verification_incomplete");
    bytes.extend(record(&StatusRecord::dispatch(&initial)));
    assert_eq!(
        read_snapshot(Cursor::new(bytes), Some("dispatch-test"))
            .err()
            .as_deref(),
        Some("import_ledger_duplicate_dispatch")
    );
}

#[test]
fn dispatched_batch_cannot_change_its_commitment() {
    let initial = batch("dispatch-test", "local journal test");
    let mut bytes = record(&initial);
    bytes.extend(record(&StatusRecord::dispatch(&initial)));
    let mut changed = initial;
    changed.sha256 = "b".repeat(64);
    bytes.extend(record(&changed));
    assert_eq!(
        read_snapshot(Cursor::new(bytes), None).err().as_deref(),
        Some("import_ledger_invalid")
    );
}

#[test]
fn native_dispatch_response_must_match_its_durable_wire_commitment() {
    let initial = batch("native-dispatch", "local journal test");
    let hash = "c".repeat(64);
    let intent = StatusRecord::dispatch_native(&initial, hash.clone());
    let mut bytes = record(&initial);
    bytes.extend(record(&intent));
    for (response_hash, admitted) in [(hash, true), ("d".repeat(64), false)] {
        let response = StatusRecord::response(
            &initial,
            DispatchResponse {
                request_sha256: response_hash,
                response_sha256: "e".repeat(64),
                bytes: 1,
                outcome: None,
            },
        );
        let mut with_response = bytes.clone();
        with_response.extend(record(&response));
        let result = read_snapshot(Cursor::new(with_response), Some("native-dispatch"));
        if admitted {
            assert!(result.unwrap().unwrap().dispatched);
        } else {
            assert_eq!(result.err().as_deref(), Some("import_ledger_invalid"));
        }
    }
    for invalid in ["", "not-a-hash"] {
        let mut bytes = record(&initial);
        bytes.extend(record(&StatusRecord::dispatch_native(
            &initial,
            invalid.into(),
        )));
        assert_eq!(
            read_snapshot(Cursor::new(bytes), None).err().as_deref(),
            Some("import_ledger_invalid")
        );
    }
    let mut misplaced = serde_json::to_value(StatusRecord::from(&initial)).unwrap();
    misplaced["native_request_sha256"] = serde_json::json!("c".repeat(64));
    let mut bytes = record(&initial);
    bytes.extend(record(&misplaced));
    assert_eq!(
        read_snapshot(Cursor::new(bytes), None).err().as_deref(),
        Some("import_ledger_invalid")
    );
}
