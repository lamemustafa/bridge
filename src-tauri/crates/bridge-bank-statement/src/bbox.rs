//! Pages from `pdftotext -bbox-layout` XML.
//!
//! Not used on the production path. It exists so the sanitised real-statement
//! captures in `scripts/fixtures/*-bbox-capture.xml` — the only fixtures whose
//! geometry this repository did not write — can drive the same parser, and so
//! the Rust port can be compared with the Python reference on identical input.

use crate::geometry::{Page, Word};
use regex::Regex;
use std::sync::LazyLock;

fn unescape(text: &str) -> String {
    [
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&apos;", "'"),
        ("&amp;", "&"),
    ]
    .iter()
    .fold(text.to_string(), |text, (entity, character)| {
        text.replace(entity, character)
    })
}

/// Every `<page ...>` of a capture, words in document order.
pub fn read_pages(xml: &str) -> Vec<Page> {
    static WORD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?s)<word xMin="([\d.]+)" yMin="([\d.]+)" xMax="([\d.]+)" yMax="([\d.]+)">(.*?)</word>"#,
        )
        .unwrap()
    });
    xml.split("<page ")
        .skip(1)
        .map(|page| {
            WORD.captures_iter(page)
                .filter_map(|found| {
                    Some(Word::new(
                        found[1].parse().ok()?,
                        found[2].parse().ok()?,
                        found[3].parse().ok()?,
                        found[4].parse().ok()?,
                        unescape(&found[5]),
                    ))
                })
                .collect()
        })
        .collect()
}
