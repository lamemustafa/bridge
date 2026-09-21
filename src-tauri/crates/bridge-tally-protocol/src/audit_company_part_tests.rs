use super::*;

const GUID: &str = "00000000-0000-4000-8000-00000000c0de";

/// A synthetic company part in the shape of a captured single-object company
/// export (licensed TallyPrime 7.1): the `CMPINFO` object counters, including
/// a `COMPANY` counter with no GUID, precede the one company definition.
fn part(company: &str) -> String {
    format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><STATUS>1</STATUS><PRODMAJORVER>1</PRODMAJORVER><PRODMINORVER>1</PRODMINORVER><PRODMAJORREL>7</PRODMAJORREL><PRODMINORREL>1</PRODMINORREL><PRODTYPE>5</PRODTYPE></HEADER><BODY><DESC><CMPINFO><COMPANY>0</COMPANY><GROUP>0</GROUP><LEDGER>0</LEDGER></CMPINFO></DESC><DATA><TALLYMESSAGE>{company}</TALLYMESSAGE></DATA></BODY></ENVELOPE>"#
    )
}

fn company(inner: &str) -> String {
    format!(
        r#"<COMPANY NAME="Bridge &amp; Synthetic Book" RESERVEDNAME="" REQNAME="Bridge &amp; Synthetic Book">{inner}</COMPANY>"#
    )
}

fn complete() -> String {
    format!(
        r#"<GUID TYPE="String">{}</GUID><BOOKSFROM TYPE="Date">20250401</BOOKSFROM><STARTINGFROM TYPE="Date">20250401</STARTINGFROM><ISINTEGRATED TYPE="Logical">Yes</ISINTEGRATED><ISINVENTORYON TYPE="Logical">Yes</ISINVENTORYON><NAME.LIST TYPE="String"><NAME>Bridge &amp; Synthetic Book</NAME></NAME.LIST>"#,
        GUID.to_ascii_uppercase()
    )
}

#[test]
fn a_captured_shape_part_is_admitted_by_guid_and_books_from() {
    let admitted = admit_audit_company_part(&part(&company(&complete())), GUID, "20250401")
        .expect("the synthetic part is admitted");
    assert_eq!(
        admitted,
        AuditCompanyPart {
            name: "Bridge & Synthetic Book".to_string(),
            guid: GUID.to_string(),
            books_from_yyyymmdd: "20250401".to_string(),
            is_integrated: Some("Yes".to_string()),
        }
    );
}

#[test]
fn the_part_must_be_the_verified_book_not_a_sibling_or_another_company() {
    let body = part(&company(&complete()));
    assert_eq!(
        admit_audit_company_part(&body, "00000000-0000-4000-8000-00000000beef", "20250401"),
        Err(AuditCompanyPartError::GuidMismatch)
    );
    // A year-split sibling keeps its parent's GUID; only BOOKSFROM differs.
    assert_eq!(
        admit_audit_company_part(&body, GUID, "20260401"),
        Err(AuditCompanyPartError::BooksFromMismatch)
    );
}

#[test]
fn exactly_one_company_definition_is_required() {
    let one = company(&complete());
    for (body, expected) in [
        (part(""), AuditCompanyPartError::NotExactlyOneCompany),
        (
            part(&format!("{one}{one}")),
            AuditCompanyPartError::NotExactlyOneCompany,
        ),
        (
            part(&one).replace(
                "</DATA>",
                &format!("<TALLYMESSAGE>{one}</TALLYMESSAGE></DATA>"),
            ),
            AuditCompanyPartError::NotExactlyOneCompany,
        ),
        (
            part(&format!("{one}<LEDGER NAME=\"x\"/>")),
            AuditCompanyPartError::NotExactlyOneCompany,
        ),
        // A company elsewhere (the CMPINFO counter) is never the part.
        (
            part("").replace(
                "<COMPANY>0</COMPANY>",
                &format!("<COMPANY>0</COMPANY>{one}"),
            ),
            AuditCompanyPartError::NotExactlyOneCompany,
        ),
    ] {
        assert_eq!(
            admit_audit_company_part(&body, GUID, "20250401"),
            Err(expected)
        );
    }
}

#[test]
fn identity_fields_occur_exactly_once_and_integration_at_most_once() {
    let whole = complete();
    for field in ["GUID", "BOOKSFROM", "ISINTEGRATED"] {
        let start = whole.find(&format!("<{field} ")).unwrap();
        let end = whole.find(&format!("</{field}>")).unwrap() + field.len() + 3;
        let element = &whole[start..end];
        let missing = whole.replacen(element, "", 1);
        let admitted = admit_audit_company_part(&part(&company(&missing)), GUID, "20250401");
        if field == "ISINTEGRATED" {
            // The stock test reports "unknown" for a missing tag.
            assert_eq!(admitted.unwrap().is_integrated, None);
        } else {
            assert_eq!(
                admitted,
                Err(AuditCompanyPartError::FieldMissingOrRepeated),
                "{field} missing"
            );
        }
        let repeated = format!("{whole}{element}");
        assert_eq!(
            admit_audit_company_part(&part(&company(&repeated)), GUID, "20250401"),
            Err(AuditCompanyPartError::FieldMissingOrRepeated),
            "{field} repeated"
        );
    }
    let nameless = company(&complete()).replacen(r#"NAME="Bridge &amp; Synthetic Book" "#, "", 1);
    assert_eq!(
        admit_audit_company_part(&part(&nameless), GUID, "20250401"),
        Err(AuditCompanyPartError::FieldMissingOrRepeated)
    );
    // A GUID nested deeper than the company's own children is not its GUID.
    let nested = complete().replacen(
        &format!(
            r#"<GUID TYPE="String">{}</GUID>"#,
            GUID.to_ascii_uppercase()
        ),
        &format!(r#"<X.LIST><GUID>{GUID}</GUID></X.LIST>"#),
        1,
    );
    assert_eq!(
        admit_audit_company_part(&part(&company(&nested)), GUID, "20250401"),
        Err(AuditCompanyPartError::FieldMissingOrRepeated)
    );
}

#[test]
fn an_empty_guid_never_matches() {
    let empty = complete().replacen(&GUID.to_ascii_uppercase(), "", 1);
    assert_eq!(
        admit_audit_company_part(&part(&company(&empty)), "", "20250401"),
        Err(AuditCompanyPartError::GuidMismatch)
    );
}

#[test]
fn a_failed_or_malformed_response_is_refused() {
    let body = part(&company(&complete()));
    assert_eq!(
        admit_audit_company_part(
            &body.replace("<STATUS>1</STATUS>", "<STATUS>0</STATUS>"),
            GUID,
            "20250401"
        ),
        Err(AuditCompanyPartError::StatusNotSuccess)
    );
    assert_eq!(
        admit_audit_company_part(&body.replace("<STATUS>1</STATUS>", ""), GUID, "20250401"),
        Err(AuditCompanyPartError::StatusNotSuccess)
    );
    assert_eq!(
        admit_audit_company_part(&body.replace("</ENVELOPE>", ""), GUID, "20250401"),
        Err(AuditCompanyPartError::Malformed)
    );
    assert_eq!(
        admit_audit_company_part(&format!("<!DOCTYPE x>{body}"), GUID, "20250401"),
        Err(AuditCompanyPartError::Malformed)
    );
}

#[test]
fn a_forbidden_character_reference_in_the_part_does_not_block_admission() {
    // Tally writes `&#4;` into reserved names; the consumers decode it, and
    // admission must not refuse a part for it.
    let body = part(&company(&format!(
        "{}<RESERVEDTEXT>&#4; Primary</RESERVEDTEXT>",
        complete()
    )));
    assert!(admit_audit_company_part(&body, GUID, "20250401").is_ok());
}

#[test]
fn character_references_outside_the_read_fields_do_not_block_admission() {
    let body = part(&company(&format!(
        "{}<ADDRESS.LIST><ADDRESS>Line one&#13;&#10;Line two &#8377; &nbsp;</ADDRESS></ADDRESS.LIST>",
        complete()
    )));
    assert!(admit_audit_company_part(&body, GUID, "20250401").is_ok());
    let referenced = complete().replacen(
        "<BOOKSFROM TYPE=\"Date\">20250401</BOOKSFROM>",
        "<BOOKSFROM TYPE=\"Date\">&#50;0250401</BOOKSFROM>",
        1,
    );
    assert_ne!(referenced, complete());
    assert_eq!(
        admit_audit_company_part(&part(&company(&referenced)), GUID, "20250401")
            .unwrap()
            .books_from_yyyymmdd,
        "20250401"
    );
}

#[test]
fn a_guid_bearing_company_outside_the_message_is_refused() {
    // The engines take the first COMPANY with a GUID anywhere; this one comes
    // first in document order.
    let decoy = format!("<COMPANY>0<GUID>{GUID}</GUID></COMPANY>");
    let body = part(&company(&complete())).replace("<COMPANY>0</COMPANY>", &decoy);
    assert_eq!(
        admit_audit_company_part(&body, GUID, "20250401"),
        Err(AuditCompanyPartError::NotExactlyOneCompany)
    );
}

#[test]
fn content_after_the_envelope_is_refused() {
    let body = part(&company(&complete()));
    for trailing in [format!("{body}<ENVELOPE/>"), format!("{body}trailing")] {
        assert_eq!(
            admit_audit_company_part(&trailing, GUID, "20250401"),
            Err(AuditCompanyPartError::Malformed)
        );
    }
    let padded = format!("\u{feff}<?xml version=\"1.0\"?>\r\n{body}\r\n");
    assert!(admit_audit_company_part(&padded, GUID, "20250401").is_ok());
}

#[test]
fn a_read_field_with_a_child_element_is_refused() {
    let split = complete().replacen(
        &format!(
            r#"<GUID TYPE="String">{}</GUID>"#,
            GUID.to_ascii_uppercase()
        ),
        &format!(
            r#"<GUID TYPE="String">{}<X/></GUID>"#,
            GUID.to_ascii_uppercase()
        ),
        1,
    );
    assert_ne!(split, complete());
    assert_eq!(
        admit_audit_company_part(&part(&company(&split)), GUID, "20250401"),
        Err(AuditCompanyPartError::Malformed)
    );
}

#[test]
fn an_empty_integration_flag_reads_as_unknown() {
    for empty in [
        r#"<ISINTEGRATED TYPE="Logical"/>"#,
        r#"<ISINTEGRATED TYPE="Logical"> </ISINTEGRATED>"#,
    ] {
        let flag = complete().replacen(
            r#"<ISINTEGRATED TYPE="Logical">Yes</ISINTEGRATED>"#,
            empty,
            1,
        );
        assert_ne!(flag, complete());
        assert_eq!(
            admit_audit_company_part(&part(&company(&flag)), GUID, "20250401")
                .unwrap()
                .is_integrated,
            None
        );
    }
}

#[test]
fn a_second_message_is_refused_even_when_it_is_empty() {
    let body = part(&company(&complete())).replace("</DATA>", "<TALLYMESSAGE/></DATA>");
    assert_eq!(
        admit_audit_company_part(&body, GUID, "20250401"),
        Err(AuditCompanyPartError::NotExactlyOneCompany)
    );
}

#[test]
fn a_company_elsewhere_carrying_only_books_from_is_refused() {
    // The Rust engine reads BOOKSFROM from the first COMPANY that has one,
    // whatever its GUID.
    let decoy = "<COMPANY>0<BOOKSFROM>20240401</BOOKSFROM></COMPANY>";
    let body = part(&company(&complete())).replace("<COMPANY>0</COMPANY>", decoy);
    assert_ne!(body, part(&company(&complete())));
    assert_eq!(
        admit_audit_company_part(&body, GUID, "20250401"),
        Err(AuditCompanyPartError::NotExactlyOneCompany)
    );
}

#[test]
fn cdata_wrapped_fields_read_as_the_consumer_reads_them() {
    let guid_upper = GUID.to_ascii_uppercase();
    let wrapped = complete()
        .replacen(
            &format!(r#"<GUID TYPE="String">{guid_upper}</GUID>"#),
            &format!(r#"<GUID TYPE="String"><![CDATA[{guid_upper}]]></GUID>"#),
            1,
        )
        .replacen(
            r#"<BOOKSFROM TYPE="Date">20250401</BOOKSFROM>"#,
            r#"<BOOKSFROM TYPE="Date">2025<![CDATA[04]]>01</BOOKSFROM>"#,
            1,
        )
        .replacen(
            r#"<ISINTEGRATED TYPE="Logical">Yes</ISINTEGRATED>"#,
            r#"<ISINTEGRATED TYPE="Logical"><![CDATA[Yes]]></ISINTEGRATED>"#,
            1,
        );
    assert_eq!(wrapped.matches("CDATA").count(), 3);
    let admitted = admit_audit_company_part(&part(&company(&wrapped)), GUID, "20250401")
        .expect("CDATA text is element text");
    assert_eq!(admitted.guid, GUID);
    assert_eq!(admitted.books_from_yyyymmdd, "20250401");
    assert_eq!(admitted.is_integrated.as_deref(), Some("Yes"));
    assert_eq!(
        admit_audit_company_part(
            &format!("{}<![CDATA[x]]>", part(&company(&complete()))),
            GUID,
            "20250401"
        ),
        Err(AuditCompanyPartError::Malformed)
    );
}
