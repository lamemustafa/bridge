use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use zeroize::Zeroize;

const DEFAULT_BASE_URL: &str = "https://complyeaze.com";
const API_BASE_PATH: &str = "/axal/api/v1";
const CREDENTIAL_SESSION_IDLE_TTL: Duration = Duration::from_secs(15 * 60);
const CREDENTIAL_SESSION_ABSOLUTE_TTL: Duration = Duration::from_secs(8 * 60 * 60);

struct CredentialSession {
    credentials: Arc<AxalCredentials>,
    bound_workspace: Option<String>,
    created_at: Instant,
    last_used_at: Instant,
}

static CREDENTIAL_SESSIONS: OnceLock<Mutex<HashMap<String, CredentialSession>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Deserialize)]
pub struct AxalCredentials {
    pub api_key: String,
    pub api_id: String,
    pub integration: IntegrationKind,
    pub base_url: Option<String>,
}

impl Drop for AxalCredentials {
    fn drop(&mut self) {
        self.api_key.zeroize();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResponse {
    pub valid: bool,
    pub status: Option<String>,
    #[serde(rename = "lastSynced")]
    pub last_synced: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AxalSessionResponse {
    pub credential_session_id: String,
    pub validation: ValidationResponse,
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

#[derive(Deserialize)]
pub struct DscSyncRequest {
    #[serde(rename = "credentialSessionId")]
    pub credential_session_id: String,
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

pub async fn establish_credential_session(
    credentials: AxalCredentials,
) -> anyhow::Result<AxalSessionResponse> {
    let credentials = Arc::new(credentials);
    let validation = validate_api_key(&credentials).await?;
    if !validation.valid {
        anyhow::bail!("AXAL rejected the supplied credentials");
    }
    let credential_session_id = uuid::Uuid::new_v4().to_string();
    let mut sessions = credential_sessions()
        .lock()
        .map_err(|_| anyhow::anyhow!("AXAL credential session storage is unavailable"))?;
    let now = Instant::now();
    purge_expired_credential_sessions(&mut sessions, now);
    if sessions.len() >= 32 {
        if let Some(oldest) = sessions
            .iter()
            .min_by_key(|(_, session)| session.created_at)
            .map(|(id, _)| id.clone())
        {
            sessions.remove(&oldest);
        }
    }
    sessions.insert(
        credential_session_id.clone(),
        CredentialSession {
            credentials,
            bound_workspace: None,
            created_at: now,
            last_used_at: now,
        },
    );
    Ok(AxalSessionResponse {
        credential_session_id,
        validation,
    })
}

pub async fn validate_api_key(credentials: &AxalCredentials) -> anyhow::Result<ValidationResponse> {
    validate_credentials(credentials)?;
    let client = api_client()?;
    let response = client
        .post(endpoint(
            credentials.base_url.as_deref(),
            "/integrations/validate-key",
        )?)
        .headers(auth_headers(credentials)?)
        .json(&serde_json::json!({}))
        .send()
        .await?;

    parse_response::<ValidationResponse>(response).await
}

pub async fn check_connection_status(
    credential_session_id: &str,
) -> anyhow::Result<ConnectionStatusResponse> {
    let credentials = credentials_for_session(credential_session_id, None)?;
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

    let status = parse_response::<ConnectionStatusResponse>(response).await?;
    bind_workspace(credential_session_id, &status.workspace.id)?;
    Ok(status)
}

pub async fn sync_dsc_certificates(request: DscSyncRequest) -> anyhow::Result<DscSyncResponse> {
    let credentials =
        credentials_for_session(&request.credential_session_id, Some(IntegrationKind::Dsc))?;
    validate_credentials_for(&credentials, IntegrationKind::Dsc)?;
    if request.certificates.is_empty() {
        anyhow::bail!("No certificate data provided for sync");
    }
    validate_identifier("workspace external ID", &request.workspace_external_id)?;
    validate_workspace_binding(
        &request.credential_session_id,
        &request.workspace_external_id,
    )?;

    let client = api_client()?;
    let payload = serde_json::json!({
        "workspaceExternalId": request.workspace_external_id,
        "certificates": request.certificates,
        "syncTimestamp": chrono::Utc::now().to_rfc3339(),
    });
    let response = client
        .post(endpoint(
            credentials.base_url.as_deref(),
            "/integrations/sync/dsc",
        )?)
        .headers(auth_headers(&credentials)?)
        .json(&payload)
        .send()
        .await?;

    parse_response::<DscSyncResponse>(response).await
}

pub fn endpoint(base_url: Option<&str>, path: &str) -> anyhow::Result<reqwest::Url> {
    let configured_origins =
        env::var("BRIDGE_AXAL_ALLOWED_ORIGINS")
            .map(Some)
            .or_else(|error| match error {
                env::VarError::NotPresent => Ok(None),
                env::VarError::NotUnicode(_) => Err(anyhow::anyhow!(
                    "BRIDGE_AXAL_ALLOWED_ORIGINS is not valid Unicode"
                )),
            })?;
    endpoint_with_allowed_origins(base_url, path, configured_origins.as_deref())
}

fn endpoint_with_allowed_origins(
    base_url: Option<&str>,
    path: &str,
    configured_origins: Option<&str>,
) -> anyhow::Result<reqwest::Url> {
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
    validate_axal_origin(&url, configured_origins)?;

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

pub fn validate_credentials_for(
    credentials: &AxalCredentials,
    expected: IntegrationKind,
) -> anyhow::Result<()> {
    validate_credentials(credentials)?;
    if credentials.integration != expected {
        anyhow::bail!("AXAL integration does not match the requested operation");
    }
    Ok(())
}

pub fn credentials_for_session(
    credential_session_id: &str,
    expected_integration: Option<IntegrationKind>,
) -> anyhow::Result<Arc<AxalCredentials>> {
    if credential_session_id.is_empty() {
        anyhow::bail!("AXAL credential session is required");
    }
    let mut sessions = credential_sessions()
        .lock()
        .map_err(|_| anyhow::anyhow!("AXAL credential session storage is unavailable"))?;
    let now = Instant::now();
    purge_expired_credential_sessions(&mut sessions, now);
    let session = sessions
        .get_mut(credential_session_id)
        .ok_or_else(|| anyhow::anyhow!("AXAL credential session is invalid or expired"))?;
    session.last_used_at = now;
    let credentials = session.credentials.clone();
    if expected_integration.is_some_and(|expected| credentials.integration != expected) {
        anyhow::bail!("AXAL credential session does not match the requested operation");
    }
    Ok(credentials)
}

pub fn revoke_credential_session(credential_session_id: &str) -> anyhow::Result<()> {
    let mut sessions = credential_sessions()
        .lock()
        .map_err(|_| anyhow::anyhow!("AXAL credential session storage is unavailable"))?;
    sessions.remove(credential_session_id);
    Ok(())
}

fn purge_expired_credential_sessions(
    sessions: &mut HashMap<String, CredentialSession>,
    now: Instant,
) {
    sessions.retain(|_, session| {
        now.duration_since(session.last_used_at) <= CREDENTIAL_SESSION_IDLE_TTL
            && now.duration_since(session.created_at) <= CREDENTIAL_SESSION_ABSOLUTE_TTL
    });
}

fn credential_sessions() -> &'static Mutex<HashMap<String, CredentialSession>> {
    CREDENTIAL_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn validate_workspace_binding(
    credential_session_id: &str,
    workspace_external_id: &str,
) -> anyhow::Result<()> {
    validate_identifier("workspace external ID", workspace_external_id)?;
    let mut sessions = credential_sessions()
        .lock()
        .map_err(|_| anyhow::anyhow!("AXAL credential session storage is unavailable"))?;
    let now = Instant::now();
    purge_expired_credential_sessions(&mut sessions, now);
    let session = sessions
        .get_mut(credential_session_id)
        .ok_or_else(|| anyhow::anyhow!("AXAL credential session is invalid or expired"))?;
    session.last_used_at = now;
    match session.bound_workspace.as_deref() {
        Some(bound_workspace) if bound_workspace == workspace_external_id => Ok(()),
        _ => anyhow::bail!("Check AXAL connection status before syncing this workspace"),
    }
}

fn bind_workspace(credential_session_id: &str, workspace_id: &str) -> anyhow::Result<()> {
    validate_identifier("workspace ID", workspace_id)?;
    let mut sessions = credential_sessions()
        .lock()
        .map_err(|_| anyhow::anyhow!("AXAL credential session storage is unavailable"))?;
    let now = Instant::now();
    purge_expired_credential_sessions(&mut sessions, now);
    let session = sessions
        .get_mut(credential_session_id)
        .ok_or_else(|| anyhow::anyhow!("AXAL credential session is invalid or expired"))?;
    session.bound_workspace = Some(workspace_id.to_string());
    session.last_used_at = now;
    Ok(())
}

fn validate_axal_origin(
    url: &reqwest::Url,
    configured_origins: Option<&str>,
) -> anyhow::Result<()> {
    let candidate = url.origin().ascii_serialization();
    let default_origin = reqwest::Url::parse(DEFAULT_BASE_URL)?
        .origin()
        .ascii_serialization();
    if candidate == default_origin {
        return Ok(());
    }

    for raw_origin in configured_origins.unwrap_or_default().split(',') {
        let raw_origin = raw_origin.trim();
        if raw_origin.is_empty() {
            continue;
        }
        let allowed = reqwest::Url::parse(raw_origin).map_err(|_| {
            anyhow::anyhow!("BRIDGE_AXAL_ALLOWED_ORIGINS must contain valid HTTPS origins")
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
                "BRIDGE_AXAL_ALLOWED_ORIGINS must contain exact HTTPS origins without paths"
            );
        }
        if candidate == allowed.origin().ascii_serialization() {
            return Ok(());
        }
    }

    anyhow::bail!(
        "AXAL base URL origin is not trusted; configure BRIDGE_AXAL_ALLOWED_ORIGINS before launch"
    )
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
    use super::{
        api_client, bind_workspace, credential_sessions, credentials_for_session, endpoint,
        endpoint_with_allowed_origins, revoke_credential_session, validate_workspace_binding,
        AxalCredentials, CredentialSession, IntegrationKind, CREDENTIAL_SESSION_ABSOLUTE_TTL,
    };
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
            Some("https://complyeaze.com/bridge/"),
            "/integrations/validate-key",
        )
        .expect("valid endpoint");
        assert_eq!(
            url.as_str(),
            "https://complyeaze.com/bridge/axal/api/v1/integrations/validate-key"
        );
    }

    #[test]
    fn endpoint_rejects_path_traversal() {
        assert!(endpoint(Some("https://complyeaze.com"), "/../admin").is_err());
    }

    #[test]
    fn endpoint_rejects_untrusted_origins_and_alternate_ports() {
        for value in [
            "https://example.com",
            "https://127.0.0.1",
            "https://complyeaze.com:8443",
            "https://complyeaze.example",
        ] {
            assert!(endpoint_with_allowed_origins(Some(value), "/x", None).is_err());
        }
    }

    #[test]
    fn endpoint_accepts_an_explicit_exact_custom_origin() {
        let url = endpoint_with_allowed_origins(
            Some("https://bridge.example/tenant"),
            "/x",
            Some("https://bridge.example"),
        )
        .expect("explicit trusted origin");
        assert_eq!(url.as_str(), "https://bridge.example/tenant/axal/api/v1/x");
    }

    #[test]
    fn credential_sessions_bind_integration_and_workspace() {
        let session_id = uuid::Uuid::new_v4().to_string();
        let credentials = Arc::new(AxalCredentials {
            api_key: "synthetic-secret".to_string(),
            api_id: "synthetic-id".to_string(),
            integration: IntegrationKind::Documents,
            base_url: None,
        });
        credential_sessions()
            .lock()
            .expect("credential session lock")
            .insert(
                session_id.clone(),
                CredentialSession {
                    credentials: credentials.clone(),
                    bound_workspace: None,
                    created_at: std::time::Instant::now(),
                    last_used_at: std::time::Instant::now(),
                },
            );

        assert!(credentials_for_session(&session_id, Some(IntegrationKind::Documents)).is_ok());
        assert!(credentials_for_session(&session_id, Some(IntegrationKind::Dsc)).is_err());
        bind_workspace(&session_id, "workspace-synthetic").expect("bind workspace");
        assert!(validate_workspace_binding(&session_id, "workspace-synthetic").is_ok());
        assert!(validate_workspace_binding(&session_id, "workspace-other").is_err());
        revoke_credential_session(&session_id).expect("revoke session");
        assert!(credentials_for_session(&session_id, None).is_err());

        let replacement_id = uuid::Uuid::new_v4().to_string();
        credential_sessions()
            .lock()
            .expect("credential session lock")
            .insert(
                replacement_id.clone(),
                CredentialSession {
                    credentials: credentials.clone(),
                    bound_workspace: None,
                    created_at: std::time::Instant::now(),
                    last_used_at: std::time::Instant::now(),
                },
            );
        assert!(validate_workspace_binding(&replacement_id, "workspace-synthetic").is_err());
        bind_workspace(&replacement_id, "workspace-synthetic").expect("bind replacement");
        assert!(validate_workspace_binding(&replacement_id, "workspace-synthetic").is_ok());
        revoke_credential_session(&replacement_id).expect("revoke replacement");

        let expired_id = uuid::Uuid::new_v4().to_string();
        credential_sessions()
            .lock()
            .expect("credential session lock")
            .insert(
                expired_id.clone(),
                CredentialSession {
                    credentials,
                    bound_workspace: Some("workspace-synthetic".to_string()),
                    created_at: std::time::Instant::now()
                        - CREDENTIAL_SESSION_ABSOLUTE_TTL
                        - Duration::from_secs(1),
                    last_used_at: std::time::Instant::now(),
                },
            );
        assert!(credentials_for_session(&expired_id, None).is_err());
    }

    #[tokio::test]
    async fn api_client_does_not_follow_redirects() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("first request");
            let mut request = vec![0_u8; 4096];
            let read = socket.read(&mut request).await.expect("read request");
            assert!(String::from_utf8_lossy(&request[..read])
                .to_ascii_lowercase()
                .contains("authorization: bearer"));
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://{address}/second\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .expect("write redirect");
            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_err()
        });

        let response = api_client()
            .expect("client")
            .get(format!("http://{address}/first"))
            .header("authorization", "Bearer synthetic-secret")
            .send()
            .await
            .expect("redirect response");
        assert!(response.status().is_redirection());
        assert!(server.await.expect("server task"));
    }
}
