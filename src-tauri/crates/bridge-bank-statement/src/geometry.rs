//! Word boxes, visual lines and the cell-wrap heuristic.
//!
//! Coordinates are PDF points with the origin at the **top-left** of the page
//! and y growing downwards, which is what `pdftotext -bbox-layout` reports and
//! what every column bound in [`crate::bank`] was calibrated against.
//! [`crate::pdf`] converts PDFium's bottom-left page space into this one.

use crate::text::{squash, strip};

/// One word as the PDF laid it out.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub text: String,
}

impl Word {
    pub fn new(x0: f64, y0: f64, x1: f64, y1: f64, text: impl Into<String>) -> Self {
        Self {
            x0,
            y0,
            x1,
            y1,
            text: text.into(),
        }
    }
}

/// The words of one page, in any order.
pub type Page = Vec<Word>;

/// A visual line: the y of its first word, and its words left to right.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub y: f64,
    pub words: Vec<Word>,
}

/// Python's `round(value, 1)`: correctly rounded on the exact binary value,
/// ties to even. Rust's precision formatting rounds the same way.
fn round_one(value: f64) -> f64 {
    format!("{value:.1}").parse().unwrap_or(value)
}

/// Words grouped into visual lines, each sorted left to right (`_lines`).
///
/// A word joins the **first** existing line whose first word sits within 3pt
/// of it, so the grouping depends on visiting words top to bottom; that is why
/// they are sorted by rounded y before grouping, as the reference does.
pub fn lines(page: &[Word]) -> Vec<Line> {
    let mut words: Vec<Word> = page
        .iter()
        .filter_map(|word| {
            let text = strip(&word.text);
            (!text.is_empty()).then(|| Word {
                text: text.to_string(),
                ..word.clone()
            })
        })
        .collect();
    words.sort_by(|left, right| {
        round_one(left.y0)
            .total_cmp(&round_one(right.y0))
            .then(left.x0.total_cmp(&right.x0))
    });
    let mut grouped: Vec<Line> = Vec::new();
    for word in words {
        match grouped
            .iter_mut()
            .find(|line| (line.y - word.y0).abs() < 3.0)
        {
            Some(line) => line.words.push(word),
            None => grouped.push(Line {
                y: word.y0,
                words: vec![word],
            }),
        }
    }
    for line in &mut grouped {
        line.words
            .sort_by(|left, right| left.x0.total_cmp(&right.x0));
    }
    grouped.sort_by(|left, right| left.y.total_cmp(&right.y));
    grouped
}

/// Default tolerance for [`dewrap`], in points.
pub const WRAP_TOLERANCE: f64 = 4.0;

/// Rejoin cell-wrapped text (`_dewrap`). `fragments` are `(text, x_max)` per
/// printed line, in visual order.
///
/// A fragment whose right edge reaches the cell boundary was broken mid-token,
/// so the next fragment joins with no separator; anything shorter ended at a
/// real space. Its one failure mode is a line that happens to end at a space
/// exactly at the boundary, which costs a space in display text but never
/// corrupts a reference number — and that is the property this exists for: a
/// 12-digit UPI/UTR reference split across two lines must rejoin intact.
pub fn dewrap(fragments: &[(String, f64)], right_edge: f64, tolerance: f64) -> String {
    let mut out = String::new();
    for (index, (text, _)) in fragments.iter().enumerate() {
        if index > 0 {
            let hard = fragments[index - 1].1 >= right_edge - tolerance;
            if !hard {
                out.push(' ');
            }
        }
        out.push_str(text);
    }
    squash(&out)
}

/// Does this line carry every word of any one anchor group (`_matches`)?
///
/// Whole groups rather than single words, so a counterparty called
/// "... LIMITED" is not mistaken for the "HDFC BANK LIMITED" footer.
pub fn matches(line: &Line, anchors: &[&[&str]]) -> bool {
    anchors.iter().any(|anchor| {
        anchor
            .iter()
            .all(|token| line.words.iter().any(|word| word.text == *token))
    })
}
