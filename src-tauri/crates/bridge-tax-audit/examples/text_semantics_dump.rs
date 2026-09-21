// SPDX-License-Identifier: Apache-2.0
//! Print this toolchain's per-code-point text properties for `parity/text_semantics.py`: a header
//! line with the Unicode version, then one line per code point,
//! `CODEPOINT<TAB>ALPHANUMERIC<TAB>WHITESPACE<TAB>UPPER<TAB>LOWER` (flags 0/1, mappings in hex).

fn hex(s: impl Iterator<Item = char>) -> String {
    s.map(|c| format!("{:X}", u32::from(c)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let (a, b, c) = char::UNICODE_VERSION;
    println!("Rust char tables, Unicode {a}.{b}.{c}");
    for cp in 0u32..=0x0010_FFFF {
        if let Some(ch) = char::from_u32(cp) {
            println!(
                "{cp:X}\t{}\t{}\t{}\t{}",
                u8::from(ch.is_alphanumeric()),
                u8::from(ch.is_whitespace()),
                hex(ch.to_uppercase()),
                hex(ch.to_lowercase())
            );
        }
    }
}
