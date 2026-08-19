//! Renders a [`PartyStatement`] as a printable, text-extractable PDF.
//!
//! The PDF uses the PDF-standard Helvetica fonts so it needs no bundled font
//! asset. Those fonts do not contain the rupee glyph, therefore amounts are
//! intentionally written as `INR 1,234.50`: no accounting value can disappear
//! behind an unsupported glyph.

use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};

use super::party_statement::PartyStatement;
use crate::tally::ExposureDirection;

const PAGE_WIDTH: f32 = 595.0;
const PAGE_HEIGHT: f32 = 842.0;
const MARGIN: f32 = 42.0;
const BODY_FONT_SIZE: f32 = 9.0;
const HEADING_FONT_SIZE: f32 = 12.0;
const LINE_HEIGHT: f32 = 14.0;
const MAX_LINE_BYTES: usize = 82;
const LINES_PER_PAGE: usize = 52;
const BODY_LINES_PER_PAGE: usize = LINES_PER_PAGE - 1;

#[derive(Debug, thiserror::Error)]
pub enum PartyStatementPdfError {
    #[error("Bridge could not read a statement date for the PDF ({0})")]
    InvalidDate(String),
    #[error("Bridge could not represent an amount in the PDF ({0})")]
    InvalidAmount(String),
    #[error("Bridge could not classify a statement bill direction ({0})")]
    InvalidDirection(String),
    #[error("Bridge found an inconsistent statement age state")]
    InvalidAgeState,
    #[error("Bridge could not represent statement text in the PDF's built-in font")]
    UnsupportedText,
    #[error("Bridge could not allocate PDF pages for this statement")]
    TooManyPages,
}

#[derive(Debug)]
struct PdfLine {
    text: String,
    bold: bool,
}

/// Allocates indirect PDF object references without making later document
/// layouts depend on hand-maintained numeric offsets.
#[derive(Debug)]
struct PdfObjectAllocator {
    next_id: i32,
}

impl PdfObjectAllocator {
    fn new() -> Self {
        Self { next_id: 1 }
    }

    fn allocate(&mut self) -> Result<Ref, PartyStatementPdfError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(PartyStatementPdfError::TooManyPages)?;
        Ok(Ref::new(id))
    }
}

impl PdfLine {
    fn body(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
        }
    }

    fn bold(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: true,
        }
    }
}

/// Renders `statement` as in-memory PDF bytes.
///
/// Every display amount takes the same fail-closed conversion boundary as the
/// XLSX export before it reaches the document. The PDF keeps the validated
/// decimal text, rather than writing the lossy floating-point value, so the
/// printed figure remains the exact model value.
pub fn render_party_statement_pdf(
    statement: &PartyStatement,
) -> Result<Vec<u8>, PartyStatementPdfError> {
    let lines = statement_lines(statement)?;
    let page_count = lines.len().div_ceil(BODY_LINES_PER_PAGE).max(1);
    let page_count_i32 =
        i32::try_from(page_count).map_err(|_| PartyStatementPdfError::TooManyPages)?;

    let mut object_ids = PdfObjectAllocator::new();
    let catalog_id = object_ids.allocate()?;
    let pages_id = object_ids.allocate()?;
    let regular_font_id = object_ids.allocate()?;
    let bold_font_id = object_ids.allocate()?;
    let regular_font = Name(b"F1");
    let bold_font = Name(b"F2");
    let mut pdf = Pdf::new();

    pdf.catalog(catalog_id).pages(pages_id);
    let page_ids: Vec<_> = (0..page_count)
        .map(|_| object_ids.allocate())
        .collect::<Result<_, _>>()?;
    pdf.pages(pages_id)
        .kids(page_ids.iter().copied())
        .count(page_count_i32);
    pdf.type1_font(regular_font_id)
        .base_font(Name(b"Helvetica"));
    pdf.type1_font(bold_font_id)
        .base_font(Name(b"Helvetica-Bold"));

    for (page_index, page_lines) in lines.chunks(BODY_LINES_PER_PAGE).enumerate() {
        let page_id = page_ids[page_index];
        let content_id = object_ids.allocate()?;
        {
            let mut page = pdf.page(page_id);
            page.parent(pages_id)
                .media_box(Rect::new(0.0, 0.0, PAGE_WIDTH, PAGE_HEIGHT))
                .contents(content_id);
            page.resources()
                .fonts()
                .pair(regular_font, regular_font_id)
                .pair(bold_font, bold_font_id);
        }

        let mut content = Content::new();
        content.begin_text();
        let page_header = format!(
            "Party statement | Company: {} | Party: {} | Page {} of {page_count}",
            statement.company,
            statement.party,
            page_index + 1,
        );
        content
            .set_font(bold_font, BODY_FONT_SIZE)
            .set_text_matrix([1.0, 0.0, 0.0, 1.0, MARGIN, PAGE_HEIGHT - MARGIN])
            .show(Str(page_header.as_bytes()));
        for (line_index, line) in page_lines.iter().enumerate() {
            let font = if line.bold { bold_font } else { regular_font };
            let font_size = if line.bold {
                HEADING_FONT_SIZE
            } else {
                BODY_FONT_SIZE
            };
            let y = PAGE_HEIGHT - MARGIN - ((line_index + 1) as f32 * LINE_HEIGHT);
            content
                .set_font(font, font_size)
                .set_text_matrix([1.0, 0.0, 0.0, 1.0, MARGIN, y])
                .show(Str(line.text.as_bytes()));
        }
        content.end_text();
        pdf.stream(content_id, &content.finish());
    }

    Ok(pdf.finish())
}

fn statement_lines(statement: &PartyStatement) -> Result<Vec<PdfLine>, PartyStatementPdfError> {
    let mut lines = Vec::new();
    push_wrapped(&mut lines, "Party statement", true)?;
    push_label_value(&mut lines, "Company", &statement.company)?;
    push_label_value(&mut lines, "Party", &statement.party)?;
    push_label_value(
        &mut lines,
        "As of",
        &display_date(&statement.as_of_yyyymmdd)?,
    )?;
    lines.push(PdfLine::body(""));

    if !statement.unallocated.is_zero() {
        push_wrapped(
            &mut lines,
            "Also carries exposure with no bill reference -- shown separately below, not aged.",
            false,
        )?;
        lines.push(PdfLine::body(""));
    }

    push_wrapped(
        &mut lines,
        "Reference | Bill date | Due date | Direction | Amount | Age (days) | Bucket",
        true,
    )?;
    for bill in &statement.bills {
        let amount = display_amount(&bill.amount)?;
        let (age, bucket) = match (bill.age_days, bill.bucket) {
            (Some(age_days), Some(bucket)) => (age_days.to_string(), bucket.label()),
            (None, None) => ("Not due".to_string(), "Unaged"),
            _ => return Err(PartyStatementPdfError::InvalidAgeState),
        };
        let row = format!(
            "{} | {} | {} | {} | {} | {} | {}",
            bill.reference,
            display_date(&bill.bill_date)?,
            display_date(&bill.due_date)?,
            bill_direction_label(bill.kind)?,
            amount,
            age,
            bucket,
        );
        push_wrapped(&mut lines, &row, false)?;
    }

    push_label_value(
        &mut lines,
        "Total bill magnitudes (not net)",
        &display_amount(&statement.bill_total)?,
    )?;
    if !statement.unallocated.is_zero() {
        let direction =
            statement
                .unallocated_direction
                .ok_or(PartyStatementPdfError::InvalidDirection(
                    "unallocated direction missing".to_string(),
                ))?;
        push_label_value(
            &mut lines,
            &format!(
                "Unallocated {} (no bill reference)",
                exposure_direction_label(direction)
            ),
            &display_amount(&statement.unallocated)?,
        )?;
        push_label_value(
            &mut lines,
            "Grand total",
            &display_amount(&statement.grand_total)?,
        )?;
    }
    Ok(lines)
}

fn bill_direction_label(kind: &str) -> Result<&'static str, PartyStatementPdfError> {
    match kind {
        "receivable" => Ok(exposure_direction_label(ExposureDirection::Receivable)),
        "payable" => Ok(exposure_direction_label(ExposureDirection::Payable)),
        _ => Err(PartyStatementPdfError::InvalidDirection(kind.to_string())),
    }
}

fn exposure_direction_label(direction: ExposureDirection) -> &'static str {
    match direction {
        ExposureDirection::Receivable => "Receivable",
        ExposureDirection::Payable => "Payable",
    }
}

fn push_label_value(
    lines: &mut Vec<PdfLine>,
    label: &str,
    value: &str,
) -> Result<(), PartyStatementPdfError> {
    push_wrapped(lines, &format!("{label}: {value}"), false)
}

fn push_wrapped(
    lines: &mut Vec<PdfLine>,
    text: &str,
    bold: bool,
) -> Result<(), PartyStatementPdfError> {
    if !text.bytes().all(|byte| matches!(byte, b' '..=b'~')) {
        return Err(PartyStatementPdfError::UnsupportedText);
    }
    if text.is_empty() {
        lines.push(PdfLine::body(""));
        return Ok(());
    }
    for chunk in text.as_bytes().chunks(MAX_LINE_BYTES) {
        let text = std::str::from_utf8(chunk).expect("ASCII was checked above");
        lines.push(if bold {
            PdfLine::bold(text)
        } else {
            PdfLine::body(text)
        });
    }
    Ok(())
}

fn display_date(yyyymmdd: &str) -> Result<String, PartyStatementPdfError> {
    if yyyymmdd.len() != 8 || !yyyymmdd.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(PartyStatementPdfError::InvalidDate(yyyymmdd.to_string()));
    }
    let year = yyyymmdd[0..4]
        .parse::<i32>()
        .map_err(|_| PartyStatementPdfError::InvalidDate(yyyymmdd.to_string()))?;
    let month = yyyymmdd[4..6]
        .parse::<u32>()
        .map_err(|_| PartyStatementPdfError::InvalidDate(yyyymmdd.to_string()))?;
    let day = yyyymmdd[6..8]
        .parse::<u32>()
        .map_err(|_| PartyStatementPdfError::InvalidDate(yyyymmdd.to_string()))?;
    chrono::NaiveDate::from_ymd_opt(year, month, day)
        .map(|date| date.format("%d-%b-%Y").to_string())
        .ok_or_else(|| PartyStatementPdfError::InvalidDate(yyyymmdd.to_string()))
}

fn display_amount(
    amount: &bridge_tally_core::ExactDecimal,
) -> Result<String, PartyStatementPdfError> {
    amount_text_for_pdf(amount.as_str())
}

fn amount_text_for_pdf(text: &str) -> Result<String, PartyStatementPdfError> {
    let value = text
        .parse::<f64>()
        .map_err(|_| PartyStatementPdfError::InvalidAmount(text.to_string()))?;
    if !value.is_finite() {
        return Err(PartyStatementPdfError::InvalidAmount(text.to_string()));
    }
    Ok(format!("INR {}", indian_grouped_decimal(text)))
}

fn indian_grouped_decimal(text: &str) -> String {
    let (sign, unsigned) = text
        .strip_prefix('-')
        .map_or(("", text), |value| ("-", value));
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let mut grouped = String::with_capacity(text.len() + text.len() / 2);
    grouped.push_str(sign);
    let first_group_len = if whole.len() <= 3 {
        whole.len()
    } else {
        let prefix_len = (whole.len() - 3) % 2;
        if prefix_len == 0 {
            2
        } else {
            prefix_len
        }
    };
    grouped.push_str(&whole[..first_group_len]);
    let mut remainder = &whole[first_group_len..];
    while !remainder.is_empty() {
        grouped.push(',');
        let group_len = if remainder.len() == 3 { 3 } else { 2 };
        grouped.push_str(&remainder[..group_len]);
        remainder = &remainder[group_len..];
    }
    if !fraction.is_empty() {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::party_statement::build_party_statement;
    use crate::reports::party_statement_xlsx::render_party_statement_xlsx;
    use crate::tally::{ExposureDirection, OpenBillRow, UnallocatedParty};
    use bridge_tally_core::ExactDecimal;
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
            kind: "receivable",
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
                let Some(close) = content_remainder.find(')') else {
                    break;
                };
                extracted.push_str(&content_remainder[..close]);
                extracted.push('\n');
                content_remainder = &content_remainder[close + 1..];
            }
            remainder = &remainder[end + b"\nendstream".len()..];
        }
        extracted
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn xlsx_sheet_xml(xlsx: &[u8]) -> String {
        let mut archive = ZipArchive::new(Cursor::new(xlsx)).expect("well-formed XLSX archive");
        let mut sheet = archive
            .by_name("xl/worksheets/sheet1.xml")
            .expect("statement worksheet exists");
        let mut xml = String::new();
        sheet
            .read_to_string(&mut xml)
            .expect("statement worksheet is UTF-8 XML");
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
    fn renders_bill_direction_for_mixed_party_documents() {
        let mut payable = bill("BILL-1", "1250.75", 40);
        payable.kind = "payable";
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
    fn long_party_and_bill_names_are_wrapped_without_being_clipped() {
        let long_party = "Synthetic Party With A Deliberately Long Ledger Name That Exceeds A Single Printable Statement Line";
        let long_reference = "SYNTHETIC-REFERENCE-WITH-A-DELIBERATELY-LONG-BILL-NAME-THAT-MUST-WRAP-WITHOUT-CLIPPING";
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

        let text = extracted_text(&render_party_statement_pdf(&statement).unwrap());
        let identity =
            "Party statement | Company: Synthetic Books Pvt Ltd | Party: Synthetic Party";
        assert_eq!(text.matches(identity).count(), 3);
        assert!(text.contains("Page 1 of 3"));
        assert!(text.contains("Page 2 of 3"));
        assert!(text.contains("Page 3 of 3"));
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
        assert!(pdf_text.contains("Grand total: INR 1,600"));
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
    fn unsupported_statement_text_fails_instead_of_becoming_blank() {
        let party = "Party with a non-core glyph: \u{20b9}";
        let mut source_bill = bill("INV-1", "10.00", 5);
        source_bill.party = party.to_string();
        let statement = build_party_statement(
            "Synthetic Books Pvt Ltd",
            "20260808",
            party,
            &[source_bill],
            &[],
        )
        .unwrap();

        assert!(matches!(
            render_party_statement_pdf(&statement),
            Err(PartyStatementPdfError::UnsupportedText)
        ));
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
}
