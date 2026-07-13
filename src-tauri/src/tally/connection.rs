use reqwest::header::CONTENT_TYPE;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;

use super::xml_parser::{TallyLedger, TallyVoucher};
use super::{
    serial_queue::SerialTallyQueue,
    tdl_engine,
    xml_parser::{self, TallyCompany},
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TallyConfig {
    pub host: String,
    pub port: u16,
}

impl Default for TallyConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 9000,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum TallyProduct {
    TallyPrime,
    #[serde(rename = "Tally ERP 9")]
    TallyErp9,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionStatus {
    pub reachable: bool,
    pub server_text: String,
    pub product: TallyProduct,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct TallyClient {
    config: TallyConfig,
    http: reqwest::Client,
    queue: SerialTallyQueue,
}

impl TallyClient {
    pub fn new(config: TallyConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(Policy::none())
            .build()
            .expect("reqwest client should build");

        Self {
            config,
            http,
            queue: SerialTallyQueue::default(),
        }
    }

    pub async fn check_connection(&self) -> anyhow::Result<ConnectionStatus> {
        let url = tally_endpoint(&self.config, "/status")?;
        let result = self
            .queue
            .run(|| async {
                let response = self
                    .http
                    .get(url)
                    .header(CONTENT_TYPE, "text/xml")
                    .send()
                    .await?
                    .error_for_status()?;
                response_text_limited(response, 1024 * 1024).await
            })
            .await;

        match result {
            Ok(server_text) => Ok(ConnectionStatus {
                reachable: is_supported_tally_status(&server_text),
                product: detect_product(&server_text),
                server_text,
                error: None,
            }),
            Err(error) => Ok(ConnectionStatus {
                reachable: false,
                server_text: String::new(),
                product: TallyProduct::Unknown,
                error: Some(error.to_string()),
            }),
        }
    }

    pub async fn post_xml(&self, xml: String) -> anyhow::Result<String> {
        let url = tally_endpoint(&self.config, "/")?;
        self.queue
            .run(|| async {
                let response = self
                    .http
                    .post(url)
                    .header(CONTENT_TYPE, "text/xml; charset=utf-8")
                    .body(xml)
                    .send()
                    .await?
                    .error_for_status()?;
                response_text_limited(response, 32 * 1024 * 1024).await
            })
            .await
    }

    pub async fn fetch_companies(&self) -> anyhow::Result<Vec<TallyCompany>> {
        let xml = self.post_xml(tdl_engine::company_list_request()).await?;
        xml_parser::parse_companies(&xml)
    }

    pub async fn fetch_ledgers(&self, company: &str) -> anyhow::Result<Vec<TallyLedger>> {
        let xml = self.post_xml(tdl_engine::ledgers_request(company)).await?;
        xml_parser::parse_ledgers(&xml)
    }

    pub async fn fetch_vouchers(
        &self,
        company: &str,
        from: &str,
        to: &str,
    ) -> anyhow::Result<Vec<TallyVoucher>> {
        let xml = self
            .post_xml(tdl_engine::vouchers_request(company, from, to))
            .await?;
        xml_parser::parse_vouchers(&xml)
    }
}

fn tally_endpoint(config: &TallyConfig, path: &str) -> anyhow::Result<reqwest::Url> {
    let host = config.host.trim();
    if host.is_empty()
        || host.len() > 253
        || host.chars().any(char::is_control)
        || host.contains(['/', '\\', '?', '#', '@'])
    {
        anyhow::bail!("Tally host must be a hostname or IP address without a URL scheme or path");
    }
    if config.port == 0 {
        anyhow::bail!("Tally port must be between 1 and 65535");
    }

    let mut url = reqwest::Url::parse("http://localhost")?;
    if let Ok(ip_address) = host.parse::<IpAddr>() {
        if !ip_address.is_loopback() {
            anyhow::bail!("Tally connections are restricted to this computer (loopback)");
        }
        url.set_ip_host(ip_address)
            .map_err(|_| anyhow::anyhow!("Tally host is invalid"))?;
    } else {
        if !host.eq_ignore_ascii_case("localhost") {
            anyhow::bail!("Tally connections are restricted to localhost or a loopback IP");
        }
        url.set_ip_host("127.0.0.1".parse::<IpAddr>().expect("valid loopback IP"))
            .map_err(|_| anyhow::anyhow!("Tally host is invalid"))?;
    }
    url.set_port(Some(config.port))
        .map_err(|_| anyhow::anyhow!("Tally port is invalid"))?;
    url.set_path(path);
    Ok(url)
}

async fn response_text_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> anyhow::Result<String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        anyhow::bail!("Tally response exceeded the {max_bytes}-byte limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            anyhow::bail!("Tally response exceeded the {max_bytes}-byte limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("Tally returned a response that was not valid UTF-8"))
}

fn is_supported_tally_status(text: &str) -> bool {
    matches!(
        detect_product(text),
        TallyProduct::TallyPrime | TallyProduct::TallyErp9
    )
}

fn detect_product(text: &str) -> TallyProduct {
    let normalized = text.to_ascii_lowercase();
    if normalized.contains("tallyprime server is running") {
        TallyProduct::TallyPrime
    } else if normalized.contains("tally erp 9") || normalized.contains("tally.erp 9") {
        TallyProduct::TallyErp9
    } else {
        TallyProduct::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::{detect_product, tally_endpoint, TallyConfig, TallyProduct};

    #[test]
    fn detects_tallyprime_status() {
        assert!(matches!(
            detect_product("TallyPrime Server is Running"),
            TallyProduct::TallyPrime
        ));
    }

    #[test]
    fn detects_erp9_status() {
        assert!(matches!(
            detect_product("Tally ERP 9 Server is Running"),
            TallyProduct::TallyErp9
        ));
    }

    #[test]
    fn validates_tally_endpoint_components() {
        assert_eq!(
            tally_endpoint(&TallyConfig::default(), "/status")
                .expect("localhost endpoint")
                .as_str(),
            "http://127.0.0.1:9000/status"
        );
        let config = TallyConfig {
            host: "::1".to_string(),
            port: 9000,
        };
        assert_eq!(
            tally_endpoint(&config, "/status")
                .expect("IPv6 endpoint")
                .as_str(),
            "http://[::1]:9000/status"
        );

        for host in ["http://localhost", "localhost/path", "user@localhost", ""] {
            let invalid = TallyConfig {
                host: host.to_string(),
                port: 9000,
            };
            assert!(tally_endpoint(&invalid, "/status").is_err());
        }

        for host in [
            "192.168.1.10",
            "10.0.0.5",
            "169.254.1.1",
            "224.0.0.1",
            "8.8.8.8",
            "tally.internal",
        ] {
            let remote = TallyConfig {
                host: host.to_string(),
                port: 9000,
            };
            assert!(tally_endpoint(&remote, "/status").is_err());
        }
    }
}
