//! A tally-read-v1, opened and verified (`docs/tax-audit/read-format-v1.md` section 7, rules
//! C1-C7, C10).
//!
//! [`Read::open`] admits a directory only if its manifest is well formed (C1), every path is
//! safe (C2), every stored file and its decoded content match their declared sha256 and length
//! (C3, for every part, consumed or not) and the part table is consistent (C4). [`Read::check`]
//! then applies the rules that need the engagement: company pin and books-from (C5), period
//! (C5), high-water bracket (C6), voucher windows (C7) and Education-mode dates (C10). The
//! remaining rules (C5 identity, C8, C9) need parsed parts and live in `book.rs`.
//!
//! Company identity is (GUID, books_from), never the name: a Tally split company keeps its
//! parent's GUID (one measured pair also differed only in books_from), so the GUID alone lets a
//! read of one pass as the other.
//!
//! The verified decoded content is kept in memory and is what the book parses, so the bytes
//! parsed are the bytes hashed. Gzip blobs make one stored-byte verification pass before their
//! decoder opens a fresh stream, then verify that second stored stream before retaining content.
//!
//! C1 here is a typed subset of the reference engine's JSON Schema (required keys, enums,
//! patterns, the conditional requirements), not a JSON Schema validator. Unknown keys and
//! unknown part kinds are ignored, as a v1.x manifest requires.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use bridge_tally_primitives::TallyDate;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{AuditError, Result};
use crate::xml::MAX_CONTENT_BYTES;

const REQUIRED_KINDS: [&str; 6] = [
    "company",
    "groups",
    "ledgers",
    "trial_balance",
    "voucher_types",
    "vouchers",
];
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

/// A source of one `tally-read-v1` manifest and its stored blobs.
///
/// The reader owns the consumer contract; a store only supplies byte streams.  In particular,
/// a missing key is distinct from a stream that could not be read, so C3 can refuse a missing
/// declared blob without treating it as an empty one.
pub trait ReadStore {
    /// The exact stored bytes of `manifest.json`.
    fn manifest_bytes(&self) -> Result<Box<dyn io::Read + '_>>;

    /// The exact stored bytes named by a manifest blob path, or `None` when that key is absent.
    /// Each call returns a fresh stream positioned at byte zero: gzip verification opens the
    /// declared key twice and compares both streams with the manifest before returning content.
    fn stored_blob(&self, path: &str) -> Result<Option<Box<dyn io::Read + '_>>>;
}

/// The company an engagement's reads must come from: `[client.tally]` `company_guid` and
/// `books_from`. The GUID is kept lowercase; GUID case is not identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyPin {
    pub guid: String,
    pub books_from: TallyDate,
}

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
    /// The directory root for [`Read::open`], empty for a storage-backed read.
    pub root: PathBuf,
    pub company_guid: String,
    /// `company.books_from`, when the manifest records it.
    pub books_from: Option<TallyDate>,
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

pub(crate) fn is_uuid(s: &str) -> bool {
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

/// The directory implementation of [`ReadStore`].  C2 belongs here because only a directory
/// has symlinks or a path that can escape its read root.
struct DirectoryReadStore<'a> {
    root: &'a Path,
}

impl ReadStore for DirectoryReadStore<'_> {
    fn manifest_bytes(&self) -> Result<Box<dyn io::Read + '_>> {
        let manifest_path = self.root.join("manifest.json");
        let meta = std::fs::symlink_metadata(&manifest_path);
        if !meta.is_ok_and(|m| m.file_type().is_file()) {
            return Err(AuditError::refused(
                "C2-manifest",
                format!("no regular manifest.json in {}", self.root.display()),
            ));
        }
        std::fs::File::open(&manifest_path)
            .map(|file| Box::new(file) as Box<dyn io::Read>)
            .map_err(|source| AuditError::Io {
                path: manifest_path.display().to_string(),
                source,
            })
    }

    fn stored_blob(&self, rel: &str) -> Result<Option<Box<dyn io::Read + '_>>> {
        let path = safe_path(self.root, rel)?;
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(AuditError::Io {
                    path: rel.to_string(),
                    source,
                });
            }
        };
        if !meta.file_type().is_file() {
            return Ok(None);
        }
        std::fs::File::open(&path)
            .map(|file| Some(Box::new(file) as Box<dyn io::Read>))
            .map_err(|source| AuditError::Io {
                path: rel.to_string(),
                source,
            })
    }
}

struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    bytes: u64,
    max_bytes: u64,
    latched_source_error: Option<io::Error>,
}

impl<R> HashingReader<R> {
    fn new(inner: R, declared_bytes: u64) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            bytes: 0,
            // Read one byte past the declared size. That is enough to prove a length mismatch
            // without following an unbounded corrupt stream. `u64::MAX` has no representable
            // successor, so it remains its own cap.
            max_bytes: declared_bytes.saturating_add(1),
            latched_source_error: None,
        }
    }

    fn finish(self) -> (u64, String) {
        (self.bytes, crate::canonical::hex(&self.hasher.finalize()))
    }

    fn take_source_error(&mut self) -> Option<io::Error> {
        self.latched_source_error.take()
    }
}

impl<R: io::Read> io::Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.max_bytes.saturating_sub(self.bytes);
        if remaining == 0 {
            return Ok(0);
        }
        let allowed = usize::try_from(remaining.min(buf.len() as u64))
            .expect("the buffer length bounds this conversion");
        if self.latched_source_error.is_some() {
            return Err(io::Error::other("stored reader failed"));
        }
        let read = loop {
            match self.inner.read(&mut buf[..allowed]) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    self.latched_source_error = Some(error);
                    return Err(io::Error::other("stored reader failed"));
                }
                Ok(read) => break read,
            }
        };
        self.hasher.update(&buf[..read]);
        self.bytes = self.bytes.saturating_add(read as u64);
        Ok(read)
    }
}

fn stored_mismatch(blob: &Blob, what: &str) -> AuditError {
    AuditError::refused(
        "C3-stored-hash",
        format!(
            "{what}: stored bytes of {} do not match the manifest",
            blob.path
        ),
    )
}

fn ensure_stored(blob: &Blob, what: &str, stored_bytes: u64, stored_sha256: &str) -> Result<()> {
    if stored_bytes != blob.stored_bytes || stored_sha256 != blob.stored_sha256 {
        Err(stored_mismatch(blob, what))
    } else {
        Ok(())
    }
}

fn open_stored_blob<'a>(
    store: &'a dyn ReadStore,
    blob: &Blob,
    what: &str,
) -> Result<Box<dyn io::Read + 'a>> {
    store.stored_blob(&blob.path)?.ok_or_else(|| {
        AuditError::refused("C3-missing", format!("{what}: {} not found", blob.path))
    })
}

/// Hash a stored blob before decoding it. The reader stops after the declared length plus one
/// byte, so a wrong declaration is refused without consuming an unbounded source.
fn verify_stored_blob(store: &dyn ReadStore, blob: &Blob, what: &str) -> Result<()> {
    let stored_reader = open_stored_blob(store, blob, what)?;
    let mut hashing_reader = HashingReader::new(stored_reader, blob.stored_bytes);
    if let Err(error) = io::copy(&mut hashing_reader, &mut io::sink()) {
        return Err(AuditError::Io {
            path: blob.path.clone(),
            source: hashing_reader.take_source_error().unwrap_or(error),
        });
    }
    let (stored_bytes, stored_sha256) = hashing_reader.finish();
    ensure_stored(blob, what, stored_bytes, &stored_sha256)
}

fn collect_content<R: io::Read>(
    reader: &mut R,
    what: &str,
    max_bytes: Option<usize>,
    stream_error: impl Fn(io::Error) -> AuditError,
) -> Result<(Vec<u8>, u64, String)> {
    let mut content = Vec::new();
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(&stream_error)?;
        if read == 0 {
            break;
        }
        if let Some(limit) = max_bytes {
            if content.len().saturating_add(read) > limit {
                return Err(AuditError::refused(
                    "C3-size",
                    format!("{what}: decompressed size exceeds {limit} bytes"),
                ));
            }
        }
        hasher.update(&buffer[..read]);
        bytes = bytes.saturating_add(read as u64);
        content.extend_from_slice(&buffer[..read]);
    }
    Ok((content, bytes, crate::canonical::hex(&hasher.finalize())))
}

/// C2 + C3 for one stored blob: stream both byte domains and return the verified content.
fn verified_blob_from(
    store: &dyn ReadStore,
    blob: &Blob,
    what: &str,
    max_content_bytes: usize,
) -> Result<Vec<u8>> {
    let file_name = blob.path.rsplit('/').next().unwrap_or_default();
    // Case-sensitive, as the reference engine's `name.endswith(".gz")` is.
    let gz_name = Path::new(file_name).extension().is_some_and(|e| e == "gz");
    if blob.gzip != gz_name {
        return Err(AuditError::refused(
            "C2-storage",
            format!("{what}: storage does not match file name {file_name:?}"),
        ));
    }
    let (content, content_bytes, content_sha256) = if blob.gzip {
        // C3 records the stored checksum specifically so corrupt storage is detected before
        // decompression. Re-open and check the second stream too: a mutable store cannot swap a
        // valid first stream for different decoded bytes between these two operations.
        verify_stored_blob(store, blob, what)?;
        let hashing_reader =
            HashingReader::new(open_stored_blob(store, blob, what)?, blob.stored_bytes);
        let mut gzip_decoder = flate2::read::MultiGzDecoder::new(hashing_reader);
        if let Some(source) = gzip_decoder.get_mut().take_source_error() {
            return Err(AuditError::Io {
                path: blob.path.clone(),
                source,
            });
        }
        let decoded_result =
            collect_content(&mut gzip_decoder, what, Some(max_content_bytes), |error| {
                AuditError::refused(
                    "C3-content-hash",
                    format!("{what}: gzip does not decode: {error}"),
                )
            });
        let mut hashing_reader = gzip_decoder.into_inner();
        if let Some(source) = hashing_reader.take_source_error() {
            return Err(AuditError::Io {
                path: blob.path.clone(),
                source,
            });
        }
        let drain = io::copy(&mut hashing_reader, &mut io::sink());
        let source_error = hashing_reader.take_source_error();
        let (stored_bytes, stored_sha256) = hashing_reader.finish();
        if let Some(source) = source_error {
            return Err(AuditError::Io {
                path: blob.path.clone(),
                source,
            });
        }
        drain.map_err(|source| AuditError::Io {
            path: blob.path.clone(),
            source,
        })?;
        ensure_stored(blob, what, stored_bytes, &stored_sha256)?;
        let (content, content_bytes, content_sha256) = decoded_result?;
        (content, content_bytes, content_sha256)
    } else {
        let mut hashing_reader =
            HashingReader::new(open_stored_blob(store, blob, what)?, blob.stored_bytes);
        let content_result =
            collect_content(&mut hashing_reader, what, None, |source| AuditError::Io {
                path: blob.path.clone(),
                source,
            });
        if let Some(source) = hashing_reader.take_source_error() {
            return Err(AuditError::Io {
                path: blob.path.clone(),
                source,
            });
        }
        let (stored_bytes, stored_sha256) = hashing_reader.finish();
        ensure_stored(blob, what, stored_bytes, &stored_sha256)?;
        content_result?
    };
    if content_bytes != blob.bytes || content_sha256 != blob.sha256 {
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
    /// C1-C4 through the directory compatibility adapter.
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_inner(&DirectoryReadStore { root }, root.to_path_buf(), None)
    }

    /// C1-C4 from a storage-neutral source, bound to the handle a producer returned.
    ///
    /// `read_id` and `manifest_sha256` identify an immutable read.  A caller that has no handle
    /// uses [`Read::open`] for an exported directory instead.
    pub fn open_from(store: &dyn ReadStore, read_id: &str, manifest_sha256: &str) -> Result<Self> {
        Self::open_inner(store, PathBuf::new(), Some((read_id, manifest_sha256)))
    }

    /// C1-C4. Every part's bytes, and every stored request's, are verified before this returns.
    #[allow(clippy::too_many_lines)] // one pass over the manifest, in the reference order
    fn open_inner(
        store: &dyn ReadStore,
        root: PathBuf,
        handle: Option<(&str, &str)>,
    ) -> Result<Self> {
        let manifest_bytes = {
            let mut manifest_reader = store.manifest_bytes()?;
            let mut manifest_bytes = Vec::new();
            manifest_reader
                .read_to_end(&mut manifest_bytes)
                .map_err(|source| AuditError::Io {
                    path: "manifest.json".to_string(),
                    source,
                })?;
            manifest_bytes
        };
        if let Some((expected_read_id, expected_manifest_sha256)) = handle {
            if !is_sha256(expected_manifest_sha256)
                || crate::canonical::hex(&Sha256::digest(&manifest_bytes))
                    != expected_manifest_sha256
            {
                return Err(AuditError::refused(
                    "C1-handle",
                    "manifest bytes do not match the supplied read handle",
                ));
            }
            if expected_read_id.is_empty() {
                return Err(AuditError::refused(
                    "C1-handle",
                    "read handle has an empty read_id",
                ));
            }
        }
        let text = String::from_utf8(manifest_bytes).map_err(|source| AuditError::Io {
            path: "manifest.json".to_string(),
            source: io::Error::new(io::ErrorKind::InvalidData, source),
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
        if let Some((expected_read_id, _)) = handle {
            if read_id != expected_read_id {
                return Err(AuditError::refused(
                    "C1-handle",
                    "manifest read_id does not match the supplied read handle",
                ));
            }
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
        let books_from = opt_string(company, "books_from", "company")?
            .map(|s| iso_date(s, "company/books_from"))
            .transpose()?;
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
            part.content = verified_blob_from(store, &d.response, &part.id, MAX_CONTENT_BYTES)?;
            if let Some(request) = &d.request {
                verified_blob_from(
                    store,
                    request,
                    &format!("{} request", part.id),
                    MAX_CONTENT_BYTES,
                )?;
            }
            parts.push(part);
        }
        Ok(Self {
            root,
            company_guid: company_guid.to_string(),
            books_from,
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

    /// C5 (client pin, period, books-from), C6, C7, C10.
    pub fn check(
        &self,
        period: &Window,
        allow_unbracketed: bool,
        pin: Option<&CompanyPin>,
    ) -> Result<()> {
        if let Some(pin) = pin {
            if !self.company_guid.eq_ignore_ascii_case(&pin.guid) {
                return Err(AuditError::refused(
                    "C5-client",
                    format!(
                        "read of company {}, engagement is pinned to {}",
                        self.company_guid, pin.guid
                    ),
                ));
            }
            if self.books_from.as_ref() != Some(&pin.books_from) {
                return Err(AuditError::refused(
                    "C5-client",
                    format!(
                        "read's company books begin {}, engagement is pinned to books beginning {}",
                        self.books_from
                            .as_ref()
                            .map_or("unrecorded".to_string(), iso),
                        iso(&pin.books_from)
                    ),
                ));
            }
        }
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
        // A company whose books begin after the period ends cannot hold it (the split read for
        // its parent's year). Books that begin inside the period, a first year, are admitted.
        if let Some(books_from) = &self.books_from {
            if *books_from > self.period.to {
                return Err(AuditError::refused(
                    "C5-books-from",
                    format!(
                        "this company's books begin {}, after the read period ends {} (a split \
company keeps its parent's GUID)",
                        iso(books_from),
                        iso(&self.period.to)
                    ),
                ));
            }
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::io::{Cursor, Write as _};

    use flate2::write::GzEncoder;
    use flate2::Compression;

    use super::*;

    struct MemoryStore {
        blobs: BTreeMap<String, Vec<u8>>,
    }

    impl ReadStore for MemoryStore {
        fn manifest_bytes(&self) -> Result<Box<dyn io::Read + '_>> {
            Err(AuditError::Config("not used by this unit test".to_string()))
        }

        fn stored_blob(&self, path: &str) -> Result<Option<Box<dyn io::Read + '_>>> {
            Ok(self
                .blobs
                .get(path)
                .cloned()
                .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn io::Read>))
        }
    }

    struct SwitchingStore {
        first: Vec<u8>,
        second: Vec<u8>,
        opens: Cell<usize>,
    }

    impl ReadStore for SwitchingStore {
        fn manifest_bytes(&self) -> Result<Box<dyn io::Read + '_>> {
            Err(AuditError::Config("not used by this unit test".to_string()))
        }

        fn stored_blob(&self, _path: &str) -> Result<Option<Box<dyn io::Read + '_>>> {
            let opens = self.opens.get();
            self.opens.set(opens + 1);
            let bytes = if opens == 0 {
                &self.first
            } else {
                &self.second
            };
            Ok(Some(Box::new(Cursor::new(bytes.clone()))))
        }
    }

    #[derive(Clone)]
    enum StoredRead {
        Bytes(Vec<u8>),
        InterruptedOnce(Vec<u8>),
        BytesThenError(Vec<u8>, io::ErrorKind),
        Error(io::ErrorKind),
    }

    struct InterruptedOnceReader {
        inner: Cursor<Vec<u8>>,
        interrupted: bool,
    }

    impl io::Read for InterruptedOnceReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            self.inner.read(buf)
        }
    }

    struct ErrorReader(io::ErrorKind);

    impl io::Read for ErrorReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from(self.0))
        }
    }

    struct BytesThenErrorReader {
        inner: Cursor<Vec<u8>>,
        error: io::ErrorKind,
    }

    impl io::Read for BytesThenErrorReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buf)?;
            if read == 0 {
                Err(io::Error::from(self.error))
            } else {
                Ok(read)
            }
        }
    }

    struct ScriptedStore {
        reads: Vec<StoredRead>,
        opens: Cell<usize>,
    }

    impl ReadStore for ScriptedStore {
        fn manifest_bytes(&self) -> Result<Box<dyn io::Read + '_>> {
            Err(AuditError::Config("not used by this unit test".to_string()))
        }

        fn stored_blob(&self, _path: &str) -> Result<Option<Box<dyn io::Read + '_>>> {
            let open = self.opens.get();
            self.opens.set(open + 1);
            let read = self.reads.get(open).unwrap_or_else(|| {
                panic!(
                    "unexpected stored-blob open {open}; expected {}",
                    self.reads.len()
                )
            });
            let reader: Box<dyn io::Read> = match read {
                StoredRead::Bytes(bytes) => Box::new(Cursor::new(bytes.clone())),
                StoredRead::InterruptedOnce(bytes) => Box::new(InterruptedOnceReader {
                    inner: Cursor::new(bytes.clone()),
                    interrupted: false,
                }),
                StoredRead::BytesThenError(bytes, error) => Box::new(BytesThenErrorReader {
                    inner: Cursor::new(bytes.clone()),
                    error: *error,
                }),
                StoredRead::Error(kind) => Box::new(ErrorReader(*kind)),
            };
            Ok(Some(reader))
        }
    }

    fn gzip(content: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(content).unwrap();
        encoder.finish().unwrap()
    }

    fn gzip_blob(path: &str, stored: &[u8], content: &[u8]) -> Blob {
        Blob {
            path: path.to_string(),
            gzip: true,
            stored_sha256: crate::canonical::hex(&Sha256::digest(stored)),
            stored_bytes: stored.len() as u64,
            sha256: crate::canonical::hex(&Sha256::digest(content)),
            bytes: content.len() as u64,
        }
    }

    fn identity_blob(path: &str, content: &[u8]) -> Blob {
        Blob {
            path: path.to_string(),
            gzip: false,
            stored_sha256: crate::canonical::hex(&Sha256::digest(content)),
            stored_bytes: content.len() as u64,
            sha256: crate::canonical::hex(&Sha256::digest(content)),
            bytes: content.len() as u64,
        }
    }

    fn assert_io_kind(error: AuditError, expected: io::ErrorKind) {
        match error {
            AuditError::Io { source, .. } => assert_eq!(source.kind(), expected),
            other => panic!("expected a typed I/O error, got {other:?}"),
        }
    }

    #[test]
    fn gzip_content_over_the_cap_is_refused_without_collecting_the_extra_chunk() {
        let content = vec![b'x'; 1_025];
        let stored = gzip(&content);
        let blob = gzip_blob("parts/large.xml.gz", &stored, &content);
        let store = MemoryStore {
            blobs: BTreeMap::from([(blob.path.clone(), stored)]),
        };

        let error = verified_blob_from(&store, &blob, "large", 1_024).unwrap_err();
        assert_eq!(error.code(), Some("C3-size"));
    }

    #[test]
    fn malformed_gzip_with_a_wrong_stored_hash_is_refused_before_a_second_open() {
        let expected = gzip(b"valid");
        let blob = gzip_blob("parts/corrupt.xml.gz", &expected, b"valid");
        let store = SwitchingStore {
            first: b"not a gzip stream".to_vec(),
            second: expected.clone(),
            opens: Cell::new(0),
        };

        let error = verified_blob_from(&store, &blob, "corrupt", 1_024).unwrap_err();
        assert_eq!(error.code(), Some("C3-stored-hash"));
        assert_eq!(
            store.opens.get(),
            1,
            "decoder must not open an unverified blob"
        );
    }

    #[test]
    fn malformed_gzip_after_a_matching_stored_check_is_content_refusal() {
        let stored = b"not a gzip stream".to_vec();
        let blob = gzip_blob("parts/malformed.xml.gz", &stored, b"valid");
        let store = MemoryStore {
            blobs: BTreeMap::from([(blob.path.clone(), stored)]),
        };

        let error = verified_blob_from(&store, &blob, "malformed", 1_024).unwrap_err();
        assert_eq!(error.code(), Some("C3-content-hash"));
    }

    #[test]
    fn a_first_gzip_stream_with_the_wrong_stored_hash_is_refused_before_decode() {
        let content = b"same decoded content";
        let expected = gzip(content);
        let mut first = expected.clone();
        first[4] = first[4].wrapping_add(1);
        let blob = gzip_blob("parts/first-changing.xml.gz", &expected, content);
        let store = SwitchingStore {
            first,
            second: expected,
            opens: Cell::new(0),
        };

        let error = verified_blob_from(&store, &blob, "first-changing", 1_024).unwrap_err();
        assert_eq!(error.code(), Some("C3-stored-hash"));
        assert_eq!(
            store.opens.get(),
            1,
            "decoder must not open an unverified blob"
        );
    }

    #[test]
    fn a_store_that_changes_between_gzip_passes_is_refused() {
        let content = b"same decoded content";
        let first = gzip(content);
        let mut second = first.clone();
        // MTIME occupies bytes 4..8 in a gzip member header. It changes the stored hash while
        // preserving both the length and every decoded byte, so only the second stored check can
        // distinguish these streams.
        second[4] = second[4].wrapping_add(1);
        assert_eq!(first.len(), second.len());
        let blob = gzip_blob("parts/changing.xml.gz", &first, content);
        let store = SwitchingStore {
            first,
            second,
            opens: Cell::new(0),
        };

        let error = verified_blob_from(&store, &blob, "changing", 1_024).unwrap_err();
        assert_eq!(error.code(), Some("C3-stored-hash"));
        assert_eq!(store.opens.get(), 2);
    }

    #[test]
    fn valid_multi_member_gzip_content_is_verified_and_returned() {
        let first = gzip(b"first ");
        let second = gzip(b"second");
        let mut stored = first;
        stored.extend(second);
        let content = b"first second";
        let blob = gzip_blob("parts/multi.xml.gz", &stored, content);
        let store = MemoryStore {
            blobs: BTreeMap::from([(blob.path.clone(), stored)]),
        };

        assert_eq!(
            verified_blob_from(&store, &blob, "multi", 1_024).unwrap(),
            content
        );
    }

    #[test]
    fn interrupted_stored_reads_retry_for_identity_and_gzip() {
        let identity_content = b"identity";
        let identity = identity_blob("parts/identity.xml", identity_content);
        let identity_store = ScriptedStore {
            reads: vec![StoredRead::InterruptedOnce(identity_content.to_vec())],
            opens: Cell::new(0),
        };
        assert_eq!(
            verified_blob_from(&identity_store, &identity, "identity", 1_024).unwrap(),
            identity_content
        );

        let gzip_content = b"gzip";
        let gzip_stored = gzip(gzip_content);
        let gzip = gzip_blob("parts/retry.xml.gz", &gzip_stored, gzip_content);
        let gzip_store = ScriptedStore {
            reads: vec![
                StoredRead::InterruptedOnce(gzip_stored.clone()),
                StoredRead::InterruptedOnce(gzip_stored),
            ],
            opens: Cell::new(0),
        };
        assert_eq!(
            verified_blob_from(&gzip_store, &gzip, "gzip", 1_024).unwrap(),
            gzip_content
        );
        assert_eq!(gzip_store.opens.get(), 2);
    }

    #[test]
    fn persistent_noninterrupted_stored_read_is_a_typed_io_error() {
        let blob = identity_blob("parts/unavailable.xml", b"unavailable");
        let store = ScriptedStore {
            reads: vec![StoredRead::Error(io::ErrorKind::PermissionDenied)],
            opens: Cell::new(0),
        };

        assert_io_kind(
            verified_blob_from(&store, &blob, "unavailable", 1_024).unwrap_err(),
            io::ErrorKind::PermissionDenied,
        );
    }

    #[test]
    fn a_second_gzip_stream_transport_failure_is_a_typed_io_error() {
        let content = b"gzip transport";
        let stored = gzip(content);
        let blob = gzip_blob("parts/transport.xml.gz", &stored, content);
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::ConnectionReset] {
            let store = ScriptedStore {
                reads: vec![
                    StoredRead::Bytes(stored.clone()),
                    StoredRead::BytesThenError(stored.clone(), kind),
                ],
                opens: Cell::new(0),
            };
            assert_io_kind(
                verified_blob_from(&store, &blob, "transport", 1_024).unwrap_err(),
                kind,
            );
            assert_eq!(store.opens.get(), 2);
        }
    }

    #[test]
    fn a_second_gzip_prefix_then_transport_failure_is_not_a_stored_mismatch() {
        let content = b"gzip prefix";
        let stored = gzip(content);
        let blob = gzip_blob("parts/prefix.xml.gz", &stored, content);
        let store = ScriptedStore {
            reads: vec![
                StoredRead::Bytes(stored.clone()),
                StoredRead::BytesThenError(
                    stored[..stored.len() / 2].to_vec(),
                    io::ErrorKind::TimedOut,
                ),
            ],
            opens: Cell::new(0),
        };

        assert_io_kind(
            verified_blob_from(&store, &blob, "prefix", 1_024).unwrap_err(),
            io::ErrorKind::TimedOut,
        );
        assert_eq!(store.opens.get(), 2);
    }

    #[test]
    fn a_drain_transport_failure_wins_over_matching_malformed_gzip() {
        let stored = b"not a gzip stream".to_vec();
        let blob = gzip_blob("parts/drain.xml.gz", &stored, b"valid");
        let store = ScriptedStore {
            reads: vec![
                StoredRead::Bytes(stored.clone()),
                StoredRead::BytesThenError(stored, io::ErrorKind::ConnectionReset),
            ],
            opens: Cell::new(0),
        };

        assert_io_kind(
            verified_blob_from(&store, &blob, "drain", 1_024).unwrap_err(),
            io::ErrorKind::ConnectionReset,
        );
        assert_eq!(store.opens.get(), 2);
    }
}
