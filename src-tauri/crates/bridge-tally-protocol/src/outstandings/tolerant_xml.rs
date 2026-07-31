use std::borrow::Cow;

pub(super) fn sanitize_invalid_numeric_references(xml: &str) -> Cow<'_, str> {
    let mut scan = 0_usize;
    let mut copy_from = 0_usize;
    let mut output = None::<String>;
    while let Some(offset) = xml[scan..].find("&#") {
        let start = scan + offset;
        let Some(relative_end) = xml[start + 2..].find(';') else {
            break;
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
            target.push('\u{fffd}');
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
}
