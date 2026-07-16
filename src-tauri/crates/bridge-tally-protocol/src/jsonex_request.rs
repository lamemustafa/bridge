//! Dormant, deterministic request bytes for two native-JSON collection
//! examples documented by Tally Solutions.
//!
//! This module has no HTTP client and every output is explicitly ineligible
//! for dispatch. The documented requests do not bind a date range, and their
//! responses do not echo enough company/query evidence for Bridge Core.

use std::fmt;

use serde::Serialize;

pub const DOCUMENTED_LEDGER_REQUEST_PROFILE_V1: &str =
    "tally.documented-json-ledger-collection-docx-2025-11/1";
pub const DOCUMENTED_VOUCHER_REQUEST_PROFILE_V1: &str =
    "tally.documented-json-tspl-voucher-collection-docx-2025-11/1";
const MAX_COMPANY_NAME_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonExRequestWireEncoding {
    PlainAsciiUtf8,
    Utf8Bom,
    Utf16LeBom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonExResponseEncodingExpectation {
    Unspecified,
    Utf16Le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonExRequestBuildError {
    InvalidCompanyName,
    MultilingualCompanyRequiresBomProfile,
    InvalidByteLimit,
    RequestTooLarge,
    SerializationFailed,
}

impl JsonExRequestBuildError {
    pub fn safe_code(self) -> &'static str {
        match self {
            Self::InvalidCompanyName => "jsonex_request_company_invalid",
            Self::MultilingualCompanyRequiresBomProfile => {
                "jsonex_request_multilingual_company_requires_bom"
            }
            Self::InvalidByteLimit => "jsonex_request_byte_limit_invalid",
            Self::RequestTooLarge => "jsonex_request_too_large",
            Self::SerializationFailed => "jsonex_request_serialization_failed",
        }
    }
}

impl fmt::Display for JsonExRequestBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.safe_code())
    }
}

impl std::error::Error for JsonExRequestBuildError {}

#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedJsonExCompanyName(String);

impl ValidatedJsonExCompanyName {
    pub fn new(value: impl Into<String>) -> Result<Self, JsonExRequestBuildError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_COMPANY_NAME_BYTES
            || value.chars().all(char::is_whitespace)
            || value.chars().any(char::is_control)
        {
            return Err(JsonExRequestBuildError::InvalidCompanyName);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JsonExRequestHeaders {
    pub content_type: &'static str,
    pub version: &'static str,
    pub tally_request: &'static str,
    pub request_type: &'static str,
    pub id: &'static str,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DormantJsonExRequest {
    pub profile_id: &'static str,
    pub headers: JsonExRequestHeaders,
    pub wire_encoding: JsonExRequestWireEncoding,
    pub response_encoding_expectation: JsonExResponseEncodingExpectation,
    pub body: Vec<u8>,
}

impl DormantJsonExRequest {
    pub const fn dispatch_eligible(&self) -> bool {
        false
    }

    pub const fn company_identity_verifiable_from_documented_response(&self) -> bool {
        false
    }

    pub const fn date_range_bound(&self) -> bool {
        false
    }
}

pub fn build_documented_ledger_request_v1(
    company: &ValidatedJsonExCompanyName,
    wire_encoding: JsonExRequestWireEncoding,
    max_encoded_bytes: usize,
) -> Result<DormantJsonExRequest, JsonExRequestBuildError> {
    let body = LedgerRequestBody {
        static_variables: static_variables(company.as_str()),
        fetch_list: ["Name", "Parent", "Closing Balance"],
    };
    build_request(
        DOCUMENTED_LEDGER_REQUEST_PROFILE_V1,
        "Ledger",
        &body,
        company,
        wire_encoding,
        max_encoded_bytes,
    )
}

pub fn build_documented_voucher_request_v1(
    company: &ValidatedJsonExCompanyName,
    wire_encoding: JsonExRequestWireEncoding,
    max_encoded_bytes: usize,
) -> Result<DormantJsonExRequest, JsonExRequestBuildError> {
    let body = VoucherRequestBody {
        static_variables: static_variables(company.as_str()),
        tdl_message: [TdlMessage {
            definitions: [Definition {
                metadata: DefinitionMetadata {
                    name: "TSPLVoucherColl",
                    definition_type: "Collection",
                },
                attributes: [
                    DefinitionAttribute::Type { value: "Voucher" },
                    DefinitionAttribute::NativeMethod {
                        value: "VoucherNumber, VoucherTypeName, Date, Amount",
                    },
                ],
            }],
        }],
    };
    build_request(
        DOCUMENTED_VOUCHER_REQUEST_PROFILE_V1,
        "TSPLVoucherColl",
        &body,
        company,
        wire_encoding,
        max_encoded_bytes,
    )
}

fn build_request<T: Serialize>(
    profile_id: &'static str,
    id: &'static str,
    body: &T,
    company: &ValidatedJsonExCompanyName,
    wire_encoding: JsonExRequestWireEncoding,
    max_encoded_bytes: usize,
) -> Result<DormantJsonExRequest, JsonExRequestBuildError> {
    if max_encoded_bytes == 0 {
        return Err(JsonExRequestBuildError::InvalidByteLimit);
    }
    if wire_encoding == JsonExRequestWireEncoding::PlainAsciiUtf8 && !company.as_str().is_ascii() {
        return Err(JsonExRequestBuildError::MultilingualCompanyRequiresBomProfile);
    }
    let json =
        serde_json::to_string(body).map_err(|_| JsonExRequestBuildError::SerializationFailed)?;
    let (content_type, response_encoding_expectation, encoded) = match wire_encoding {
        JsonExRequestWireEncoding::PlainAsciiUtf8 => (
            "application/json",
            JsonExResponseEncodingExpectation::Unspecified,
            json.into_bytes(),
        ),
        JsonExRequestWireEncoding::Utf8Bom => {
            let mut bytes = Vec::with_capacity(json.len().saturating_add(3));
            bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
            bytes.extend_from_slice(json.as_bytes());
            (
                "application/json;charset=utf-8",
                JsonExResponseEncodingExpectation::Utf16Le,
                bytes,
            )
        }
        JsonExRequestWireEncoding::Utf16LeBom => {
            let mut bytes = Vec::with_capacity(json.len().saturating_mul(2).saturating_add(2));
            bytes.extend_from_slice(&[0xff, 0xfe]);
            for unit in json.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            (
                "application/json;charset=utf-16",
                JsonExResponseEncodingExpectation::Utf16Le,
                bytes,
            )
        }
    };
    if encoded.len() > max_encoded_bytes {
        return Err(JsonExRequestBuildError::RequestTooLarge);
    }
    Ok(DormantJsonExRequest {
        profile_id,
        headers: JsonExRequestHeaders {
            content_type,
            version: "1",
            tally_request: "Export",
            request_type: "Collection",
            id,
        },
        wire_encoding,
        response_encoding_expectation,
        body: encoded,
    })
}

#[derive(Serialize)]
struct StaticVariable<'a> {
    name: &'static str,
    value: &'a str,
}

fn static_variables(company: &str) -> [StaticVariable<'_>; 2] {
    [
        StaticVariable {
            name: "svExportFormat",
            value: "jsonex",
        },
        StaticVariable {
            name: "svCurrentCompany",
            value: company,
        },
    ]
}

#[derive(Serialize)]
struct LedgerRequestBody<'a> {
    static_variables: [StaticVariable<'a>; 2],
    #[serde(rename = "fetch_List")]
    fetch_list: [&'static str; 3],
}

#[derive(Serialize)]
struct VoucherRequestBody<'a> {
    static_variables: [StaticVariable<'a>; 2],
    #[serde(rename = "tdlmessage")]
    tdl_message: [TdlMessage; 1],
}

#[derive(Serialize)]
struct TdlMessage {
    definitions: [Definition; 1],
}

#[derive(Serialize)]
struct Definition {
    metadata: DefinitionMetadata,
    attributes: [DefinitionAttribute; 2],
}

#[derive(Serialize)]
struct DefinitionMetadata {
    name: &'static str,
    #[serde(rename = "type")]
    definition_type: &'static str,
}

#[derive(Serialize)]
#[serde(untagged)]
enum DefinitionAttribute {
    Type {
        #[serde(rename = "Type")]
        value: &'static str,
    },
    NativeMethod {
        #[serde(rename = "Native Method")]
        value: &'static str,
    },
}
