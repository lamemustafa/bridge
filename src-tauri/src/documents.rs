use std::collections::VecDeque;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::axal::{
    auth_headers, endpoint, response_text_limited, sanitized_server_message, validate_credentials,
    validate_identifier, AxalCredentials,
};

const DEFAULT_MAX_FILE_SIZE: u64 = 1_500 * 1024 * 1024;
const DEFAULT_BATCH_SIZE: usize = 20;

#[derive(Debug, Clone, Deserialize)]
pub struct ScanDocumentsRequest {
    pub paths: Vec<String>,
    #[serde(default = "default_true")]
    pub use_hash: bool,
    pub max_file_size: Option<u64>,
    pub excluded_extensions: Option<Vec<String>>,
    #[serde(default = "default_true")]
    pub exclude_hidden_files: bool,
    #[serde(default = "default_true")]
    pub exclude_zero_byte_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentFile {
    pub full_path: String,
    pub relative_path: String,
    pub size: u64,
    pub mtime: i64,
    pub extension: Option<String>,
    pub mime_type: String,
    pub hash: Option<String>,
    pub content_hash: Option<String>,
    pub server_file_key: Option<String>,
    pub multipart_info: Option<MultipartUploadInfo>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanDocumentsResponse {
    pub files: Vec<DocumentFile>,
    pub total_size: u64,
    pub skipped: Vec<SkippedFile>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDocumentsRequest {
    pub credentials: AxalCredentials,
    pub workspace_external_id: String,
    pub files: Vec<DocumentFile>,
    pub max_files_per_batch: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
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
    duplicates: Vec<serde_json::Value>,
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
    if request.paths.is_empty() {
        anyhow::bail!("Provide at least one file or folder path");
    }

    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let excluded = normalized_exclusions(request.excluded_extensions.clone());
    let max_file_size = request.max_file_size.unwrap_or(DEFAULT_MAX_FILE_SIZE);

    for raw_path in &request.paths {
        let root = PathBuf::from(raw_path.trim());
        if !root.exists() {
            skipped.push(SkippedFile {
                path: root.display().to_string(),
                reason: "Path does not exist".to_string(),
            });
            continue;
        }

        if root.is_file() {
            match inspect_file(
                &root,
                root.parent().unwrap_or_else(|| Path::new("")),
                &request,
                max_file_size,
                &excluded,
            )
            .await
            {
                Ok(Some(file)) => files.push(file),
                Ok(None) => {}
                Err(error) => skipped.push(SkippedFile {
                    path: root.display().to_string(),
                    reason: error.to_string(),
                }),
            }
            continue;
        }

        let mut queue = VecDeque::from([root.clone()]);
        while let Some(path) = queue.pop_front() {
            let entries = match std::fs::read_dir(&path) {
                Ok(entries) => entries,
                Err(error) => {
                    skipped.push(SkippedFile {
                        path: path.display().to_string(),
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
                            path: path.display().to_string(),
                            reason: error.to_string(),
                        });
                        continue;
                    }
                };
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    queue.push_back(entry_path);
                } else if entry_path.is_file() {
                    match inspect_file(&entry_path, &root, &request, max_file_size, &excluded).await
                    {
                        Ok(Some(file)) => files.push(file),
                        Ok(None) => {}
                        Err(error) => skipped.push(SkippedFile {
                            path: entry_path.display().to_string(),
                            reason: error.to_string(),
                        }),
                    }
                }
            }
        }
    }

    let total_size = files.iter().map(|file| file.size).sum();
    Ok(ScanDocumentsResponse {
        files,
        total_size,
        skipped,
    })
}

pub async fn sync_documents(
    request: SyncDocumentsRequest,
) -> anyhow::Result<SyncDocumentsResponse> {
    validate_credentials(&request.credentials)?;
    validate_identifier("workspace external ID", &request.workspace_external_id)?;
    if request.files.is_empty() {
        anyhow::bail!("No files selected for document sync");
    }

    for file in &request.files {
        validate_document_file(file).await?;
    }

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

    for batch in request.files.chunks(batch_size) {
        let presign = request_presigned_urls(
            &client,
            &request.credentials,
            &request.workspace_external_id,
            batch,
        )
        .await?;
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
                &request.credentials,
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
    let metadata = tokio::fs::metadata(path).await?;
    let size = metadata.len();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");

    if request.exclude_hidden_files && file_name.starts_with('.') {
        return Ok(None);
    }
    if request.exclude_zero_byte_files && size == 0 {
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
    let hash = if request.use_hash {
        Some(hash_file(path).await?)
    } else {
        None
    };

    Ok(Some(DocumentFile {
        full_path: path.display().to_string(),
        relative_path,
        size,
        mtime: modified.as_millis() as i64,
        mime_type: mime_type(extension.as_deref()),
        extension,
        content_hash: hash.clone(),
        hash,
        server_file_key: None,
        multipart_info: None,
    }))
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
    let disk_file = tokio::fs::File::open(&file.full_path).await?;
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

    let expected_size: u64 = document.parts.iter().map(|part| part.expected_size).sum();
    if expected_size != file.size {
        anyhow::bail!(
            "Multipart upload size mismatch: AXAL expected {expected_size} bytes for {} bytes",
            file.size
        );
    }

    let mut completed = Vec::with_capacity(document.parts.len());
    let mut offset = 0_u64;
    for part in &document.parts {
        let completed_part = upload_file_part(client, file, part, offset).await?;
        offset += part.expected_size;
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
    file: &DocumentFile,
    part: &PresignedPart,
    offset: u64,
) -> anyhow::Result<CompletedMultipartPart> {
    let url = validate_upload_url(&part.url)?;
    let mut disk_file = tokio::fs::File::open(&file.full_path).await?;
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
    excluded_extensions
        .unwrap_or_else(default_exclusions)
        .into_iter()
        .map(|extension| {
            let extension = extension.trim().to_lowercase();
            if extension.starts_with('.') {
                extension
            } else {
                format!(".{extension}")
            }
        })
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

fn validate_upload_url(value: &str) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("AXAL returned an invalid upload URL"))?;
    if url.scheme() != "https" || url.host_str().is_none() {
        anyhow::bail!("AXAL upload URLs must use HTTPS and include a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("AXAL upload URLs must not contain credentials");
    }
    Ok(url)
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

    let metadata = tokio::fs::metadata(&file.full_path).await?;
    if !metadata.is_file() {
        anyhow::bail!("Document path is not a regular file");
    }
    if metadata.len() != file.size {
        anyhow::bail!("Document size changed after scanning; scan the file again");
    }
    Ok(())
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{clean_etag, validate_upload_url, PresignResponse};

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
        assert!(validate_upload_url("https://storage.example/file?signature=ok").is_ok());
        assert!(validate_upload_url("http://storage.example/file").is_err());
        assert!(validate_upload_url("https://user:pass@storage.example/file").is_err());
    }
}
