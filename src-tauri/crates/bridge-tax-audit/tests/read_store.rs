// SPDX-License-Identifier: Apache-2.0
//! Storage-neutral C3 controls. The existing consumer and parity suites keep exercising the
//! directory adapter; these tests prove the same reader refuses corrupt or absent keyed blobs.

mod common;

use std::collections::BTreeMap;
use std::io::Cursor;

use bridge_tax_audit::read::{Read, ReadStore};
use bridge_tax_audit::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

struct MemoryStore {
    manifest: Vec<u8>,
    blobs: BTreeMap<String, Vec<u8>>,
}

impl MemoryStore {
    fn synthetic() -> Self {
        let root = common::fixtures().join("synthetic-read");
        let manifest = std::fs::read(root.join("manifest.json")).unwrap();
        let parsed: Value = serde_json::from_slice(&manifest).unwrap();
        let mut blobs = BTreeMap::new();
        for part in parsed["parts"].as_array().unwrap() {
            for key in ["response", "request"] {
                let blob = &part[key];
                if blob["stored"] == Value::Bool(false) {
                    continue;
                }
                if let Some(path) = blob["path"].as_str() {
                    blobs.insert(path.to_string(), std::fs::read(root.join(path)).unwrap());
                }
            }
        }
        Self { manifest, blobs }
    }

    fn manifest(&self) -> Value {
        serde_json::from_slice(&self.manifest).unwrap()
    }

    fn set_manifest(&mut self, manifest: &Value) {
        self.manifest = serde_json::to_vec(manifest).unwrap();
    }

    fn handle(&self) -> (String, String) {
        (
            self.manifest()["read_id"].as_str().unwrap().to_string(),
            hex(&self.manifest),
        )
    }
}

impl ReadStore for MemoryStore {
    fn manifest_bytes(&self) -> Result<Box<dyn std::io::Read + '_>> {
        Ok(Box::new(Cursor::new(self.manifest.clone())))
    }

    fn stored_blob(&self, path: &str) -> Result<Option<Box<dyn std::io::Read + '_>>> {
        Ok(self
            .blobs
            .get(path)
            .cloned()
            .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn std::io::Read>))
    }
}

fn hex(bytes: &[u8]) -> String {
    bridge_tax_audit::canonical::hex(&Sha256::digest(bytes))
}

fn part_mut<'a>(manifest: &'a mut Value, id: &str) -> &'a mut Value {
    manifest["parts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|part| part["id"] == id)
        .unwrap()
}

fn code(result: Result<Read>) -> &'static str {
    match result {
        Err(error) => error
            .code()
            .unwrap_or_else(|| panic!("not a rule refusal: {error}")),
        Ok(_) => panic!("the read was admitted"),
    }
}

fn open(store: &MemoryStore) -> Result<Read> {
    let (read_id, manifest_sha256) = store.handle();
    Read::open_from(store, &read_id, &manifest_sha256)
}

#[test]
fn an_in_memory_store_admits_the_same_synthetic_read_as_the_directory_adapter() {
    assert!(open(&MemoryStore::synthetic()).is_ok());
}

#[test]
fn a_missing_declared_blob_is_refused() {
    let mut store = MemoryStore::synthetic();
    let manifest = store.manifest();
    let path = manifest["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|part| part["id"] == "vouchers-2025-04-01")
        .unwrap()["response"]["path"]
        .as_str()
        .unwrap();
    store.blobs.remove(path);
    assert_eq!(code(open(&store)), "C3-missing");
}

#[test]
fn stored_hash_and_length_mismatches_are_refused() {
    let mut hash_store = MemoryStore::synthetic();
    let mut hash_manifest = hash_store.manifest();
    part_mut(&mut hash_manifest, "vouchers-2025-04-01")["response"]["stored_sha256"] =
        json!("0".repeat(64));
    hash_store.set_manifest(&hash_manifest);
    assert_eq!(code(open(&hash_store)), "C3-stored-hash");

    let mut length_store = MemoryStore::synthetic();
    let mut length_manifest = length_store.manifest();
    let stored_bytes = part_mut(&mut length_manifest, "vouchers-2025-04-01")["response"]
        ["stored_bytes"]
        .as_u64()
        .unwrap();
    part_mut(&mut length_manifest, "vouchers-2025-04-01")["response"]["stored_bytes"] =
        json!(stored_bytes + 1);
    length_store.set_manifest(&length_manifest);
    assert_eq!(code(open(&length_store)), "C3-stored-hash");
}

#[test]
fn decoded_hash_and_length_mismatches_are_refused() {
    let mut hash_store = MemoryStore::synthetic();
    let mut hash_manifest = hash_store.manifest();
    part_mut(&mut hash_manifest, "vouchers-2025-04-01")["response"]["sha256"] =
        json!("0".repeat(64));
    hash_store.set_manifest(&hash_manifest);
    assert_eq!(code(open(&hash_store)), "C3-content-hash");

    let mut length_store = MemoryStore::synthetic();
    let mut length_manifest = length_store.manifest();
    let bytes = part_mut(&mut length_manifest, "vouchers-2025-04-01")["response"]["bytes"]
        .as_u64()
        .unwrap();
    part_mut(&mut length_manifest, "vouchers-2025-04-01")["response"]["bytes"] = json!(bytes + 1);
    length_store.set_manifest(&length_manifest);
    assert_eq!(code(open(&length_store)), "C3-content-hash");
}

#[test]
fn a_manifest_that_does_not_match_the_supplied_handle_is_refused() {
    let store = MemoryStore::synthetic();
    let (read_id, _) = store.handle();
    assert_eq!(
        code(Read::open_from(&store, &read_id, &"0".repeat(64))),
        "C1-handle"
    );
}
