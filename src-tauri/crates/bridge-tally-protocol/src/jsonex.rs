//! Feature-gated parsing for the native JSON collection envelopes documented
//! by TallyPrime 7.0+.
//!
//! The official Ledger and Voucher examples do not echo a stable company
//! identity, Bridge schema, exact query profile, source counts, or a requested
//! date range. Consequently every successful result in this module is named
//! `Unbound` and is ineligible for canonical Core accounting or transport
//! qualification. The production Bridge application does not enable this
//! Cargo feature.

use std::{collections::BTreeMap, fmt};

use serde::{
    de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};
use serde_json::value::RawValue;

use crate::{decode_tally_text_bytes_limited, TallyTextDecodeError, TallyTextEncoding};

pub const DOCUMENTED_LEDGER_COLLECTION_PROFILE_V1: &str =
    "tally.documented-jsonex-ledger-collection/1";
pub const DOCUMENTED_VOUCHER_COLLECTION_PROFILE_V1: &str =
    "tally.documented-jsonex-voucher-collection/1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonExLimits {
    pub max_encoded_bytes: usize,
    pub max_decoded_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_object_members: usize,
    pub max_array_items: usize,
    pub max_string_bytes: usize,
    pub max_records: usize,
    pub max_record_decoded_bytes: usize,
}

impl Default for JsonExLimits {
    fn default() -> Self {
        Self {
            max_encoded_bytes: 8 * 1024 * 1024,
            max_decoded_bytes: 8 * 1024 * 1024,
            max_depth: 32,
            max_nodes: 250_000,
            max_object_members: 512,
            max_array_items: 50_000,
            max_string_bytes: 1024 * 1024,
            max_records: 25_000,
            max_record_decoded_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonExExpectedEncoding {
    Utf8,
    Utf16Le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonExProtocolError {
    ResponseTooLarge,
    DecodedResponseTooLarge,
    UnsupportedContentType,
    EncodingMismatch,
    UnsupportedEncoding,
    InvalidEncoding,
    EmbeddedNull,
    MalformedJson,
    DuplicateField,
    ResourceLimitExceeded,
    StatusMissing,
    StatusInvalid,
    ApplicationRejected,
    WrongContainer,
    RecordLimitExceeded,
    RecordTooLarge,
    ProfileMismatch,
    TypedValueMismatch,
}

impl JsonExProtocolError {
    pub fn safe_code(self) -> &'static str {
        match self {
            Self::ResponseTooLarge => "jsonex_response_too_large",
            Self::DecodedResponseTooLarge => "jsonex_decoded_response_too_large",
            Self::UnsupportedContentType => "jsonex_content_type_unsupported",
            Self::EncodingMismatch => "jsonex_encoding_mismatch",
            Self::UnsupportedEncoding => "jsonex_encoding_unsupported",
            Self::InvalidEncoding => "jsonex_encoding_invalid",
            Self::EmbeddedNull => "jsonex_embedded_null",
            Self::MalformedJson => "jsonex_malformed_json",
            Self::DuplicateField => "jsonex_duplicate_field",
            Self::ResourceLimitExceeded => "jsonex_resource_limit_exceeded",
            Self::StatusMissing => "jsonex_status_missing",
            Self::StatusInvalid => "jsonex_status_invalid",
            Self::ApplicationRejected => "jsonex_application_rejected",
            Self::WrongContainer => "jsonex_wrong_container",
            Self::RecordLimitExceeded => "jsonex_record_limit_exceeded",
            Self::RecordTooLarge => "jsonex_record_too_large",
            Self::ProfileMismatch => "jsonex_profile_mismatch",
            Self::TypedValueMismatch => "jsonex_typed_value_mismatch",
        }
    }
}

impl fmt::Display for JsonExProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.safe_code())
    }
}

impl std::error::Error for JsonExProtocolError {}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JsonExApplicationStatus {
    Success,
    Failure,
}

#[derive(Clone, PartialEq, Eq)]
pub struct JsonExEnvelopeEvidence {
    pub profile_id: &'static str,
    pub status: JsonExApplicationStatus,
    pub encoding: TallyTextEncoding,
    pub encoded_bytes: u64,
    pub decoded_bytes: u64,
    pub record_count: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub enum JsonExTextPresence {
    Absent,
    Empty,
    Value(String),
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnboundJsonExLedgerRecord {
    pub name: String,
    pub parent: JsonExTextPresence,
    pub closing_balance: JsonExTextPresence,
    pub opening_balance: JsonExTextPresence,
    pub language_names: Vec<Vec<String>>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnboundJsonExLedgerCollection {
    pub records: Vec<UnboundJsonExLedgerRecord>,
    pub evidence: JsonExEnvelopeEvidence,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnboundJsonExVoucherRecord {
    pub remote_id: String,
    pub guid: String,
    pub date_yyyymmdd: String,
    pub voucher_type: String,
    pub voucher_number: Option<String>,
    pub amount: JsonExTextPresence,
    pub all_ledger_entry_count: u64,
    pub inventory_entry_count: u64,
    pub invoice_ledger_entry_count: u64,
    pub batch_allocation_count: u64,
    pub accounting_allocation_count: u64,
    pub bill_allocation_count: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct UnboundJsonExVoucherCollection {
    pub records: Vec<UnboundJsonExVoucherRecord>,
    pub evidence: JsonExEnvelopeEvidence,
}

pub fn parse_documented_ledger_collection_v1(
    bytes: &[u8],
    observed_content_type: &str,
    expected_encoding: JsonExExpectedEncoding,
    limits: JsonExLimits,
) -> Result<UnboundJsonExLedgerCollection, JsonExProtocolError> {
    let parsed = parse_collection_envelope(
        bytes,
        observed_content_type,
        expected_encoding,
        limits,
        DOCUMENTED_LEDGER_COLLECTION_PROFILE_V1,
    )?;
    validate_ledger_metadata(&parsed.metadata)?;
    let records = parsed
        .records
        .iter()
        .map(parse_ledger_record)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(UnboundJsonExLedgerCollection {
        records,
        evidence: parsed.evidence,
    })
}

pub fn parse_documented_voucher_collection_v1(
    bytes: &[u8],
    observed_content_type: &str,
    expected_encoding: JsonExExpectedEncoding,
    limits: JsonExLimits,
) -> Result<UnboundJsonExVoucherCollection, JsonExProtocolError> {
    let parsed = parse_collection_envelope(
        bytes,
        observed_content_type,
        expected_encoding,
        limits,
        DOCUMENTED_VOUCHER_COLLECTION_PROFILE_V1,
    )?;
    validate_voucher_metadata(&parsed.metadata)?;
    let records = parsed
        .records
        .iter()
        .map(parse_voucher_record)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(UnboundJsonExVoucherCollection {
        records,
        evidence: parsed.evidence,
    })
}

struct ParsedCollectionEnvelope {
    metadata: StrictJson,
    records: Vec<StrictJson>,
    evidence: JsonExEnvelopeEvidence,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCollectionEnvelope<'a> {
    status: &'a str,
    #[serde(borrow)]
    data: RawCollectionData<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCollectionData<'a> {
    #[serde(rename = "metadata", borrow)]
    _metadata: &'a RawValue,
    #[serde(borrow)]
    collection: Vec<&'a RawValue>,
}

fn parse_collection_envelope(
    bytes: &[u8],
    observed_content_type: &str,
    expected_encoding: JsonExExpectedEncoding,
    limits: JsonExLimits,
    profile_id: &'static str,
) -> Result<ParsedCollectionEnvelope, JsonExProtocolError> {
    validate_limits_configuration(limits)?;
    let decoded = decode_jsonex_bytes(bytes, observed_content_type, expected_encoding, limits)?;
    let strict = parse_strict_json(&decoded.text, limits)?;
    let root = as_object(&strict).ok_or(JsonExProtocolError::MalformedJson)?;
    let status = root
        .get("status")
        .ok_or(JsonExProtocolError::StatusMissing)
        .and_then(|value| as_str(value).ok_or(JsonExProtocolError::StatusInvalid))?;
    match status {
        "0" => return Err(JsonExProtocolError::ApplicationRejected),
        "1" => {}
        _ => return Err(JsonExProtocolError::StatusInvalid),
    }
    require_allowed_keys(
        root,
        &["status", "data"],
        JsonExProtocolError::WrongContainer,
    )?;
    let data = root
        .get("data")
        .and_then(as_object)
        .ok_or(JsonExProtocolError::WrongContainer)?;
    require_allowed_keys(
        data,
        &["metadata", "collection"],
        JsonExProtocolError::WrongContainer,
    )?;
    let strict_records = data
        .get("collection")
        .and_then(as_array)
        .ok_or(JsonExProtocolError::WrongContainer)?;
    if strict_records.len() > limits.max_records {
        return Err(JsonExProtocolError::RecordLimitExceeded);
    }

    let raw: RawCollectionEnvelope<'_> =
        serde_json::from_str(&decoded.text).map_err(|_| JsonExProtocolError::WrongContainer)?;
    if raw.status != "1" {
        return Err(JsonExProtocolError::StatusInvalid);
    }
    for record in &raw.data.collection {
        if record.get().len() > limits.max_record_decoded_bytes {
            return Err(JsonExProtocolError::RecordTooLarge);
        }
    }

    let mut root = into_object(strict).ok_or(JsonExProtocolError::WrongContainer)?;
    let mut data = root
        .remove("data")
        .and_then(into_object)
        .ok_or(JsonExProtocolError::WrongContainer)?;
    let metadata = data
        .remove("metadata")
        .ok_or(JsonExProtocolError::WrongContainer)?;
    let records = data
        .remove("collection")
        .and_then(into_array)
        .ok_or(JsonExProtocolError::WrongContainer)?;
    Ok(ParsedCollectionEnvelope {
        metadata,
        evidence: JsonExEnvelopeEvidence {
            profile_id,
            status: JsonExApplicationStatus::Success,
            encoding: decoded.encoding,
            encoded_bytes: u64::try_from(bytes.len())
                .map_err(|_| JsonExProtocolError::ResourceLimitExceeded)?,
            decoded_bytes: u64::try_from(decoded.text.len())
                .map_err(|_| JsonExProtocolError::ResourceLimitExceeded)?,
            record_count: u64::try_from(records.len())
                .map_err(|_| JsonExProtocolError::ResourceLimitExceeded)?,
        },
        records,
    })
}

fn validate_limits_configuration(limits: JsonExLimits) -> Result<(), JsonExProtocolError> {
    if limits.max_encoded_bytes == 0
        || limits.max_decoded_bytes == 0
        || limits.max_depth == 0
        || limits.max_nodes == 0
        || limits.max_object_members == 0
        || limits.max_array_items == 0
        || limits.max_string_bytes == 0
        || limits.max_records == 0
        || limits.max_record_decoded_bytes == 0
    {
        return Err(JsonExProtocolError::ResourceLimitExceeded);
    }
    Ok(())
}

fn decode_jsonex_bytes(
    bytes: &[u8],
    observed_content_type: &str,
    expected_encoding: JsonExExpectedEncoding,
    limits: JsonExLimits,
) -> Result<crate::DecodedTallyText, JsonExProtocolError> {
    let declared_charset = parse_content_type(observed_content_type)?;
    let decoded =
        decode_tally_text_bytes_limited(bytes, limits.max_encoded_bytes).map_err(|error| {
            match error {
                TallyTextDecodeError::TooLarge => JsonExProtocolError::ResponseTooLarge,
                TallyTextDecodeError::InvalidUtf8
                | TallyTextDecodeError::InvalidUtf16Le
                | TallyTextDecodeError::InvalidUtf16Be => JsonExProtocolError::InvalidEncoding,
            }
        })?;
    if decoded.text.len() > limits.max_decoded_bytes {
        return Err(JsonExProtocolError::DecodedResponseTooLarge);
    }
    if decoded.text.contains('\0') {
        return Err(JsonExProtocolError::EmbeddedNull);
    }
    if decoded.encoding == TallyTextEncoding::Utf16BeBom {
        return Err(JsonExProtocolError::UnsupportedEncoding);
    }
    let actual_utf16 = decoded.encoding == TallyTextEncoding::Utf16LeBom;
    let expected_utf16 = expected_encoding == JsonExExpectedEncoding::Utf16Le;
    if actual_utf16 != expected_utf16 {
        return Err(JsonExProtocolError::EncodingMismatch);
    }
    if declared_charset.is_some_and(|charset| charset != expected_encoding) {
        return Err(JsonExProtocolError::EncodingMismatch);
    }
    Ok(decoded)
}

fn parse_content_type(
    content_type: &str,
) -> Result<Option<JsonExExpectedEncoding>, JsonExProtocolError> {
    if content_type.chars().any(char::is_control) {
        return Err(JsonExProtocolError::UnsupportedContentType);
    }
    let mut parts = content_type.split(';');
    if !parts.next().is_some_and(|media| {
        media
            .trim_matches(' ')
            .eq_ignore_ascii_case("application/json")
    }) {
        return Err(JsonExProtocolError::UnsupportedContentType);
    }
    let mut charset = None;
    for parameter in parts {
        let (name, value) = parameter
            .split_once('=')
            .ok_or(JsonExProtocolError::UnsupportedContentType)?;
        if !name.trim_matches(' ').eq_ignore_ascii_case("charset") || charset.is_some() {
            return Err(JsonExProtocolError::UnsupportedContentType);
        }
        let value = value.trim_matches(' ');
        let starts_quoted = value.starts_with('"');
        let ends_quoted = value.ends_with('"');
        if starts_quoted != ends_quoted {
            return Err(JsonExProtocolError::UnsupportedContentType);
        }
        let value =
            if starts_quoted {
                if value.len() < 2 {
                    return Err(JsonExProtocolError::UnsupportedContentType);
                }
                let inner = &value[1..value.len() - 1];
                if inner.chars().any(|character| {
                    character == '"' || character == '\\' || character.is_control()
                }) {
                    return Err(JsonExProtocolError::UnsupportedContentType);
                }
                inner
            } else {
                if value.chars().any(|character| {
                    character == '"' || character == '\\' || character.is_control()
                }) {
                    return Err(JsonExProtocolError::UnsupportedContentType);
                }
                value
            };
        charset = Some(
            if value.eq_ignore_ascii_case("utf-8") || value.eq_ignore_ascii_case("utf8") {
                JsonExExpectedEncoding::Utf8
            } else if value.eq_ignore_ascii_case("utf-16") || value.eq_ignore_ascii_case("utf-16le")
            {
                JsonExExpectedEncoding::Utf16Le
            } else {
                return Err(JsonExProtocolError::UnsupportedContentType);
            },
        );
    }
    Ok(charset)
}

#[derive(Clone, PartialEq)]
enum StrictJson {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Fractional,
    String(String),
    Array(Vec<StrictJson>),
    Object(BTreeMap<String, StrictJson>),
}

#[derive(Default)]
struct JsonLimitState {
    nodes: usize,
}

struct StrictJsonSeed<'a> {
    limits: JsonExLimits,
    state: &'a mut JsonLimitState,
    depth: usize,
    reject_before_value: bool,
}

impl<'de> DeserializeSeed<'de> for StrictJsonSeed<'_> {
    type Value = StrictJson;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        if self.reject_before_value || self.depth > self.limits.max_depth {
            return Err(D::Error::custom("jsonex_resource_limit_exceeded"));
        }
        self.state.nodes = self
            .state
            .nodes
            .checked_add(1)
            .ok_or_else(|| D::Error::custom("jsonex_resource_limit_exceeded"))?;
        if self.state.nodes > self.limits.max_nodes {
            return Err(D::Error::custom("jsonex_resource_limit_exceeded"));
        }
        deserializer.deserialize_any(StrictJsonVisitor {
            limits: self.limits,
            state: self.state,
            depth: self.depth,
        })
    }
}

struct StrictJsonVisitor<'a> {
    limits: JsonExLimits,
    state: &'a mut JsonLimitState,
    depth: usize,
}

impl<'de> Visitor<'de> for StrictJsonVisitor<'_> {
    type Value = StrictJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a duplicate-free bounded JSON value")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJson::Null)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJson::Null)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJson::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJson::Signed(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJson::Unsigned(value))
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(StrictJson::Fractional)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value.len() > self.limits.max_string_bytes {
            return Err(E::custom("jsonex_resource_limit_exceeded"));
        }
        Ok(StrictJson::String(value.to_string()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value.len() > self.limits.max_string_bytes {
            return Err(E::custom("jsonex_resource_limit_exceeded"));
        }
        Ok(StrictJson::String(value))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        loop {
            let reject_before_value = values.len() >= self.limits.max_array_items;
            let next = sequence.next_element_seed(StrictJsonSeed {
                limits: self.limits,
                state: &mut *self.state,
                depth: self.depth + 1,
                reject_before_value,
            })?;
            match next {
                Some(value) => values.push(value),
                None => break,
            }
        }
        Ok(StrictJson::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.len() >= self.limits.max_object_members
                || key.len() > self.limits.max_string_bytes
            {
                return Err(A::Error::custom("jsonex_resource_limit_exceeded"));
            }
            if values.contains_key(&key) {
                return Err(A::Error::custom("jsonex_duplicate_field"));
            }
            let value = map.next_value_seed(StrictJsonSeed {
                limits: self.limits,
                state: &mut *self.state,
                depth: self.depth + 1,
                reject_before_value: false,
            })?;
            values.insert(key, value);
        }
        Ok(StrictJson::Object(values))
    }
}

fn parse_strict_json(text: &str, limits: JsonExLimits) -> Result<StrictJson, JsonExProtocolError> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let mut state = JsonLimitState::default();
    let value = StrictJsonSeed {
        limits,
        state: &mut state,
        depth: 1,
        reject_before_value: false,
    }
    .deserialize(&mut deserializer)
    .map_err(|error| {
        if error.to_string().contains("jsonex_duplicate_field") {
            JsonExProtocolError::DuplicateField
        } else if error.to_string().contains("jsonex_resource_limit_exceeded") {
            JsonExProtocolError::ResourceLimitExceeded
        } else {
            JsonExProtocolError::MalformedJson
        }
    })?;
    deserializer
        .end()
        .map_err(|_| JsonExProtocolError::MalformedJson)?;
    Ok(value)
}

fn validate_ledger_metadata(value: &StrictJson) -> Result<(), JsonExProtocolError> {
    let metadata = required_object(value)?;
    require_allowed_keys(
        metadata,
        &["is_mst_dep_type", "mst_dep_type"],
        JsonExProtocolError::ProfileMismatch,
    )?;
    if !required_bool(metadata, "is_mst_dep_type")?
        || required_string(metadata, "mst_dep_type")? != "8"
    {
        return Err(JsonExProtocolError::ProfileMismatch);
    }
    Ok(())
}

fn parse_ledger_record(
    value: &StrictJson,
) -> Result<UnboundJsonExLedgerRecord, JsonExProtocolError> {
    let record = required_object(value)?;
    require_allowed_keys(
        record,
        &[
            "metadata",
            "parent",
            "ledgerphone",
            "ledgercontact",
            "closingbalance",
            "onaccountvalue",
            "tbalopening",
            "closingonacctvalue",
            "closingdronacctvalue",
            "ledopeningbalance",
            "languagename",
        ],
        JsonExProtocolError::ProfileMismatch,
    )?;
    let metadata = required_object(required_field(record, "metadata")?)?;
    require_allowed_keys(
        metadata,
        &["type", "name", "reservedname"],
        JsonExProtocolError::ProfileMismatch,
    )?;
    if required_string(metadata, "type")? != "Ledger" {
        return Err(JsonExProtocolError::ProfileMismatch);
    }
    let name = required_string(metadata, "name")?.to_string();
    required_string(metadata, "reservedname")?;
    let parent = typed_text_presence(record, "parent", "String", TextRule::Any)?;
    typed_text_presence(record, "ledgerphone", "String", TextRule::Any)?;
    typed_text_presence(record, "ledgercontact", "String", TextRule::Any)?;
    let closing_balance = typed_text_presence(
        record,
        "closingbalance",
        "Amount",
        TextRule::OptionalExactDecimal,
    )?;
    typed_text_presence(
        record,
        "onaccountvalue",
        "Amount",
        TextRule::OptionalExactDecimal,
    )?;
    typed_text_presence(
        record,
        "tbalopening",
        "Amount",
        TextRule::OptionalExactDecimal,
    )?;
    typed_text_presence(
        record,
        "closingonacctvalue",
        "Amount",
        TextRule::OptionalExactDecimal,
    )?;
    typed_logical_optional(record, "closingdronacctvalue")?;
    let opening_balance = typed_text_presence(
        record,
        "ledopeningbalance",
        "Amount",
        TextRule::OptionalExactDecimal,
    )?;
    let language_names = match record.get("languagename") {
        None => Vec::new(),
        Some(value) => parse_language_names(value)?,
    };
    Ok(UnboundJsonExLedgerRecord {
        name,
        parent,
        closing_balance,
        opening_balance,
        language_names,
    })
}

fn parse_language_names(value: &StrictJson) -> Result<Vec<Vec<String>>, JsonExProtocolError> {
    as_array(value)
        .ok_or(JsonExProtocolError::ProfileMismatch)?
        .iter()
        .map(|entry| {
            let entry = required_object(entry)?;
            require_allowed_keys(
                entry,
                &["name", "languageid"],
                JsonExProtocolError::ProfileMismatch,
            )?;
            if let Some(language_id) = entry.get("languageid") {
                typed_text_value(language_id, "Number", TextRule::UnsignedInteger)?;
            }
            let names = as_array(required_field(entry, "name")?)
                .ok_or(JsonExProtocolError::ProfileMismatch)?;
            if names.len() < 2 {
                return Err(JsonExProtocolError::ProfileMismatch);
            }
            let descriptor = required_object(&names[0])?;
            require_allowed_keys(
                descriptor,
                &["metadata", "type"],
                JsonExProtocolError::ProfileMismatch,
            )?;
            if !required_bool(descriptor, "metadata")?
                || required_string(descriptor, "type")? != "String"
            {
                return Err(JsonExProtocolError::ProfileMismatch);
            }
            names[1..]
                .iter()
                .map(|name| {
                    as_str(name)
                        .map(str::to_string)
                        .ok_or(JsonExProtocolError::TypedValueMismatch)
                })
                .collect()
        })
        .collect()
}

fn validate_voucher_metadata(value: &StrictJson) -> Result<(), JsonExProtocolError> {
    let metadata = required_object(value)?;
    require_allowed_keys(
        metadata,
        &["is_cmp_dep_type", "cmp_locus", "cmp_dep_type"],
        JsonExProtocolError::ProfileMismatch,
    )?;
    if !required_bool(metadata, "is_cmp_dep_type")?
        || required_unsigned(metadata, "cmp_locus")? != 4
        || required_unsigned(metadata, "cmp_dep_type")? != 64
    {
        return Err(JsonExProtocolError::ProfileMismatch);
    }
    Ok(())
}

fn parse_voucher_record(
    value: &StrictJson,
) -> Result<UnboundJsonExVoucherRecord, JsonExProtocolError> {
    let record = required_object(value)?;
    require_allowed_keys(
        record,
        &[
            "metadata",
            "date",
            "guid",
            "vouchertypename",
            "vouchernumber",
            "reference",
            "serialmaster",
            "areserialmaster",
            "numberingstyle",
            "persistedview",
            "isdeleted",
            "asoriginal",
            "isdeemedpositive",
            "isinvoice",
            "aspayslip",
            "isdeletedvchretained",
            "isnegisposset",
            "masterid",
            "voucherkey",
            "voucherretainkey",
            "reuseholeid",
            "amount",
            "vouchernumberseries",
            "allledgerentries",
            "allinventoryentries",
            "ledgerentries",
        ],
        JsonExProtocolError::ProfileMismatch,
    )?;
    let metadata = required_object(required_field(record, "metadata")?)?;
    require_allowed_keys(
        metadata,
        &["type", "remoteid", "vchkey", "vchtype", "objview"],
        JsonExProtocolError::ProfileMismatch,
    )?;
    if required_string(metadata, "type")? != "Voucher" {
        return Err(JsonExProtocolError::ProfileMismatch);
    }
    let remote_id = required_string(metadata, "remoteid")?.to_string();
    required_string(metadata, "vchkey")?;
    required_string(metadata, "vchtype")?;
    required_string(metadata, "objview")?;
    let date_yyyymmdd = typed_required_text(record, "date", "Date", TextRule::Date)?.to_string();
    let guid = required_string(record, "guid")?.to_string();
    let voucher_type = required_string(record, "vouchertypename")?.to_string();
    let voucher_number = optional_string(record, "vouchernumber")?.map(str::to_string);
    for field in ["reference", "serialmaster", "areserialmaster"] {
        typed_text_presence(record, field, "String", TextRule::Any)?;
    }
    for field in ["numberingstyle", "persistedview"] {
        optional_string(record, field)?;
    }
    typed_text_presence(record, "vouchernumberseries", "String", TextRule::Any)?;
    for field in [
        "isdeleted",
        "asoriginal",
        "isinvoice",
        "aspayslip",
        "isdeletedvchretained",
    ] {
        optional_bool(record, field)?;
    }
    typed_logical_optional(record, "isdeemedpositive")?;
    typed_logical_optional(record, "isnegisposset")?;
    for field in ["masterid", "voucherkey", "voucherretainkey", "reuseholeid"] {
        typed_text_presence(record, field, "Number", TextRule::UnsignedInteger)?;
    }
    let amount = typed_text_presence(record, "amount", "Amount", TextRule::OptionalExactDecimal)?;

    let all_ledger_entry_count = validate_object_array(record, "allledgerentries", |entry| {
        validate_ledger_allocation(entry, false)?;
        Ok(())
    })?;
    let mut batch_allocation_count = 0_u64;
    let mut accounting_allocation_count = 0_u64;
    let inventory_entry_count = validate_object_array(record, "allinventoryentries", |entry| {
        let (batches, accounting) = validate_inventory_allocation(entry)?;
        batch_allocation_count = batch_allocation_count
            .checked_add(batches)
            .ok_or(JsonExProtocolError::ResourceLimitExceeded)?;
        accounting_allocation_count = accounting_allocation_count
            .checked_add(accounting)
            .ok_or(JsonExProtocolError::ResourceLimitExceeded)?;
        Ok(())
    })?;
    let mut bill_allocation_count = 0_u64;
    let invoice_ledger_entry_count = validate_object_array(record, "ledgerentries", |entry| {
        bill_allocation_count = bill_allocation_count
            .checked_add(validate_ledger_allocation(entry, true)?)
            .ok_or(JsonExProtocolError::ResourceLimitExceeded)?;
        Ok(())
    })?;

    Ok(UnboundJsonExVoucherRecord {
        remote_id,
        guid,
        date_yyyymmdd,
        voucher_type,
        voucher_number,
        amount,
        all_ledger_entry_count,
        inventory_entry_count,
        invoice_ledger_entry_count,
        batch_allocation_count,
        accounting_allocation_count,
        bill_allocation_count,
    })
}

fn validate_ledger_allocation(
    value: &StrictJson,
    allow_bills: bool,
) -> Result<u64, JsonExProtocolError> {
    let entry = required_object(value)?;
    let allowed = if allow_bills {
        &[
            "ledgername",
            "isdeemedpositive",
            "islastdeemedpositive",
            "amount",
            "vatassessablevalue",
            "billallocations",
        ][..]
    } else {
        &[
            "ledgername",
            "isdeemedpositive",
            "islastdeemedpositive",
            "amount",
            "vatassessablevalue",
        ][..]
    };
    require_allowed_keys(entry, allowed, JsonExProtocolError::ProfileMismatch)?;
    typed_required_text(entry, "ledgername", "String", TextRule::NonEmptyText)?;
    typed_required_logical(entry, "isdeemedpositive")?;
    typed_required_logical(entry, "islastdeemedpositive")?;
    typed_required_text(entry, "amount", "Amount", TextRule::ExactDecimal)?;
    typed_text_presence(
        entry,
        "vatassessablevalue",
        "Amount",
        TextRule::OptionalExactDecimal,
    )?;
    if allow_bills {
        validate_object_array(entry, "billallocations", |allocation| {
            let allocation = required_object(allocation)?;
            require_allowed_keys(
                allocation,
                &["amount"],
                JsonExProtocolError::ProfileMismatch,
            )?;
            typed_required_text(allocation, "amount", "Amount", TextRule::ExactDecimal)?;
            Ok(())
        })
    } else {
        Ok(0)
    }
}

fn validate_inventory_allocation(value: &StrictJson) -> Result<(u64, u64), JsonExProtocolError> {
    let entry = required_object(value)?;
    require_allowed_keys(
        entry,
        &[
            "stockitemname",
            "addlamount",
            "isdeemedpositive",
            "islastdeemedpositive",
            "rate",
            "discount",
            "amount",
            "actualqty",
            "billedqty",
            "batchallocations",
            "accountingallocations",
        ],
        JsonExProtocolError::ProfileMismatch,
    )?;
    typed_required_text(entry, "stockitemname", "String", TextRule::NonEmptyText)?;
    typed_text_presence(
        entry,
        "addlamount",
        "Amount",
        TextRule::OptionalExactDecimal,
    )?;
    typed_required_logical(entry, "isdeemedpositive")?;
    typed_required_logical(entry, "islastdeemedpositive")?;
    typed_required_text(entry, "rate", "Rate", TextRule::NonEmpty)?;
    typed_required_text(entry, "discount", "Number", TextRule::UnsignedInteger)?;
    typed_required_text(entry, "amount", "Amount", TextRule::ExactDecimal)?;
    typed_required_text(entry, "actualqty", "Quantity", TextRule::NonEmpty)?;
    typed_required_text(entry, "billedqty", "Quantity", TextRule::NonEmpty)?;
    let batches = validate_object_array(entry, "batchallocations", validate_batch_allocation)?;
    let accounting = validate_object_array(
        entry,
        "accountingallocations",
        validate_accounting_allocation,
    )?;
    Ok((batches, accounting))
}

fn validate_batch_allocation(value: &StrictJson) -> Result<(), JsonExProtocolError> {
    let allocation = required_object(value)?;
    require_allowed_keys(
        allocation,
        &[
            "batchname",
            "indentno",
            "orderno",
            "trackingnumber",
            "addlamount",
            "batchdiscount",
            "amount",
            "actualqty",
            "billedqty",
            "batchrate",
        ],
        JsonExProtocolError::ProfileMismatch,
    )?;
    for field in ["batchname", "indentno", "orderno", "trackingnumber"] {
        typed_required_text(allocation, field, "String", TextRule::Any)?;
    }
    typed_text_presence(
        allocation,
        "addlamount",
        "Amount",
        TextRule::OptionalExactDecimal,
    )?;
    typed_required_text(
        allocation,
        "batchdiscount",
        "Number",
        TextRule::UnsignedInteger,
    )?;
    typed_required_text(allocation, "amount", "Amount", TextRule::ExactDecimal)?;
    typed_required_text(allocation, "actualqty", "Quantity", TextRule::NonEmpty)?;
    typed_required_text(allocation, "billedqty", "Quantity", TextRule::NonEmpty)?;
    typed_required_text(allocation, "batchrate", "Rate", TextRule::NonEmpty)?;
    Ok(())
}

fn validate_accounting_allocation(value: &StrictJson) -> Result<(), JsonExProtocolError> {
    let allocation = required_object(value)?;
    require_allowed_keys(
        allocation,
        &[
            "ledgername",
            "isdeemedpositive",
            "islastdeemedpositive",
            "amount",
        ],
        JsonExProtocolError::ProfileMismatch,
    )?;
    typed_required_text(allocation, "ledgername", "String", TextRule::NonEmptyText)?;
    typed_required_logical(allocation, "isdeemedpositive")?;
    typed_required_logical(allocation, "islastdeemedpositive")?;
    typed_required_text(allocation, "amount", "Amount", TextRule::ExactDecimal)?;
    Ok(())
}

fn validate_object_array<F>(
    object: &BTreeMap<String, StrictJson>,
    field: &str,
    mut validate: F,
) -> Result<u64, JsonExProtocolError>
where
    F: FnMut(&StrictJson) -> Result<(), JsonExProtocolError>,
{
    let Some(value) = object.get(field) else {
        return Ok(0);
    };
    let values = as_array(value).ok_or(JsonExProtocolError::ProfileMismatch)?;
    for value in values {
        if !matches!(value, StrictJson::Object(_)) {
            return Err(JsonExProtocolError::ProfileMismatch);
        }
        validate(value)?;
    }
    u64::try_from(values.len()).map_err(|_| JsonExProtocolError::ResourceLimitExceeded)
}

#[derive(Clone, Copy)]
enum TextRule {
    Any,
    NonEmpty,
    NonEmptyText,
    ExactDecimal,
    OptionalExactDecimal,
    UnsignedInteger,
    Date,
}

fn typed_text_presence(
    object: &BTreeMap<String, StrictJson>,
    field: &str,
    expected_type: &str,
    rule: TextRule,
) -> Result<JsonExTextPresence, JsonExProtocolError> {
    match object.get(field) {
        None => Ok(JsonExTextPresence::Absent),
        Some(value) => {
            let value = typed_text_value(value, expected_type, rule)?;
            if value.is_empty() {
                Ok(JsonExTextPresence::Empty)
            } else {
                Ok(JsonExTextPresence::Value(value.to_string()))
            }
        }
    }
}

fn typed_required_text<'a>(
    object: &'a BTreeMap<String, StrictJson>,
    field: &str,
    expected_type: &str,
    rule: TextRule,
) -> Result<&'a str, JsonExProtocolError> {
    typed_text_value(required_field(object, field)?, expected_type, rule)
}

fn typed_text_value<'a>(
    value: &'a StrictJson,
    expected_type: &str,
    rule: TextRule,
) -> Result<&'a str, JsonExProtocolError> {
    let wrapper = required_object(value)?;
    require_allowed_keys(
        wrapper,
        &["type", "value"],
        JsonExProtocolError::TypedValueMismatch,
    )?;
    if required_string(wrapper, "type")? != expected_type {
        return Err(JsonExProtocolError::TypedValueMismatch);
    }
    let value = required_string(wrapper, "value")?;
    let valid = match rule {
        TextRule::Any => true,
        TextRule::NonEmpty => !value.is_empty(),
        TextRule::NonEmptyText => !value.is_empty() && !value.chars().any(char::is_control),
        TextRule::ExactDecimal => is_exact_decimal(value),
        TextRule::OptionalExactDecimal => value.is_empty() || is_exact_decimal(value),
        TextRule::UnsignedInteger => is_unsigned_integer(value.trim_matches(' ')),
        TextRule::Date => is_valid_yyyymmdd(value),
    };
    if !valid {
        return Err(JsonExProtocolError::TypedValueMismatch);
    }
    Ok(value)
}

fn typed_logical_optional(
    object: &BTreeMap<String, StrictJson>,
    field: &str,
) -> Result<Option<bool>, JsonExProtocolError> {
    object
        .get(field)
        .map(|value| typed_logical_value(value).map(Some))
        .unwrap_or(Ok(None))
}

fn typed_required_logical(
    object: &BTreeMap<String, StrictJson>,
    field: &str,
) -> Result<bool, JsonExProtocolError> {
    typed_logical_value(required_field(object, field)?)
}

fn typed_logical_value(value: &StrictJson) -> Result<bool, JsonExProtocolError> {
    let wrapper = required_object(value)?;
    require_allowed_keys(
        wrapper,
        &["type", "value"],
        JsonExProtocolError::TypedValueMismatch,
    )?;
    if required_string(wrapper, "type")? != "Logical" {
        return Err(JsonExProtocolError::TypedValueMismatch);
    }
    as_bool(required_field(wrapper, "value")?).ok_or(JsonExProtocolError::TypedValueMismatch)
}

fn is_exact_decimal(value: &str) -> bool {
    let bytes = value.as_bytes();
    let body = bytes.strip_prefix(b"-").unwrap_or(bytes);
    let mut parts = body.split(|byte| *byte == b'.');
    let whole = parts.next().unwrap_or_default();
    let fraction = parts.next();
    !whole.is_empty()
        && whole.iter().all(u8::is_ascii_digit)
        && fraction.is_none_or(|part| !part.is_empty() && part.iter().all(u8::is_ascii_digit))
        && parts.next().is_none()
}

fn is_unsigned_integer(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_valid_yyyymmdd(value: &str) -> bool {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let year = value[0..4].parse::<u32>().unwrap_or_default();
    let month = value[4..6].parse::<u32>().unwrap_or_default();
    let day = value[6..8].parse::<u32>().unwrap_or_default();
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    year != 0 && (1..=days).contains(&day)
}

fn require_allowed_keys(
    object: &BTreeMap<String, StrictJson>,
    allowed: &[&str],
    error: JsonExProtocolError,
) -> Result<(), JsonExProtocolError> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(error);
    }
    Ok(())
}

fn required_field<'a>(
    object: &'a BTreeMap<String, StrictJson>,
    field: &str,
) -> Result<&'a StrictJson, JsonExProtocolError> {
    object
        .get(field)
        .ok_or(JsonExProtocolError::ProfileMismatch)
}

fn required_object(
    value: &StrictJson,
) -> Result<&BTreeMap<String, StrictJson>, JsonExProtocolError> {
    as_object(value).ok_or(JsonExProtocolError::ProfileMismatch)
}

fn required_string<'a>(
    object: &'a BTreeMap<String, StrictJson>,
    field: &str,
) -> Result<&'a str, JsonExProtocolError> {
    as_str(required_field(object, field)?).ok_or(JsonExProtocolError::TypedValueMismatch)
}

fn optional_string<'a>(
    object: &'a BTreeMap<String, StrictJson>,
    field: &str,
) -> Result<Option<&'a str>, JsonExProtocolError> {
    object
        .get(field)
        .map(|value| as_str(value).ok_or(JsonExProtocolError::TypedValueMismatch))
        .transpose()
}

fn required_bool(
    object: &BTreeMap<String, StrictJson>,
    field: &str,
) -> Result<bool, JsonExProtocolError> {
    as_bool(required_field(object, field)?).ok_or(JsonExProtocolError::TypedValueMismatch)
}

fn optional_bool(
    object: &BTreeMap<String, StrictJson>,
    field: &str,
) -> Result<Option<bool>, JsonExProtocolError> {
    object
        .get(field)
        .map(|value| as_bool(value).ok_or(JsonExProtocolError::TypedValueMismatch))
        .transpose()
}

fn required_unsigned(
    object: &BTreeMap<String, StrictJson>,
    field: &str,
) -> Result<u64, JsonExProtocolError> {
    match required_field(object, field)? {
        StrictJson::Unsigned(value) => Ok(*value),
        StrictJson::Signed(value) if *value >= 0 => Ok(*value as u64),
        _ => Err(JsonExProtocolError::TypedValueMismatch),
    }
}

fn as_object(value: &StrictJson) -> Option<&BTreeMap<String, StrictJson>> {
    match value {
        StrictJson::Object(value) => Some(value),
        _ => None,
    }
}

fn as_array(value: &StrictJson) -> Option<&[StrictJson]> {
    match value {
        StrictJson::Array(value) => Some(value),
        _ => None,
    }
}

fn as_str(value: &StrictJson) -> Option<&str> {
    match value {
        StrictJson::String(value) => Some(value),
        _ => None,
    }
}

fn as_bool(value: &StrictJson) -> Option<bool> {
    match value {
        StrictJson::Bool(value) => Some(*value),
        _ => None,
    }
}

fn into_object(value: StrictJson) -> Option<BTreeMap<String, StrictJson>> {
    match value {
        StrictJson::Object(value) => Some(value),
        _ => None,
    }
}

fn into_array(value: StrictJson) -> Option<Vec<StrictJson>> {
    match value {
        StrictJson::Array(value) => Some(value),
        _ => None,
    }
}
