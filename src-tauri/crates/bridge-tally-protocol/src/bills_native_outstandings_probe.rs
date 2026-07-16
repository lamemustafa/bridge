//! Sealed candidate for observing Tally's native `Ledger Outstandings` report.
//!
//! This module deliberately provides no parser, transport integration, or
//! accounting authority. The request is available only through a non-default
//! feature and remains outside the production [`crate::xml_read_profiles::ReadOnlyProfile`].

use std::fmt;

use sha2::{Digest, Sha256};

const TEMPLATE_COMPANY: &str = "BRIDGE TEMPLATE COMPANY";
const TEMPLATE_LEDGER: &str = "BRIDGE TEMPLATE LEDGER";
const TEMPLATE_TO_DATE: &str = "20000101";
const SCOPE_HASH_DOMAIN: &[u8] = b"bridge.tally.native-ledger-outstandings.scope/1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeOutstandingsProbeValidationError {
    CompanyInvalid,
    LedgerInvalid,
    DateInvalid,
}

impl fmt::Display for NativeOutstandingsProbeValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::CompanyInvalid => "native outstandings probe company input was invalid",
            Self::LedgerInvalid => "native outstandings probe ledger input was invalid",
            Self::DateInvalid => "native outstandings probe date input was invalid",
        })
    }
}

impl std::error::Error for NativeOutstandingsProbeValidationError {}

#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedProbeCompanyName(String);

impl ValidatedProbeCompanyName {
    pub fn new(value: impl Into<String>) -> Result<Self, NativeOutstandingsProbeValidationError> {
        let value = value.into();
        if !valid_name(&value) {
            return Err(NativeOutstandingsProbeValidationError::CompanyInvalid);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ValidatedProbeCompanyName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedProbeCompanyName([redacted])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedProbeLedgerName(String);

impl ValidatedProbeLedgerName {
    pub fn new(value: impl Into<String>) -> Result<Self, NativeOutstandingsProbeValidationError> {
        let value = value.into();
        if !valid_name(&value) {
            return Err(NativeOutstandingsProbeValidationError::LedgerInvalid);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ValidatedProbeLedgerName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedProbeLedgerName([redacted])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedProbeToDate(String);

impl ValidatedProbeToDate {
    pub fn new(value: impl Into<String>) -> Result<Self, NativeOutstandingsProbeValidationError> {
        let value = value.into();
        if !valid_yyyymmdd(&value) {
            return Err(NativeOutstandingsProbeValidationError::DateInvalid);
        }
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ValidatedProbeToDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedProbeToDate([redacted])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeOutstandingsObservationPosture {
    Unobserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeOutstandingsDispatchPosture {
    CandidateOnlyNoTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeOutstandingsProbeProfileId {
    LedgerOutstandingsCandidateV0,
}

impl NativeOutstandingsProbeProfileId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LedgerOutstandingsCandidateV0 => "native_ledger_outstandings_candidate_v0",
        }
    }

    pub fn template_sha256(self) -> String {
        match self {
            Self::LedgerOutstandingsCandidateV0 => sha256_hex(
                render_ledger_outstandings(TEMPLATE_COMPANY, TEMPLATE_LEDGER, TEMPLATE_TO_DATE)
                    .as_bytes(),
            ),
        }
    }
}

/// Exact native-report scope. Values are retained only to seal the request and
/// compute its scope commitment; `Debug` never exposes them.
#[derive(Clone, PartialEq, Eq)]
pub struct NativeLedgerOutstandingsProbeScope {
    company: ValidatedProbeCompanyName,
    ledger: ValidatedProbeLedgerName,
    to_date: ValidatedProbeToDate,
}

impl NativeLedgerOutstandingsProbeScope {
    pub fn new(
        company: ValidatedProbeCompanyName,
        ledger: ValidatedProbeLedgerName,
        to_date: ValidatedProbeToDate,
    ) -> Self {
        Self {
            company,
            ledger,
            to_date,
        }
    }

    pub fn seal(&self) -> SealedNativeLedgerOutstandingsProbe {
        let rendered_xml = render_ledger_outstandings(
            self.company.as_str(),
            self.ledger.as_str(),
            self.to_date.as_str(),
        );
        SealedNativeLedgerOutstandingsProbe {
            rendered_xml_sha256: sha256_hex(rendered_xml.as_bytes()),
            scope_sha256: scope_sha256(
                self.company.as_str(),
                self.ledger.as_str(),
                self.to_date.as_str(),
            ),
            rendered_xml,
        }
    }
}

impl fmt::Debug for NativeLedgerOutstandingsProbeScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeLedgerOutstandingsProbeScope")
            .field("company", &"[redacted]")
            .field("ledger", &"[redacted]")
            .field("to_date", &"[redacted]")
            .field(
                "observation_posture",
                &NativeOutstandingsObservationPosture::Unobserved,
            )
            .field(
                "dispatch_posture",
                &NativeOutstandingsDispatchPosture::CandidateOnlyNoTransport,
            )
            .finish()
    }
}

/// Immutable request candidate. Nothing in this crate can dispatch it.
#[derive(Clone, PartialEq, Eq)]
pub struct SealedNativeLedgerOutstandingsProbe {
    rendered_xml: String,
    rendered_xml_sha256: String,
    scope_sha256: String,
}

impl SealedNativeLedgerOutstandingsProbe {
    pub fn profile_id(&self) -> NativeOutstandingsProbeProfileId {
        NativeOutstandingsProbeProfileId::LedgerOutstandingsCandidateV0
    }

    pub fn observation_posture(&self) -> NativeOutstandingsObservationPosture {
        NativeOutstandingsObservationPosture::Unobserved
    }

    pub fn dispatch_posture(&self) -> NativeOutstandingsDispatchPosture {
        NativeOutstandingsDispatchPosture::CandidateOnlyNoTransport
    }

    pub fn template_sha256(&self) -> String {
        self.profile_id().template_sha256()
    }

    pub fn request_sha256(&self) -> &str {
        &self.rendered_xml_sha256
    }

    pub fn scope_sha256(&self) -> &str {
        &self.scope_sha256
    }

    /// Exact candidate bytes for offline review and golden testing. No Bridge
    /// transport accepts this type in this feature.
    pub fn rendered_xml(&self) -> &str {
        &self.rendered_xml
    }
}

impl fmt::Debug for SealedNativeLedgerOutstandingsProbe {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SealedNativeLedgerOutstandingsProbe")
            .field("profile_id", &self.profile_id())
            .field("observation_posture", &self.observation_posture())
            .field("dispatch_posture", &self.dispatch_posture())
            .field("request_present", &!self.rendered_xml.is_empty())
            .finish()
    }
}

fn render_ledger_outstandings(company: &str, ledger: &str, to_date: &str) -> String {
    format!(
        r#"
<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>Export</TALLYREQUEST>
        <TYPE>Data</TYPE>
        <ID>Ledger Outstandings</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY>
                <LedgerName>{}</LedgerName>
                <SVTODATE TYPE="Date">{}</SVTODATE>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
                <EXPLODEFLAG>Yes</EXPLODEFLAG>
            </STATICVARIABLES>
        </DESC>
    </BODY>
</ENVELOPE>
"#,
        xml_escape(company),
        xml_escape(ledger),
        xml_escape(to_date),
    )
    .trim()
    .to_string()
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.trim() == value
        && !value.chars().any(char::is_control)
        && value.chars().all(valid_xml_1_0_scalar)
}

fn valid_xml_1_0_scalar(value: char) -> bool {
    matches!(value as u32, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

fn valid_yyyymmdd(value: &str) -> bool {
    if value.len() != 8 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let year = value[0..4].parse::<u16>().ok();
    let month = value[4..6].parse::<u8>().ok();
    let day = value[6..8].parse::<u8>().ok();
    let (Some(year), Some(month), Some(day)) = (year, month, day) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let maximum_day = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=maximum_day).contains(&day)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn scope_sha256(company: &str, ledger: &str, to_date: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(SCOPE_HASH_DOMAIN);
    hash_field(
        &mut digest,
        NativeOutstandingsProbeProfileId::LedgerOutstandingsCandidateV0
            .as_str()
            .as_bytes(),
    );
    hash_field(&mut digest, company.as_bytes());
    hash_field(&mut digest, ledger.as_bytes());
    hash_field(&mut digest, to_date.as_bytes());
    hex_lower(&digest.finalize())
}

fn hash_field(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(&Sha256::digest(bytes))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(company: &str, ledger: &str, to_date: &str) -> SealedNativeLedgerOutstandingsProbe {
        NativeLedgerOutstandingsProbeScope::new(
            ValidatedProbeCompanyName::new(company).unwrap(),
            ValidatedProbeLedgerName::new(ledger).unwrap(),
            ValidatedProbeToDate::new(to_date).unwrap(),
        )
        .seal()
    }

    #[test]
    fn golden_native_ledger_outstandings_request_is_exact() {
        let sealed = probe("BRIDGE SYNTHETIC BOOK", "BRIDGE PARTY", "20260402");
        assert_eq!(
            sealed.rendered_xml(),
            r#"<ENVELOPE>
    <HEADER>
        <VERSION>1</VERSION>
        <TALLYREQUEST>Export</TALLYREQUEST>
        <TYPE>Data</TYPE>
        <ID>Ledger Outstandings</ID>
    </HEADER>
    <BODY>
        <DESC>
            <STATICVARIABLES>
                <SVCURRENTCOMPANY>BRIDGE SYNTHETIC BOOK</SVCURRENTCOMPANY>
                <LedgerName>BRIDGE PARTY</LedgerName>
                <SVTODATE TYPE="Date">20260402</SVTODATE>
                <SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT>
                <EXPLODEFLAG>Yes</EXPLODEFLAG>
            </STATICVARIABLES>
        </DESC>
    </BODY>
</ENVELOPE>"#
        );
        assert_eq!(
            sealed.profile_id().as_str(),
            "native_ledger_outstandings_candidate_v0"
        );
        assert_eq!(
            sealed.observation_posture(),
            NativeOutstandingsObservationPosture::Unobserved
        );
        assert_eq!(
            sealed.dispatch_posture(),
            NativeOutstandingsDispatchPosture::CandidateOnlyNoTransport
        );
    }

    #[test]
    fn validation_rejects_ambiguous_names_and_invalid_dates() {
        for value in [
            "",
            " ",
            " leading",
            "trailing ",
            "line\nbreak",
            "\u{0}",
            "\u{fffe}",
            "\u{ffff}",
        ] {
            assert_eq!(
                ValidatedProbeCompanyName::new(value),
                Err(NativeOutstandingsProbeValidationError::CompanyInvalid)
            );
            assert_eq!(
                ValidatedProbeLedgerName::new(value),
                Err(NativeOutstandingsProbeValidationError::LedgerInvalid)
            );
        }
        assert_eq!(
            ValidatedProbeCompanyName::new("x".repeat(256)),
            Err(NativeOutstandingsProbeValidationError::CompanyInvalid)
        );
        for date in ["2026-04-02", "20260229", "20261301", "00000101"] {
            assert_eq!(
                ValidatedProbeToDate::new(date),
                Err(NativeOutstandingsProbeValidationError::DateInvalid)
            );
        }
        assert!(ValidatedProbeToDate::new("20240229").is_ok());
    }

    #[test]
    fn dynamic_values_are_escaped_and_cannot_inject_requests_or_variables() {
        let sealed = probe(
            "BRIDGE & <BOOK> \"Q\"",
            "X</LedgerName><TALLYREQUEST>Import</TALLYREQUEST><LedgerName>'",
            "20260402",
        );
        let xml = sealed.rendered_xml();
        assert!(xml.contains("BRIDGE &amp; &lt;BOOK&gt; &quot;Q&quot;"));
        assert!(xml.contains("&lt;/LedgerName&gt;&lt;TALLYREQUEST&gt;Import"));
        assert!(xml.contains("&apos;"));
        assert_eq!(xml.matches("<TALLYREQUEST>").count(), 1);
        assert_eq!(xml.matches("<LedgerName>").count(), 1);
        assert!(!xml.contains("<TALLYREQUEST>Import"));
    }

    #[test]
    fn candidate_contains_only_the_documented_read_only_native_surface() {
        let xml = probe("BRIDGE SYNTHETIC BOOK", "BRIDGE PARTY", "20260402")
            .rendered_xml()
            .to_ascii_uppercase();
        for required in [
            "<TALLYREQUEST>EXPORT</TALLYREQUEST>",
            "<TYPE>DATA</TYPE>",
            "<ID>LEDGER OUTSTANDINGS</ID>",
            "<SVCURRENTCOMPANY>",
            "<LEDGERNAME>",
            "<SVTODATE TYPE=\"DATE\">",
            "<SVEXPORTFORMAT>$$SYSNAME:XML</SVEXPORTFORMAT>",
            "<EXPLODEFLAG>YES</EXPLODEFLAG>",
        ] {
            assert!(xml.contains(required), "missing {required}");
        }
        for forbidden in [
            "IMPORT",
            "CREATE",
            "ALTER",
            "DELETE",
            "OBJECT",
            "COLLECTION",
        ] {
            assert!(!xml.contains(&format!("<TALLYREQUEST>{forbidden}")));
            assert!(!xml.contains(&format!("<TYPE>{forbidden}")));
        }
        assert!(!xml.contains("<TDL>"));
    }

    #[test]
    fn exact_request_template_and_scope_hashes_are_stable() {
        let sealed = probe("BRIDGE SYNTHETIC BOOK", "BRIDGE PARTY", "20260402");
        assert_eq!(
            sealed.template_sha256(),
            "bc3b87484adb9a10cc15f6c9042853bb1047278896bcf0f495b93e7e6b428526"
        );
        assert_eq!(
            sealed.request_sha256(),
            "e99eebe225f8b023fe55b4d151c0fd18315b61580df5d47945235a8a6bda3822"
        );
        assert_eq!(
            sealed.scope_sha256(),
            "0d6c4c4ee82e34025c2ce3084d64b309dc6676440b41c749b0c0ab3531efe399"
        );
        assert_eq!(
            sealed.request_sha256(),
            sha256_hex(sealed.rendered_xml().as_bytes())
        );

        let other_scope = probe("BRIDGE SYNTHETIC BOOK", "BRIDGE PARTY 2", "20260402");
        assert_ne!(sealed.request_sha256(), other_scope.request_sha256());
        assert_ne!(sealed.scope_sha256(), other_scope.scope_sha256());
        assert_eq!(sealed.template_sha256(), other_scope.template_sha256());
    }

    #[test]
    fn debug_output_redacts_scope_values_and_request_bytes() {
        let company = ValidatedProbeCompanyName::new("SECRET COMPANY").unwrap();
        let ledger = ValidatedProbeLedgerName::new("SECRET LEDGER").unwrap();
        let to_date = ValidatedProbeToDate::new("20260402").unwrap();
        let company_debug = format!("{company:?}");
        let ledger_debug = format!("{ledger:?}");
        let date_debug = format!("{to_date:?}");
        let sealed = NativeLedgerOutstandingsProbeScope::new(company, ledger, to_date).seal();
        let hashes = [
            sealed.template_sha256(),
            sealed.request_sha256().to_string(),
            sealed.scope_sha256().to_string(),
        ];
        for debug in [
            company_debug,
            ledger_debug,
            date_debug,
            format!("{sealed:?}"),
        ] {
            assert!(!debug.contains("SECRET"));
            assert!(!debug.contains("20260402"));
            assert!(!debug.contains("<ENVELOPE>"));
            for hash in &hashes {
                assert!(!debug.contains(hash));
            }
        }
    }
}
