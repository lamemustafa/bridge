use std::borrow::Cow;

pub(super) fn sanitize_invalid_numeric_references(xml: &str) -> Cow<'_, str> {
    sanitize_invalid_numeric_references_with_marker_search_observer(xml, || {})
}

fn sanitize_invalid_numeric_references_with_marker_search_observer(
    xml: &str,
    mut observe_replacement_marker_search: impl FnMut(),
) -> Cow<'_, str> {
    let mut scan = 0_usize;
    let mut copy_from = 0_usize;
    let mut output = None::<String>;
    observe_replacement_marker_search();
    let mut replacement_marker = xml.find('\u{fffd}');
    while scan < xml.len() {
        if replacement_marker.is_some_and(|marker| marker < scan) {
            observe_replacement_marker_search();
            replacement_marker = xml[scan..].find('\u{fffd}').map(|offset| scan + offset);
        }
        let numeric_reference = xml[scan..].find("&#").map(|offset| scan + offset);
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
        if parsed.is_some_and(|value| !is_xml_10_char(value) || value == 0xfffd) {
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
    use serde::Deserialize;

    use super::{
        sanitize_invalid_numeric_references,
        sanitize_invalid_numeric_references_with_marker_search_observer,
    };

    #[derive(Debug, Deserialize)]
    struct TextValue {
        #[serde(rename = "$text")]
        value: String,
    }

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
    fn numeric_references_remain_injective_after_xml_deserialisation() {
        let parse = |xml| {
            quick_xml::de::from_str::<TextValue>(&sanitize_invalid_numeric_references(xml))
                .expect("sanitised XML deserialises like parser.rs")
                .value
        };

        let illegal_reference = parse("<A>ACME&#4;LTD</A>");
        let literal_encoded_form = parse("<A>ACME\u{fffd}#4;LTD</A>");
        let decimal_replacement_reference = parse("<A>ACME&#65533;#4;LTD</A>");
        let hex_replacement_reference = parse("<A>ACME&#xFFFD;#4;LTD</A>");

        assert_eq!(illegal_reference, "ACME\u{fffd}#4;LTD");
        assert_eq!(literal_encoded_form, "ACME\u{fffd}#65533;#4;LTD");
        assert_ne!(illegal_reference, literal_encoded_form);
        assert_eq!(decimal_replacement_reference, literal_encoded_form);
        assert_eq!(hex_replacement_reference, literal_encoded_form);
    }

    #[test]
    fn no_marker_search_work_stays_constant_as_numeric_references_grow() {
        let replacement_marker_searches = |references| {
            let xml = format!("<A>{}</A>", "&#4;".repeat(references));
            let mut searches = 0usize;
            let _ = sanitize_invalid_numeric_references_with_marker_search_observer(&xml, || {
                searches += 1;
            });
            searches
        };

        let small = replacement_marker_searches(1_000);
        let large = replacement_marker_searches(20_000);
        assert!(
            large <= small + 1,
            "a no-marker input must not rescan for U+FFFD once per numeric reference: {small} -> {large}"
        );
    }
}
