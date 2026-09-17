use super::{
    api_client, bind_workspace, credential_sessions, credentials_for_session, endpoint,
    endpoint_with_allowed_origins, purge_expired_credential_sessions, revoke_credential_session,
    validate_workspace_binding, AxalCredentials, CredentialSession, IntegrationKind,
    CREDENTIAL_SESSION_ABSOLUTE_TTL,
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
    assert!(credentials_for_session(&session_id, Some(IntegrationKind::Tally)).is_err());
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
    let created_at = std::time::Instant::now();
    let last_used_at = created_at
        .checked_add(CREDENTIAL_SESSION_ABSOLUTE_TTL)
        .expect("credential absolute lifetime");
    credential_sessions()
        .lock()
        .expect("credential session lock")
        .insert(
            expired_id.clone(),
            CredentialSession {
                credentials,
                bound_workspace: Some("workspace-synthetic".to_string()),
                created_at,
                last_used_at,
            },
        );
    let expiry_check = last_used_at
        .checked_add(Duration::from_secs(1))
        .expect("credential expiry deadline");
    let mut sessions = credential_sessions()
        .lock()
        .expect("credential session lock");
    purge_expired_credential_sessions(&mut sessions, expiry_check);
    assert!(!sessions.contains_key(&expired_id));
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
