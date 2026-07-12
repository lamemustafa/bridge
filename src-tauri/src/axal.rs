use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "https://complyeaze.com";
const API_BASE_PATH: &str = "/axal/api/v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntegrationKind {
    Tally,
    Documents,
    Dsc,
}

impl IntegrationKind {
    fn as_header_value(self) -> &'static str {
        match self {
            Self::Tally => "TALLY_PRIME",
            Self::Documents => "DOCUMENT_SYNC",
            Self::Dsc => "DSC_MANAGEMENT",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AxalCredentials {
    pub api_key: String,
    pub api_id: String,
    pub integration: IntegrationKind,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResponse {
    pub valid: bool,
    pub status: Option<String>,
    #[serde(rename = "lastSynced")]
    pub last_synced: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "billingPlan")]
    pub billing_plan: String,
    #[serde(rename = "storageUsed")]
    pub storage_used: u64,
    #[serde(rename = "storageLimit")]
    pub storage_limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatusResponse {
    pub connected: bool,
    pub status: String,
    #[serde(rename = "lastSyncedAt")]
    pub last_synced_at: Option<String>,
    pub workspace: WorkspaceInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateMetadata {
    pub organization: Option<String>,
    pub issuer: Option<String>,
    pub fingerprint: Option<String>,
    #[serde(rename = "tokenType")]
    pub token_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateData {
    #[serde(rename = "holderName")]
    pub holder_name: String,
    pub provider: String,
    #[serde(rename = "serialNumber")]
    pub serial_number: String,
    #[serde(rename = "tokenType")]
    pub token_type: String,
    #[serde(rename = "class")]
    pub certificate_class: String,
    pub purpose: String,
    #[serde(rename = "issueDate")]
    pub issue_date: String,
    #[serde(rename = "expirationDate")]
    pub expiration_date: String,
    #[serde(rename = "clientName")]
    pub client_name: String,
    pub metadata: CertificateMetadata,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DscSyncRequest {
    pub credentials: AxalCredentials,
    #[serde(rename = "workspaceExternalId")]
    pub workspace_external_id: String,
    pub certificates: Vec<CertificateData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DscSyncResults {
    pub created: u64,
    pub updated: u64,
    pub skipped: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DscSyncResponse {
    pub success: bool,
    pub message: String,
    pub results: Option<DscSyncResults>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorResponse {
    error: Option<String>,
    code: Option<String>,
}

pub async fn validate_api_key(credentials: AxalCredentials) -> anyhow::Result<ValidationResponse> {
    validate_credentials(&credentials)?;
    let client = api_client()?;
    let response = client
        .post(endpoint(
            credentials.base_url.as_deref(),
            "/integrations/validate-key",
        )?)
        .headers(auth_headers(&credentials)?)
        .json(&serde_json::json!({}))
        .send()
        .await?;

    parse_response::<ValidationResponse>(response).await
}

pub async fn check_connection_status(
    credentials: AxalCredentials,
) -> anyhow::Result<ConnectionStatusResponse> {
    validate_credentials(&credentials)?;
    let client = api_client()?;
    let response = client
        .get(endpoint(
            credentials.base_url.as_deref(),
            "/integrations/connection-status",
        )?)
        .headers(auth_headers(&credentials)?)
        .send()
        .await?;

    parse_response::<ConnectionStatusResponse>(response).await
}

pub async fn sync_dsc_certificates(request: DscSyncRequest) -> anyhow::Result<DscSyncResponse> {
    validate_credentials(&request.credentials)?;
    if request.certificates.is_empty() {
        anyhow::bail!("No certificate data provided for sync");
    }
    validate_identifier("workspace external ID", &request.workspace_external_id)?;

    let client = api_client()?;
    let payload = serde_json::json!({
        "workspaceExternalId": request.workspace_external_id,
        "certificates": request.certificates,
        "syncTimestamp": chrono::Utc::now().to_rfc3339(),
    });
    let response = client
        .post(endpoint(
            request.credentials.base_url.as_deref(),
            "/integrations/sync/dsc",
        )?)
        .headers(auth_headers(&request.credentials)?)
        .json(&payload)
        .send()
        .await?;

    parse_response::<DscSyncResponse>(response).await
}

pub fn endpoint(base_url: Option<&str>, path: &str) -> anyhow::Result<reqwest::Url> {
    if !path.starts_with('/') || path.contains("..") || path.contains('?') || path.contains('#') {
        anyhow::bail!("Invalid AXAL API path");
    }

    let base_url = base_url
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .unwrap_or(DEFAULT_BASE_URL);
    let mut url = reqwest::Url::parse(base_url)
        .map_err(|_| anyhow::anyhow!("AXAL base URL must be a valid HTTPS URL"))?;

    if url.scheme() != "https" || url.host_str().is_none() {
        anyhow::bail!("AXAL base URL must use HTTPS and include a host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("AXAL base URL must not contain credentials");
    }
    if url.query().is_some() || url.fragment().is_some() {
        anyhow::bail!("AXAL base URL must not contain a query or fragment");
    }

    let base_path = url.path().trim_end_matches('/');
    url.set_path(&format!("{base_path}{API_BASE_PATH}{path}"));
    Ok(url)
}

pub fn auth_headers(credentials: &AxalCredentials) -> anyhow::Result<HeaderMap> {
    validate_credentials(credentials)?;
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", credentials.api_key.trim()))?,
    );
    headers.insert(
        "x-integration-type",
        HeaderValue::from_static(credentials.integration.as_header_value()),
    );
    headers.insert(
        "x-api-id",
        HeaderValue::from_str(credentials.api_id.trim())?,
    );
    Ok(headers)
}

pub fn api_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        // Never forward AXAL credentials or integration headers through redirects.
        .redirect(Policy::none())
        .build()?)
}

pub fn validate_credentials(credentials: &AxalCredentials) -> anyhow::Result<()> {
    validate_secret("API key", &credentials.api_key)?;
    validate_identifier("API ID", &credentials.api_id)?;
    let _ = endpoint(
        credentials.base_url.as_deref(),
        "/integrations/validate-key",
    )?;
    Ok(())
}

fn validate_secret(name: &str, value: &str) -> anyhow::Result<()> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{name} is required");
    }
    if value.len() > 4096 || value.chars().any(char::is_control) {
        anyhow::bail!("{name} is invalid");
    }
    Ok(())
}

pub fn validate_identifier(name: &str, value: &str) -> anyhow::Result<()> {
    let value = value.trim();
    if value.is_empty() {
        anyhow::bail!("{name} is required");
    }
    if value.len() > 256 || value.chars().any(char::is_control) {
        anyhow::bail!("{name} is invalid");
    }
    Ok(())
}

async fn parse_response<T>(response: reqwest::Response) -> anyhow::Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let status = response.status();
    let text = response_text_limited(response, 1024 * 1024).await?;

    if !status.is_success() {
        let api_error = serde_json::from_str::<ApiErrorResponse>(&text).ok();
        let message = api_error
            .and_then(|error| error.error.or(error.code))
            .map(|message| sanitized_server_message(&message))
            .unwrap_or_else(|| format!("AXAL server returned {status}"));
        anyhow::bail!(message);
    }

    if text.trim().is_empty() {
        anyhow::bail!("AXAL server returned an empty response");
    }

    serde_json::from_str::<T>(&text)
        .map_err(|error| anyhow::anyhow!("Invalid AXAL response: {error}"))
}

pub(crate) async fn response_text_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> anyhow::Result<String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        anyhow::bail!("AXAL response exceeded the {max_bytes}-byte limit");
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            anyhow::bail!("AXAL response exceeded the {max_bytes}-byte limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("AXAL returned invalid UTF-8"))
}

pub(crate) fn sanitized_server_message(message: &str) -> String {
    let sanitized: String = message
        .chars()
        .take(512)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if sanitized.trim().is_empty() {
        "AXAL server rejected the request".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::endpoint;

    #[test]
    fn endpoint_requires_https() {
        let error = endpoint(Some("http://example.com"), "/integrations/validate-key")
            .expect_err("plaintext endpoints must be rejected");
        assert!(error.to_string().contains("HTTPS"));
    }

    #[test]
    fn endpoint_rejects_embedded_credentials_and_url_metadata() {
        assert!(endpoint(Some("https://user:pass@example.com"), "/x").is_err());
        assert!(endpoint(Some("https://example.com?next=evil"), "/x").is_err());
        assert!(endpoint(Some("https://example.com#fragment"), "/x").is_err());
    }

    #[test]
    fn endpoint_preserves_an_optional_deployment_prefix() {
        let url = endpoint(
            Some("https://example.com/bridge/"),
            "/integrations/validate-key",
        )
        .expect("valid endpoint");
        assert_eq!(
            url.as_str(),
            "https://example.com/bridge/axal/api/v1/integrations/validate-key"
        );
    }

    #[test]
    fn endpoint_rejects_path_traversal() {
        assert!(endpoint(Some("https://example.com"), "/../admin").is_err());
    }
}
