use std::borrow::Cow;

pub(super) fn sanitize_invalid_numeric_references(xml: &str) -> Cow<'_, str> {
    let mut scan = 0_usize;
    let mut copy_from = 0_usize;
    let mut output = None::<String>;
    while scan < xml.len() {
        let numeric_reference = xml[scan..].find("&#").map(|offset| scan + offset);
        let replacement_marker = xml[scan..].find('\u{fffd}').map(|offset| scan + offset);
        let Some(start) = [numeric_reference, replacement_marker]
            .into_iter()
            .flatten()
            .min()
        else {
            break;
        };

        if replacement_marker == Some(start) {
            let target = output.get_or_insert_with(|| String::with_capacity(xml.len()));
            target.push_str(&xml[copy_from..start]);
            // U+FFFD is XML-legal source text, so it cannot be used as an
            // unescaped marker for an illegal reference. Encode literal U+FFFD
            // through the same grammar; this keeps the transformation
            // injective even for source containing the previous `\u{fffd}#4;`
            // representation.
            target.push('\u{fffd}');
            target.push_str("#65533;");
            copy_from = start + '\u{fffd}'.len_utf8();
            scan = copy_from;
            continue;
        }

        let Some(relative_end) = xml[start + 2..].find(';') else {
            scan = start + 2;
            continue;
        };
        let end = start + 2 + relative_end;
        if end - start > 12 {
            scan = start + 2;
            continue;
        }
        let token = &xml[start + 2..end];
        let parsed = token
            .strip_prefix('x')
            .or_else(|| token.strip_prefix('X'))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .or_else(|| token.parse::<u32>().ok());
        if parsed.is_some_and(|value| !is_xml_10_char(value)) {
            let target = output.get_or_insert_with(|| String::with_capacity(xml.len()));
            target.push_str(&xml[copy_from..start]);
            // Preserve the numeric identity in a self-escaping marker.
            // Mapping every illegal code point to bare U+FFFD is lossy, and a
            // marker that leaves literal U+FFFD untouched collides with source
            // text already holding that marker. Literal U+FFFD is encoded
            // above as `U+FFFD#65533;`, so every emitted `U+FFFD#<n>;` denotes
            // exactly one source atom while remaining XML-1.0 legal.
            target.push('\u{fffd}');
            target.push_str(&format!("#{};", parsed.unwrap_or_default()));
            copy_from = end + 1;
        }
        scan = end + 1;
    }
    if let Some(mut output) = output {
        output.push_str(&xml[copy_from..]);
        Cow::Owned(output)
    } else {
        Cow::Borrowed(xml)
    }
}

fn is_xml_10_char(value: u32) -> bool {
    matches!(value, 0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF)
}

#[cfg(test)]
mod tests {
    use quick_xml::{events::Event, Reader};

    use super::sanitize_invalid_numeric_references;

    #[test]
    fn real_invalid_character_reference_is_narrowly_repaired() {
        let capture = include_str!("../../tests/fixtures/unit_a_invalid_char_ref_live.xml");
        assert!(capture.contains("&#4;"));
        let sanitized = sanitize_invalid_numeric_references(capture);
        assert!(!sanitized.contains("&#4;"));
        assert!(sanitized.contains('\u{fffd}'));
        let mut reader = Reader::from_str(&sanitized);
        loop {
            if matches!(
                reader.read_event().expect("all other XML remains strict"),
                Event::Eof
            ) {
                break;
            }
        }
    }

    #[test]
    fn legal_numeric_reference_is_not_changed() {
        let input = "<A>&#9;&#x20;</A>";
        assert!(matches!(
            sanitize_invalid_numeric_references(input),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn illegal_reference_and_its_literal_encoded_form_remain_distinct() {
        let illegal_reference = "<A>ACME&#4;LTD</A>";
        let literal_encoded_form = "<A>ACME\u{fffd}#4;LTD</A>";

        assert_ne!(
            sanitize_invalid_numeric_references(illegal_reference),
            sanitize_invalid_numeric_references(literal_encoded_form),
            "a source literal marker must not collide with an illegal reference"
        );
        assert_eq!(
            sanitize_invalid_numeric_references(literal_encoded_form),
            "<A>ACME\u{fffd}#65533;#4;LTD</A>"
        );
    }
}
