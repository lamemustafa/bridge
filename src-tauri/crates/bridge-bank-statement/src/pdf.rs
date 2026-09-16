//! Word boxes from a password-protected PDF, through a bundled PDFium.
//!
//! Replaces the reference's `pdftotext -bbox-layout -upw/-opw`. Two differences
//! matter:
//!
//! * **One password attempt covers both passwords.** PDFium's standard security
//!   handler checks a supplied password as the user password and then as the
//!   owner password, so an owner-password-only statement (which `pdftotext
//!   -upw` rejects) opens without a second attempt. The owner-password-only
//!   fixture is the test of that claim.
//! * **Words are assembled here from characters.** PDFium reports characters
//!   with boxes; `pdftotext` reported words. A word ends at whitespace (PDFium's
//!   own generated spaces included), at a change of baseline, or at a horizontal
//!   gap wider than a tenth of the font size — poppler's `minWordBreakSpace`.
//!   Character boxes are PDFium's *loose* boxes (font ascent to descent, advance
//!   width), the nearest equivalent of poppler's word boxes; tight glyph boxes
//!   would move a fragment's right edge by up to a glyph's side bearing, which
//!   is the quantity the wrap heuristic reads.
//!
//!   Measured on the synthetic fixtures: every horizontal edge agrees with
//!   `pdftotext` to within 0.01pt. Vertical edges are shifted by a constant per
//!   font — 1.43pt on 7pt Courier — because PDFium substitutes a system face for
//!   a non-embedded base-14 font and takes its ascent from that face, where
//!   poppler uses the standard AFM metrics. A shift that is uniform within a
//!   font changes neither line grouping nor anchors; a line mixing two
//!   non-embedded fonts could group differently from `pdftotext`, which the
//!   balance replay would then have to catch. Embedded fonts were not measured.
//!
//! Coordinates are converted to the top-left page space of `pdftotext`,
//! relative to the crop box (or the media box when a page has no crop box).
//! A rotated page is refused rather than guessed at: every column bound assumes
//! the page is upright.
//!
//! The password is borrowed, never stored, and never formatted into a message.
//! Residual, stated rather than assumed: `pdfium-render` copies it into a
//! `CString` for the FFI call and PDFium keeps its own copy inside the open
//! document; neither copy is zeroised.

use crate::geometry::{Page, Word};
use crate::refusal::Refusal;
use crate::text::is_space;
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// A statement larger than this is refused before PDFium parses it.
pub const MAX_PDF_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_PAGES: i32 = 500;
pub const MAX_CHARS_PER_PAGE: usize = 250_000;

/// PDFium, bound once per process.
///
/// `pdfium-render` keeps the library bindings in a process-wide cell, so a
/// second library path cannot be bound after the first; asking for one is
/// refused instead of silently using the first.
pub struct PdfEngine {
    pdfium: Pdfium,
}

impl std::fmt::Debug for PdfEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PdfEngine")
    }
}

static ENGINE: OnceLock<(PathBuf, Result<PdfEngine, String>)> = OnceLock::new();

/// The PDFium library's platform file name (`libpdfium.dylib`, `pdfium.dll`,
/// `libpdfium.so`) inside `directory`.
pub fn platform_library_path(directory: &Path) -> PathBuf {
    Pdfium::pdfium_platform_library_name_at_path(directory)
}

/// The shared engine, loading PDFium from `library` on first use.
pub fn engine(library: &Path) -> Result<&'static PdfEngine, Refusal> {
    let (bound_path, bound) = ENGINE.get_or_init(|| {
        let loaded = Pdfium::bind_to_library(library)
            .map(|bindings| PdfEngine {
                pdfium: Pdfium::new(bindings),
            })
            .map_err(|error| format!("{error:?}"));
        (library.to_path_buf(), loaded)
    });
    if bound_path != library {
        return Err(Refusal::new(
            "pdf_engine_unavailable",
            "PDFium is already bound from a different library path in this process",
        ));
    }
    bound.as_ref().map_err(|error| {
        Refusal::new(
            "pdf_engine_unavailable",
            format!("the bundled PDFium library could not be loaded ({error})"),
        )
    })
}

struct Glyph {
    character: char,
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    size: f64,
}

fn close_word(glyphs: &mut Vec<Glyph>, words: &mut Page) {
    if glyphs.is_empty() {
        return;
    }
    let mut word = Word::new(
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
        String::new(),
    );
    for glyph in glyphs.iter() {
        word.x0 = word.x0.min(glyph.left);
        word.x1 = word.x1.max(glyph.right);
        word.y0 = word.y0.min(glyph.top);
        word.y1 = word.y1.max(glyph.bottom);
        word.text.push(glyph.character);
    }
    words.push(word);
    glyphs.clear();
}

/// Split a page's characters, in PDFium's text order, into words.
fn assemble_words(glyphs: impl IntoIterator<Item = Option<Glyph>>) -> Page {
    let mut words = Page::new();
    let mut current: Vec<Glyph> = Vec::new();
    for glyph in glyphs {
        let Some(glyph) = glyph else {
            close_word(&mut current, &mut words);
            continue;
        };
        if is_space(glyph.character) || glyph.character.is_control() {
            close_word(&mut current, &mut words);
            continue;
        }
        if let Some(previous) = current.last() {
            let size = previous.size.max(glyph.size).max(1.0);
            let gap = glyph.left - previous.right;
            let new_baseline = (glyph.bottom - previous.bottom).abs() > 0.5 * size;
            if new_baseline || gap > 0.1 * size || gap < -0.5 * size {
                close_word(&mut current, &mut words);
            }
        }
        current.push(glyph);
    }
    close_word(&mut current, &mut words);
    words
}

/// The character a glyph printed.
///
/// PDFium reports a hyphen that ends a line as U+0002, its hyphenation marker,
/// with the hyphen's own geometry. Statement narrations wrap at hyphens
/// constantly (`NEFT DR-ZZZZ0000001-` / `ACME…`), and every HDFC party boundary
/// is a hyphen, so the marker is read back as the hyphen that was printed.
/// Residual: a line-ending soft hyphen (U+00AD) arrives the same way and is
/// also read as `-`.
fn printed_character(reported: Option<char>) -> char {
    match reported {
        Some('\u{2}') => '-',
        Some(character) => character,
        None => '\u{fffd}',
    }
}

fn unreadable(message: &str) -> Refusal {
    Refusal::new("unreadable_pdf", message)
}

/// Every page's words, top-left coordinates.
pub fn extract_pages(engine: &PdfEngine, pdf: &[u8], password: &str) -> Result<Vec<Page>, Refusal> {
    if pdf.len() > MAX_PDF_BYTES {
        return Err(unreadable(
            "the statement is larger than the accepted PDF size",
        ));
    }
    if password.contains('\0') {
        // pdfium-render panics converting such a password to a C string
        return Err(Refusal::new(
            "unusable_password",
            "the password contains a NUL character, which a PDF password cannot carry",
        ));
    }
    let document = engine
        .pdfium
        .load_pdf_from_byte_slice(pdf, Some(password))
        .map_err(|error| match error {
            PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
                unreadable(
                    "cannot open the statement: the password is not its user or owner password",
                )
            }
            _ => unreadable("cannot open the statement: it is not a PDF PDFium can read"),
        })?;
    let pages = document.pages();
    if pages.len() > MAX_PAGES {
        return Err(unreadable(
            "the statement has more pages than the accepted limit",
        ));
    }
    let mut out = Vec::new();
    for page in pages.iter() {
        let rotation = page
            .rotation()
            .map_err(|_| unreadable("a page's rotation could not be read"))?;
        if rotation != PdfPageRenderRotation::None {
            return Err(Refusal::new(
                "unsupported_page_rotation",
                "a page is rotated; the column geometry assumes an upright page",
            ));
        }
        let boundary = page
            .boundaries()
            .crop()
            .or_else(|_| page.boundaries().media())
            .map_err(|_| unreadable("a page has no readable crop or media box"))?;
        let origin_left = f64::from(boundary.bounds.left().value);
        let origin_top = f64::from(boundary.bounds.top().value);
        let text = page
            .text()
            .map_err(|_| unreadable("a page's text could not be extracted"))?;
        let chars = text.chars();
        if chars.len() > MAX_CHARS_PER_PAGE {
            return Err(unreadable(
                "a page carries more characters than the accepted limit",
            ));
        }
        let glyphs = chars.iter().map(|character| {
            let bounds = character.loose_bounds().ok()?;
            Some(Glyph {
                character: printed_character(character.unicode_char()),
                left: f64::from(bounds.left().value) - origin_left,
                right: f64::from(bounds.right().value) - origin_left,
                top: origin_top - f64::from(bounds.top().value),
                bottom: origin_top - f64::from(bounds.bottom().value),
                size: f64::from(character.scaled_font_size().value),
            })
        });
        out.push(assemble_words(glyphs));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(character: char, left: f64, bottom: f64) -> Option<Glyph> {
        Some(Glyph {
            character,
            left,
            right: left + 4.2,
            top: bottom - 5.5,
            bottom,
            size: 7.0,
        })
    }

    #[test]
    fn words_break_at_space_gap_and_baseline_but_not_between_adjacent_glyphs() {
        let words = assemble_words([
            glyph('A', 10.0, 20.0),
            glyph('B', 14.2, 20.0),
            glyph(' ', 18.4, 20.0),
            glyph('C', 22.6, 20.0),
            // a gap wider than a tenth of the font size
            glyph('D', 27.6, 20.0),
            // a new baseline directly below
            glyph('E', 31.8, 32.0),
        ]);
        let texts: Vec<&str> = words.iter().map(|word| word.text.as_str()).collect();
        assert_eq!(texts, ["AB", "C", "D", "E"]);
        assert!((words[0].x1 - 18.4).abs() < 1e-9);
        assert!((words[0].y0 - 14.5).abs() < 1e-9);
    }
}
