//! Renders a [`PartyStatement`] as a printable, text-extractable PDF.
//!
//! The PDF uses the PDF-standard Helvetica fonts with WinAnsiEncoding, so it
//! needs no bundled font asset. Text outside that encoding is shown as a
//! visible codepoint marker rather than aborting or silently changing the
//! statement. Those fonts do not contain the rupee glyph, therefore amounts
//! are intentionally written as `INR 1,234.50`: no accounting value can
//! disappear behind an unsupported glyph.

use pdf_writer::{Content, Name, Pdf, Rect, Ref, Str};

use super::party_statement::PartyStatement;
use crate::tally::ExposureDirection;

const PAGE_WIDTH: f32 = 595.0;
const PAGE_HEIGHT: f32 = 842.0;
const MARGIN: f32 = 42.0;
const BODY_FONT_SIZE: f32 = 9.0;
const HEADING_FONT_SIZE: f32 = 12.0;
const LINE_HEIGHT: f32 = 14.0;
const LINES_PER_PAGE: usize = 52;
const HEADER_IDENTITY_LINE_LIMIT: usize = 8;
const PRINTABLE_WIDTH_THOUSANDTHS_OF_POINT: u32 = ((PAGE_WIDTH - (2.0 * MARGIN)) as u32) * 1_000;

// Adobe's standard Helvetica advance widths in 1/1000 em, indexed by the
// WinAnsi byte written to each PDF text operand. Keeping each complete font
// table adjacent to the encoder makes the layout contract auditable: these
// are the exact Type 1 fonts declared below, not an average-character guess.
const HELVETICA_REGULAR_WIDTHS: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
    350, 556, 350, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 350, 611, 350, 350,
    222, 222, 333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 350, 500, 667, 278, 333, 556,
    556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333, 400, 584, 333, 333, 333, 556,
    537, 278, 333, 333, 365, 556, 834, 834, 834, 611, 667, 667, 667, 667, 667, 667, 1000, 722, 667,
    667, 667, 667, 278, 278, 278, 278, 722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722,
    722, 667, 667, 611, 556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278,
    278, 556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
];

const HELVETICA_BOLD_WIDTHS: [u16; 256] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
    350, 556, 350, 278, 556, 500, 1000, 556, 556, 333, 1000, 667, 333, 1000, 350, 611, 350, 350,
    278, 278, 500, 500, 350, 556, 1000, 333, 1000, 556, 333, 944, 350, 500, 667, 278, 333, 556,
    556, 556, 556, 280, 556, 333, 737, 370, 556, 584, 333, 737, 333, 400, 584, 333, 333, 333, 611,
    556, 278, 333, 333, 365, 556, 834, 834, 834, 611, 722, 722, 722, 722, 722, 722, 1000, 722, 667,
    667, 667, 667, 278, 278, 278, 278, 722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722,
    722, 667, 667, 611, 556, 556, 556, 556, 556, 556, 889, 556, 556, 556, 556, 556, 278, 278, 278,
    278, 611, 611, 611, 611, 611, 611, 611, 584, 611, 611, 611, 611, 611, 556, 611, 556,
];

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
    #[error("Bridge could not allocate PDF pages for this statement")]
    TooManyPages,
}

#[derive(Debug)]
struct PdfLine {
    text: Vec<u8>,
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
    fn body(text: impl Into<Vec<u8>>) -> Self {
        Self {
            text: text.into(),
            bold: false,
        }
    }

    fn bold(text: impl Into<Vec<u8>>) -> Self {
        Self {
            text: text.into(),
            bold: true,
        }
    }
}

fn text_width_thousandths_of_point(text: &[u8], bold: bool, font_size: u16) -> u32 {
    let widths = if bold {
        &HELVETICA_BOLD_WIDTHS
    } else {
        &HELVETICA_REGULAR_WIDTHS
    };
    text.iter()
        .map(|byte| u32::from(widths[usize::from(*byte)]) * u32::from(font_size))
        .sum()
}

fn text_fits_printable_width(text: &[u8], bold: bool, font_size: u16) -> bool {
    text_width_thousandths_of_point(text, bold, font_size) <= PRINTABLE_WIDTH_THOUSANDTHS_OF_POINT
}

fn wrap_to_printable_width(text: Vec<u8>, bold: bool, font_size: u16) -> Vec<Vec<u8>> {
    let widths = if bold {
        &HELVETICA_BOLD_WIDTHS
    } else {
        &HELVETICA_REGULAR_WIDTHS
    };
    let mut lines = Vec::new();
    let mut line = Vec::new();
    let mut line_width = 0_u32;

    for byte in text {
        let byte_width = u32::from(widths[usize::from(byte)]) * u32::from(font_size);
        if !line.is_empty()
            && line_width
                .checked_add(byte_width)
                .is_none_or(|width| width > PRINTABLE_WIDTH_THOUSANDTHS_OF_POINT)
        {
            lines.push(line);
            line = Vec::new();
            line_width = 0;
        }
        line_width = line_width
            .checked_add(byte_width)
            .expect("Helvetica line width fits in u32");
        line.push(byte);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    debug_assert!(lines
        .iter()
        .all(|line| text_fits_printable_width(line, bold, font_size)));
    lines
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
    let header_identity_lines = page_header_identity_lines(statement);
    let header_line_count = 1 + header_identity_lines.len(); // Counter plus identity.
    let body_lines_per_page = LINES_PER_PAGE
        .checked_sub(header_line_count)
        .filter(|line_count| *line_count > 0)
        .ok_or(PartyStatementPdfError::TooManyPages)?;
    let page_count = lines.len().div_ceil(body_lines_per_page).max(1);
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
        .base_font(Name(b"Helvetica"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));
    pdf.type1_font(bold_font_id)
        .base_font(Name(b"Helvetica-Bold"))
        .encoding_predefined(Name(b"WinAnsiEncoding"));

    for (page_index, page_lines) in lines.chunks(body_lines_per_page).enumerate() {
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
        let page_counter = format!("Page {} of {page_count}", page_index + 1);
        debug_assert!(text_fits_printable_width(
            page_counter.as_bytes(),
            true,
            BODY_FONT_SIZE as u16
        ));
        content
            .set_font(bold_font, BODY_FONT_SIZE)
            .set_text_matrix([1.0, 0.0, 0.0, 1.0, MARGIN, PAGE_HEIGHT - MARGIN])
            .show(Str(page_counter.as_bytes()));
        for (header_line_index, header_line) in header_identity_lines.iter().enumerate() {
            let y = PAGE_HEIGHT - MARGIN - ((header_line_index + 1) as f32 * LINE_HEIGHT);
            content
                .set_font(bold_font, BODY_FONT_SIZE)
                .set_text_matrix([1.0, 0.0, 0.0, 1.0, MARGIN, y])
                .show(Str(header_line));
        }
        for (line_index, line) in page_lines.iter().enumerate() {
            let font = if line.bold { bold_font } else { regular_font };
            let font_size = if line.bold {
                HEADING_FONT_SIZE
            } else {
                BODY_FONT_SIZE
            };
            let y = PAGE_HEIGHT - MARGIN - ((header_line_count + line_index) as f32 * LINE_HEIGHT);
            content
                .set_font(font, font_size)
                .set_text_matrix([1.0, 0.0, 0.0, 1.0, MARGIN, y])
                .show(Str(&line.text));
        }
        content.end_text();
        pdf.stream(content_id, &content.finish());
    }

    Ok(pdf.finish())
}

fn page_header_identity_lines(statement: &PartyStatement) -> Vec<Vec<u8>> {
    let company = display_pdf_text("company name", &statement.company);
    let party = display_pdf_text("party name", &statement.party);
    let identity = format!("Party statement | Company: {company} | Party: {party}");
    let mut lines = wrap_to_printable_width(
        encode_win_ansi(&identity)
            .expect("header identity values were rendered for WinAnsi before layout"),
        true,
        BODY_FONT_SIZE as u16,
    );
    if lines.len() > HEADER_IDENTITY_LINE_LIMIT {
        lines.truncate(HEADER_IDENTITY_LINE_LIMIT - 1);
        lines.push(b"[Identity continued on first page]".to_vec());
    }
    lines
}

fn statement_lines(statement: &PartyStatement) -> Result<Vec<PdfLine>, PartyStatementPdfError> {
    let mut lines = Vec::new();
    push_wrapped(&mut lines, "Party statement", true)?;
    push_label_value(
        &mut lines,
        "Company",
        &display_pdf_text("company name", &statement.company),
    )?;
    push_label_value(
        &mut lines,
        "Party",
        &display_pdf_text("party name", &statement.party),
    )?;
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
            display_pdf_text("bill reference", &bill.reference),
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
    push_wrapped(&mut lines, "Ageing subtotals", true)?;
    for (bucket, subtotal) in [
        ("Not yet due", &statement.subtotals.not_yet_due),
        ("0-30 days", &statement.subtotals.days_0_30),
        ("31-60 days", &statement.subtotals.days_31_60),
        ("61-90 days", &statement.subtotals.days_61_90),
        ("90+ days", &statement.subtotals.days_90_plus),
    ] {
        push_label_value(&mut lines, bucket, &display_amount(subtotal)?)?;
    }
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
    if text.is_empty() {
        lines.push(PdfLine::body(Vec::new()));
        return Ok(());
    }
    let text = encode_win_ansi(text).unwrap_or_else(|| degraded_text("statement text", text));
    let font_size = if bold {
        HEADING_FONT_SIZE as u16
    } else {
        BODY_FONT_SIZE as u16
    };
    for chunk in wrap_to_printable_width(text, bold, font_size) {
        lines.push(if bold {
            PdfLine::bold(chunk)
        } else {
            PdfLine::body(chunk)
        });
    }
    Ok(())
}

/// Makes an unrepresentable source value conspicuous without inventing a
/// replacement name. The marker preserves each source scalar as a codepoint,
/// so operators can distinguish it from a real ledger name and reconcile it
/// against the source system.
fn display_pdf_text(field: &str, text: &str) -> String {
    if encode_win_ansi(text).is_some() {
        text.to_string()
    } else {
        String::from_utf8(degraded_text(field, text)).expect("marker is ASCII")
    }
}

fn degraded_text(field: &str, text: &str) -> Vec<u8> {
    let codepoints = text
        .chars()
        .map(|character| format!("U+{:04X}", character as u32))
        .collect::<Vec<_>>()
        .join(" ");
    format!("[{field} rendering degraded: {codepoints}]").into_bytes()
}

/// Encodes the complete PDF WinAnsi repertoire. PDF strings are bytes, not
/// UTF-8: writing raw UTF-8 under a Type 1 font gives each byte a different
/// glyph. C1 control characters intentionally have no mapping.
fn encode_win_ansi(text: &str) -> Option<Vec<u8>> {
    text.chars().map(win_ansi_byte).collect()
}

fn win_ansi_byte(character: char) -> Option<u8> {
    match character {
        '\u{20}'..='\u{7e}' => Some(character as u8),
        // WinAnsi maps U+00A0 to a visible space glyph and U+00AD to a visible
        // hyphen. Do not silently replace either source scalar with that glyph.
        '\u{a1}'..='\u{ac}' | '\u{ae}'..='\u{ff}' => Some(character as u8),
        '\u{20ac}' => Some(0x80),
        '\u{201a}' => Some(0x82),
        '\u{192}' => Some(0x83),
        '\u{201e}' => Some(0x84),
        '\u{2026}' => Some(0x85),
        '\u{2020}' => Some(0x86),
        '\u{2021}' => Some(0x87),
        '\u{2c6}' => Some(0x88),
        '\u{2030}' => Some(0x89),
        '\u{160}' => Some(0x8a),
        '\u{2039}' => Some(0x8b),
        '\u{152}' => Some(0x8c),
        '\u{17d}' => Some(0x8e),
        '\u{2018}' => Some(0x91),
        '\u{2019}' => Some(0x92),
        '\u{201c}' => Some(0x93),
        '\u{201d}' => Some(0x94),
        '\u{2022}' => Some(0x95),
        '\u{2013}' => Some(0x96),
        '\u{2014}' => Some(0x97),
        '\u{2dc}' => Some(0x98),
        '\u{2122}' => Some(0x99),
        '\u{161}' => Some(0x9a),
        '\u{203a}' => Some(0x9b),
        '\u{153}' => Some(0x9c),
        '\u{17e}' => Some(0x9e),
        '\u{178}' => Some(0x9f),
        _ => None,
    }
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

    fn expected_page_count(statement: &PartyStatement) -> usize {
        let body_lines_per_page =
            LINES_PER_PAGE - (1 + page_header_identity_lines(statement).len());
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
            let party = string_from_codepoints(&[
                0x0050, 0x0061, 0x0072, 0x0074, 0x0079, 0x0020, codepoint,
            ]);
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

        let pdf_text = extracted_text(&render_party_statement_pdf(&statement).unwrap());
        assert!(pdf_text.contains("Ageing subtotals"));
        for (bucket, subtotal) in [
            ("Not yet due", &statement.subtotals.not_yet_due),
            ("0-30 days", &statement.subtotals.days_0_30),
            ("31-60 days", &statement.subtotals.days_31_60),
            ("61-90 days", &statement.subtotals.days_61_90),
            ("90+ days", &statement.subtotals.days_90_plus),
        ] {
            assert!(pdf_text.contains(&format!(
                "{bucket}: INR {}",
                indian_grouped_decimal(subtotal.as_str())
            )));
        }

        let xlsx = render_party_statement_xlsx(&statement).unwrap();
        let xlsx_xml = xlsx_sheet_xml(&xlsx);
        assert!(xlsx_xml.contains("Ageing subtotals"));
        for subtotal in [
            &statement.subtotals.not_yet_due,
            &statement.subtotals.days_0_30,
            &statement.subtotals.days_31_60,
            &statement.subtotals.days_61_90,
            &statement.subtotals.days_90_plus,
        ] {
            assert!(xlsx_xml.contains(&format!("<v>{}</v>", subtotal.as_str())));
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
            build_party_statement("Synthetic Books Pvt Ltd", "20260808", &party, &bills, &[])
                .unwrap();

        let header_lines = page_header_identity_lines(&statement);
        assert!(header_lines.len() > 1);
        assert!(header_lines.len() <= HEADER_IDENTITY_LINE_LIMIT);
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
}
