use super::*;
use crate::reports::party_statement::build_party_statement;
use crate::reports::party_statement_xlsx::render_party_statement_xlsx;
use crate::tally::{ExposureDirection, OpenBillRow, OutstandingsAgeingAnchor, UnallocatedParty};
use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::native_outstandings::parse_native_ledger_snapshot;
use std::io::{Cursor, Read};
use zip::ZipArchive;

#[test]
fn pdf_object_allocator_issues_unique_sequential_references() {
    let mut allocator = PdfObjectAllocator::new();
    let ids = (0..8)
        .map(|_| {
            allocator
                .allocate()
                .expect("eight PDF references fit")
                .get()
        })
        .collect::<Vec<_>>();

    assert_eq!(ids, (1..=8).collect::<Vec<_>>());
}

fn bill(reference: &str, amount: &str, age_days: u32) -> OpenBillRow {
    OpenBillRow {
        party: "Synthetic Party".to_string(),
        reference: reference.to_string(),
        bill_date: "20260101".to_string(),
        due_date: "20260201".to_string(),
        amount: ExactDecimal::parse(amount).unwrap(),
        age_days: Some(age_days),
        kind: ExposureDirection::Receivable,
    }
}

fn extracted_text(pdf: &[u8]) -> String {
    // pdf-writer emits uncompressed content streams. Extracting literal
    // text operands here tests the generated document rather than merely
    // inspecting the model that was meant to be written.
    let mut extracted = String::new();
    let mut remainder = pdf;
    while let Some(start) = find_bytes(remainder, b"stream\n") {
        remainder = &remainder[start + b"stream\n".len()..];
        let Some(end) = find_bytes(remainder, b"\nendstream") else {
            break;
        };
        let content =
            std::str::from_utf8(&remainder[..end]).expect("statement text streams are ASCII");
        let mut content_remainder = content;
        while let Some(open) = content_remainder.find('(') {
            content_remainder = &content_remainder[open + 1..];
            let Some((text, remainder_after_text)) = pdf_string_text(content_remainder) else {
                break;
            };
            extracted.push_str(&text);
            extracted.push('\n');
            content_remainder = remainder_after_text;
        }
        remainder = &remainder[end + b"\nendstream".len()..];
    }
    extracted
}

fn pdf_string_text(input: &str) -> Option<(String, &str)> {
    let mut text = String::with_capacity(input.len());
    let mut escaped = false;
    let mut nested_parentheses = 0usize;
    for (offset, character) in input.char_indices() {
        if escaped {
            text.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '(' {
            nested_parentheses += 1;
            text.push(character);
        } else if character == ')' {
            if nested_parentheses == 0 {
                return Some((text, &input[offset + character.len_utf8()..]));
            }
            nested_parentheses -= 1;
            text.push(character);
        } else {
            text.push(character);
        }
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn expected_page_count(statement: &PartyStatement) -> usize {
    let body_lines_per_page = LINES_PER_PAGE - (1 + page_header_identity_lines(statement).len());
    statement_lines(statement)
        .unwrap()
        .len()
        .div_ceil(body_lines_per_page)
        .max(1)
}

fn contains_pdf_text(pdf: &[u8], text: &str) -> bool {
    pdf.windows(text.len())
        .any(|bytes| bytes == text.as_bytes())
}

fn assert_all_planned_document_lines_fit(statement: &PartyStatement) {
    for header_line in page_header_identity_lines(statement) {
        assert!(text_fits_printable_width(
            &header_line,
            true,
            BODY_FONT_SIZE as u16
        ));
    }
    let lines = statement_lines(statement).unwrap();
    for line in lines {
        let font_size = if line.bold {
            HEADING_FONT_SIZE
        } else {
            BODY_FONT_SIZE
        };
        assert!(text_fits_printable_width(
            &line.text,
            line.bold,
            font_size as u16
        ));
    }
    let page_count = expected_page_count(statement);
    for page_number in 1..=page_count {
        assert!(text_fits_printable_width(
            format!("Page {page_number} of {page_count}").as_bytes(),
            true,
            BODY_FONT_SIZE as u16
        ));
    }
}

fn xlsx_sheet_xml(xlsx: &[u8]) -> String {
    let mut archive = ZipArchive::new(Cursor::new(xlsx)).expect("well-formed XLSX archive");
    let mut xml = String::new();
    for name in ["xl/worksheets/sheet1.xml", "xl/sharedStrings.xml"] {
        let mut entry = archive.by_name(name).expect("statement XML entry exists");
        entry
            .read_to_string(&mut xml)
            .expect("statement XML is UTF-8");
    }
    xml
}

#[test]
fn renders_extractable_inr_text_instead_of_an_unsupported_rupee_glyph() {
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &[bill("INV-1", "1250.75", 40)],
        &[],
    )
    .unwrap();

    let pdf = render_party_statement_pdf(&statement).unwrap();
    let text = extracted_text(&pdf);
    assert!(text.contains("INR 1,250.75"));
    assert!(!text.contains('\u{20b9}'));
}

#[test]
fn pdf_and_xlsx_disclose_the_statement_selected_ageing_basis() {
    let mut statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &[bill("INV-1", "1250.75", 40)],
        &[],
    )
    .unwrap();
    statement.ageing_anchor = OutstandingsAgeingAnchor::BillDate;

    let xlsx_text = xlsx_sheet_xml(&render_party_statement_xlsx(&statement).unwrap());
    assert!(statement_lines(&statement)
        .unwrap()
        .iter()
        .any(|line| line.text == b"Ageing basis: Bill date"));
    assert!(xlsx_text.contains("Ageing basis"));
    assert!(xlsx_text.contains("Bill date"));
}

#[test]
fn renders_bill_direction_for_mixed_party_documents() {
    let mut payable = bill("BILL-1", "1250.75", 40);
    payable.kind = ExposureDirection::Payable;
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &[bill("INV-1", "1250.75", 40), payable],
        &[],
    )
    .unwrap();

    let text = extracted_text(&render_party_statement_pdf(&statement).unwrap());
    assert!(text.contains("Reference | Bill date | Due date | Direction | Amount"));
    assert!(text.contains("INV-1 | 01-Jan-2026 | 01-Feb-2026 | Receivable"));
    assert!(text.contains("BILL-1 | 01-Jan-2026 | 01-Feb-2026 | Payable"));
}

#[test]
fn mixed_direction_bucket_subtotals_are_explicit_in_both_formats() {
    let mut payable = bill("BILL-80", "80.00", 20);
    payable.kind = ExposureDirection::Payable;
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &[bill("INV-100", "100.00", 20), payable],
        &[],
    )
    .unwrap();

    let pdf = render_party_statement_pdf(&statement).unwrap();
    let pdf_text = extracted_text(&pdf);
    let joined_pdf_text = pdf_text.replace('\n', "");
    let xlsx_text = xlsx_sheet_xml(&render_party_statement_xlsx(&statement).unwrap());
    assert!(joined_pdf_text.contains("Ageing subtotals by direction (magnitudes; not net)"));
    assert!(joined_pdf_text.contains("Receivable | 0-30 days"));
    assert!(joined_pdf_text.contains("Payable | 0-30 days"));
    assert!(xlsx_text.contains("Ageing subtotals by direction (magnitudes; not net)"));
    assert!(xlsx_text.contains("Receivable | 0-30 days"));
    assert!(xlsx_text.contains("Payable | 0-30 days"));
    assert_eq!(statement.subtotals.total().unwrap(), statement.bill_total);
}

#[test]
fn long_company_identity_cannot_evict_party_from_continuation_headers() {
    let company = "C".repeat(700);
    let bills = (0..150)
        .map(|index| {
            let mut source_bill = bill(&format!("INV-{index:03}"), "1.00", 1);
            source_bill.party = "Bounded Party".to_string();
            source_bill
        })
        .collect::<Vec<_>>();
    let statement =
        build_party_statement(&company, "20260808", "Bounded Party", &bills, &[]).unwrap();

    let header = page_header_identity_lines(&statement)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert!(contains_pdf_text(
        &render_party_statement_pdf(&statement).unwrap(),
        "Page 2 of"
    ));
    let header = std::str::from_utf8(&header).unwrap();
    assert!(header.contains("Company identity continues in statement body"));
    assert!(header.contains("Party: Bounded Party"));
}

#[test]
fn pathological_identities_keep_party_counter_and_width_bounds() {
    let company = string_from_codepoints(&[0x0936; 200]);
    let party = string_from_codepoints(&[0x0905; 200]);
    let bills = (0..150)
        .map(|index| {
            let mut source_bill = bill(&format!("INV-{index:03}"), "1.00", 1);
            source_bill.party = party.clone();
            source_bill
        })
        .collect::<Vec<_>>();
    let statement = build_party_statement(&company, "20260808", &party, &bills, &[]).unwrap();

    let pdf = render_party_statement_pdf(&statement).unwrap();
    let header = page_header_identity_lines(&statement)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let header = std::str::from_utf8(&header).unwrap();
    assert!(header.contains("Company identity continues in statement body"));
    assert!(header.contains("Party: [party name rendering degraded:"));
    assert!(header.contains("Party identity continues in statement body"));
    assert!(contains_pdf_text(&pdf, "Page 2 of"));
    assert_all_planned_document_lines_fit(&statement);
}

#[test]
fn long_party_and_bill_names_are_wrapped_without_being_clipped() {
    let long_party = "Synthetic Party With A Deliberately Long Ledger Name That Exceeds A Single Printable Statement Line";
    let long_reference =
        "SYNTHETIC-REFERENCE-WITH-A-DELIBERATELY-LONG-BILL-NAME-THAT-MUST-WRAP-WITHOUT-CLIPPING";
    let mut source_bill = bill(long_reference, "1.00", 1);
    source_bill.party = long_party.to_string();
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        long_party,
        &[source_bill],
        &[],
    )
    .unwrap();

    let text = extracted_text(&render_party_statement_pdf(&statement).unwrap());
    // Line wrapping inserts extraction boundaries, but must not lose any
    // character from either untrusted Tally field.
    let joined = text.replace('\n', "");
    assert!(joined.contains(long_party));
    assert!(joined.contains(long_reference));
}

#[test]
fn wide_win_ansi_bill_reference_never_exceeds_the_printable_width() {
    let reference = string_from_codepoints(&[0x00c6; 40]);
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &[bill(&reference, "1.00", 1)],
        &[],
    )
    .unwrap();

    assert!(render_party_statement_pdf(&statement).is_ok());
    assert_all_planned_document_lines_fit(&statement);
}

#[test]
fn non_identity_win_ansi_scalars_degrade_visibly() {
    for codepoint in [0x00a0, 0x00ad] {
        let party =
            string_from_codepoints(&[0x0050, 0x0061, 0x0072, 0x0074, 0x0079, 0x0020, codepoint]);
        let mut source_bill = bill("INV-1", "1.00", 1);
        source_bill.party = party.clone();
        let statement = build_party_statement(
            "Synthetic Books Pvt Ltd",
            "20260808",
            &party,
            &[source_bill],
            &[],
        )
        .unwrap();

        let pdf = render_party_statement_pdf(&statement).unwrap();
        assert!(contains_pdf_text(
            &pdf,
            &format!("party name rendering degraded: U+0050 U+0061 U+0072 U+0074 U+0079 U+0020 U+{codepoint:04X}")
        ));
    }
}

#[test]
fn latin_1_win_ansi_audit_excludes_every_non_identity_scalar() {
    for scalar in 0x00a0..=0x00ff {
        let character = char::from_u32(scalar).expect("Latin-1 scalar");
        let expected = match scalar {
            // WinAnsi's byte values draw a space and a hyphen, not the
            // source NO-BREAK SPACE or SOFT HYPHEN scalar.
            0x00a0 | 0x00ad => None,
            _ => Some(scalar as u8),
        };
        assert_eq!(win_ansi_byte(character), expected, "U+{scalar:04X}");
    }
}

#[test]
fn every_pdf_page_repeats_the_statement_identity() {
    let bills = (0..110)
        .map(|index| bill(&format!("INV-{index:03}"), "1.00", 1))
        .collect::<Vec<_>>();
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &bills,
        &[],
    )
    .unwrap();

    let pdf = render_party_statement_pdf(&statement).unwrap();
    let page_count = expected_page_count(&statement);
    for identity_line in page_header_identity_lines(&statement) {
        let identity_line = std::str::from_utf8(&identity_line).unwrap();
        assert!(contains_pdf_text(&pdf, identity_line));
    }
    for page_number in 1..=page_count {
        assert!(contains_pdf_text(
            &pdf,
            &format!("Page {page_number} of {page_count}")
        ));
    }
}

#[test]
fn xlsx_and_pdf_render_the_same_model_total() {
    let bills = vec![bill("INV-1", "1250.75", 40), bill("INV-2", "49.25", 4)];
    let unallocated = vec![UnallocatedParty {
        party: "Synthetic Party".to_string(),
        amount: ExactDecimal::parse("300.00").unwrap(),
        direction: ExposureDirection::Receivable,
    }];
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &bills,
        &unallocated,
    )
    .unwrap();

    let xlsx = render_party_statement_xlsx(&statement).unwrap();
    let pdf_text = extracted_text(&render_party_statement_pdf(&statement).unwrap());
    assert_eq!(statement.grand_total.as_str(), "1600");
    // `1600` appears only in the XLSX grand-total cell for this fixture.
    assert!(xlsx_sheet_xml(&xlsx).contains("<v>1600</v>"));
    assert!(pdf_text.contains("Grand total magnitudes (not net): INR 1,600"));
}

#[test]
fn renders_not_due_and_unallocated_direction_in_the_pdf_text() {
    let mut future_due = bill("FUTURE-1", "100.00", 0);
    future_due.age_days = None;
    let unallocated = vec![UnallocatedParty {
        party: "Synthetic Party".to_string(),
        amount: ExactDecimal::parse("42.00").unwrap(),
        direction: ExposureDirection::Payable,
    }];
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &[future_due],
        &unallocated,
    )
    .unwrap();
    let pdf = render_party_statement_pdf(&statement).unwrap();
    let text = extracted_text(&pdf);
    let joined = text.replace('\n', "");
    assert!(text.contains("FUTURE-1"));
    assert!(pdf
        .windows(b"Not due".len())
        .any(|bytes| bytes == b"Not due"));
    assert!(pdf.windows(b"Unaged".len()).any(|bytes| bytes == b"Unaged"));
    assert!(joined.contains("Unaged"));
    assert!(joined.contains("Unallocated Payable"));
}

#[test]
fn unrepresentable_party_name_keeps_the_document_bills_and_totals() {
    let party = string_from_codepoints(&[
        0x0050, 0x0061, 0x0072, 0x0074, 0x0079, 0x0085, 0x0020, 0x20b9,
    ]);
    let mut source_bill = bill("INV-1", "10.00", 5);
    source_bill.party = party.to_string();
    let unrepresentable = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        &party,
        &[source_bill],
        &[],
    )
    .unwrap();
    let mut ascii_bill = bill("INV-1", "10.00", 5);
    ascii_bill.party = "ASCII Party".to_string();
    let ascii = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "ASCII Party",
        &[ascii_bill],
        &[],
    )
    .unwrap();

    let pdf = render_party_statement_pdf(&unrepresentable).expect("PDF still renders");
    assert!(pdf
        .windows(b"party name rendering degraded: U+0050".len())
        .any(|bytes| bytes == b"party name rendering degraded: U+0050"));
    assert!(pdf.windows(b"U+0085".len()).any(|bytes| bytes == b"U+0085"));
    assert!(pdf.windows(b"U+20B9".len()).any(|bytes| bytes == b"U+20B9"));
    assert!(extracted_text(&pdf).contains("INV-1"));
    let xlsx = render_party_statement_xlsx(&unrepresentable).expect("XLSX still renders");
    let xlsx_xml = xlsx_sheet_xml(&xlsx);
    assert!(xlsx_xml.contains(&party));
    assert!(xlsx_xml.contains("INV-1"));
    assert_eq!(unrepresentable.bill_total, ascii.bill_total);
    assert_eq!(unrepresentable.grand_total, ascii.grand_total);
}

#[test]
fn every_aarav_ledger_name_renders_in_both_statement_formats() {
    let ledgers = parse_native_ledger_snapshot(include_str!(
        "../../crates/bridge-tally-protocol/tests/fixtures/native/ledger_snapshot_aarav.xml"
    ))
    .expect("captured ledger snapshot parses");
    assert_eq!(
        ledgers.len(),
        88,
        "fixture coverage must not silently shrink"
    );

    for ledger in ledgers {
        let mut source_bill = bill("INV-1", "10.00", 5);
        source_bill.party = ledger.name.clone();
        let statement = build_party_statement(
            "Synthetic Books Pvt Ltd",
            "20260808",
            &ledger.name,
            &[source_bill],
            &[],
        )
        .expect("a billed fixture ledger produces a statement");

        assert!(
            render_party_statement_pdf(&statement).is_ok(),
            "PDF must render ledger {:?}",
            ledger.name
        );
        assert!(
            render_party_statement_xlsx(&statement).is_ok(),
            "XLSX must render ledger {:?}",
            ledger.name
        );
    }
}

#[test]
fn ageing_subtotals_are_rendered_exactly_in_both_formats() {
    let mut not_yet_due = bill("NOT-DUE", "50.875", 0);
    not_yet_due.age_days = None;
    let bills = [
        bill("D0", "10.25", 1),
        bill("D31", "20.50", 31),
        bill("D61", "30.75", 61),
        bill("D90", "40.125", 91),
        not_yet_due,
    ];
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        "Synthetic Party",
        &bills,
        &[],
    )
    .unwrap();

    let pdf = render_party_statement_pdf(&statement).unwrap();
    let pdf_text = extracted_text(&pdf);
    assert!(pdf_text.contains("Ageing subtotals by direction (magnitudes; not net)"));
    for (direction, subtotals) in statement.subtotals.by_direction() {
        for (bucket, subtotal) in [
            ("Not yet due", &subtotals.not_yet_due),
            ("0-30 days", &subtotals.days_0_30),
            ("31-60 days", &subtotals.days_31_60),
            ("61-90 days", &subtotals.days_61_90),
            ("90+ days", &subtotals.days_90_plus),
        ] {
            assert!(pdf_text.contains(&format!(
                "{direction} | {bucket}: INR {}",
                indian_grouped_decimal(subtotal.as_str())
            )));
        }
    }

    let xlsx = render_party_statement_xlsx(&statement).unwrap();
    let xlsx_xml = xlsx_sheet_xml(&xlsx);
    assert!(xlsx_xml.contains("Ageing subtotals by direction (magnitudes; not net)"));
    for (direction, subtotals) in statement.subtotals.by_direction() {
        for (bucket, subtotal) in [
            ("Not yet due", &subtotals.not_yet_due),
            ("0-30 days", &subtotals.days_0_30),
            ("31-60 days", &subtotals.days_31_60),
            ("61-90 days", &subtotals.days_61_90),
            ("90+ days", &subtotals.days_90_plus),
        ] {
            assert!(xlsx_xml.contains(&format!("{direction} | {bucket}")));
            assert!(xlsx_xml.contains(&format!("<v>{}</v>", subtotal.as_str())));
        }
    }
    assert_eq!(statement.subtotals.total().unwrap(), statement.bill_total);
}

#[test]
fn degraded_multi_page_headers_wrap_identity_and_keep_every_page_counter() {
    let party = string_from_codepoints(&[0x0936; 18]);
    let bills = (0..150)
        .map(|index| {
            let mut source_bill = bill(&format!("INV-{index:03}"), "1.00", 1);
            source_bill.party = party.clone();
            source_bill
        })
        .collect::<Vec<_>>();
    let statement =
        build_party_statement("Synthetic Books Pvt Ltd", "20260808", &party, &bills, &[]).unwrap();

    let header_lines = page_header_identity_lines(&statement);
    assert!(header_lines.len() > 1);
    assert!(header_lines.len() <= 2 * HEADER_IDENTITY_FIELD_LINE_LIMIT);
    assert!(header_lines.iter().all(|line| text_fits_printable_width(
        line,
        true,
        BODY_FONT_SIZE as u16
    )));
    assert!(!header_lines
        .iter()
        .any(|line| line == b"[Identity continued on first page]"));

    let pdf = render_party_statement_pdf(&statement).unwrap();
    let page_count = expected_page_count(&statement);
    assert!(page_count > 1, "fixture must span continuation pages");
    for page_number in 1..=page_count {
        let page_counter = format!("Page {page_number} of {page_count}");
        assert!(text_fits_printable_width(
            page_counter.as_bytes(),
            true,
            BODY_FONT_SIZE as u16
        ));
        assert!(contains_pdf_text(&pdf, &page_counter));
    }
}

#[test]
fn win_ansi_covers_the_aarav_latin_and_typographic_names() {
    let name = string_from_codepoints(&[
        0x005a, 0x005a, 0x0020, 0x0043, 0x0061, 0x0066, 0x00e9, 0x0020, 0x004e, 0x0061, 0x00ef,
        0x0076, 0x0065, 0x0020, 0x201c, 0x0051, 0x0075, 0x006f, 0x0074, 0x0065, 0x0064, 0x201d,
        0x0020, 0x2014, 0x0020, 0x2026,
    ]);

    assert_eq!(
        encode_win_ansi(&name),
        Some(vec![
            b'Z', b'Z', b' ', b'C', b'a', b'f', 0xe9, b' ', b'N', b'a', 0xef, b'v', b'e', b' ',
            0x93, b'Q', b'u', b'o', b't', b'e', b'd', 0x94, b' ', 0x97, b' ', 0x85,
        ])
    );
}

#[test]
fn rendered_pdf_declares_win_ansi_and_emits_its_extended_bytes() {
    let party = string_from_codepoints(&[
        0x0043, 0x0061, 0x0066, 0x00e9, 0x0020, 0x201c, 0x0051, 0x201d, 0x0020, 0x2014, 0x0020,
        0x2026,
    ]);
    let mut source_bill = bill("INV-1", "10.00", 5);
    source_bill.party = party.clone();
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "20260808",
        &party,
        &[source_bill],
        &[],
    )
    .unwrap();

    let pdf = render_party_statement_pdf(&statement).unwrap();
    let encoded_party = encode_win_ansi(&party).expect("party is WinAnsi-representable");
    let encoded_party_hex = encoded_party
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    assert!(pdf
        .windows(b"/Encoding /WinAnsiEncoding".len())
        .any(|bytes| { bytes == b"/Encoding /WinAnsiEncoding" }));
    assert!(pdf
        .windows(encoded_party_hex.len())
        .any(|bytes| bytes == encoded_party_hex.as_bytes()));
}

fn string_from_codepoints(codepoints: &[u32]) -> String {
    codepoints
        .iter()
        .map(|codepoint| char::from_u32(*codepoint).expect("valid test codepoint"))
        .collect()
}

#[test]
fn invalid_statement_date_fails_instead_of_using_a_default() {
    let statement = build_party_statement(
        "Synthetic Books Pvt Ltd",
        "not-a-date",
        "Synthetic Party",
        &[bill("INV-1", "10.00", 5)],
        &[],
    )
    .expect("the renderer owns document-date validation");

    assert!(matches!(
        render_party_statement_pdf(&statement),
        Err(PartyStatementPdfError::InvalidDate(value)) if value == "not-a-date"
    ));
}

#[test]
fn an_unrepresentable_amount_fails_instead_of_becoming_zero() {
    assert_eq!(
        amount_text_for_pdf("12345678.20").unwrap(),
        "INR 1,23,45,678.20"
    );
    assert!(matches!(
        amount_text_for_pdf("1e999"),
        Err(PartyStatementPdfError::InvalidAmount(value)) if value == "1e999"
    ));
}
