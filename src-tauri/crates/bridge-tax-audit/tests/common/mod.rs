// SPDX-License-Identifier: Apache-2.0
#![allow(dead_code)] // each test binary uses a different subset

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use bridge_tax_audit::{cash_44ab_canonical, rules_for, Engagement, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn golden() -> Value {
    let text = std::fs::read_to_string(fixtures().join("golden/synthetic.cash_44ab.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The synthetic engagement, pointed at `read_dir` instead of the committed read.
pub fn engagement(read_dir: &Path, allow_unbracketed: bool) -> Engagement {
    let text = std::fs::read_to_string(fixtures().join("synthetic-engagement.toml")).unwrap();
    let mut e = Engagement::from_toml(&text, &fixtures()).unwrap();
    e.read_dir = read_dir.to_path_buf();
    e.allow_unbracketed_read = allow_unbracketed;
    e
}

pub fn run(read_dir: &Path, allow_unbracketed: bool) -> Result<Value> {
    let e = engagement(read_dir, allow_unbracketed);
    cash_44ab_canonical(&e, &rules_for(&e)?)
}

fn hex(bytes: &[u8]) -> String {
    bridge_tax_audit::canonical::hex(&Sha256::digest(bytes))
}

/// A scratch copy of the committed synthetic read, removed on drop.
pub struct ScratchRead {
    pub dir: PathBuf,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

impl ScratchRead {
    pub fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bridge-tax-audit-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("parts")).unwrap();
        let src = fixtures().join("synthetic-read");
        std::fs::copy(src.join("manifest.json"), dir.join("manifest.json")).unwrap();
        for entry in std::fs::read_dir(src.join("parts")).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), dir.join("parts").join(entry.file_name())).unwrap();
        }
        Self { dir }
    }

    pub fn manifest(&self) -> Value {
        serde_json::from_str(&std::fs::read_to_string(self.dir.join("manifest.json")).unwrap())
            .unwrap()
    }

    pub fn set_manifest(&self, manifest: &Value) {
        std::fs::write(
            self.dir.join("manifest.json"),
            serde_json::to_string_pretty(manifest).unwrap(),
        )
        .unwrap();
    }

    /// Replace `from` with `to` once in an identity-stored part's bytes. With `rehash`, the
    /// manifest's hashes and lengths are updated so C3 admits the altered bytes.
    pub fn edit_part(&self, part_id: &str, from: &str, to: &str, rehash: bool) {
        let mut manifest = self.manifest();
        let part = manifest["parts"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["id"] == part_id)
            .unwrap();
        assert_eq!(part["response"]["storage"], "identity");
        let path = self.dir.join(part["response"]["path"].as_str().unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text.matches(from).count(),
            1,
            "{from:?} must occur exactly once"
        );
        let bytes = text.replacen(from, to, 1).into_bytes();
        std::fs::write(&path, &bytes).unwrap();
        if rehash {
            let response = &mut part["response"];
            for (hash, len) in [("sha256", "bytes"), ("stored_sha256", "stored_bytes")] {
                response[hash] = Value::from(hex(&bytes));
                response[len] = Value::from(bytes.len());
            }
            self.set_manifest(&manifest);
        }
    }
}

impl Drop for ScratchRead {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
