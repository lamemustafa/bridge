use std::{borrow::Cow, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductStatus {
    TallyPrime,
    TallyErp9,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fixture {
    ProductStatus(ProductStatus),
    ExportStatusOne,
    ExportStatusZero,
    ExportStatusMissing,
    ExportStatusInvalid,
    NormalExport,
    EmptyExport,
    DuplicateIdentity,
    WrongCompany,
    VoucherExport,
    InconsistentDateFilter,
    RecordCountMismatch,
    MalformedExportMetadata,
    DuplicateExportMetadata,
    ExactDecimals,
    ImportCounters,
    ImportDuplicate,
    ImportPartial,
    MalformedXml,
    TruncatedXml,
    /// Bridge-shaped JSON used only to test semantic-reference plumbing. It
    /// is not an official Tally JSONEX response envelope or a parity fixture.
    SyntheticJsonSemanticReference,
    UnsupportedCapability,
    /// Caller-owned synthetic XML for qualification runner and transport tests.
    /// The simulator never treats this as a real Tally grammar fixture.
    SyntheticXml(String),
    Oversized {
        minimum_bytes: usize,
    },
}

impl Fixture {
    pub fn body(&self) -> Cow<'_, str> {
        match self {
            Self::ProductStatus(ProductStatus::TallyPrime) => {
                Cow::Borrowed("<RESPONSE>TallyPrime Server is Running</RESPONSE>")
            }
            Self::ProductStatus(ProductStatus::TallyErp9) => {
                Cow::Borrowed("<RESPONSE>Tally ERP 9 Server is Running</RESPONSE>")
            }
            Self::ProductStatus(ProductStatus::Unknown) => {
                Cow::Borrowed("<RESPONSE>BRIDGE SYNTHETIC UNKNOWN PRODUCT</RESPONSE>")
            }
            Self::ExportStatusOne => {
                Cow::Borrowed(include_str!("../fixtures/export_status_1.xml"))
            }
            Self::ExportStatusZero => {
                Cow::Borrowed(include_str!("../fixtures/export_status_0.xml"))
            }
            Self::ExportStatusMissing => {
                Cow::Borrowed(include_str!("../fixtures/export_status_missing.xml"))
            }
            Self::ExportStatusInvalid => {
                Cow::Borrowed(include_str!("../fixtures/export_status_invalid.xml"))
            }
            Self::NormalExport => Cow::Borrowed(include_str!("../fixtures/normal_export.xml")),
            Self::EmptyExport => Cow::Borrowed(include_str!("../fixtures/empty_export.xml")),
            Self::DuplicateIdentity => {
                Cow::Borrowed(include_str!("../fixtures/duplicate_identity.xml"))
            }
            Self::WrongCompany => Cow::Borrowed(include_str!("../fixtures/wrong_company.xml")),
            Self::VoucherExport => Cow::Borrowed(include_str!("../fixtures/voucher_export.xml")),
            Self::InconsistentDateFilter => Cow::Borrowed(include_str!(
                "../fixtures/inconsistent_date_filter.xml"
            )),
            Self::RecordCountMismatch => {
                Cow::Borrowed(include_str!("../fixtures/record_count_mismatch.xml"))
            }
            Self::MalformedExportMetadata => Cow::Borrowed(include_str!(
                "../fixtures/malformed_export_metadata.xml"
            )),
            Self::DuplicateExportMetadata => Cow::Borrowed(include_str!(
                "../fixtures/duplicate_export_metadata.xml"
            )),
            Self::ExactDecimals => {
                Cow::Borrowed(include_str!("../fixtures/exact_decimals.xml"))
            }
            Self::ImportCounters => {
                Cow::Borrowed(include_str!("../fixtures/import_counters.xml"))
            }
            Self::ImportDuplicate => {
                Cow::Borrowed(include_str!("../fixtures/import_duplicate.xml"))
            }
            Self::ImportPartial => {
                Cow::Borrowed(include_str!("../fixtures/import_partial.xml"))
            }
            Self::MalformedXml => Cow::Borrowed(include_str!("../fixtures/malformed.xml")),
            Self::TruncatedXml => Cow::Borrowed(include_str!("../fixtures/truncated.xml")),
            Self::SyntheticJsonSemanticReference => {
                Cow::Borrowed(include_str!("../fixtures/synthetic_json_semantic_reference.json"))
            }
            Self::UnsupportedCapability => Cow::Borrowed(
                "<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>0</STATUS></HEADER><BODY><DATA><LINEERROR>BRIDGE_SYNTHETIC_CAPABILITY_UNSUPPORTED</LINEERROR></DATA></BODY></ENVELOPE>",
            ),
            Self::SyntheticXml(xml) => Cow::Borrowed(xml.as_str()),
            Self::Oversized { minimum_bytes } => {
                let prefix = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA>";
                let suffix = "</DATA></BODY></ENVELOPE>";
                let padding = minimum_bytes.saturating_sub(prefix.len() + suffix.len());
                Cow::Owned(format!("{prefix}{}{suffix}", "X".repeat(padding)))
            }
        }
    }

    pub fn content_type(&self) -> &'static str {
        if matches!(self, Self::SyntheticJsonSemanticReference) {
            "application/json"
        } else {
            "text/xml"
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    Immediate,
    SlowHeaders(Duration),
    SlowBody { chunk_bytes: usize, delay: Duration },
    ResetBeforeBody,
    ResetAfterRequestProcessed { delay: Duration },
}

/// Synthetic HTTP response framing. These modes characterize Bridge's client
/// resilience; they are not claims about framing guaranteed by Tally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseFraming {
    ContentLength,
    ConnectionClose,
    Chunked { chunk_bytes: usize },
    DeclaredContentLength { bytes: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseContentEncoding {
    None,
    Identity,
    Gzip,
    DuplicateIdentityThenGzip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioPlan {
    pub fixture: Fixture,
    pub encoding: WireEncoding,
    pub delivery: Delivery,
    pub http_status: u16,
    pub framing: ResponseFraming,
    pub content_encoding: ResponseContentEncoding,
    pub redirect_location: Option<String>,
}

impl ScenarioPlan {
    pub fn new(fixture: Fixture) -> Self {
        Self {
            fixture,
            encoding: WireEncoding::Utf8,
            delivery: Delivery::Immediate,
            http_status: 200,
            framing: ResponseFraming::ContentLength,
            content_encoding: ResponseContentEncoding::None,
            redirect_location: None,
        }
    }

    pub fn with_encoding(mut self, encoding: WireEncoding) -> Self {
        self.encoding = encoding;
        self
    }

    pub fn with_delivery(mut self, delivery: Delivery) -> Self {
        self.delivery = delivery;
        self
    }

    pub fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = status;
        self
    }

    pub fn with_framing(mut self, framing: ResponseFraming) -> Self {
        self.framing = framing;
        self
    }

    pub fn with_content_encoding(mut self, encoding: ResponseContentEncoding) -> Self {
        self.content_encoding = encoding;
        self
    }

    pub fn with_redirect_location(mut self, location: impl Into<String>) -> Self {
        self.redirect_location = Some(location.into());
        self
    }

    pub fn response_bytes(&self) -> Vec<u8> {
        encode(self.fixture.body().as_ref(), self.encoding)
    }
}

pub fn encode(text: &str, encoding: WireEncoding) -> Vec<u8> {
    match encoding {
        WireEncoding::Utf8 => text.as_bytes().to_vec(),
        WireEncoding::Utf8Bom => [b"\xEF\xBB\xBF".as_slice(), text.as_bytes()].concat(),
        WireEncoding::Utf16Le => {
            let mut bytes = vec![0xFF, 0xFE];
            bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            bytes
        }
        WireEncoding::Utf16Be => {
            let mut bytes = vec![0xFE, 0xFF];
            bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
            bytes
        }
    }
}

pub fn decode(bytes: &[u8]) -> Result<String, &'static str> {
    if let Some(payload) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(payload.to_vec()).map_err(|_| "invalid_utf8");
    }
    if let Some(payload) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        if payload.len() % 2 != 0 {
            return Err("invalid_utf16le");
        }
        let units = payload
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units).map_err(|_| "invalid_utf16le");
    }
    if let Some(payload) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        if payload.len() % 2 != 0 {
            return Err("invalid_utf16be");
        }
        let units = payload
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        return String::from_utf16(&units).map_err(|_| "invalid_utf16be");
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "invalid_utf8")
}
