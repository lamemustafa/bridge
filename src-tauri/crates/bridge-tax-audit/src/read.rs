//! A tally-read-v1 directory, opened and verified (`docs/tax-audit/read-format-v1.md` section 7,
//! rules C1-C7, C10).
//!
//! [`Read::open`] admits a directory only if its manifest is well formed (C1), every path is
//! safe (C2), every stored file and its decoded content match their declared sha256 and length
//! (C3, for every part, consumed or not) and the part table is consistent (C4). [`Read::check`]
//! then applies the rules that need the engagement: period (C5), high-water bracket (C6),
//! voucher windows (C7) and Education-mode dates (C10). The remaining rules (C5 identity, C8,
//! C9) need parsed vouchers and live in `book.rs`.
//!
//! The verified content is kept in memory and is what the book parses, so the bytes parsed are
//! the bytes hashed; there is no second read of the file to fall out of step with the first.
//!
//! C1 here is a typed subset of the reference engine's JSON Schema (required keys, enums,
//! patterns, the conditional requirements), not a JSON Schema validator. Unknown keys and
//! unknown part kinds are ignored, as a v1.x manifest requires.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use bridge_tally_primitives::TallyDate;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{AuditError, Result};
use crate::xml::MAX_CONTENT_BYTES;

const REQUIRED_KINDS: [&str; 5] = ["company", "groups", "ledgers", "trial_balance", "vouchers"];
const SINGLETON_KINDS: [&str; 10] = [
    "company",
    "company_list",
    "groups",
    "ledgers",
    "trial_balance",
    "voucher_types",
    "stock_items",
    "report_profit_and_loss",
    "report_balance_sheet",
    "voucher_status_list",
];
const WINDOWED_KINDS: [&str; 4] = [
    "vouchers",
    "trial_balance",
    "report_profit_and_loss",
    "report_balance_sheet",
];

/// An inclusive date window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub from: TallyDate,
    pub to: TallyDate,
}

/// The high-water mark on one side of the bracket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighWater {
    pub alter_voucher_id: Option<u64>,
    pub alter_master_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Part {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub window: Option<Window>,
    pub as_of: Option<TallyDate>,
    pub rows: Option<u64>,
    pub alter_id_max: Option<u64>,
    /// `scope` of a `voucher_status_list`: (from, to, exhaustive).
    pub scope: Option<(Window, bool)>,
    /// The file name the part is stored under (the last segment of `response.path`).
    pub stored_name: String,
    pub response_sha256: String,
    /// The verified decoded content.
    pub content: Vec<u8>,
}

#[derive(Debug)]
pub struct Read {
    pub root: PathBuf,
    pub company_guid: String,
    pub read_at: String,
    pub period: Window,
    pub education_mode: Option<bool>,
    pub status: String,
    pub before: Option<HighWater>,
    pub after: Option<HighWater>,
    pub parts: Vec<Part>,
}

fn schema(detail: impl Into<String>) -> AuditError {
    AuditError::refused("C1-schema", detail)
}

fn obj<'a>(value: &'a Value, at: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| schema(format!("{at}: not an object")))
}

fn has(map: &Map<String, Value>, keys: &[&str], at: &str) -> Result<()> {
    match keys.iter().find(|key| !map.contains_key(**key)) {
        Some(key) => Err(schema(format!("{at}: '{key}' is a required property"))),
        None => Ok(()),
    }
}

fn string<'a>(map: &'a Map<String, Value>, key: &str, at: &str) -> Result<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| schema(format!("{at}/{key}: not a string")))
}

fn opt_string<'a>(map: &'a Map<String, Value>, key: &str, at: &str) -> Result<Option<&'a str>> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(schema(format!("{at}/{key}: not a string or null"))),
    }
}

fn opt_count(map: &Map<String, Value>, key: &str, at: &str) -> Result<Option<u64>> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| schema(format!("{at}/{key}: not a non-negative integer or null"))),
    }
}

fn count(map: &Map<String, Value>, key: &str, at: &str) -> Result<u64> {
    map.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| schema(format!("{at}/{key}: not a non-negative integer")))
}

fn one_of(value: &str, allowed: &[&str], at: &str) -> Result<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(schema(format!("{at}: {value:?} is not one of {allowed:?}")))
    }
}

fn matches_class(s: &str, first: fn(char) -> bool, rest: fn(char) -> bool, max: usize) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(first) && chars.all(rest) && s.chars().count() <= max
}

fn is_part_ref(s: &str) -> bool {
    matches_class(
        s,
        |c| c.is_ascii_lowercase() || c.is_ascii_digit(),
        |c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'),
        128,
    )
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `^\d{4}-\d{2}-\d{2}$` and a real calendar date.
fn iso_date(s: &str, at: &str) -> Result<TallyDate> {
    let b = s.as_bytes();
    let shaped = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !shaped {
        return Err(schema(format!("{at}: {s:?} is not a YYYY-MM-DD date")));
    }
    TallyDate::parse(s.replace('-', ""))
        .map_err(|_| AuditError::refused("C1-date", format!("{at}: {s:?} is not a calendar date")))
}

fn window(value: &Value, at: &str) -> Result<Window> {
    let map = obj(value, at)?;
    has(map, &["from", "to"], at)?;
    Ok(Window {
        from: iso_date(string(map, "from", at)?, &format!("{at}/from"))?,
        to: iso_date(string(map, "to", at)?, &format!("{at}/to"))?,
    })
}

fn high_water(value: &Value, at: &str) -> Result<Option<HighWater>> {
    if value.is_null() {
        return Ok(None);
    }
    let map = obj(value, at)?;
    has(map, &["alter_voucher_id", "alter_master_id"], at)?;
    if let Some(part) = opt_string(map, "part", at)? {
        if !is_part_ref(part) {
            return Err(schema(format!("{at}/part: {part:?} is not a part id")));
        }
    }
    Ok(Some(HighWater {
        alter_voucher_id: opt_count(map, "alter_voucher_id", at)?,
        alter_master_id: opt_count(map, "alter_master_id", at)?,
    }))
}

/// The schema's stored-blob path pattern: relative, `[A-Za-z0-9._/-]{1,512}`, no backslash,
/// no `.` or `..` segment.
fn is_safe_path_text(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 512
        && !p.starts_with('/')
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'))
        && p.split('/').all(|seg| !matches!(seg, "" | "." | ".."))
}

struct Blob {
    path: String,
    gzip: bool,
    stored_sha256: String,
    stored_bytes: u64,
    sha256: String,
    bytes: u64,
}

fn blob(value: &Value, at: &str) -> Result<Blob> {
    let map = obj(value, at)?;
    has(
        map,
        &[
            "path",
            "storage",
            "stored_sha256",
            "stored_bytes",
            "sha256",
            "bytes",
        ],
        at,
    )?;
    let path = string(map, "path", at)?;
    if !is_safe_path_text(path) {
        return Err(schema(format!(
            "{at}/path: {path:?} does not match the path pattern"
        )));
    }
    let storage = string(map, "storage", at)?;
    one_of(storage, &["gzip", "identity"], &format!("{at}/storage"))?;
    let stored_sha256 = string(map, "stored_sha256", at)?;
    let sha256 = string(map, "sha256", at)?;
    if !is_sha256(stored_sha256) || !is_sha256(sha256) {
        return Err(schema(format!(
            "{at}: a sha256 is not 64 lowercase hex characters"
        )));
    }
    Ok(Blob {
        path: path.to_string(),
        gzip: storage == "gzip",
        stored_sha256: stored_sha256.to_string(),
        stored_bytes: count(map, "stored_bytes", at)?,
        sha256: sha256.to_string(),
        bytes: count(map, "bytes", at)?,
    })
}

/// A part's `response`: a stored blob plus its media type, encoding and transformation.
fn response_blob<'a>(value: &'a Value, at: &str) -> Result<(Blob, &'a str, &'a str)> {
    let response_at = format!("{at}/response");
    let response = blob(value, &response_at)?;
    let rmap = obj(value, &response_at)?;
    has(
        rmap,
        &[
            "media_type",
            "text_encoding",
            "wire_exact",
            "transformation",
        ],
        &response_at,
    )?;
    let media = string(rmap, "media_type", &response_at)?;
    one_of(
        media,
        &["application/xml", "application/json"],
        &response_at,
    )?;
    one_of(
        string(rmap, "text_encoding", &response_at)?,
        &["utf-16le", "utf-8"],
        &response_at,
    )?;
    let transformation = string(rmap, "transformation", &response_at)?;
    one_of(
        transformation,
        &["none", "reencoded_utf8", "derived", "unknown"],
        &response_at,
    )?;
    let wire_exact = rmap
        .get("wire_exact")
        .and_then(Value::as_bool)
        .ok_or_else(|| schema(format!("{response_at}/wire_exact: not a boolean")))?;
    if wire_exact && transformation != "none" {
        return Err(schema(format!(
            "{response_at}: wire_exact content must have transformation \"none\""
        )));
    }
    Ok((response, media, transformation))
}

struct DeclaredPart {
    part: Part,
    response: Blob,
    request: Option<Blob>,
    derived_from: Vec<String>,
}

fn declared_part(value: &Value, index: usize) -> Result<DeclaredPart> {
    let at = format!("parts/{index}");
    let map = obj(value, &at)?;
    has(map, &["id", "kind", "name", "response"], &at)?;
    let id = string(map, "id", &at)?;
    let kind = string(map, "kind", &at)?;
    let name = string(map, "name", &at)?;
    if !is_part_ref(id) {
        return Err(schema(format!("{at}/id: {id:?} is not a part id")));
    }
    let kind_ok = matches_class(
        kind,
        |c| c.is_ascii_lowercase(),
        |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_',
        64,
    );
    let name_ok = matches_class(
        name,
        |c| c.is_ascii_alphanumeric(),
        |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'),
        128,
    );
    if !kind_ok || !name_ok {
        return Err(schema(format!(
            "{at}: kind or name does not match its pattern"
        )));
    }

    let (response, media, transformation) = response_blob(&map["response"], &at)?;

    let request = match map.get("request") {
        None | Some(Value::Null) => {
            let reason = map.get("request_absent_reason").and_then(Value::as_str);
            if reason.is_none_or(str::is_empty) {
                return Err(schema(format!(
                    "{at}: a part without a request needs request_absent_reason"
                )));
            }
            None
        }
        Some(req) => {
            let rq = obj(req, &format!("{at}/request"))?;
            if rq.get("stored") == Some(&Value::Bool(false)) {
                let sha = string(rq, "sha256", &format!("{at}/request"))?;
                if !is_sha256(sha) {
                    return Err(schema(format!("{at}/request/sha256: not a sha256")));
                }
                None
            } else {
                Some(blob(req, &format!("{at}/request"))?)
            }
        }
    };

    let window_value = map.get("window").filter(|v| !v.is_null());
    if WINDOWED_KINDS.contains(&kind) && window_value.is_none() {
        return Err(schema(format!("{at}: a {kind} part needs a window")));
    }
    let part_window = window_value
        .map(|v| window(v, &format!("{at}/window")))
        .transpose()?;
    if kind == "vouchers" && !(map.contains_key("rows") && map.contains_key("alter_id_max")) {
        return Err(schema(format!(
            "{at}: a vouchers part needs rows and alter_id_max"
        )));
    }
    if kind == "stock_summary" && map.get("as_of").is_none_or(Value::is_null) {
        return Err(schema(format!("{at}: a stock_summary part needs as_of")));
    }
    let as_of = opt_string(map, "as_of", &at)?
        .map(|s| iso_date(s, &format!("{at}/as_of")))
        .transpose()?;
    let scope = if let Some(scope) = map.get("scope") {
        let smap = obj(scope, &format!("{at}/scope"))?;
        has(smap, &["from", "to", "exhaustive"], &format!("{at}/scope"))?;
        let exhaustive = smap
            .get("exhaustive")
            .and_then(Value::as_bool)
            .ok_or_else(|| schema(format!("{at}/scope/exhaustive: not a boolean")))?;
        Some((window(scope, &format!("{at}/scope"))?, exhaustive))
    } else {
        None
    };
    if kind == "voucher_status_list"
        && (scope.is_none() || media != "application/json" || transformation != "derived")
    {
        return Err(schema(format!(
            "{at}: a voucher_status_list is derived JSON with a scope"
        )));
    }
    let derived_from = match map.get("derived_from") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .filter(|s| is_part_ref(s))
                    .map(str::to_string)
                    .ok_or_else(|| schema(format!("{at}/derived_from: not a list of part ids")))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(schema(format!("{at}/derived_from: not an array"))),
    };
    let stored_name = response
        .path
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    Ok(DeclaredPart {
        part: Part {
            id: id.to_string(),
            kind: kind.to_string(),
            name: name.to_string(),
            window: part_window,
            as_of,
            rows: opt_count(map, "rows", &at)?,
            alter_id_max: opt_count(map, "alter_id_max", &at)?,
            scope,
            stored_name,
            response_sha256: response.sha256.clone(),
            content: Vec::new(),
        },
        response,
        request,
        derived_from,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    crate::canonical::hex(&Sha256::digest(bytes))
}

/// C2: relative, inside the read, no symlink anywhere below the root.
fn safe_path(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.contains('\\')
        || rel.starts_with('/')
        || rel.split('/').any(|s| matches!(s, "" | "." | ".."))
    {
        return Err(AuditError::refused(
            "C2-path",
            format!("unsafe part path {rel:?}"),
        ));
    }
    let mut current = root.to_path_buf();
    for segment in rel.split('/') {
        current.push(segment);
        if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(AuditError::refused(
                "C2-symlink",
                format!("{rel:?} passes through a symlink"),
            ));
        }
    }
    let canonical_root = root.canonicalize().map_err(|source| AuditError::Io {
        path: root.display().to_string(),
        source,
    })?;
    if let Ok(resolved) = current.canonicalize() {
        if !resolved.starts_with(&canonical_root)
            || resolved.components().count() <= canonical_root.components().count()
        {
            return Err(AuditError::refused(
                "C2-path",
                format!("{rel:?} resolves outside the read"),
            ));
        }
    }
    Ok(current)
}

/// C2 + C3 for one stored blob: returns the verified content.
fn verified_blob(root: &Path, blob: &Blob, what: &str) -> Result<Vec<u8>> {
    let path = safe_path(root, &blob.path)?;
    let file_name = blob.path.rsplit('/').next().unwrap_or_default();
    // Case-sensitive, as the reference engine's `name.endswith(".gz")` is.
    let gz_name = Path::new(file_name).extension().is_some_and(|e| e == "gz");
    if blob.gzip != gz_name {
        return Err(AuditError::refused(
            "C2-storage",
            format!("{what}: storage does not match file name {file_name:?}"),
        ));
    }
    if !path.is_file() {
        return Err(AuditError::refused(
            "C3-missing",
            format!("{what}: {} not found", blob.path),
        ));
    }
    let stored = std::fs::read(&path).map_err(|source| AuditError::Io {
        path: blob.path.clone(),
        source,
    })?;
    if stored.len() as u64 != blob.stored_bytes || sha256_hex(&stored) != blob.stored_sha256 {
        return Err(AuditError::refused(
            "C3-stored-hash",
            format!(
                "{what}: stored bytes of {} do not match the manifest",
                blob.path
            ),
        ));
    }
    let content = if blob.gzip {
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(stored.as_slice())
            .take(MAX_CONTENT_BYTES as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|e| {
                AuditError::refused(
                    "C3-content-hash",
                    format!("{what}: gzip does not decode: {e}"),
                )
            })?;
        if out.len() > MAX_CONTENT_BYTES {
            return Err(AuditError::refused(
                "C3-size",
                format!("{what}: decompressed size exceeds {MAX_CONTENT_BYTES} bytes"),
            ));
        }
        out
    } else {
        stored
    };
    if content.len() as u64 != blob.bytes || sha256_hex(&content) != blob.sha256 {
        return Err(AuditError::refused(
            "C3-content-hash",
            format!(
                "{what}: content of {} does not match the manifest",
                blob.path
            ),
        ));
    }
    Ok(content)
}

impl Read {
    /// C1-C4. Every part's bytes, and every stored request's, are verified before this returns.
    #[allow(clippy::too_many_lines)] // one pass over the manifest, in the reference order
    pub fn open(root: &Path) -> Result<Self> {
        let manifest_path = root.join("manifest.json");
        let meta = std::fs::symlink_metadata(&manifest_path);
        if !meta.is_ok_and(|m| m.file_type().is_file()) {
            return Err(AuditError::refused(
                "C2-manifest",
                format!("no regular manifest.json in {}", root.display()),
            ));
        }
        let text = std::fs::read_to_string(&manifest_path).map_err(|source| AuditError::Io {
            path: manifest_path.display().to_string(),
            source,
        })?;
        let manifest: Value = serde_json::from_str(&text)
            .map_err(|e| AuditError::refused("C1-format", format!("manifest is not JSON: {e}")))?;
        let top = manifest
            .as_object()
            .filter(|m| m.get("format").and_then(Value::as_str) == Some("tally-read"))
            .ok_or_else(|| AuditError::refused("C1-format", "not a tally-read manifest"))?;
        let version = top
            .get("format_version")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if version.split('.').next() != Some("1") {
            return Err(AuditError::refused(
                "C1-major",
                format!("format_version {version:?}; this reader is v1"),
            ));
        }

        // C1: the typed subset of the schema.
        has(
            top,
            &[
                "format",
                "format_version",
                "read_id",
                "created_at",
                "read_at",
                "producer",
                "company",
                "tally",
                "period",
                "consistency",
                "parts",
            ],
            "(root)",
        )?;
        let minor = version.strip_prefix("1.").unwrap_or_default();
        let minor_ok = minor == "0"
            || (!minor.is_empty()
                && !minor.starts_with('0')
                && minor.bytes().all(|b| b.is_ascii_digit()));
        if !minor_ok {
            return Err(schema(format!(
                "format_version {version:?} is not MAJOR.MINOR"
            )));
        }
        let read_id = string(top, "read_id", "(root)")?;
        if !matches_class(
            read_id,
            |c| c.is_ascii_alphanumeric(),
            |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'),
            128,
        ) {
            return Err(schema("read_id does not match its pattern"));
        }
        for key in ["created_at", "read_at"] {
            if string(top, key, "(root)")?.chars().count() < 10 {
                return Err(schema(format!("{key} is shorter than 10 characters")));
            }
        }
        let producer = obj(&top["producer"], "producer")?;
        has(producer, &["name", "version", "kind"], "producer")?;
        for key in ["name", "version"] {
            if string(producer, key, "producer")?.is_empty() {
                return Err(schema(format!("producer/{key} is empty")));
            }
        }
        one_of(
            string(producer, "kind", "producer")?,
            &["bridge", "legacy-wrap", "capture-wrap", "synthetic"],
            "producer/kind",
        )?;
        let company = obj(&top["company"], "company")?;
        has(company, &["guid", "name"], "company")?;
        let company_guid = string(company, "guid", "company")?;
        if !is_uuid(company_guid) || string(company, "name", "company")?.is_empty() {
            return Err(schema("company guid or name is malformed"));
        }
        if let Some(books_from) = opt_string(company, "books_from", "company")? {
            iso_date(books_from, "company/books_from")?;
        }
        let tally = obj(&top["tally"], "tally")?;
        has(tally, &["basis", "license_tier", "education_mode"], "tally")?;
        let basis = string(tally, "basis", "tally")?;
        one_of(basis, &["observed", "not_recorded"], "tally/basis")?;
        let tier = opt_string(tally, "license_tier", "tally")?;
        if let Some(tier) = tier {
            one_of(tier, &["Silver", "Gold", "Education"], "tally/license_tier")?;
        }
        let education_mode = match tally.get("education_mode") {
            Some(Value::Bool(b)) => Some(*b),
            Some(Value::Null) => None,
            _ => return Err(schema("tally/education_mode: not a boolean or null")),
        };
        let evidence_part = opt_string(tally, "evidence_part", "tally")?;
        if basis == "observed"
            && (tier.is_none() || education_mode.is_none() || evidence_part.is_none())
        {
            return Err(schema(
                "tally: an observed basis needs license_tier, education_mode and evidence_part",
            ));
        }
        if basis == "not_recorded" && (tier.is_some() || education_mode.is_some()) {
            return Err(schema(
                "tally: a not_recorded basis has null license_tier and education_mode",
            ));
        }
        if let Some(part) = evidence_part {
            if !is_part_ref(part) {
                return Err(schema("tally/evidence_part: not a part id"));
            }
        }
        let period = window(&top["period"], "period")?;
        let consistency = obj(&top["consistency"], "consistency")?;
        has(consistency, &["status", "before", "after"], "consistency")?;
        let status = string(consistency, "status", "consistency")?;
        one_of(
            status,
            &["unchanged", "moved", "not_recorded"],
            "consistency/status",
        )?;
        let before = high_water(&consistency["before"], "consistency/before")?;
        let after = high_water(&consistency["after"], "consistency/after")?;
        let parts_value = top
            .get("parts")
            .and_then(Value::as_array)
            .filter(|parts| !parts.is_empty())
            .ok_or_else(|| schema("parts: not a non-empty array"))?;
        let declared: Vec<DeclaredPart> = parts_value
            .iter()
            .enumerate()
            .map(|(i, v)| declared_part(v, i))
            .collect::<Result<_>>()?;

        // C4.
        let ids: Vec<&str> = declared.iter().map(|d| d.part.id.as_str()).collect();
        if ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
            return Err(AuditError::refused(
                "C4-duplicate-id",
                "part ids are not unique",
            ));
        }
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for d in &declared {
            *kinds.entry(d.part.kind.as_str()).or_default() += 1;
        }
        for kind in SINGLETON_KINDS {
            if kinds.get(kind).copied().unwrap_or(0) > 1 {
                return Err(AuditError::refused(
                    "C4-singleton",
                    format!("{} parts of singleton kind {kind:?}", kinds[kind]),
                ));
            }
        }
        for kind in REQUIRED_KINDS {
            if !kinds.contains_key(kind) {
                return Err(AuditError::refused(
                    "C4-required",
                    format!("no part of kind {kind:?}"),
                ));
            }
        }
        let hw_part = |side: &str| {
            consistency
                .get(side)
                .and_then(Value::as_object)
                .and_then(|m| m.get("part"))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let references = [
            evidence_part.map(str::to_string),
            hw_part("before"),
            hw_part("after"),
        ]
        .into_iter()
        .flatten()
        .chain(declared.iter().flat_map(|d| d.derived_from.iter().cloned()));
        for reference in references {
            if !ids.contains(&reference.as_str()) {
                return Err(AuditError::refused(
                    "C4-dangling",
                    format!("reference to unknown part {reference:?}"),
                ));
            }
        }

        // C2 + C3 for every part, consumed or not: a manifest is one unit.
        let mut parts = Vec::with_capacity(declared.len());
        for d in declared {
            let mut part = d.part;
            part.content = verified_blob(root, &d.response, &part.id)?;
            if let Some(request) = &d.request {
                verified_blob(root, request, &format!("{} request", part.id))?;
            }
            parts.push(part);
        }
        Ok(Self {
            root: root.to_path_buf(),
            company_guid: company_guid.to_string(),
            read_at: string(top, "read_at", "(root)")?.to_string(),
            period,
            education_mode,
            status: status.to_string(),
            before,
            after,
            parts,
        })
    }

    pub fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Part> {
        self.parts.iter().filter(move |p| p.kind == kind)
    }

    pub fn one(&self, kind: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.kind == kind)
    }

    /// Vouchers parts sorted by window start.
    pub fn voucher_parts(&self) -> Vec<&Part> {
        let mut parts: Vec<&Part> = self.of_kind("vouchers").collect();
        parts.sort_by(|a, b| window_of(a).from.cmp(&window_of(b).from));
        parts
    }

    /// C5 (period), C6, C7, C10.
    pub fn check(&self, period: &Window, allow_unbracketed: bool) -> Result<()> {
        if &self.period != period {
            return Err(AuditError::refused(
                "C5-period",
                format!(
                    "read period {}..{} != engagement period {}..{}",
                    iso(&self.period.from),
                    iso(&self.period.to),
                    iso(&period.from),
                    iso(&period.to)
                ),
            ));
        }
        self.check_consistency(allow_unbracketed)?;
        self.check_windows()?;
        if self.education_mode == Some(true) {
            for part in &self.parts {
                let ends = part.window.iter().map(|w| &w.to).chain(part.as_of.iter());
                for end in ends {
                    let day = &end.as_str()[6..8];
                    if !matches!(day, "01" | "02" | "31") {
                        return Err(AuditError::refused(
                            "C10-education-date",
                            format!(
                                "{}: Education mode serves only day 1, 2 or 31; {} would have been widened silently",
                                part.id,
                                iso(end)
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// C6: recompute the bracket status from the recorded values; never trust the label.
    fn check_consistency(&self, allow_unbracketed: bool) -> Result<()> {
        let actual = match (&self.before, &self.after) {
            (Some(b), Some(a)) if b.alter_master_id.is_some() && a.alter_master_id.is_some() => {
                if b == a {
                    "unchanged"
                } else {
                    "moved"
                }
            }
            _ => "not_recorded",
        };
        if self.status != actual {
            return Err(AuditError::refused(
                "C6-status",
                format!(
                    "declared consistency {:?} but the recorded values say {actual:?}",
                    self.status
                ),
            ));
        }
        if actual == "moved" {
            return Err(AuditError::refused(
                "C6-moved",
                format!(
                    "company high-water moved during the read: before {:?}, after {:?}",
                    self.before, self.after
                ),
            ));
        }
        if actual == "not_recorded" && !allow_unbracketed {
            return Err(AuditError::refused(
                "C6-unbracketed",
                "no high-water bracket; allow_unbracketed_read is only for a read taken before brackets existed",
            ));
        }
        Ok(())
    }

    /// C7: voucher windows sorted, disjoint, contiguous, covering exactly the period.
    fn check_windows(&self) -> Result<()> {
        let parts = self.voucher_parts();
        for part in &parts {
            let w = window_of(part);
            if w.from > w.to {
                return Err(AuditError::refused(
                    "C7-window",
                    format!(
                        "{}: window from {} after to {}",
                        part.id,
                        iso(&w.from),
                        iso(&w.to)
                    ),
                ));
            }
        }
        let (first, last) = (window_of(parts[0]), window_of(parts[parts.len() - 1]));
        if first.from != self.period.from || last.to != self.period.to {
            return Err(AuditError::refused(
                "C7-coverage",
                format!(
                    "voucher windows cover {}..{}, period is {}..{}",
                    iso(&first.from),
                    iso(&last.to),
                    iso(&self.period.from),
                    iso(&self.period.to)
                ),
            ));
        }
        for pair in parts.windows(2) {
            let (a, b) = (window_of(pair[0]), window_of(pair[1]));
            if b.from <= a.to {
                return Err(AuditError::refused(
                    "C7-overlap",
                    format!("{} and {} overlap", pair[0].id, pair[1].id),
                ));
            }
            let next = a.to.next_day().map_err(|_| {
                AuditError::refused(
                    "C7-window",
                    format!("{}: window end has no next day", pair[0].id),
                )
            })?;
            if b.from != next {
                return Err(AuditError::refused(
                    "C7-gap",
                    format!(
                        "no voucher window covers {}..{}",
                        iso(&next),
                        iso(&b.from.previous_day().unwrap_or_else(|_| b.from.clone()))
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// A vouchers part always has a window: C1 refuses one without.
pub(crate) fn window_of(part: &Part) -> &Window {
    part.window
        .as_ref()
        .expect("C1 guarantees a window on every vouchers part")
}

/// `YYYY-MM-DD` for a validated date.
pub fn iso(date: &TallyDate) -> String {
    let s = date.as_str();
    format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8])
}
