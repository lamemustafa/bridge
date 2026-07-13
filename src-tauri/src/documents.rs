use std::collections::{HashMap, HashSet, VecDeque};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::axal::{
    auth_headers, credentials_for_session, endpoint, response_text_limited,
    sanitized_server_message, validate_credentials_for, validate_identifier,
    validate_workspace_binding, AxalCredentials, IntegrationKind,
};

const DEFAULT_MAX_FILE_SIZE: u64 = 1_500 * 1024 * 1024;
const DEFAULT_BATCH_SIZE: usize = 20;
const MAX_SCANNED_FILES: usize = 10_000;
const MAX_SCAN_SESSIONS: usize = 8;
const SELECTION_TTL: Duration = Duration::from_secs(10 * 60);
const SCAN_SESSION_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Default)]
struct ScanRegistry {
    sessions: HashMap<String, ScanSession>,
}

struct ScanSession {
    files: HashMap<String, DocumentFile>,
    created_at: Instant,
}

struct AuthorizedSelection {
    path: PathBuf,
    created_at: Instant,
}

static SCAN_REGISTRY: OnceLock<Mutex<ScanRegistry>> = OnceLock::new();
static AUTHORIZED_SELECTIONS: OnceLock<Mutex<HashMap<String, AuthorizedSelection>>> =
    OnceLock::new();

#[derive(Debug, Clone, Deserialize)]
pub struct ScanDocumentsRequest {
    pub selection_ids: Vec<String>,
    #[serde(default = "default_true")]
    pub use_hash: bool,
    pub max_file_size: Option<u64>,
    pub excluded_extensions: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub exclude_hidden_files: bool,
    #[serde(default = "default_true")]
    pub exclude_zero_byte_files: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedDocumentPath {
    pub selection_id: String,
    pub display_name: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentFile {
    pub scan_id: String,
    pub relative_path: String,
    pub size: u64,
    pub mtime: i64,
    pub extension: Option<String>,
    pub mime_type: String,
    pub hash: Option<String>,
    pub content_hash: Option<String>,
    pub server_file_key: Option<String>,
    pub multipart_info: Option<MultipartUploadInfo>,
    #[serde(skip)]
    native_path: PathBuf,
    #[serde(skip)]
    integrity_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultipartUploadInfo {
    pub upload_id: String,
    pub parts: Vec<CompletedMultipartPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedMultipartPart {
    pub part_number: u32,
    pub etag: String,
    pub size: u64,
    pub bytes_read: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanDocumentsResponse {
    pub scan_session_id: String,
    pub files: Vec<DocumentFile>,
    pub total_size: u64,
    pub skipped: Vec<SkippedFile>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDocumentsRequest {
    pub credential_session_id: String,
    pub workspace_external_id: String,
    pub scan_session_id: String,
    pub files: Vec<DocumentFile>,
    pub max_files_per_batch: Option<usize>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDocumentsResponse {
    pub success: bool,
    pub uploaded_files: Vec<DocumentFile>,
    pub failed_files: Vec<FailedDocument>,
    pub duplicate_count: usize,
    pub batch_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedDocument {
    pub relative_path: String,
    pub error: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresignResponse {
    batch_id: String,
    documents: Vec<PresignedDocument>,
    #[serde(default)]
    duplicates: Vec<DuplicateDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateDocument {
    relative_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresignedDocument {
    relative_path: String,
    file_key: String,
    url: Option<String>,
    #[serde(default)]
    is_multipart: bool,
    upload_id: Option<String>,
    #[serde(default)]
    parts: Vec<PresignedPart>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresignedPart {
    part_number: u32,
    url: String,
    expected_size: u64,
}

pub async fn scan_documents(
    request: ScanDocumentsRequest,
) -> anyhow::Result<ScanDocumentsResponse> {
    if request.selection_ids.is_empty() {
        anyhow::bail!("Select at least one file or folder with the native picker");
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut relative_paths = HashSet::new();
    let excluded = normalized_exclusions(request.excluded_extensions.clone());
    let max_file_size = request
        .max_file_size
        .unwrap_or(DEFAULT_MAX_FILE_SIZE)
        .min(DEFAULT_MAX_FILE_SIZE);

    for selection_id in &request.selection_ids {
        if files.len() >= MAX_SCANNED_FILES {
            break;
        }
        let Some(supplied_root) = consume_authorized_selection(selection_id)? else {
            skipped.push(SkippedFile {
                path: "Selection".to_string(),
                reason: "Selection is invalid, expired, or already used".to_string(),
            });
            continue;
        };
        let display_root = display_name(&supplied_root);
        let root_metadata = match std::fs::symlink_metadata(&supplied_root) {
            Ok(metadata) => metadata,
            Err(error) => {
                skipped.push(SkippedFile {
                    path: display_root.clone(),
                    reason: format!("Could not inspect path: {error}"),
                });
                continue;
            }
        };
        if is_link_or_reparse_point(&root_metadata) {
            skipped.push(SkippedFile {
                path: display_root.clone(),
                reason: "Symbolic links and filesystem reparse points are not scanned".to_string(),
            });
            continue;
        }
        let root = match std::fs::canonicalize(&supplied_root) {
            Ok(root) => root,
            Err(error) => {
                skipped.push(SkippedFile {
                    path: display_root.clone(),
                    reason: format!("Could not resolve path: {error}"),
                });
                continue;
            }
        };
        if root_metadata.is_file() {
            match inspect_file(
                &root,
                root.parent().unwrap_or_else(|| Path::new("")),
                &request,
                max_file_size,
                &excluded,
            )
            .await
            {
                Ok(Some(file)) => {
                    push_unique_file(&mut files, &mut relative_paths, file, &mut skipped)
                }
                Ok(None) => {}
                Err(error) => skipped.push(SkippedFile {
                    path: display_root.clone(),
                    reason: error.to_string(),
                }),
            }
            continue;
        }
        if !root_metadata.is_dir() {
            skipped.push(SkippedFile {
                path: display_root,
                reason: "Path is not a regular file or directory".to_string(),
            });
            continue;
        }

        let mut queue = VecDeque::from([root.clone()]);
        let mut visited_directories = HashSet::from([root.clone()]);
        while let Some(path) = queue.pop_front() {
            if files.len() >= MAX_SCANNED_FILES {
                break;
            }
            let entries = match std::fs::read_dir(&path) {
                Ok(entries) => entries,
                Err(error) => {
                    skipped.push(SkippedFile {
                        path: relative_display(&path, &root),
                        reason: error.to_string(),
                    });
                    continue;
                }
            };

            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        skipped.push(SkippedFile {
                            path: relative_display(&path, &root),
                            reason: error.to_string(),
                        });
                        continue;
                    }
                };
                let entry_path = entry.path();
                let entry_metadata = match std::fs::symlink_metadata(&entry_path) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        skipped.push(SkippedFile {
                            path: relative_display(&entry_path, &root),
                            reason: format!("Could not inspect path: {error}"),
                        });
                        continue;
                    }
                };
                if is_link_or_reparse_point(&entry_metadata) {
                    skipped.push(SkippedFile {
                        path: relative_display(&entry_path, &root),
                        reason: "Symbolic links and filesystem reparse points are not scanned"
                            .to_string(),
                    });
                    continue;
                }
                if is_hidden_name(&entry_path) || is_hidden_metadata(&entry_metadata) {
                    continue;
                }
                let canonical_entry = match std::fs::canonicalize(&entry_path) {
                    Ok(path) if path.starts_with(&root) => path,
                    Ok(_) => {
                        skipped.push(SkippedFile {
                            path: relative_display(&entry_path, &root),
                            reason: "Resolved path escaped the selected root".to_string(),
                        });
                        continue;
                    }
                    Err(error) => {
                        skipped.push(SkippedFile {
                            path: relative_display(&entry_path, &root),
                            reason: format!("Could not resolve path: {error}"),
                        });
                        continue;
                    }
                };
                if entry_metadata.is_dir() {
                    if visited_directories.insert(canonical_entry.clone()) {
                        queue.push_back(canonical_entry);
                    }
                } else if entry_metadata.is_file() {
                    match inspect_file(&canonical_entry, &root, &request, max_file_size, &excluded)
                        .await
                    {
                        Ok(Some(file)) => {
                            push_unique_file(&mut files, &mut relative_paths, file, &mut skipped)
                        }
                        Ok(None) => {}
                        Err(error) => skipped.push(SkippedFile {
                            path: relative_display(&entry_path, &root),
                            reason: error.to_string(),
                        }),
                    }
                }
            }
        }
    }

    if files.len() >= MAX_SCANNED_FILES {
        skipped.push(SkippedFile {
            path: ".".to_string(),
            reason: format!("Scan stopped at the {MAX_SCANNED_FILES}-file safety limit"),
        });
    }
    let total_size = files.iter().map(|file| file.size).sum();
    let registry_files = files
        .iter()
        .cloned()
        .map(|file| (file.scan_id.clone(), file))
        .collect();
    let mut registry = scan_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("Document scan registry is unavailable"))?;
    let now = Instant::now();
    registry
        .sessions
        .retain(|_, session| now.duration_since(session.created_at) <= SCAN_SESSION_TTL);
    if registry.sessions.len() >= MAX_SCAN_SESSIONS {
        if let Some(oldest) = registry
            .sessions
            .iter()
            .min_by_key(|(_, session)| session.created_at)
            .map(|(id, _)| id.clone())
        {
            registry.sessions.remove(&oldest);
        }
    }
    registry.sessions.insert(
        session_id.clone(),
        ScanSession {
            files: registry_files,
            created_at: now,
        },
    );
    Ok(ScanDocumentsResponse {
        scan_session_id: session_id,
        files,
        total_size,
        skipped,
    })
}

pub fn authorize_selected_paths(paths: Vec<PathBuf>) -> anyhow::Result<Vec<SelectedDocumentPath>> {
    let mut authorized = authorized_selections()
        .lock()
        .map_err(|_| anyhow::anyhow!("Document selection registry is unavailable"))?;
    let now = Instant::now();
    authorized.retain(|_, selection| now.duration_since(selection.created_at) <= SELECTION_TTL);
    let mut selections = Vec::new();
    for path in paths {
        let metadata = std::fs::symlink_metadata(&path)?;
        if is_link_or_reparse_point(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
            anyhow::bail!("Selected path is not a regular file or directory");
        }
        let canonical = std::fs::canonicalize(&path)?;
        if authorized.len() >= MAX_SCANNED_FILES {
            authorized.clear();
        }
        let selection_id = uuid::Uuid::new_v4().to_string();
        let display_name = display_name(&path);
        authorized.insert(
            selection_id.clone(),
            AuthorizedSelection {
                path: canonical,
                created_at: now,
            },
        );
        selections.push(SelectedDocumentPath {
            selection_id,
            display_name,
        });
    }
    Ok(selections)
}

fn authorized_selections() -> &'static Mutex<HashMap<String, AuthorizedSelection>> {
    AUTHORIZED_SELECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn consume_authorized_selection(selection_id: &str) -> anyhow::Result<Option<PathBuf>> {
    let mut authorized = authorized_selections()
        .lock()
        .map_err(|_| anyhow::anyhow!("Document selection registry is unavailable"))?;
    let now = Instant::now();
    authorized.retain(|_, selection| now.duration_since(selection.created_at) <= SELECTION_TTL);
    Ok(authorized
        .remove(selection_id)
        .map(|selection| selection.path))
}

pub fn revoke_document_authorizations(
    selection_ids: &[String],
    scan_session_id: Option<&str>,
) -> anyhow::Result<()> {
    let mut selections = authorized_selections()
        .lock()
        .map_err(|_| anyhow::anyhow!("Document selection registry is unavailable"))?;
    for selection_id in selection_ids {
        selections.remove(selection_id);
    }
    drop(selections);

    if let Some(scan_session_id) = scan_session_id.filter(|value| !value.is_empty()) {
        scan_registry()
            .lock()
            .map_err(|_| anyhow::anyhow!("Document scan registry is unavailable"))?
            .sessions
            .remove(scan_session_id);
    }
    Ok(())
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Selected item")
        .to_string()
}

pub async fn sync_documents(
    request: SyncDocumentsRequest,
) -> anyhow::Result<SyncDocumentsResponse> {
    let credentials = credentials_for_session(
        &request.credential_session_id,
        Some(IntegrationKind::Documents),
    )?;
    validate_credentials_for(&credentials, IntegrationKind::Documents)?;
    validate_identifier("workspace external ID", &request.workspace_external_id)?;
    validate_workspace_binding(
        &request.credential_session_id,
        &request.workspace_external_id,
    )?;
    if request.files.is_empty() {
        anyhow::bail!("No files selected for document sync");
    }
    let files = resolve_scanned_files(&request.scan_session_id, &request.files)?;

    let batch_size = request
        .max_files_per_batch
        .unwrap_or(DEFAULT_BATCH_SIZE)
        .clamp(1, 100);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30 * 60))
        .redirect(Policy::none())
        .build()?;
    let mut uploaded_files = Vec::new();
    let mut failed_files = Vec::new();
    let mut duplicate_count = 0;
    let mut batch_ids = Vec::new();

    for batch in files.chunks(batch_size) {
        let presign =
            request_presigned_urls(&client, &credentials, &request.workspace_external_id, batch)
                .await?;
        validate_presign_mapping(batch, &presign)?;
        duplicate_count += presign.duplicates.len();
        batch_ids.push(presign.batch_id.clone());

        let mut confirmed_files = Vec::new();
        for document in presign.documents {
            let Some(mut file) = batch
                .iter()
                .find(|file| file.relative_path == document.relative_path)
                .cloned()
            else {
                continue;
            };
            file.server_file_key = Some(document.file_key.clone());

            if document.is_multipart {
                match upload_multipart_file(&client, &document, &mut file).await {
                    Ok(()) => {
                        confirmed_files.push(file.clone());
                        uploaded_files.push(file);
                    }
                    Err(error) => failed_files.push(FailedDocument {
                        relative_path: file.relative_path,
                        error: error.to_string(),
                    }),
                }
            } else {
                let Some(url) = document.url else {
                    failed_files.push(FailedDocument {
                        relative_path: file.relative_path,
                        error: "AXAL did not return an upload URL".to_string(),
                    });
                    continue;
                };

                match upload_single_file(&client, &url, &file).await {
                    Ok(()) => {
                        confirmed_files.push(file.clone());
                        uploaded_files.push(file);
                    }
                    Err(error) => failed_files.push(FailedDocument {
                        relative_path: file.relative_path,
                        error: error.to_string(),
                    }),
                }
            }
        }

        if !confirmed_files.is_empty() {
            confirm_batch_upload(
                &client,
                &credentials,
                &request.workspace_external_id,
                &presign.batch_id,
                &confirmed_files,
            )
            .await?;
        }
    }

    Ok(SyncDocumentsResponse {
        success: failed_files.is_empty(),
        uploaded_files,
        failed_files,
        duplicate_count,
        batch_ids,
    })
}

async fn inspect_file(
    path: &Path,
    root: &Path,
    request: &ScanDocumentsRequest,
    max_file_size: u64,
    excluded: &[String],
) -> anyhow::Result<Option<DocumentFile>> {
    let link_metadata = tokio::fs::symlink_metadata(path).await?;
    if is_link_or_reparse_point(&link_metadata) || !link_metadata.is_file() {
        anyhow::bail!("Document is not an authorized regular file");
    }
    let canonical_path = tokio::fs::canonicalize(path).await?;
    if !canonical_path.starts_with(root) {
        anyhow::bail!("Document resolved outside the selected root");
    }
    let metadata = tokio::fs::metadata(&canonical_path).await?;
    let size = metadata.len();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");

    if file_name.starts_with('.') || is_hidden_metadata(&link_metadata) {
        return Ok(None);
    }
    if size == 0 {
        return Ok(None);
    }
    if size > max_file_size {
        return Ok(None);
    }

    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{}", value.to_lowercase()));
    if extension
        .as_ref()
        .is_some_and(|extension| excluded.iter().any(|item| item == extension))
    {
        return Ok(None);
    }

    let modified = metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?;
    let relative_path = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let integrity_hash = hash_file(&canonical_path).await?;
    let hash = request.use_hash.then(|| integrity_hash.clone());

    Ok(Some(DocumentFile {
        scan_id: uuid::Uuid::new_v4().to_string(),
        relative_path,
        size,
        mtime: modified.as_millis() as i64,
        mime_type: mime_type(extension.as_deref()),
        extension,
        content_hash: hash.clone(),
        hash,
        server_file_key: None,
        multipart_info: None,
        native_path: canonical_path,
        integrity_hash,
    }))
}

fn scan_registry() -> &'static Mutex<ScanRegistry> {
    SCAN_REGISTRY.get_or_init(|| Mutex::new(ScanRegistry::default()))
}

fn resolve_scanned_files(
    session_id: &str,
    requested_files: &[DocumentFile],
) -> anyhow::Result<Vec<DocumentFile>> {
    let mut registry = scan_registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("Document scan registry is unavailable"))?;
    let now = Instant::now();
    registry
        .sessions
        .retain(|_, session| now.duration_since(session.created_at) <= SCAN_SESSION_TTL);
    if session_id.is_empty() || !registry.sessions.contains_key(session_id) {
        anyhow::bail!("Document scan session is missing or expired; scan the files again");
    }

    let session = registry
        .sessions
        .get_mut(session_id)
        .ok_or_else(|| anyhow::anyhow!("Document scan session is missing or expired"))?;
    let mut seen = HashSet::new();
    for requested in requested_files {
        if requested.scan_id.is_empty() || !seen.insert(requested.scan_id.clone()) {
            anyhow::bail!("Document scan ID is missing or duplicated");
        }
        if !session.files.contains_key(&requested.scan_id) {
            anyhow::bail!("Document scan ID is invalid, expired, or already used");
        }
    }
    let resolved = requested_files
        .iter()
        .map(|requested| {
            session
                .files
                .remove(&requested.scan_id)
                .ok_or_else(|| anyhow::anyhow!("Document scan ID is invalid or expired"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    if session.files.is_empty() {
        registry.sessions.remove(session_id);
    }
    Ok(resolved)
}

fn push_unique_file(
    files: &mut Vec<DocumentFile>,
    relative_paths: &mut HashSet<String>,
    file: DocumentFile,
    skipped: &mut Vec<SkippedFile>,
) {
    if files.len() >= MAX_SCANNED_FILES {
        return;
    }
    if !relative_paths.insert(file.relative_path.clone()) {
        skipped.push(SkippedFile {
            path: file.relative_path,
            reason: "Duplicate relative path from another selected root".to_string(),
        });
        return;
    }
    files.push(file);
}

fn relative_display(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_hidden_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.starts_with('.'))
}

#[cfg(windows)]
fn is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn is_hidden_metadata(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0002;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0004;
    metadata.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
}

#[cfg(target_os = "macos")]
fn is_hidden_metadata(metadata: &std::fs::Metadata) -> bool {
    use std::os::darwin::fs::MetadataExt;
    const UF_HIDDEN: u32 = 0x0000_8000;
    metadata.st_flags() & UF_HIDDEN != 0
}

#[cfg(all(not(windows), not(target_os = "macos")))]
fn is_hidden_metadata(_metadata: &std::fs::Metadata) -> bool {
    false
}

async fn hash_file(path: &Path) -> anyhow::Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];

    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hex_upper(&hasher.finalize()))
}

async fn request_presigned_urls(
    client: &reqwest::Client,
    credentials: &AxalCredentials,
    workspace_external_id: &str,
    files: &[DocumentFile],
) -> anyhow::Result<PresignResponse> {
    let payload = serde_json::json!({
        "workspaceExternalId": workspace_external_id,
        "files": files.iter().map(|file| serde_json::json!({
            "relativePath": file.relative_path,
            "size": file.size,
            "mtime": file.mtime,
            "hash": file.hash,
            "mimeType": file.mime_type,
            "contentHash": file.content_hash.as_ref().or(file.hash.as_ref()),
            "tags": [],
        })).collect::<Vec<_>>(),
        "idempotencyKey": uuid::Uuid::new_v4().to_string(),
        "timestamp": chrono::Utc::now().timestamp_millis(),
    });

    let response = client
        .post(endpoint(
            credentials.base_url.as_deref(),
            "/integrations/sync/documents/presigned-urls",
        )?)
        .headers(auth_headers(credentials)?)
        .json(&payload)
        .send()
        .await?;

    parse_json_response(response).await
}

async fn upload_single_file(
    client: &reqwest::Client,
    url: &str,
    file: &DocumentFile,
) -> anyhow::Result<()> {
    let url = validate_upload_url(url)?;
    let disk_file = prepare_upload_snapshot(file).await?;
    let body = reqwest::Body::wrap_stream(ReaderStream::new(disk_file));
    let response = client
        .put(url)
        .header("content-type", &file.mime_type)
        .header("content-length", file.size)
        .body(body)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Upload failed with {}", response.status());
    }

    Ok(())
}

async fn upload_multipart_file(
    client: &reqwest::Client,
    document: &PresignedDocument,
    file: &mut DocumentFile,
) -> anyhow::Result<()> {
    let upload_id = document
        .upload_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("AXAL did not return a multipart upload ID"))?;
    if document.parts.is_empty() {
        anyhow::bail!("AXAL did not return multipart upload URLs");
    }

    let mut expected_part_number = 1_u32;
    for part in &document.parts {
        if part.part_number != expected_part_number || part.expected_size == 0 {
            anyhow::bail!("AXAL returned invalid multipart part metadata");
        }
        expected_part_number = expected_part_number
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("AXAL returned too many multipart parts"))?;
    }

    let expected_size = document.parts.iter().try_fold(0_u64, |total, part| {
        total
            .checked_add(part.expected_size)
            .ok_or_else(|| anyhow::anyhow!("Multipart upload size overflow"))
    })?;
    if expected_size != file.size {
        anyhow::bail!(
            "Multipart upload size mismatch: AXAL expected {expected_size} bytes for {} bytes",
            file.size
        );
    }

    let snapshot = prepare_upload_snapshot(file).await?;
    let mut completed = Vec::with_capacity(document.parts.len());
    let mut offset = 0_u64;
    for part in &document.parts {
        let completed_part = upload_file_part(client, &snapshot, part, offset).await?;
        offset = offset
            .checked_add(part.expected_size)
            .ok_or_else(|| anyhow::anyhow!("Multipart upload offset overflow"))?;
        completed.push(completed_part);
    }

    file.multipart_info = Some(MultipartUploadInfo {
        upload_id,
        parts: completed,
    });
    Ok(())
}

async fn upload_file_part(
    client: &reqwest::Client,
    snapshot: &tokio::fs::File,
    part: &PresignedPart,
    offset: u64,
) -> anyhow::Result<CompletedMultipartPart> {
    let url = validate_upload_url(&part.url)?;
    let mut disk_file = snapshot.try_clone().await?;
    disk_file.seek(SeekFrom::Start(offset)).await?;
    let body = reqwest::Body::wrap_stream(ReaderStream::new(disk_file.take(part.expected_size)));
    let response = client
        .put(url)
        .header("content-type", "application/octet-stream")
        .header("content-length", part.expected_size)
        .header("cache-control", "no-cache")
        .body(body)
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Multipart part {} upload failed with {}",
            part.part_number,
            response.status()
        );
    }

    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(clean_etag)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "AXAL storage did not return an ETag for part {}",
                part.part_number
            )
        })?;

    Ok(CompletedMultipartPart {
        part_number: part.part_number,
        etag,
        size: part.expected_size,
        bytes_read: part.expected_size,
    })
}

async fn confirm_batch_upload(
    client: &reqwest::Client,
    credentials: &AxalCredentials,
    workspace_external_id: &str,
    batch_id: &str,
    files: &[DocumentFile],
) -> anyhow::Result<()> {
    let payload = serde_json::json!({
        "workspaceExternalId": workspace_external_id,
        "batchId": batch_id,
        "files": files.iter().map(|file| {
            let file_name = Path::new(&file.relative_path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            let folder_path = Path::new(&file.relative_path)
                .parent()
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            serde_json::json!({
                "filename": file_name,
                "size": file.size,
                "mimeType": file.mime_type,
                "folderPath": folder_path,
                "fileKey": file.server_file_key,
                "status": "uploaded",
                "mtime": file.mtime,
                "contentHash": file.content_hash.as_ref().or(file.hash.as_ref()),
                "multipartInfo": file.multipart_info.as_ref().map(|info| serde_json::json!({
                    "uploadId": info.upload_id,
                    "parts": info.parts.iter().map(|part| serde_json::json!({
                        "PartNumber": part.part_number,
                        "ETag": part.etag,
                    })).collect::<Vec<_>>(),
                })),
                "tags": [],
            })
        }).collect::<Vec<_>>(),
        "idempotencyKey": uuid::Uuid::new_v4().to_string(),
        "timestamp": chrono::Utc::now().timestamp_millis(),
    });

    let response = client
        .post(endpoint(
            credentials.base_url.as_deref(),
            "/integrations/sync/documents/bulk-create",
        )?)
        .headers(auth_headers(credentials)?)
        .json(&payload)
        .send()
        .await?;

    let _: serde_json::Value = parse_json_response(response).await?;
    Ok(())
}

async fn parse_json_response<T>(response: reqwest::Response) -> anyhow::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let status = response.status();
    let text = response_text_limited(response, 1024 * 1024).await?;

    if !status.is_success() {
        let error = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(|error| error.as_str())
                    .map(str::to_string)
            })
            .map(|message| sanitized_server_message(&message))
            .unwrap_or_else(|| format!("AXAL server returned {status}"));
        anyhow::bail!(error);
    }

    serde_json::from_str::<T>(&text)
        .map_err(|error| anyhow::anyhow!("Invalid AXAL response: {error}"))
}

fn normalized_exclusions(excluded_extensions: Option<Vec<String>>) -> Vec<String> {
    let mut exclusions = default_exclusions();
    exclusions.extend(excluded_extensions.unwrap_or_default());
    exclusions
        .into_iter()
        .map(|extension| {
            let extension = extension.trim().to_lowercase();
            if extension.starts_with('.') {
                extension
            } else {
                format!(".{extension}")
            }
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

fn default_exclusions() -> Vec<String> {
    [
        ".exe",
        ".dll",
        ".msi",
        ".app",
        ".dmg",
        ".pkg",
        ".deb",
        ".rpm",
        ".sh",
        ".bat",
        ".cmd",
        ".ps1",
        ".vbs",
        ".js",
        ".jar",
        ".com",
        ".scr",
        ".tmp",
        ".temp",
        ".log",
        ".cache",
        ".pyc",
        ".class",
        ".o",
        ".obj",
        ".so",
        ".dylib",
        ".crdownload",
        ".part",
    ]
    .iter()
    .map(|value| value.to_string())
    .collect()
}

fn mime_type(extension: Option<&str>) -> String {
    match extension.unwrap_or_default() {
        ".pdf" => "application/pdf",
        ".doc" => "application/msword",
        ".docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ".xls" => "application/vnd.ms-excel",
        ".xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ".csv" => "text/csv",
        ".txt" => "text/plain",
        ".png" => "image/png",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".zip" => "application/zip",
        ".json" => "application/json",
        ".xml" => "application/xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn clean_etag(value: &str) -> String {
    value.trim_matches('"').trim_matches('\'').to_string()
}

fn validate_presign_mapping(
    requested: &[DocumentFile],
    response: &PresignResponse,
) -> anyhow::Result<()> {
    let expected = requested
        .iter()
        .map(|file| file.relative_path.as_str())
        .collect::<HashSet<_>>();
    if expected.len() != requested.len() {
        anyhow::bail!("Document batch contains duplicate relative paths");
    }

    let mut observed = HashSet::new();
    for relative_path in response
        .documents
        .iter()
        .map(|document| document.relative_path.as_str())
        .chain(
            response
                .duplicates
                .iter()
                .map(|duplicate| duplicate.relative_path.as_str()),
        )
    {
        if !expected.contains(relative_path) {
            anyhow::bail!("AXAL returned an unknown document in the presign response");
        }
        if !observed.insert(relative_path) {
            anyhow::bail!("AXAL returned a duplicate document in the presign response");
        }
    }
    if observed != expected {
        anyhow::bail!("AXAL presign response did not account for every requested document");
    }
    Ok(())
}

fn validate_upload_url(value: &str) -> anyhow::Result<reqwest::Url> {
    let configured_origins = std::env::var("BRIDGE_DOCUMENT_UPLOAD_ALLOWED_ORIGINS")
        .map(Some)
        .or_else(|error| match error {
            std::env::VarError::NotPresent => Ok(None),
            std::env::VarError::NotUnicode(_) => Err(anyhow::anyhow!(
                "BRIDGE_DOCUMENT_UPLOAD_ALLOWED_ORIGINS is not valid Unicode"
            )),
        })?;
    validate_upload_url_with_allowed_origins(value, configured_origins.as_deref())
}

fn validate_upload_url_with_allowed_origins(
    value: &str,
    configured_origins: Option<&str>,
) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("AXAL returned an invalid upload URL"))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        anyhow::bail!("AXAL upload URLs must use HTTPS and include a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("AXAL upload URLs must not contain credentials");
    }
    if url.fragment().is_some() {
        anyhow::bail!("AXAL upload URLs must not contain fragments");
    }

    let candidate = url.origin().ascii_serialization();
    let default_origin = reqwest::Url::parse("https://complyeaze.com")?
        .origin()
        .ascii_serialization();
    if candidate == default_origin {
        return Ok(url);
    }
    for raw_origin in configured_origins.unwrap_or_default().split(',') {
        let raw_origin = raw_origin.trim();
        if raw_origin.is_empty() {
            continue;
        }
        let allowed = reqwest::Url::parse(raw_origin).map_err(|_| {
            anyhow::anyhow!(
                "BRIDGE_DOCUMENT_UPLOAD_ALLOWED_ORIGINS must contain valid HTTPS origins"
            )
        })?;
        if allowed.scheme() != "https"
            || allowed.host_str().is_none()
            || !allowed.username().is_empty()
            || allowed.password().is_some()
            || allowed.query().is_some()
            || allowed.fragment().is_some()
            || allowed.path() != "/"
        {
            anyhow::bail!(
                "BRIDGE_DOCUMENT_UPLOAD_ALLOWED_ORIGINS must contain exact HTTPS origins without paths"
            );
        }
        if candidate == allowed.origin().ascii_serialization() {
            return Ok(url);
        }
    }
    anyhow::bail!(
        "Document upload origin is not trusted; configure BRIDGE_DOCUMENT_UPLOAD_ALLOWED_ORIGINS before launch"
    )
}

async fn prepare_upload_snapshot(file: &DocumentFile) -> anyhow::Result<tokio::fs::File> {
    validate_document_file(file).await?;
    let mut source = tokio::fs::File::open(&file.native_path).await?;
    let temporary = tempfile::tempfile()?;
    let mut snapshot = tokio::fs::File::from_std(temporary);
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; 1024 * 1024];

    loop {
        let read = source.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| anyhow::anyhow!("Document size overflow"))?;
        if copied > file.size {
            anyhow::bail!("Document changed after scanning; scan the file again");
        }
        hasher.update(&buffer[..read]);
        snapshot.write_all(&buffer[..read]).await?;
    }

    if copied != file.size || hex_upper(&hasher.finalize()) != file.integrity_hash {
        anyhow::bail!("Document changed after scanning; scan the file again");
    }
    snapshot.flush().await?;
    snapshot.seek(SeekFrom::Start(0)).await?;
    Ok(snapshot)
}

async fn validate_document_file(file: &DocumentFile) -> anyhow::Result<()> {
    if file.relative_path.is_empty()
        || file.relative_path.starts_with('/')
        || file.relative_path.starts_with('\\')
        || file
            .relative_path
            .split(['/', '\\'])
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        anyhow::bail!("Document relative path is invalid");
    }

    if file.native_path.as_os_str().is_empty() || file.integrity_hash.is_empty() {
        anyhow::bail!("Document scan authorization is missing; scan the file again");
    }
    let link_metadata = tokio::fs::symlink_metadata(&file.native_path).await?;
    if is_link_or_reparse_point(&link_metadata) || !link_metadata.is_file() {
        anyhow::bail!("Document path is not a regular file");
    }
    let canonical = tokio::fs::canonicalize(&file.native_path).await?;
    if canonical != file.native_path {
        anyhow::bail!("Document path changed after scanning; scan the file again");
    }
    let metadata = tokio::fs::metadata(&canonical).await?;
    if metadata.len() != file.size {
        anyhow::bail!("Document size changed after scanning; scan the file again");
    }
    let modified = metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?;
    if modified.as_millis() as i64 != file.mtime {
        anyhow::bail!("Document modification time changed after scanning; scan the file again");
    }
    Ok(())
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{
        authorize_selected_paths, clean_etag, prepare_upload_snapshot, resolve_scanned_files,
        scan_documents, validate_presign_mapping, validate_upload_url,
        validate_upload_url_with_allowed_origins, PresignResponse, ScanDocumentsRequest,
    };
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    #[test]
    fn parses_multipart_presign_response() {
        let json = r#"
{
  "batchId": "batch-1",
  "documents": [
    {
      "relativePath": "invoices/a.pdf",
      "fileKey": "workspaces/ws/documents/a.pdf",
      "isMultipart": true,
      "uploadId": "upload-1",
      "parts": [
        { "partNumber": 1, "url": "https://upload.test/1", "expectedSize": 5242880 },
        { "partNumber": 2, "url": "https://upload.test/2", "expectedSize": 128 }
      ]
    }
  ],
  "duplicates": []
}
"#;

        let response =
            serde_json::from_str::<PresignResponse>(json).expect("presign response should parse");
        let document = &response.documents[0];

        assert_eq!(response.batch_id, "batch-1");
        assert!(document.is_multipart);
        assert_eq!(document.upload_id.as_deref(), Some("upload-1"));
        assert_eq!(document.parts.len(), 2);
        assert_eq!(document.parts[0].part_number, 1);
        assert_eq!(document.parts[0].expected_size, 5_242_880);
    }

    #[test]
    fn cleans_quoted_etags() {
        assert_eq!(clean_etag("\"abc123\""), "abc123");
        assert_eq!(clean_etag("'abc123'"), "abc123");
    }

    #[test]
    fn upload_urls_require_https_without_embedded_credentials() {
        assert!(validate_upload_url_with_allowed_origins(
            "https://storage.example/file?signature=ok",
            Some("https://storage.example")
        )
        .is_ok());
        assert!(validate_upload_url("http://storage.example/file").is_err());
        assert!(validate_upload_url("https://user:pass@storage.example/file").is_err());
        assert!(validate_upload_url("https://storage.example/file").is_err());
    }

    #[tokio::test]
    async fn native_selection_and_opaque_scan_ids_authorize_sync() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let selected = directory.path().join("selected.txt");
        std::fs::write(&selected, b"authorized content").expect("write selected file");
        let selections =
            authorize_selected_paths(vec![selected.clone()]).expect("authorize native selection");
        let selection_json = serde_json::to_string(&selections).expect("serialize selections");
        assert!(!selection_json.contains(&directory.path().display().to_string()));
        let response = scan_documents(ScanDocumentsRequest {
            selection_ids: vec![
                selections[0].selection_id.clone(),
                uuid::Uuid::new_v4().to_string(),
            ],
            use_hash: true,
            max_file_size: None,
            excluded_extensions: None,
            exclude_hidden_files: false,
            exclude_zero_byte_files: false,
        })
        .await
        .expect("scan selected file");

        assert_eq!(response.files.len(), 1);
        let response_json = serde_json::to_string(&response).expect("serialize scan response");
        assert!(!response_json.contains(&directory.path().display().to_string()));
        assert!(response
            .skipped
            .iter()
            .any(|skipped| skipped.reason.contains("invalid, expired")));
        let serialized = serde_json::to_string(&response.files[0]).expect("serialize file");
        assert!(serialized.contains("scanId"));
        assert!(!serialized.contains("fullPath"));
        assert!(!serialized.contains(&selected.display().to_string()));

        let unknown: PresignResponse = serde_json::from_value(serde_json::json!({
            "batchId": "batch",
            "documents": [{
                "relativePath": "unknown.txt",
                "fileKey": "key",
                "url": "https://storage.example/file"
            }],
            "duplicates": []
        }))
        .expect("parse unknown presign");
        assert!(validate_presign_mapping(&response.files, &unknown).is_err());

        let duplicate: PresignResponse = serde_json::from_value(serde_json::json!({
            "batchId": "batch",
            "documents": [
                {
                    "relativePath": response.files[0].relative_path.clone(),
                    "fileKey": "key-1",
                    "url": "https://storage.example/1"
                },
                {
                    "relativePath": response.files[0].relative_path.clone(),
                    "fileKey": "key-2",
                    "url": "https://storage.example/2"
                }
            ],
            "duplicates": []
        }))
        .expect("parse duplicate presign");
        assert!(validate_presign_mapping(&response.files, &duplicate).is_err());

        let resolved = resolve_scanned_files(&response.scan_session_id, &response.files)
            .expect("resolve authorized scan ID");
        assert_eq!(
            resolved[0].native_path,
            std::fs::canonicalize(&selected).unwrap()
        );
        std::fs::write(&selected, vec![b'X'; "authorized content".len()])
            .expect("replace with same-size content");
        assert!(prepare_upload_snapshot(&resolved[0]).await.is_err());
        assert!(resolve_scanned_files(&response.scan_session_id, &response.files).is_err());

        let mut forged = response.files[0].clone();
        forged.scan_id = uuid::Uuid::new_v4().to_string();
        assert!(resolve_scanned_files(&response.scan_session_id, &[forged]).is_err());
        assert!(resolve_scanned_files("expired", &response.files).is_err());
    }

    #[tokio::test]
    async fn upload_snapshot_is_immutable_after_source_changes() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let selected = directory.path().join("snapshot.txt");
        std::fs::write(&selected, b"original").expect("write source");
        let selections =
            authorize_selected_paths(vec![selected.clone()]).expect("authorize selection");
        let response = scan_documents(ScanDocumentsRequest {
            selection_ids: vec![selections[0].selection_id.clone()],
            use_hash: true,
            max_file_size: None,
            excluded_extensions: None,
            exclude_hidden_files: true,
            exclude_zero_byte_files: true,
        })
        .await
        .expect("scan source");
        let files = resolve_scanned_files(&response.scan_session_id, &response.files)
            .expect("resolve source");
        let mut snapshot = prepare_upload_snapshot(&files[0]).await.expect("snapshot");
        std::fs::write(&selected, b"tampered").expect("replace source");
        snapshot.rewind().await.expect("rewind snapshot");
        let mut content = Vec::new();
        snapshot
            .read_to_end(&mut content)
            .await
            .expect("read snapshot");
        assert_eq!(content, b"original");
    }

    #[tokio::test]
    async fn independent_scan_sessions_do_not_invalidate_each_other() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let first = directory.path().join("first.txt");
        let second = directory.path().join("second.txt");
        std::fs::write(&first, b"first").expect("write first");
        std::fs::write(&second, b"second").expect("write second");
        let selections = authorize_selected_paths(vec![first, second]).expect("authorize files");
        let request = |selection_id: String| ScanDocumentsRequest {
            selection_ids: vec![selection_id],
            use_hash: true,
            max_file_size: None,
            excluded_extensions: None,
            exclude_hidden_files: true,
            exclude_zero_byte_files: true,
        };
        let first_scan = scan_documents(request(selections[0].selection_id.clone()))
            .await
            .expect("first scan");
        let second_scan = scan_documents(request(selections[1].selection_id.clone()))
            .await
            .expect("second scan");

        assert!(resolve_scanned_files(&first_scan.scan_session_id, &first_scan.files).is_ok());
        assert!(resolve_scanned_files(&second_scan.scan_session_id, &second_scan.files).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn directory_scan_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let selected = tempfile::tempdir().expect("selected directory");
        let outside = tempfile::tempdir().expect("outside directory");
        std::fs::write(outside.path().join("private.txt"), b"private").expect("outside file");
        symlink(outside.path(), selected.path().join("escape")).expect("create symlink");
        let selections = authorize_selected_paths(vec![selected.path().to_path_buf()])
            .expect("authorize directory");
        let response = scan_documents(ScanDocumentsRequest {
            selection_ids: vec![selections[0].selection_id.clone()],
            use_hash: true,
            max_file_size: None,
            excluded_extensions: None,
            exclude_hidden_files: true,
            exclude_zero_byte_files: true,
        })
        .await
        .expect("scan directory");
        assert!(response.files.is_empty());
        assert!(response.skipped.iter().any(|skipped| skipped
            .reason
            .contains("Symbolic links and filesystem reparse points")));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_hidden_flag_is_excluded() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let hidden = directory.path().join("hidden-without-dot.txt");
        std::fs::write(&hidden, b"hidden").expect("write hidden file");
        assert!(std::process::Command::new("chflags")
            .arg("hidden")
            .arg(&hidden)
            .status()
            .expect("run chflags")
            .success());
        let selections = authorize_selected_paths(vec![directory.path().to_path_buf()])
            .expect("authorize directory");
        let response = scan_documents(ScanDocumentsRequest {
            selection_ids: vec![selections[0].selection_id.clone()],
            use_hash: true,
            max_file_size: None,
            excluded_extensions: None,
            exclude_hidden_files: true,
            exclude_zero_byte_files: true,
        })
        .await
        .expect("scan directory");
        assert!(response.files.is_empty());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_junction_escape_is_rejected() {
        let selected = tempfile::tempdir().expect("selected directory");
        let outside = tempfile::tempdir().expect("outside directory");
        std::fs::write(outside.path().join("private.txt"), b"private").expect("outside file");
        let junction = selected.path().join("escape");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(outside.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("create junction");
        assert!(status.success(), "Windows junction creation failed");

        let selections = authorize_selected_paths(vec![selected.path().to_path_buf()])
            .expect("authorize directory");
        let response = scan_documents(ScanDocumentsRequest {
            selection_ids: vec![selections[0].selection_id.clone()],
            use_hash: true,
            max_file_size: None,
            excluded_extensions: None,
            exclude_hidden_files: true,
            exclude_zero_byte_files: true,
        })
        .await
        .expect("scan directory");
        assert!(response.files.is_empty());
        assert!(response.skipped.iter().any(|skipped| skipped
            .reason
            .contains("Symbolic links and filesystem reparse points")));
        std::fs::remove_dir(&junction).expect("remove junction");
    }
}
