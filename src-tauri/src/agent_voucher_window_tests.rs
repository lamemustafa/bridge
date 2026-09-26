use super::*;

// The four tests below moved here from `agent_import_tests.rs` with the #485
// helpers they pin, unchanged: the helpers now serve every bounded window read,
// not only the verification read.

#[test]
fn a_window_span_counts_both_endpoints() {
    assert_eq!(window_span_days("20260401", "20260401"), Some(1));
    assert_eq!(window_span_days("20260401", "20260402"), Some(2));
    assert_eq!(window_span_days("20260401", "20270331"), Some(365));
    // A leap year is counted by the calendar, not by arithmetic on months.
    assert_eq!(window_span_days("20240101", "20241231"), Some(366));
    assert_eq!(window_span_days("notadate", "20260401"), None);
}

#[test]
fn a_span_already_known_to_fail_is_split_without_another_read() {
    // The point of carrying the failed span forward: a sibling branch of the same
    // size is split immediately rather than spending a full deadline to relearn it.
    assert!(must_split_before_reading(Some(91), Some(91)));
    assert!(must_split_before_reading(Some(182), Some(91)));
    // Smaller than anything known to fail: read it, do not pre-split.
    assert!(!must_split_before_reading(Some(45), Some(91)));
    // Nothing has failed yet, so nothing is known: always read.
    assert!(!must_split_before_reading(Some(365), None));
    // A single day is the floor. Pre-splitting it would spin, and refusing is the
    // reader's job, not this predicate's.
    assert!(!must_split_before_reading(Some(1), Some(1)));
    // An unparseable span falls through to reading rather than being treated as a
    // failure: this is an optimisation and must never decide correctness.
    assert!(!must_split_before_reading(None, Some(30)));
}

#[test]
fn splitting_a_verification_window_partitions_it_exactly() {
    // Every split must cover the original window once and only once. A gap drops
    // vouchers from an attribution check; an overlap double-counts them.
    for (from, to) in [
        ("20260401", "20270331"), // a full financial year
        ("20260401", "20260430"), // a month
        ("20260401", "20260402"), // two days: mid must equal start
        ("20260228", "20260301"), // across a month boundary
        ("20240228", "20240301"), // across a leap day
        ("20251231", "20260101"), // across a year boundary
    ] {
        let ((left_from, left_to), (right_from, right_to)) =
            split_verification_window(from, to).expect("a multi-day window splits");
        assert_eq!(left_from, from, "left half must start where the window did");
        assert_eq!(right_to, to, "right half must end where the window did");
        let day = |value: &str| chrono::NaiveDate::parse_from_str(value, "%Y%m%d").unwrap();
        // Contiguous, no gap and no overlap: the right half starts exactly the day
        // after the left half ends.
        assert_eq!(
            day(&right_from),
            day(&left_to) + chrono::Duration::days(1),
            "{from}..{to} split with a gap or an overlap"
        );
        // And it must actually shrink, or the splitter would never terminate.
        assert!(
            day(&left_to) < day(to),
            "left half did not shrink {from}..{to}"
        );
        assert!(
            day(&right_from) > day(from),
            "right half did not shrink {from}..{to}"
        );
    }
}

#[test]
fn a_single_day_verification_window_cannot_be_split() {
    // The recursion floor. Without it the splitter would spin on a day it cannot
    // read; with it, read_verification_window refuses rather than returning a
    // verification over an incomplete window.
    assert_eq!(split_verification_window("20260401", "20260401"), None);
    // A reversed window is refused rather than silently inverted.
    assert_eq!(split_verification_window("20260430", "20260401"), None);
    // An unparseable bound is refused rather than guessed at.
    assert_eq!(split_verification_window("notadate", "20260401"), None);
}

// ---------------------------------------------------------------------------
// The pre-flight volume bound (protocol reference §11c).
// ---------------------------------------------------------------------------

use tally_protocol_simulator::{
    Fixture, ProductStatus, ResponseFraming, ScenarioPlan, SequenceSimulator, WireEncoding,
};

fn day(value: &str) -> NaiveDate {
    NaiveDate::parse_from_str(value, "%Y%m%d").unwrap()
}

/// A census from `(day, voucher count)`, with AlterIDs numbered in date order.
fn census_of(days: &[(&str, u64)]) -> WindowCensus {
    let mut next = 0;
    WindowCensus::from_rows(days.iter().flat_map(|(date, n)| {
        (0..*n)
            .map(|_| {
                next += 1;
                (day(date), next)
            })
            .collect::<Vec<_>>()
    }))
}

fn plan(
    from: &str,
    to: &str,
    census: &WindowCensus,
    bytes_per_voucher: u64,
    budget: u64,
    max_reads: usize,
) -> Result<Vec<PlannedRead>, PlanRefusal> {
    plan_window_reads(
        day(from),
        day(to),
        census,
        None,
        census.max_alter_id(),
        bytes_per_voucher,
        budget,
        max_reads,
    )
}

/// Every plan must cover its window exactly: consecutive date ranges with no
/// gap or overlap, and a day read in spans covered by spans that tile
/// `(0, ceiling]` for that day. A gap drops vouchers; an overlap reads them twice.
fn assert_tiles(plan: &[PlannedRead], from: &str, to: &str, ceiling: u64) {
    assert_eq!(plan.first().unwrap().from, day(from), "plan starts late");
    assert_eq!(plan.last().unwrap().to, day(to), "plan ends early");
    let mut index = 0;
    while index < plan.len() {
        let read = plan[index];
        let mut end = read.to;
        if let Some(span) = read.span {
            assert_eq!(read.from, read.to, "a span covers one day");
            assert_eq!(span.after, 0, "a day's spans start at the bottom");
            let mut through = span.through;
            while index + 1 < plan.len()
                && plan[index + 1].span.is_some()
                && plan[index + 1].from == read.from
            {
                index += 1;
                let next = plan[index].span.unwrap();
                assert_eq!(next.after, through, "gap or overlap in {:?}", read.from);
                through = next.through;
            }
            assert_eq!(through, ceiling, "a day's spans stop short of the ceiling");
            end = read.from;
        }
        if index + 1 < plan.len() {
            assert_eq!(
                plan[index + 1].from,
                end.succ_opt().unwrap(),
                "gap or overlap after {end}"
            );
        }
        index += 1;
    }
}

#[test]
fn a_window_predicted_within_budget_is_planned_as_itself() {
    // 30 vouchers at 1 KiB against a 64 KiB budget: one read, and that read is
    // the caller's own window — which is how a small book keeps its request.
    let census = census_of(&[("20260402", 10), ("20260415", 20)]);
    let reads = plan("20260401", "20260430", &census, 1024, 64 * 1024, 8).unwrap();
    assert_eq!(
        reads,
        [PlannedRead {
            from: day("20260401"),
            to: day("20260430"),
            span: None,
            vouchers: 30
        }]
    );
    // An empty window is one read of zero vouchers, not no read: emptiness
    // still has to be observed.
    let reads = plan("20260401", "20260430", &census_of(&[]), 1024, 64 * 1024, 8).unwrap();
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].vouchers, 0);
}

#[test]
fn a_window_over_budget_is_divided_greedily_and_exactly() {
    // Capacity is 10 vouchers a read. Days of 6, 4, 7, 2, 9 pack as
    // [6+4], [7+2], [9]; the empty days between them ride along.
    let census = census_of(&[
        ("20260403", 6),
        ("20260404", 4),
        ("20260410", 7),
        ("20260411", 2),
        ("20260420", 9),
    ]);
    let reads = plan("20260401", "20260430", &census, 100, 1000, 8).unwrap();
    assert_tiles(&reads, "20260401", "20260430", census.max_alter_id());
    assert_eq!(
        reads.iter().map(|read| read.vouchers).collect::<Vec<_>>(),
        [10, 9, 9]
    );
    assert_eq!(reads[0].to, day("20260409"));
    assert_eq!(reads[1].to, day("20260419"));
    assert!(reads.iter().all(|read| read.span.is_none()));
    // Exactly at capacity still fits; one more voucher does not.
    let exact = census_of(&[("20260401", 5), ("20260402", 5)]);
    assert_eq!(
        plan("20260401", "20260402", &exact, 100, 1000, 8)
            .unwrap()
            .len(),
        1
    );
    let over = census_of(&[("20260401", 5), ("20260402", 6)]);
    let reads = plan("20260401", "20260402", &over, 100, 1000, 8).unwrap();
    assert_tiles(&reads, "20260401", "20260402", over.max_alter_id());
    assert_eq!(reads.len(), 2);
}

#[test]
fn a_day_too_heavy_for_one_read_is_divided_by_alterid_not_refused() {
    // Capacity 10. The 25-voucher day is read alone, in AlterID spans of that
    // day holding 10, 10 and 5; the days around it are read by date.
    let census = census_of(&[("20260401", 3), ("20260415", 25), ("20260420", 2)]);
    let ceiling = 40;
    let reads = plan_window_reads(
        day("20260401"),
        day("20260430"),
        &census,
        None,
        ceiling,
        100,
        1000,
        8,
    )
    .unwrap();
    assert_tiles(&reads, "20260401", "20260430", ceiling);
    let spans = reads
        .iter()
        .filter(|read| read.span.is_some())
        .collect::<Vec<_>>();
    assert_eq!(
        spans.iter().map(|read| read.vouchers).collect::<Vec<_>>(),
        [10, 10, 5]
    );
    assert!(spans.iter().all(|read| read.from == day("20260415")));
    // Each span selects exactly the census's vouchers it was planned for.
    for read in &spans {
        let span = read.span.unwrap();
        let held = census.ids_on(day("20260415"), Some(span)).len() as u64;
        assert_eq!(held, read.vouchers);
        assert!(held * 100 <= 1000);
    }
    // The last span reaches the ceiling: an AlterID above the census's highest
    // is still inside some span while the mark is unchanged.
    assert_eq!(spans.last().unwrap().span.unwrap().through, ceiling);
}

#[test]
fn only_a_single_voucher_over_budget_is_refused() {
    let one = census_of(&[("20260401", 1)]);
    let refusal = plan("20260401", "20260401", &one, 2000, 1000, 8).unwrap_err();
    assert_eq!(
        refusal,
        PlanRefusal::VoucherOverBudget {
            day: day("20260401")
        }
    );
    assert_eq!(refusal.code(), "voucher_window_part_over_budget");
    // At exactly the budget one voucher is one read.
    assert_eq!(
        plan("20260401", "20260401", &one, 1000, 1000, 8)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_window_needing_more_reads_than_allowed_is_refused() {
    let census = census_of(&[("20260401", 10), ("20260402", 10), ("20260403", 10)]);
    let refusal = plan("20260401", "20260403", &census, 100, 1000, 2).unwrap_err();
    assert_eq!(refusal, PlanRefusal::TooManyReads { reads: 3 });
    assert_eq!(refusal.code(), "voucher_window_too_many_reads");
}

#[test]
fn a_partly_read_day_resumes_above_the_last_alterid_read() {
    // Day one holds AlterIDs 1..=5, day two 6..=7. After a span through 2 on day
    // one, the rest of day one is read from 2 upward, never from 0 again.
    let census = census_of(&[("20260401", 5), ("20260402", 2)]);
    let reads = plan_window_reads(
        day("20260401"),
        day("20260402"),
        &census,
        Some(2),
        7,
        100,
        1000,
        8,
    )
    .unwrap();
    assert_eq!(
        reads,
        [
            PlannedRead {
                from: day("20260401"),
                to: day("20260401"),
                span: Some(AlterIdSpan {
                    after: 2,
                    through: 7
                }),
                vouchers: 3
            },
            PlannedRead {
                from: day("20260402"),
                to: day("20260402"),
                span: None,
                vouchers: 2
            }
        ]
    );
}

/// A year of the heaviest book measured (§11c): 16,367 vouchers, spread evenly
/// at about 45 a day, and one day of 450 invoices imported in bulk.
fn heavy_book() -> WindowCensus {
    let mut rows = Vec::new();
    let mut date = day("20250401");
    let mut next = 0_u64;
    let mut left = 16_367_u64 - 450;
    while left > 0 {
        let today = left.min(45);
        for _ in 0..today {
            next += 1;
            rows.push((date, next));
        }
        left -= today;
        date = date.succ_opt().unwrap();
    }
    for _ in 0..450 {
        next += 1;
        rows.push((day("20250615"), next));
    }
    WindowCensus::from_rows(rows)
}

fn assert_heavy_book_divides(from: &str, to: &str, measured_wire: u64) -> usize {
    let census = heavy_book();
    let reads = plan_window_reads(
        day(from),
        day(to),
        &census,
        None,
        census.max_alter_id(),
        measured_wire,
        WINDOW_READ_BUDGET_BYTES,
        MAX_PLANNED_READS,
    )
    .unwrap();
    assert_tiles(&reads, from, to, census.max_alter_id());
    for read in &reads {
        assert!(
            read.vouchers * measured_wire <= WINDOW_READ_BUDGET_BYTES,
            "{read:?}"
        );
    }
    reads.len()
}

#[test]
fn the_heavy_book_divides_at_its_measured_named_field_cost() {
    // 35 KB of UTF-8 per voucher is 70 KB on the wire. A year divides, and the
    // day of 450 invoices is read in AlterID spans rather than refused — a bulk
    // import verification of one day's invoices must still be possible.
    let year = assert_heavy_book_divides("20250401", "20260331", 70_000);
    assert!(year > 1 && year <= MAX_PLANNED_READS);
    let bulk_day = assert_heavy_book_divides("20250615", "20250615", 70_000);
    assert!(bulk_day >= 2, "450 invoices at 70 KB do not fit one read");
}

#[test]
fn the_heavy_book_divides_at_its_measured_wildcard_cost() {
    // ~128 KB of UTF-8 per voucher with the entry wildcard: 256 KB on the wire,
    // 65 vouchers a read. A month divides; ordinary days of 45 are read whole
    // by date, and the bulk day in spans — neither is refused.
    let month = assert_heavy_book_divides("20250601", "20250630", 256_000);
    assert!(month > 1);
    assert!(assert_heavy_book_divides("20250602", "20250602", 256_000) == 1);
    assert!(assert_heavy_book_divides("20250615", "20250615", 256_000) >= 7);
    // A whole year of the wildcard is more reads than one call may spend: that
    // is refused by name, before any data read, rather than sent.
    let census = heavy_book();
    assert!(matches!(
        plan_window_reads(
            day("20250401"),
            day("20260331"),
            &census,
            None,
            census.max_alter_id(),
            256_000,
            WINDOW_READ_BUDGET_BYTES,
            MAX_PLANNED_READS,
        ),
        Err(PlanRefusal::TooManyReads { .. })
    ));
}

#[test]
fn the_conservative_defaults_stay_above_every_measured_per_voucher_cost() {
    for shape in [
        VoucherReadShape::ImportVerification,
        VoucherReadShape::Movement,
    ] {
        assert!(shape.default_wire_bytes_per_voucher() > 2 * 35_000);
    }
    assert!(VoucherReadShape::EntryWildcard.default_wire_bytes_per_voucher() > 2 * 128_000);
    // The budget is the one margin, and it sits well below the cap.
    assert!(WINDOW_READ_BUDGET_BYTES * 2 <= bridge_tally_transport::XML_RESPONSE_MAX_BYTES as u64);
}

#[test]
fn a_part_tally_cannot_serve_is_halved_by_date_then_by_alterid() {
    let census = census_of(&[("20260401", 4), ("20260402", 1)]);
    let part = |from: &str, to: &str, span| WindowPart {
        from: from.into(),
        to: to.into(),
        span,
    };
    // A date range halves by date.
    assert_eq!(
        halve_part(&part("20260401", "20260402", None), Some(&census), 5),
        Some((
            part("20260401", "20260401", None),
            part("20260402", "20260402", None)
        ))
    );
    // One day halves by its counted AlterIDs, the right half reaching the ceiling.
    assert_eq!(
        halve_part(&part("20260401", "20260401", None), Some(&census), 9),
        Some((
            part(
                "20260401",
                "20260401",
                Some(AlterIdSpan {
                    after: 0,
                    through: 2
                })
            ),
            part(
                "20260401",
                "20260401",
                Some(AlterIdSpan {
                    after: 2,
                    through: 9
                })
            ),
        ))
    );
    // A span halves inside itself.
    assert_eq!(
        halve_part(
            &part(
                "20260401",
                "20260401",
                Some(AlterIdSpan {
                    after: 2,
                    through: 9
                })
            ),
            Some(&census),
            9
        ),
        Some((
            part(
                "20260401",
                "20260401",
                Some(AlterIdSpan {
                    after: 2,
                    through: 3
                })
            ),
            part(
                "20260401",
                "20260401",
                Some(AlterIdSpan {
                    after: 3,
                    through: 9
                })
            ),
        ))
    );
    // One voucher cannot be divided; nor can a day with no census to divide by.
    assert_eq!(
        halve_part(&part("20260402", "20260402", None), Some(&census), 5),
        None
    );
    assert_eq!(
        halve_part(&part("20260401", "20260401", None), None, 5),
        None
    );
}

fn captured_utf16(bytes: &[u8]) -> String {
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

fn three_vouchers() -> String {
    captured_utf16(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    ))
}

fn empty_collection() -> String {
    captured_utf16(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-empty-collection.utf16le.xml"
    ))
}

/// The captured three-voucher response with `keep` of its vouchers, the others
/// removed from the captured bytes in memory (the first ones go first).
fn vouchers_kept(keep: usize) -> String {
    let mut xml = three_vouchers();
    for _ in keep..3 {
        let start = xml.find("<VOUCHER ").unwrap();
        let end = start + xml[start..].find("</VOUCHER>").unwrap() + "</VOUCHER>".len();
        xml.replace_range(start..end, "");
    }
    assert_eq!(xml.matches("<VOUCHER ").count(), keep);
    xml
}

/// `xml` with its vouchers, in order, given the AlterIDs and dates of `ids`:
/// the GUID, `REMOTEID`, `ALTERID`, `MASTERID` and `DATE` of each are rewritten
/// together and nothing else changes, so a response keeps its captured size
/// while describing exactly the vouchers a test's census counted.
fn relabelled(xml: &str, ids: &[(u64, &str)]) -> String {
    let mut out = String::new();
    let mut rest = xml;
    for (alter_id, date) in ids {
        let start = rest.find("<VOUCHER ").expect("a voucher to relabel");
        let end = start + rest[start..].find("</VOUCHER>").unwrap() + "</VOUCHER>".len();
        out.push_str(&rest[..start]);
        let voucher = &rest[start..end];
        let suffix = |value: u64| format!("{GUID}-{value:08x}");
        let old = regex_lite_capture(voucher, "<GUID>", "</GUID>");
        let old_alter = regex_lite_capture(voucher, "<ALTERID TYPE=\"Number\">", "</ALTERID>");
        let old_master = regex_lite_capture(voucher, "<MASTERID TYPE=\"Number\">", "</MASTERID>");
        let old_date = regex_lite_capture(voucher, "<DATE TYPE=\"Date\">", "</DATE>");
        out.push_str(
            &voucher
                .replace(&old, &suffix(*alter_id))
                .replace(
                    &format!("<ALTERID TYPE=\"Number\">{old_alter}</ALTERID>"),
                    &format!("<ALTERID TYPE=\"Number\"> {alter_id}</ALTERID>"),
                )
                .replace(
                    &format!("<MASTERID TYPE=\"Number\">{old_master}</MASTERID>"),
                    &format!("<MASTERID TYPE=\"Number\"> {alter_id}</MASTERID>"),
                )
                .replace(
                    &format!("<DATE TYPE=\"Date\">{old_date}</DATE>"),
                    &format!("<DATE TYPE=\"Date\">{date}</DATE>"),
                ),
        );
        rest = &rest[end..];
    }
    assert!(!rest.contains("<VOUCHER "), "every voucher is relabelled");
    out.push_str(rest);
    out
}

fn regex_lite_capture(text: &str, open: &str, close: &str) -> String {
    let start = text.find(open).unwrap_or_else(|| panic!("{open} present")) + open.len();
    let end = start + text[start..].find(close).unwrap();
    text[start..end].to_string()
}

#[test]
fn a_relabelled_response_describes_the_vouchers_it_names() {
    let xml = relabelled(&vouchers_kept(2), &[(7, "20260803"), (9, "20260804")]);
    let rows = parse_agent_rows(&xml, GUID).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["alter_id"], 7);
    assert_eq!(rows[1]["date"], "20260804");
    assert_eq!(rows[1].window_master_id(), Ok(Some(9)));
    let census = parse_voucher_census(&xml, ("20260803", "20260804"), None).unwrap();
    assert_eq!(census[0].guid, rows[0]["guid"].as_str().unwrap());
}

#[test]
fn the_census_reads_a_captured_voucher_row_and_ignores_cmpinfo() {
    let rows = parse_voucher_census(&three_vouchers(), ("20260801", "20260802"), None).unwrap();
    assert!(rows.iter().all(|row| !row.guid.is_empty()));
    assert_eq!(
        rows.iter()
            .map(|row| (row.day, row.alter_id))
            .collect::<Vec<_>>(),
        [
            (day("20260801"), 1),
            (day("20260801"), 2),
            (day("20260801"), 3)
        ]
    );
    // §12.7: an empty response's CMPINFO carries a bare <VOUCHER>0</VOUCHER>.
    let empty = empty_collection();
    assert!(empty.contains("<VOUCHER>0</VOUCHER>"));
    assert_eq!(
        parse_voucher_census(&empty, ("20260801", "20260802"), None),
        Ok(vec![])
    );
}

#[test]
fn a_census_that_does_not_describe_what_was_asked_is_refused() {
    let xml = three_vouchers();
    assert_eq!(
        parse_voucher_census(&xml, ("20260802", "20260803"), None),
        Err("window_not_honoured".to_string())
    );
    assert_eq!(
        parse_voucher_census(
            &xml,
            ("20260801", "20260801"),
            Some(AlterIdSpan {
                after: 1,
                through: 3
            })
        ),
        Err("window_not_honoured".to_string())
    );
    let undated = xml.replacen("<DATE TYPE=\"Date\">20260801</DATE>", "", 1);
    assert_ne!(undated, xml);
    assert_eq!(
        parse_voucher_census(&undated, ("20260801", "20260801"), None),
        Err("agent_read_protocol_invalid".to_string())
    );
    // A census row without its GUID cannot admit the part it counts.
    let first_guid = parse_voucher_census(&xml, ("20260801", "20260801"), None).unwrap()[0]
        .guid
        .clone();
    let unidentified = xml.replacen(&format!("<GUID>{first_guid}</GUID>"), "", 1);
    assert_ne!(unidentified, xml);
    assert_eq!(
        parse_voucher_census(&unidentified, ("20260801", "20260801"), None),
        Err("agent_read_protocol_invalid".to_string())
    );
}

#[test]
fn an_unobservable_high_water_mark_is_not_read_as_an_empty_book() {
    let guid = "61c6de69-1748-461c-ad3f-162cb949df9f";
    assert_eq!(
        company_marks(&mark_xml(guid, "<ALTVCHID>42</ALTVCHID>"), guid),
        Ok(CompanyMarks {
            vouchers: 42,
            masters: 7
        })
    );
    // Tally omits ALTVCHID for a company that has never held a voucher.
    assert_eq!(
        company_marks(&mark_xml(guid, ""), guid),
        Ok(CompanyMarks {
            vouchers: 0,
            masters: 7
        })
    );
    assert_eq!(
        company_marks(&mark_xml(guid, "<ALTVCHID>x</ALTVCHID>"), guid),
        Err("voucher_checkpoint_invalid".to_string())
    );
    assert_eq!(
        company_marks(&mark_xml(guid, "<ALTVCHID>42</ALTVCHID>"), "another-guid"),
        Err("company_high_water_identity_absent".to_string())
    );
    // Only a row whose master mark was itself observed is that empty book: a row
    // carrying neither mark is unobservable, not empty.
    let neither = mark_xml(guid, "").replace("<ALTMSTID>7</ALTMSTID>", "");
    assert_eq!(
        company_marks(&neither, guid),
        Err("master_checkpoint_not_observed".to_string())
    );
}

#[test]
fn an_undivided_read_is_byte_identical_to_the_request_before_the_bound() {
    let (company, from, to) = ("Synthetic Book", "20260401", "20260430");
    assert_eq!(
        VoucherReadShape::EntryWildcard
            .render(company, from, to, None)
            .unwrap(),
        render_agent_vouchers(company, from, to, None).unwrap()
    );
    assert_eq!(
        VoucherReadShape::Movement
            .render(company, from, to, None)
            .unwrap(),
        render_agent_movement_vouchers(company, from, to).unwrap()
    );
    assert_eq!(
        VoucherReadShape::ImportVerification
            .render(company, from, to, None)
            .unwrap(),
        super::super::agent_import::render_import_verification_read(company, from, to)
    );
    assert_eq!(
        render_agent_vouchers_in_span(company, from, to, None).unwrap(),
        render_agent_vouchers(company, from, to, None).unwrap()
    );
    // A span part differs by exactly its span clause, inside the date formula.
    let span = AlterIdSpan {
        after: 7,
        through: 9,
    };
    for shape in [
        VoucherReadShape::EntryWildcard,
        VoucherReadShape::ClassEntryWildcard,
        VoucherReadShape::Movement,
        VoucherReadShape::ImportVerification,
    ] {
        let whole = shape.render(company, from, to, None).unwrap();
        let part = shape.render(company, from, to, Some(span)).unwrap();
        let clause = " AND $AlterID &gt; 7 AND $AlterID &lt;= 9";
        assert_eq!(part.replacen(clause, "", 1), whole, "{shape:?}");
        assert!(
            part[..part.find("</SYSTEM>").unwrap()].ends_with(clause),
            "{shape:?}"
        );
    }
}

#[test]
fn the_census_request_is_an_admitted_light_collection_export() {
    let dated =
        render_agent_voucher_census("Synthetic & Book", "20260401", "20260430", None).unwrap();
    assert!(crate::tally::agent_read_request::AgentReadRequest::parse(dated.clone()).is_ok());
    assert!(dated.contains("<FETCH>GUID,ALTERID,DATE</FETCH>"));
    assert!(dated.contains("Synthetic &amp; Book"));
    assert!(dated
        .contains("$Date &gt;= $$Date:\"20260401\" AND $Date &lt;= $$Date:\"20260430\"</SYSTEM>"));
    let spanned = render_agent_voucher_census(
        "Synthetic & Book",
        "20260401",
        "20260401",
        Some(AlterIdSpan {
            after: 0,
            through: 4096,
        }),
    )
    .unwrap();
    assert!(spanned.contains("AND $AlterID &gt; 0 AND $AlterID &lt;= 4096</SYSTEM>"));
}

// --- Through the simulator --------------------------------------------------

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

fn mark_xml(guid: &str, altvchid: &str) -> String {
    format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>{guid}</GUID>{altvchid}<ALTMSTID>7</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>")
}

/// The marks `mark(value)` serves: the master mark of `mark_xml` is 7.
fn marks_of(vouchers: u64) -> CompanyMarks {
    CompanyMarks {
        vouchers,
        masters: 7,
    }
}

/// A high-water response carrying both marks.
fn marks_plan(vouchers: u64, masters: u64) -> ScenarioPlan {
    xml_plan(
        mark_xml(GUID, &format!("<ALTVCHID>{vouchers}</ALTVCHID>")).replace(
            "<ALTMSTID>7</ALTMSTID>",
            &format!("<ALTMSTID>{masters}</ALTMSTID>"),
        ),
    )
}

fn mark(value: u64) -> ScenarioPlan {
    xml_plan(mark_xml(GUID, &format!("<ALTVCHID>{value}</ALTVCHID>")))
}

fn company_plan() -> ScenarioPlan {
    xml_plan(captured_utf16(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml"
    )))
}

fn xml_plan(xml: String) -> ScenarioPlan {
    ScenarioPlan::new(Fixture::SyntheticXml(xml))
        .with_encoding(WireEncoding::Utf16Le)
        .with_framing(ResponseFraming::ContentLength)
}

fn status_plan() -> ScenarioPlan {
    ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
        .with_framing(ResponseFraming::ContentLength)
}

/// The six legs of one paired, identity-bracketed agent read.
fn paired(body: &ScenarioPlan) -> Vec<ScenarioPlan> {
    vec![
        company_plan(),
        body.clone(),
        status_plan(),
        body.clone(),
        status_plan(),
        company_plan(),
    ]
}

/// The two legs a read Tally will not serve costs: the identity bracket, then
/// a response declaring a length over the transport cap.
fn oversized() -> Vec<ScenarioPlan> {
    vec![
        company_plan(),
        xml_plan(three_vouchers()).with_framing(ResponseFraming::DeclaredContentLength {
            bytes: bridge_tally_transport::XML_RESPONSE_MAX_BYTES + 1,
        }),
    ]
}

fn wire_len(plan: &ScenarioPlan) -> u64 {
    tally_protocol_simulator::encode(&plan.fixture.body(), plan.encoding).len() as u64
}

fn request_sha(xml: &str) -> String {
    sha256_hex(&bridge_tally_protocol::encode_tally_xml_request_utf16le(
        xml,
    ))
}

fn identity() -> VerifiedCompanyIdentity {
    let company = company_plan().fixture.body().into_owned();
    let companies = bridge_tally_protocol::parse_companies_from_collection(&company).unwrap();
    let observed = companies
        .iter()
        .find(|row| row.guid.as_deref() == Some(GUID))
        .unwrap();
    VerifiedCompanyIdentity::from_observed_companies(
        observed.name.clone(),
        observed.guid.clone().unwrap(),
        observed.company_number.clone().unwrap(),
        observed.books_from.clone().unwrap(),
        &companies,
    )
    .unwrap()
}

fn company() -> String {
    identity().display_name().to_string()
}

fn server_at(address: std::net::SocketAddr, directory: &std::path::Path) -> Server {
    Server::new(Settings {
        endpoint: TallyEndpointConfig {
            host: address.ip().to_string(),
            port: address.port(),
        },
        data_dir: directory.to_path_buf(),
        max_rows: 500,
        max_bytes: 200_000,
        redaction: Redaction::None,
        import_enabled: false,
        writes_enabled: false,
    })
}

/// Limits under which, measured, the captured one-voucher response fits three
/// vouchers a read — and under which, before anything is measured, the default
/// fits only one.
fn three_a_read() -> WindowReadLimits {
    let budget = 3 * wire_len(&xml_plan(vouchers_kept(1)));
    WindowReadLimits {
        budget_bytes: budget,
        default_bytes_per_voucher: budget,
        max_reads: MAX_PLANNED_READS,
    }
}

async fn read_window(
    plans: Vec<ScenarioPlan>,
    (from, to): (&str, &str),
    shape: VoucherReadShape,
    source: WindowPlanSource,
    limits: WindowReadLimits,
) -> (
    Result<WindowReadOutcome<Value>, ToolFailure>,
    Vec<tally_protocol_simulator::ObservedRequest>,
) {
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let identity = identity();
    let outcome = server
        .read_voucher_window(
            &identity,
            identity.display_name(),
            from,
            to,
            shape,
            source,
            limits,
            |xml| parse_agent_rows(xml, GUID),
        )
        .await;
    (outcome, simulator.finish().unwrap())
}

/// The request bodies of the data POSTs among `observed`, by the six-leg pattern:
/// only a leg at offset 1 of a paired read.
fn assert_requests(
    observed: &[tally_protocol_simulator::ObservedRequest],
    at: &[usize],
    expected: &[String],
) {
    assert_eq!(at.len(), expected.len());
    for (leg, request) in at.iter().zip(expected) {
        assert_eq!(
            &observed[*leg].request_body_sha256,
            &request_sha(request),
            "leg {leg}"
        );
    }
}

fn part(from: &str, to: &str, span: Option<AlterIdSpan>) -> WindowPart {
    WindowPart {
        from: from.into(),
        to: to.into(),
        span,
    }
}

#[tokio::test]
async fn the_first_part_measures_the_book_and_the_rest_of_its_day_is_read_above_it() {
    // Default capacity 1: the heavy day is planned in one-voucher spans. The
    // first span measures one voucher, capacity becomes 2 (the floor at half the
    // default binds here), and the rest of that day is re-planned from above
    // the AlterID already read, then day two whole.
    let limits = three_a_read();
    let census = WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260801"), 2),
        (day("20260801"), 3),
        (day("20260802"), 4),
        (day("20260802"), 5),
    ]);
    let mut plans = paired(&xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")])));
    plans.extend(paired(&xml_plan(relabelled(
        &vouchers_kept(2),
        &[(2, "20260801"), (3, "20260801")],
    ))));
    plans.extend(paired(&xml_plan(relabelled(
        &vouchers_kept(2),
        &[(4, "20260802"), (5, "20260802")],
    ))));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census),
        limits,
    )
    .await;
    let outcome = outcome.unwrap();
    assert_eq!(outcome.rows.len(), 5);
    let expected = [
        part(
            "20260801",
            "20260801",
            Some(AlterIdSpan {
                after: 0,
                through: 1,
            }),
        ),
        part(
            "20260801",
            "20260801",
            Some(AlterIdSpan {
                after: 1,
                through: 5,
            }),
        ),
        part("20260802", "20260802", None),
    ];
    assert_eq!(outcome.reads, expected);
    let shape = VoucherReadShape::EntryWildcard;
    assert_requests(
        &observed,
        &[1, 7, 13],
        &expected
            .iter()
            .map(|read| {
                shape
                    .render(&company(), &read.from, &read.to, read.span)
                    .unwrap()
            })
            .collect::<Vec<_>>(),
    );
    assert_eq!(observed.len(), 18);
}

#[tokio::test]
async fn a_replan_after_a_whole_day_reads_the_days_after_it() {
    // Day one fits the default; day two does not. Measuring day one raises
    // capacity to 2, and the re-plan must begin on day two — not re-read day one.
    let limits = three_a_read();
    let census = WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260802"), 2),
        (day("20260802"), 3),
    ]);
    let mut plans = paired(&xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")])));
    plans.extend(paired(&xml_plan(relabelled(
        &vouchers_kept(2),
        &[(2, "20260802"), (3, "20260802")],
    ))));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census),
        limits,
    )
    .await;
    let outcome = outcome.unwrap();
    assert_eq!(
        outcome.reads,
        [
            part("20260801", "20260801", None),
            part("20260802", "20260802", None)
        ]
    );
    assert_eq!(outcome.rows.len(), 3);
    assert_eq!(observed.len(), 12);
}

#[tokio::test]
async fn a_single_voucher_over_budget_is_refused_before_any_read() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(listener.local_addr().unwrap(), directory.path());
    let identity = identity();
    let failure = server
        .read_voucher_window(
            &identity,
            identity.display_name(),
            "20260801",
            "20260802",
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Counted(WindowCensus::from_rows([(day("20260801"), 1)])),
            WindowReadLimits {
                budget_bytes: 1000,
                default_bytes_per_voucher: 1001,
                max_reads: MAX_PLANNED_READS,
            },
            |xml| parse_agent_rows(xml, GUID),
        )
        .await
        .err()
        .expect("refused");
    assert_eq!(failure.code, "voucher_window_part_over_budget");
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[tokio::test]
async fn an_unmeasured_book_is_planned_at_the_default_and_an_omitted_voucher_is_refused() {
    // Before anything is measured the plan is at the default: one voucher a
    // read, so the first request is the span holding the first counted
    // voucher alone — never one read of all three on the strength of nothing.
    //
    // #520 negative control (omission): Tally answers that span with no
    // vouchers although the census counted one in it. Before partition
    // admission the empty part was accepted, nothing was measured, and the read
    // went on to return two of the window's three vouchers as complete.
    let limits = three_a_read();
    let census = WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260801"), 2),
        (day("20260801"), 3),
    ]);
    let (outcome, observed) = read_window(
        paired(&xml_plan(empty_collection())),
        ("20260801", "20260801"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census),
        limits,
    )
    .await;
    let failure = outcome.err().expect("an omitted voucher refuses the read");
    assert_eq!(failure.code, PART_NOT_ADMITTED);
    assert!(failure.evidence.is_some(), "the part read is accounted for");
    assert_requests(
        &observed,
        &[1],
        &[VoucherReadShape::EntryWildcard
            .render(
                &company(),
                "20260801",
                "20260801",
                Some(AlterIdSpan {
                    after: 0,
                    through: 1,
                }),
            )
            .unwrap()],
    );
    assert_eq!(observed.len(), 6);
}

#[tokio::test]
async fn an_unobservable_high_water_mark_refuses_the_window_unread() {
    let foreign = mark_xml(
        "00000000-0000-4000-8000-000000000009",
        "<ALTVCHID>3</ALTVCHID>",
    );
    let (outcome, observed) = read_window(
        paired(&xml_plan(foreign)),
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate { known_marks: None },
        three_a_read(),
    )
    .await;
    let failure = outcome.err().expect("an unestimated window is refused");
    // Refused under the parser's own cause, not as an unestimated window.
    assert_eq!(failure.code, "company_high_water_identity_absent");
    assert!(
        failure.evidence.is_some(),
        "the high-water read is accounted for"
    );
    assert_eq!(observed.len(), 6);
}

#[tokio::test]
async fn a_book_whose_mark_fits_one_census_is_counted_in_one_date_census() {
    // A mark of exactly one census's rows bounds a date census of any window,
    // so the window is counted by date alone: one census, which here proves the
    // window small enough to read whole — the request it was before the bound.
    let limits = WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard);
    let mut plans = paired(&xml_plan(three_vouchers()));
    plans.extend(paired(&xml_plan(three_vouchers())));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260801"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(limits.census_capacity())),
        },
        limits,
    )
    .await;
    let outcome = outcome.unwrap();
    assert_eq!(outcome.reads, [part("20260801", "20260801", None)]);
    assert_eq!(observed.len(), 12);
    assert_requests(
        &observed,
        &[1, 7],
        &[
            render_agent_voucher_census(&company(), "20260801", "20260801", None).unwrap(),
            render_agent_vouchers(&company(), "20260801", "20260801", None).unwrap(),
        ],
    );
}

#[tokio::test]
async fn a_book_one_voucher_past_one_census_is_counted_in_alterid_spans() {
    // #520 / review: a date census is bounded only by the whole book, so a book
    // whose mark exceeds one census is counted in AlterID spans of the window,
    // each bounded before it is sent. Before the fix this book was counted in one
    // date census sized from an estimate of its density.
    let limits = WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard);
    let capacity = limits.census_capacity();
    let mut plans = paired(&xml_plan(three_vouchers()));
    plans.extend(paired(&xml_plan(empty_collection())));
    plans.extend(paired(&xml_plan(three_vouchers())));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260801"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(capacity + 1)),
        },
        limits,
    )
    .await;
    assert_eq!(outcome.unwrap().rows.len(), 3);
    assert_eq!(observed.len(), 18);
    assert_requests(
        &observed,
        &[1, 7, 13],
        &[
            render_agent_voucher_census(
                &company(),
                "20260801",
                "20260801",
                Some(AlterIdSpan {
                    after: 0,
                    through: capacity,
                }),
            )
            .unwrap(),
            render_agent_voucher_census(
                &company(),
                "20260801",
                "20260801",
                Some(AlterIdSpan {
                    after: capacity,
                    through: capacity + 1,
                }),
            )
            .unwrap(),
            render_agent_vouchers(&company(), "20260801", "20260801", None).unwrap(),
        ],
    );
}

#[tokio::test]
async fn a_book_too_large_to_count_is_refused_by_name_before_any_census() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(listener.local_addr().unwrap(), directory.path());
    let identity = identity();
    let limits = WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard);
    let failure = server
        .read_voucher_window(
            &identity,
            identity.display_name(),
            "20260801",
            "20260801",
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Estimate {
                known_marks: Some(marks_of(
                    MAX_CENSUS_READS as u64 * limits.census_capacity() + 1,
                )),
            },
            limits,
            |xml| parse_agent_rows(xml, GUID),
        )
        .await
        .err()
        .expect("refused");
    assert_eq!(failure.code, BOOK_TOO_LARGE);
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "nothing is sent"
    );
}

#[tokio::test]
async fn a_divided_read_is_bracketed_by_the_high_water_mark() {
    // A census of three vouchers on day one; the default fits one a read, so the
    // read divides. After the last part both marks are read again: unchanged,
    // the read stands; either moved, it is refused, because the parts no longer
    // describe one state of the book.
    //
    // #520 (live, 2026-09-21): a ledger renamed between two parts moves only
    // the master mark while changing the ledger's name in every voucher export.
    // Before the fix the bracket compared the voucher mark alone, and this
    // third case returned `complete`.
    for (closing, expect_ok) in [
        (marks_plan(3, 7), true),
        (marks_plan(4, 7), false),
        (marks_plan(3, 8), false),
    ] {
        let limits = three_a_read();
        let mut plans = paired(&xml_plan(three_vouchers()));
        plans.extend(paired(&xml_plan(relabelled(
            &vouchers_kept(1),
            &[(1, "20260801")],
        ))));
        plans.extend(paired(&xml_plan(relabelled(
            &vouchers_kept(2),
            &[(2, "20260801"), (3, "20260801")],
        ))));
        plans.extend(paired(&xml_plan(empty_collection())));
        plans.extend(paired(&closing));
        let (outcome, observed) = read_window(
            plans,
            ("20260801", "20260802"),
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Estimate {
                known_marks: Some(marks_of(3)),
            },
            limits,
        )
        .await;
        assert_eq!(observed.len(), 30);
        assert_eq!(
            observed[25].request_body_sha256,
            request_sha(&render_agent_company_high_water(&company()))
        );
        match outcome {
            Ok(read) => {
                assert!(expect_ok);
                assert_eq!(read.rows.len(), 3);
                assert_eq!(
                    read.reads,
                    [
                        part(
                            "20260801",
                            "20260801",
                            Some(AlterIdSpan {
                                after: 0,
                                through: 1
                            })
                        ),
                        part(
                            "20260801",
                            "20260801",
                            Some(AlterIdSpan {
                                after: 1,
                                through: 3
                            })
                        ),
                        part("20260802", "20260802", None),
                    ]
                );
            }
            Err(failure) => {
                assert!(!expect_ok);
                assert_eq!(failure.code, WINDOW_CHANGED_DURING_READ);
                assert!(failure.evidence.is_some());
            }
        }
    }
}

/// #595: every request of a window read is timed by what it was for. The
/// second part is held 200 ms on each of its two paired bodies, so it alone
/// costs at least 400 ms; each part also reports one body's bytes and its rows.
#[tokio::test]
async fn a_window_read_times_its_marks_census_and_each_part() {
    let first = xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")]));
    let second = xml_plan(relabelled(
        &vouchers_kept(2),
        &[(2, "20260801"), (3, "20260801")],
    ))
    .with_delivery(tally_protocol_simulator::Delivery::SlowHeaders(
        std::time::Duration::from_millis(200),
    ));
    let third = xml_plan(empty_collection());
    let mut plans = paired(&xml_plan(three_vouchers()));
    for body in [&first, &second, &third] {
        plans.extend(paired(body));
    }
    plans.extend(paired(&marks_plan(3, 7)));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        three_a_read(),
    )
    .await;
    assert_eq!(observed.len(), 30);
    let timings = outcome.unwrap().timings;
    // The marks were known: only the closing bracket read them.
    assert_eq!(timings.marks.requests, 1);
    assert_eq!(timings.census.requests, 1);
    let spans = timings
        .parts
        .iter()
        .map(|part| {
            (
                part.from.as_str(),
                part.to.as_str(),
                part.after,
                part.through,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        spans,
        [
            ("20260801", "20260801", Some(0), Some(1)),
            ("20260801", "20260801", Some(1), Some(3)),
            ("20260802", "20260802", None, None),
        ]
    );
    assert!(timings.parts.iter().all(|part| part.served));
    assert_eq!(
        timings
            .parts
            .iter()
            .map(|part| part.rows)
            .collect::<Vec<_>>(),
        [Some(1), Some(2), Some(0)]
    );
    assert_eq!(
        timings
            .parts
            .iter()
            .map(|part| part.bytes)
            .collect::<Vec<_>>(),
        [&first, &second, &third]
            .map(|body| Some(wire_len(body)))
            .to_vec()
    );
    assert!(timings.parts[1].ms >= 400, "{timings:?}");
    assert!(timings.parts[0].ms < timings.parts[1].ms, "{timings:?}");
    assert!(timings.parts[2].ms < timings.parts[1].ms, "{timings:?}");
    assert_eq!(timings.failed, None);
}

#[tokio::test]
async fn a_census_the_transport_refuses_is_not_divided_but_refused() {
    // Every census is bounded before it is sent: here the mark (4) is within
    // one census, so the window is one date census. Oversized all the same, it
    // means the per-row figure was wrong, not that the range was too wide, so it
    // is not halved into further censuses — the review's objection was to a
    // census whose size was only learned after Tally had built it. The window
    // is refused, and nothing more is sent.
    let one = wire_len(&xml_plan(vouchers_kept(1)));
    let limits = WindowReadLimits {
        budget_bytes: 3 * one,
        default_bytes_per_voucher: one,
        max_reads: MAX_PLANNED_READS,
    };
    assert!(limits.census_capacity() >= 4);
    let (outcome, observed) = read_window(
        oversized(),
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(4)),
        },
        limits,
    )
    .await;
    let failure = outcome.err().expect("refused");
    assert_eq!(failure.code, VOLUME_UNESTIMATED);
    assert_eq!(failure.cause, Some("census_response_too_large"));
    assert_eq!(observed.len(), 2);
    assert_requests(
        &observed,
        &[1],
        &[render_agent_voucher_census(&company(), "20260801", "20260802", None).unwrap()],
    );
}
#[tokio::test]
async fn a_verification_part_tally_cannot_serve_is_halved_left_first() {
    // #485 inside the shared reader: the undivided window is refused by the
    // transport, so it is read again in date halves — the earlier half first,
    // so rows arrive in the order one undivided read would have produced.
    //
    // #520: a read planned whole and divided only after Tally could not serve
    // it is a divided read like any other, so it is bracketed too. Before the
    // fix the bracket was gated on a census, which this read never took, and a
    // moved mark was accepted.
    let shape = VoucherReadShape::ImportVerification;
    for (closing, expect_ok) in [(1, true), (2, false)] {
        let mut plans = oversized();
        plans.extend(paired(&xml_plan(three_vouchers())));
        plans.extend(paired(&xml_plan(empty_collection())));
        plans.extend(paired(&mark(closing)));
        let (outcome, observed) = read_window(
            plans,
            ("20260801", "20260802"),
            shape,
            WindowPlanSource::Estimate {
                known_marks: Some(marks_of(1)),
            },
            WindowReadLimits::for_shape(shape),
        )
        .await;
        assert_eq!(observed.len(), 20);
        assert_eq!(
            observed[15].request_body_sha256,
            request_sha(&render_agent_company_high_water(&company()))
        );
        if !expect_ok {
            let failure = outcome.err().expect("a moved mark refuses the split read");
            assert_eq!(failure.code, WINDOW_CHANGED_DURING_READ);
            continue;
        }
        split_read_checks(outcome.unwrap(), &observed, shape);
    }
    // A shape that does not halve fails on the same response instead.
    let (outcome, _) = read_window(
        oversized(),
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(1)),
        },
        WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard),
    )
    .await;
    assert_eq!(
        outcome.err().map(|failure| failure.code).as_deref(),
        Some("response_size_limit_exceeded")
    );
}

fn split_read_checks(
    outcome: WindowReadOutcome<Value>,
    observed: &[tally_protocol_simulator::ObservedRequest],
    shape: VoucherReadShape,
) {
    assert_eq!(
        outcome.reads,
        [
            part("20260801", "20260801", None),
            part("20260802", "20260802", None)
        ]
    );
    assert_requests(
        observed,
        &[1, 3, 9],
        &[
            shape
                .render(&company(), "20260801", "20260802", None)
                .unwrap(),
            shape
                .render(&company(), "20260801", "20260801", None)
                .unwrap(),
            shape
                .render(&company(), "20260802", "20260802", None)
                .unwrap(),
        ],
    );
}

/// #680: the carry-forward #494 added, driven through the reader. A 4-day
/// window is refused, then its left 2-day half is: the right 2-day half is at
/// least a span already refused on this call, so it is split without being
/// sent, and all four days are read singly, in date order.
#[tokio::test]
async fn a_sibling_as_wide_as_a_refused_part_is_split_without_being_sent() {
    let shape = VoucherReadShape::ImportVerification;
    let mut plans = oversized();
    plans.extend(oversized());
    plans.extend(paired(&xml_plan(three_vouchers())));
    for _ in 0..3 {
        plans.extend(paired(&xml_plan(empty_collection())));
    }
    plans.extend(paired(&mark(1)));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260804"),
        shape,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(1)),
        },
        WindowReadLimits::for_shape(shape),
    )
    .await;
    let outcome = outcome.expect("the window is read in single days");
    assert_eq!(
        outcome.reads,
        ["20260801", "20260802", "20260803", "20260804"].map(|day| part(day, day, None))
    );
    let render = |from, to| shape.render(&company(), from, to, None).unwrap();
    assert_eq!(observed.len(), 34);
    assert_requests(
        &observed,
        &[1, 3, 5, 11, 17, 23],
        &[
            render("20260801", "20260804"),
            render("20260801", "20260802"),
            render("20260801", "20260801"),
            render("20260802", "20260802"),
            render("20260803", "20260803"),
            render("20260804", "20260804"),
        ],
    );
    let right_half = request_sha(&render("20260803", "20260804"));
    assert!(
        observed
            .iter()
            .all(|request| request.request_body_sha256 != right_half),
        "the right half was sent, though a part as wide was already refused"
    );
    assert_eq!(
        observed[29].request_body_sha256,
        request_sha(&render_agent_company_high_water(&company()))
    );
    // Every row one undivided read would have returned, in date order.
    assert_eq!(
        outcome.rows,
        parse_agent_rows(&three_vouchers(), GUID).unwrap()
    );
}

/// The control for the test above: a sibling narrower than every part refused
/// so far is read, not split. A 5-day window divides 3/2; the 3-day half is
/// refused, so its 2-day left part and the window's 2-day right half are both
/// read whole.
#[tokio::test]
async fn a_sibling_narrower_than_every_refused_part_is_read() {
    let shape = VoucherReadShape::ImportVerification;
    let mut plans = oversized();
    plans.extend(oversized());
    plans.extend(paired(&xml_plan(three_vouchers())));
    for _ in 0..2 {
        plans.extend(paired(&xml_plan(empty_collection())));
    }
    plans.extend(paired(&mark(1)));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260805"),
        shape,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(1)),
        },
        WindowReadLimits::for_shape(shape),
    )
    .await;
    let outcome = outcome.expect("the window is read in three parts");
    assert_eq!(
        outcome.reads,
        [
            part("20260801", "20260802", None),
            part("20260803", "20260803", None),
            part("20260804", "20260805", None),
        ]
    );
    let render = |from, to| shape.render(&company(), from, to, None).unwrap();
    assert_eq!(observed.len(), 28);
    assert_requests(
        &observed,
        &[1, 3, 5, 11, 17],
        &[
            render("20260801", "20260805"),
            render("20260801", "20260803"),
            render("20260801", "20260802"),
            render("20260803", "20260803"),
            render("20260804", "20260805"),
        ],
    );
}

#[tokio::test]
async fn a_failed_parse_keeps_the_evidence_of_the_part_just_read() {
    let (outcome, _) = read_window(
        paired(&xml_plan(
            "<ENVELOPE><HEADER><STATUS>0</STATUS></HEADER></ENVELOPE>".to_string(),
        )),
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(1)),
        },
        WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard),
    )
    .await;
    let failure = outcome.err().expect("a malformed part fails the read");
    assert!(failure.evidence.expect("the part is accounted for").bytes > 0);
}

#[tokio::test]
async fn a_supplied_count_of_another_window_is_refused_before_any_read() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(listener.local_addr().unwrap(), directory.path());
    let identity = identity();
    let failure = server
        .read_voucher_window(
            &identity,
            identity.display_name(),
            "20260801",
            "20260802",
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Counted(WindowCensus::from_rows([
                (day("20260801"), 1),
                (day("20260803"), 2),
            ])),
            three_a_read(),
            |xml| parse_agent_rows(xml, GUID),
        )
        .await
        .err()
        .expect("refused");
    assert_eq!(failure.code, "window_not_honoured");
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[tokio::test]
async fn a_small_book_sends_the_same_voucher_request_as_before_the_bound() {
    // Through the vouchers tool: the only new read is the high-water mark, and
    // the window read that follows is the request the tool sent before.
    let mut plans = vec![company_plan(), status_plan(), company_plan(), status_plan()];
    plans.extend(paired(&mark(3)));
    plans.extend(paired(&xml_plan(three_vouchers())));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let response = server_at(simulator.address(), directory.path())
        .call_tool(
            "vouchers",
            json!({"company_guid": GUID, "from": "20260801", "to": "20260831"}),
        )
        .await;
    assert_eq!(response["isError"], false, "{response}");
    assert_eq!(
        response["structuredContent"]["result"]["items"]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
    let observed = simulator.finish().unwrap();
    assert_eq!(observed.len(), 16);
    assert_requests(
        &observed,
        &[5, 11, 13],
        &[
            render_agent_company_high_water(&company()),
            render_agent_vouchers(&company(), "20260801", "20260831", None).unwrap(),
            render_agent_vouchers(&company(), "20260801", "20260831", None).unwrap(),
        ],
    );
}

#[tokio::test]
async fn a_lighter_part_never_loosens_the_plan_for_the_rest() {
    // Day one measures one heavy voucher (its narration padded in memory); day
    // two, three far lighter ones. The rest is planned at the heavier figure:
    // day three's five vouchers go in spans of three and two, not in the one
    // read the lighter part alone would allow.
    let heavy = vouchers_kept(1).replace(
        "<NARRATION TYPE=\"String\">",
        &format!("<NARRATION TYPE=\"String\">{}", "N".repeat(20_000)),
    );
    assert_ne!(heavy, vouchers_kept(1));
    // The default fits one voucher a read and its floor (half) no more than
    // the heavy measurement, so the measured figure is what plans the rest.
    let heavy_len = wire_len(&xml_plan(heavy.clone()));
    let limits = WindowReadLimits {
        budget_bytes: 3 * heavy_len,
        default_bytes_per_voucher: 2 * heavy_len,
        max_reads: MAX_PLANNED_READS,
    };
    let census = WindowCensus::from_rows(
        [
            ("20260801", 1),
            ("20260802", 2),
            ("20260802", 3),
            ("20260802", 4),
        ]
        .into_iter()
        .map(|(date, id)| (day(date), id))
        .chain((5..=9).map(|id| (day("20260803"), id))),
    );
    let mut plans = paired(&xml_plan(relabelled(&heavy, &[(1, "20260801")])));
    plans.extend(paired(&xml_plan(relabelled(
        &three_vouchers(),
        &[(2, "20260802"), (3, "20260802"), (4, "20260802")],
    ))));
    plans.extend(paired(&xml_plan(relabelled(
        &three_vouchers(),
        &[(5, "20260803"), (6, "20260803"), (7, "20260803")],
    ))));
    plans.extend(paired(&xml_plan(relabelled(
        &vouchers_kept(2),
        &[(8, "20260803"), (9, "20260803")],
    ))));
    let (outcome, _) = read_window(
        plans,
        ("20260801", "20260803"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census),
        limits,
    )
    .await;
    assert_eq!(
        outcome.unwrap().reads,
        [
            part("20260801", "20260801", None),
            part("20260802", "20260802", None),
            part(
                "20260803",
                "20260803",
                Some(AlterIdSpan {
                    after: 0,
                    through: 7
                })
            ),
            part(
                "20260803",
                "20260803",
                Some(AlterIdSpan {
                    after: 7,
                    through: 9
                })
            ),
        ]
    );
}

#[test]
fn a_light_first_measurement_cannot_plan_a_heavy_part_over_the_cap() {
    // The floor: a measured figure never falls below half the shape default.
    let default = VoucherReadShape::ImportVerification.default_wire_bytes_per_voucher();
    assert_eq!(planning_figure(default, 10_000), default / 2);
    assert_eq!(planning_figure(default, 70_000), 70_000);
    // The review's case: a first part of bank receipts at ~10 KB on the wire,
    // then inventory vouchers at the default's weight. Planned from the light
    // figure, every part must still fit the cap at the heavy weight — and fit
    // the budget at the default's own floor.
    let census = heavy_book();
    let figure = planning_figure(default, 10_000);
    let reads = plan_window_reads(
        day("20250601"),
        day("20250630"),
        &census,
        None,
        census.max_alter_id(),
        figure,
        WINDOW_READ_BUDGET_BYTES,
        MAX_PLANNED_READS,
    )
    .unwrap();
    for read in &reads {
        assert!(
            read.vouchers * figure <= WINDOW_READ_BUDGET_BYTES,
            "{read:?}"
        );
        assert!(
            read.vouchers * default <= bridge_tally_transport::XML_RESPONSE_MAX_BYTES as u64,
            "{read:?} would exceed the cap at the default's weight"
        );
    }
}

#[tokio::test]
async fn a_light_first_part_does_not_widen_the_next_beyond_the_floor() {
    // Default capacity 1; the floor allows at most 2 a read. Day one measures a
    // light voucher that alone would allow 4, so day two's four vouchers must
    // still go in two spans of two.
    let one = wire_len(&xml_plan(vouchers_kept(1)));
    let limits = WindowReadLimits {
        budget_bytes: 4 * one,
        default_bytes_per_voucher: 4 * one,
        max_reads: MAX_PLANNED_READS,
    };
    let census = WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260802"), 2),
        (day("20260802"), 3),
        (day("20260802"), 4),
        (day("20260802"), 5),
    ]);
    let mut plans = paired(&xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")])));
    plans.extend(paired(&xml_plan(relabelled(
        &vouchers_kept(2),
        &[(2, "20260802"), (3, "20260802")],
    ))));
    plans.extend(paired(&xml_plan(relabelled(
        &vouchers_kept(2),
        &[(4, "20260802"), (5, "20260802")],
    ))));
    let (outcome, _) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census),
        limits,
    )
    .await;
    assert_eq!(
        outcome.unwrap().reads,
        [
            part("20260801", "20260801", None),
            part(
                "20260802",
                "20260802",
                Some(AlterIdSpan {
                    after: 0,
                    through: 3
                })
            ),
            part(
                "20260802",
                "20260802",
                Some(AlterIdSpan {
                    after: 3,
                    through: 5
                })
            ),
        ]
    );
}

#[test]
fn a_book_whose_mark_fits_one_census_is_counted_by_date_and_a_larger_one_in_spans() {
    // The switch point, on each side. A mark of exactly one census's capacity
    // bounds a date census of any window, so that stays the one request it
    // was; one more voucher and no date census is bounded, so the book is
    // counted in AlterID spans, each bounded by construction.
    // Sized against the whole transport cap: 32 MiB at 4 KiB a row.
    let capacity = WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard).census_capacity();
    assert_eq!(capacity, 8192);
    assert_eq!(
        census_spans(capacity, capacity)
            .unwrap()
            .collect::<Vec<_>>(),
        [None]
    );
    assert_eq!(
        census_spans(capacity + 1, capacity)
            .unwrap()
            .collect::<Vec<_>>(),
        [
            Some(AlterIdSpan {
                after: 0,
                through: capacity
            }),
            Some(AlterIdSpan {
                after: capacity,
                through: capacity + 1
            }),
        ]
    );
    // An empty book is one date census too.
    assert_eq!(
        census_spans(0, capacity).unwrap().collect::<Vec<_>>(),
        [None]
    );
}

#[test]
fn census_spans_tile_the_mark_and_none_holds_more_than_one_census() {
    for mark in [4097_u64, 8192, 8193, 250_000] {
        let spans = census_spans(mark, 4096)
            .unwrap()
            .map(Option::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(spans.len() as u64, mark.div_ceil(4096), "mark {mark}");
        assert_eq!(spans.first().unwrap().after, 0);
        assert_eq!(spans.last().unwrap().through, mark);
        for pair in spans.windows(2) {
            assert_eq!(pair[0].through, pair[1].after, "gap or overlap at {mark}");
        }
        assert!(spans
            .iter()
            .all(|span| span.through - span.after <= 4096 && span.through > span.after));
    }
    // The live book of §11c.1 (a mark of about a quarter of a million) is 31
    // spans at the production width.
    assert_eq!(census_spans(249_948, 8192).unwrap().count(), 31);
}

#[test]
fn a_book_needing_more_census_spans_than_allowed_is_refused_before_any_is_made() {
    let largest = MAX_CENSUS_READS as u64 * 4096;
    assert_eq!(
        census_spans(largest, 4096).unwrap().count(),
        MAX_CENSUS_READS
    );
    assert_eq!(census_spans(largest + 1, 4096).err(), Some(BOOK_TOO_LARGE));
    // A mark far beyond any real book is refused at once, not walked: the
    // refusal is decided from the count, before a span exists.
    assert_eq!(census_spans(u64::MAX, 4096).err(), Some(BOOK_TOO_LARGE));
    // And the spans of an admitted mark are produced one at a time.
    let mut spans = census_spans(largest, 4096).unwrap();
    assert_eq!(
        spans.next(),
        Some(Some(AlterIdSpan {
            after: 0,
            through: 4096
        }))
    );
}

#[test]
fn a_census_refuses_on_an_oversized_response_and_on_a_deadline() {
    use bridge_tally_transport::TallyTransportError;
    let oversized = TallyTransportError::ResponseTooLarge {
        limit: 32 * 1024 * 1024,
        declared_by_peer: true,
    };
    // Every census is bounded before it is sent, so an oversized one is not
    // divided: its per-row figure was wrong, and the window is refused.
    assert_eq!(census_failure(oversized.safe_code()), CensusFailure::Refuse);
    // A deadline is never retried, not even in halves.
    assert_eq!(
        census_failure(TallyTransportError::RequestTimedOut.safe_code()),
        CensusFailure::Refuse
    );
    assert_eq!(
        census_failure(TallyTransportError::ConnectionFailed.safe_code()),
        CensusFailure::Propagate
    );
    assert_eq!(
        census_failure("agent_runtime_read_failed"),
        CensusFailure::Propagate
    );
}

#[tokio::test]
async fn a_census_that_times_out_refuses_the_window_and_sends_nothing_more() {
    // Takes the transport's 20 s deadline. The census never answers; the read
    // is refused as unestimated and nothing else is sent. Spare responses are
    // queued so that a retry, if one were made, would be served and seen.
    let limits = WindowReadLimits {
        budget_bytes: 16 * 1024,
        default_bytes_per_voucher: 16 * 1024,
        max_reads: MAX_PLANNED_READS,
    };
    let mut plans = vec![
        company_plan(),
        xml_plan(empty_collection()).with_delivery(
            tally_protocol_simulator::Delivery::SlowHeaders(std::time::Duration::from_secs(25)),
        ),
    ];
    for _ in 0..4 {
        plans.extend(paired(&xml_plan(empty_collection())));
    }
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let identity = identity();
    let failure = server
        .read_voucher_window(
            &identity,
            identity.display_name(),
            "20260801",
            "20260801",
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Estimate {
                known_marks: Some(marks_of(9)),
            },
            limits,
            |xml| parse_agent_rows(xml, GUID),
        )
        .await
        .err()
        .expect("a census deadline refuses the window");
    assert_eq!(failure.code, VOLUME_UNESTIMATED);
    assert_eq!(failure.cause, Some("census_deadline_exceeded"));
    // #595: the failure carries what the read cost up to it. The marks were
    // known, so the one request sent was the census, and it took the deadline.
    let timings = failure
        .window_timings
        .as_deref()
        .expect("a failed window read carries its timings");
    assert_eq!(timings.marks, RequestTally::default());
    assert_eq!(timings.census.requests, 1);
    assert!(timings.census.ms >= 19_000, "{timings:?}");
    assert!(timings.parts.is_empty());
    assert_eq!(
        timings.failed,
        Some(FailedRequest {
            kind: "census",
            ms: timings.census.ms
        })
    );
    simulator.cancel();
    let observed = simulator.finish().unwrap();
    assert_eq!(
        observed.len(),
        2,
        "nothing was sent after the timed-out census"
    );
    // A mark of 9 over a census capacity of 8: the first AlterID span.
    assert_eq!(limits.census_capacity(), 8);
    assert_eq!(
        observed[1].request_body_sha256,
        request_sha(
            &render_agent_voucher_census(
                &company(),
                "20260801",
                "20260801",
                Some(AlterIdSpan {
                    after: 0,
                    through: 8
                })
            )
            .unwrap()
        )
    );
}

#[tokio::test]
async fn a_read_divided_only_by_date_is_bracketed_too() {
    // Two days of one voucher each; the default fits one a read, so the window
    // is read as two date ranges with no AlterID span. The mark is still read
    // again afterwards, and a moved mark refuses the read.
    let heavy = vouchers_kept(1).replace(
        "<NARRATION TYPE=\"String\">",
        &format!("<NARRATION TYPE=\"String\">{}", "N".repeat(20_000)),
    );
    let heavy_len = wire_len(&xml_plan(heavy.clone()));
    let limits = WindowReadLimits {
        budget_bytes: 2 * heavy_len,
        default_bytes_per_voucher: 2 * heavy_len,
        max_reads: MAX_PLANNED_READS,
    };
    // One census: AlterIDs 2 and 3, one on each day.
    let two = vouchers_kept(2);
    let second_date = two.rfind("<DATE TYPE=\"Date\">20260801</DATE>").unwrap();
    let mut census = two.clone();
    census.replace_range(
        second_date..second_date + "<DATE TYPE=\"Date\">20260801</DATE>".len(),
        "<DATE TYPE=\"Date\">20260802</DATE>",
    );
    for (closing, expect_ok) in [(3, true), (4, false)] {
        let mut plans = paired(&xml_plan(census.clone()));
        plans.extend(paired(&xml_plan(relabelled(&heavy, &[(2, "20260801")]))));
        plans.extend(paired(&xml_plan(
            vouchers_kept(1).replace("20260801", "20260802"),
        )));
        plans.extend(paired(&mark(closing)));
        let (outcome, observed) = read_window(
            plans,
            ("20260801", "20260802"),
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Estimate {
                known_marks: Some(marks_of(3)),
            },
            limits,
        )
        .await;
        assert_eq!(observed.len(), 24);
        match outcome {
            Ok(read) => {
                assert!(expect_ok);
                assert_eq!(
                    read.reads,
                    [
                        part("20260801", "20260801", None),
                        part("20260802", "20260802", None)
                    ]
                );
            }
            Err(failure) => {
                assert!(!expect_ok);
                assert_eq!(failure.code, WINDOW_CHANGED_DURING_READ);
            }
        }
    }
}

#[test]
fn a_census_row_with_alterid_zero_is_refused() {
    // A day divided by AlterID starts every span above 0, so a voucher with
    // AlterID 0 could never be read in parts. Refuse the census instead.
    let zero = three_vouchers().replacen(
        "<ALTERID TYPE=\"Number\"> 1</ALTERID>",
        "<ALTERID TYPE=\"Number\"> 0</ALTERID>",
        1,
    );
    assert_ne!(zero, three_vouchers());
    assert_eq!(
        parse_voucher_census(&zero, ("20260801", "20260801"), None),
        Err("agent_read_protocol_invalid".to_string())
    );
}

// ---------------------------------------------------------------------------
// #520: the replay/evidence contract. Each test below is a labelled negative
// control with an unchanged-book control beside it.
// ---------------------------------------------------------------------------

/// The divided first read the replay tests repeat: a census of three vouchers on
/// day one under a mark of 3, read in spans `(0,1]` and `(1,3]`, then day two
/// whole, and closed at an unchanged mark.
fn divided_first_read() -> (Vec<ScenarioPlan>, Vec<ScenarioPlan>) {
    let parts = vec![
        xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")])),
        xml_plan(relabelled(
            &vouchers_kept(2),
            &[(2, "20260801"), (3, "20260801")],
        )),
        xml_plan(empty_collection()),
    ];
    let mut first = paired(&xml_plan(three_vouchers()));
    for part in &parts {
        first.extend(paired(part));
    }
    first.extend(paired(&marks_plan(3, 7)));
    (first, parts)
}

async fn first_read() -> WindowReadOutcome<Value> {
    let (plans, _) = divided_first_read();
    let (outcome, _) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        three_a_read(),
    )
    .await;
    let outcome = outcome.unwrap();
    assert!(is_divided(&outcome.reads));
    outcome
}

#[tokio::test]
async fn a_replay_refuses_a_voucher_created_above_the_first_reads_ceiling() {
    // #520 P1. A voucher posted in the window after the first read closed takes
    // AlterID 4, above the ceiling (3) the replayed spans were planned to. Both
    // old-ceiling responses exclude it and are byte-identical to the first
    // read's, so the two snapshots match; before the fix the replay read no
    // mark and the corroboration was accepted with the posting missing from
    // both. The replay now closes against the first read's witness.
    let first = first_read().await;
    let witness = first.witness.clone().expect("a divided read has a witness");
    assert_eq!(witness.marks, marks_of(3));
    for (closing, expect_ok) in [(marks_plan(3, 7), true), (marks_plan(4, 7), false)] {
        let (_, parts) = divided_first_read();
        let mut plans = Vec::new();
        for part in &parts {
            plans.extend(paired(part));
        }
        plans.extend(paired(&closing));
        let (outcome, observed) = read_window(
            plans,
            ("20260801", "20260802"),
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Replay {
                parts: first.reads.clone(),
                witness: Some(witness.clone()),
            },
            three_a_read(),
        )
        .await;
        assert_eq!(observed.len(), 24, "three parts and the closing marks");
        match outcome {
            Ok(replay) => {
                assert!(expect_ok);
                // The data the two reads compare is identical either way,
                // which is exactly why the marks must decide.
                assert_eq!(
                    replay.evidence.response_sha256,
                    first.evidence.response_sha256
                );
            }
            Err(failure) => {
                assert!(!expect_ok);
                assert_eq!(failure.code, WINDOW_CHANGED_DURING_READ);
                assert!(failure.evidence.is_some());
            }
        }
    }
}

#[tokio::test]
async fn a_replay_of_a_divided_read_without_its_witness_is_refused_unread() {
    let first = first_read().await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(listener.local_addr().unwrap(), directory.path());
    let identity = identity();
    let failure = server
        .read_voucher_window(
            &identity,
            identity.display_name(),
            "20260801",
            "20260802",
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Replay {
                parts: first.reads,
                witness: None,
            },
            three_a_read(),
            |xml| parse_agent_rows(xml, GUID),
        )
        .await
        .err()
        .expect("refused");
    assert_eq!(failure.code, REPLAY_UNWITNESSED);
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}

#[tokio::test]
async fn a_replayed_part_is_admitted_against_the_first_reads_census() {
    // The replay reads no census of its own, so it is admitted against the
    // witness's: a replayed part that returns a different voucher in the same
    // span is refused, even where the first read's snapshot would differ anyway.
    let first = first_read().await;
    let substituted = xml_plan(relabelled(
        &vouchers_kept(2),
        &[(2, "20260801"), (3, "20260801")],
    ));
    let mut plans = paired(&xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")])));
    // Same AlterIDs as counted, but the second voucher is not the one counted.
    let other = relabelled(&vouchers_kept(2), &[(2, "20260801"), (3, "20260801")])
        .replace(&format!("{GUID}-00000003"), &format!("{GUID}-000000ff"));
    assert_ne!(other, substituted.fixture.body());
    plans.extend(paired(&xml_plan(other)));
    let (outcome, _) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Replay {
            parts: first.reads,
            witness: first.witness,
        },
        three_a_read(),
    )
    .await;
    assert_eq!(
        outcome.err().map(|failure| failure.code).as_deref(),
        Some(PART_NOT_ADMITTED)
    );
}

#[tokio::test]
async fn a_part_returning_a_voucher_its_census_did_not_count_is_refused() {
    // #520 P1, partition admission (substitution preserving the count). The
    // census counted AlterID 1 on day one and 2 on day two; day one's part
    // returns one voucher — AlterID 5, which the census never counted. The
    // count matches, so a count check alone passes it; before the fix the read
    // returned it as the window's. The unchanged control is the same read with
    // the counted voucher.
    for (day_one, expect_ok) in [(1, true), (5, false)] {
        let census = WindowCensus::from_rows([(day("20260801"), 1), (day("20260802"), 2)]);
        let mut plans = paired(&xml_plan(relabelled(
            &vouchers_kept(1),
            &[(day_one, "20260801")],
        )));
        if expect_ok {
            plans.extend(paired(&xml_plan(relabelled(
                &vouchers_kept(1),
                &[(2, "20260802")],
            ))));
        }
        let (outcome, _) = read_window(
            plans,
            ("20260801", "20260802"),
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Counted(census),
            three_a_read(),
        )
        .await;
        match outcome {
            Ok(read) => {
                assert!(expect_ok);
                assert_eq!(read.rows.len(), 2);
            }
            Err(failure) => {
                assert!(!expect_ok);
                assert_eq!(failure.code, PART_NOT_ADMITTED);
                assert!(failure.evidence.is_some());
            }
        }
    }
}

#[tokio::test]
async fn a_voucher_returned_by_two_parts_is_refused_across_the_union() {
    // #520 P1, union identity. Two parts, each valid on its own: day one returns
    // GUID ...01 as AlterID 1, day two returns the same GUID as AlterID 2 (the
    // voucher re-dated between the parts). Each response admits its own rows,
    // and the census here carries no GUIDs, so only the union can see it; before
    // the fix movement summed it twice. The control returns two vouchers.
    for (duplicate, expect_ok) in [(false, true), (true, false)] {
        let census = WindowCensus::from_rows([(day("20260801"), 1), (day("20260802"), 2)]);
        let mut second = relabelled(&vouchers_kept(1), &[(2, "20260802")]);
        if duplicate {
            second = second.replace(&format!("{GUID}-00000002"), &format!("{GUID}-00000001"));
        }
        let mut plans = paired(&xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")])));
        plans.extend(paired(&xml_plan(second)));
        let (outcome, _) = read_window(
            plans,
            ("20260801", "20260802"),
            VoucherReadShape::Movement,
            WindowPlanSource::Counted(census),
            three_a_read(),
        )
        .await;
        match outcome {
            Ok(read) => {
                assert!(expect_ok);
                assert_eq!(read.rows.len(), 2);
            }
            Err(failure) => {
                assert!(!expect_ok);
                assert_eq!(failure.code, "voucher_source_identity_invalid");
                assert!(failure.evidence.is_some());
            }
        }
    }
}

#[tokio::test]
async fn the_read_allowance_is_spent_at_dispatch_including_reactive_splits() {
    // #520 P2. The window is planned whole — one read, well within an allowance
    // of two — and Tally cannot serve it, so it is divided into two days: three
    // data requests in all. Before the fix the allowance was checked only when a
    // measurement re-planned, so all three were sent under an allowance of two.
    // The control spends exactly three under an allowance of three.
    let shape = VoucherReadShape::ImportVerification;
    for (allowance, expect_ok) in [(3, true), (2, false)] {
        let limits = WindowReadLimits {
            max_reads: allowance,
            ..WindowReadLimits::for_shape(shape)
        };
        let mut plans = oversized();
        plans.extend(paired(&xml_plan(three_vouchers())));
        if expect_ok {
            plans.extend(paired(&xml_plan(empty_collection())));
            plans.extend(paired(&mark(1)));
        }
        let (outcome, observed) = read_window(
            plans,
            ("20260801", "20260802"),
            shape,
            WindowPlanSource::Estimate {
                known_marks: Some(marks_of(1)),
            },
            limits,
        )
        .await;
        match outcome {
            Ok(read) => {
                assert!(expect_ok);
                assert_eq!(read.reads.len(), 2);
                assert_eq!(observed.len(), 20);
            }
            Err(failure) => {
                assert!(!expect_ok);
                assert_eq!(failure.code, "voucher_window_too_many_reads");
                assert!(failure.evidence.is_some(), "the reads made are kept");
                // The oversized attempt and day one: the third is never sent.
                assert_eq!(observed.len(), 8);
            }
        }
    }
}

#[tokio::test]
async fn a_divided_reads_evidence_is_folded_in_the_order_it_was_sent() {
    // Review: the closing bracket is read after the data parts, so every fold of
    // this read's evidence must put it after them, not beside the opening reads.
    let read = first_read().await;
    let opening = read.preflight_evidence.clone().expect("the census");
    let closing = read.closing_evidence.clone().expect("the closing marks");
    let in_order = combine_evidence(
        combine_evidence(opening.clone(), read.evidence.clone()),
        closing.clone(),
    );
    let out_of_order = combine_evidence(combine_evidence(opening, closing), read.evidence.clone());
    let all = read.all_evidence();
    assert_eq!(
        (all.request_sha256, all.response_sha256, all.bytes),
        (
            in_order.request_sha256.clone(),
            in_order.response_sha256.clone(),
            in_order.bytes
        )
    );
    assert_ne!(in_order.request_sha256, out_of_order.request_sha256);
}

#[test]
fn the_pre_post_request_is_admitted_on_what_verification_measured() {
    // Review of #520: the whole-window pre-post request is admitted on the
    // verification read's own measurement, not the pre-flight's default-cost
    // prediction — which refused any window already holding 171 vouchers in
    // the verification shape, however light the book.
    let evidence = |bytes: usize| Evidence {
        request_sha256: String::new(),
        response_sha256: String::new(),
        bytes,
        state: "complete",
        read_at: None,
        duration_ms: None,
        reason_code: None,
    };
    let whole = [part("20260801", "20260831", None)];
    let divided = [
        part("20260801", "20260815", None),
        part("20260816", "20260831", None),
    ];
    let budget = usize::try_from(WINDOW_READ_BUDGET_BYTES).unwrap();
    // Undivided: that read was the whole request, whatever its size.
    assert!(WindowServed::of(&whole, &evidence(4 * budget), false).fits_one_request());
    // Divided, a light book: both parts together (one copy each) fit.
    let light = WindowServed::of(&divided, &evidence(2 * budget), false);
    assert_eq!(light.data_bytes, WINDOW_READ_BUDGET_BYTES);
    assert!(light.fits_one_request());
    // Divided, and one byte more than the budget: refused.
    assert!(!WindowServed::of(&divided, &evidence(2 * budget + 2), false).fits_one_request());
}

#[tokio::test]
async fn a_corroborating_replay_carries_the_first_reads_witness() {
    // Review of #520: verify_import's corroboration builds its replay with
    // `replay_of`; dropping the witness there went unnoticed by every test.
    let first = first_read().await;
    let witness = first.witness.clone().expect("a divided read has a witness");
    match WindowPlanSource::replay_of(first.reads.clone(), first.witness) {
        WindowPlanSource::Replay {
            parts,
            witness: Some(carried),
        } => {
            assert_eq!(parts, first.reads);
            assert_eq!(carried, witness);
        }
        _ => panic!("the replay must carry the witness"),
    }
}
#[test]
fn a_bound_refusal_with_a_concrete_next_step_names_it() {
    // The too-large book must say that narrowing the window does not help —
    // the retry a caller would otherwise try first.
    let book = refusal_remediation(BOOK_TOO_LARGE).expect("guidance");
    assert!(
        book.contains("A shorter date window will not help"),
        "{book}"
    );
    let post = refusal_remediation("import_post_window_not_bounded").expect("guidance");
    assert!(
        post.contains("Build the batch again over fewer days"),
        "{post}"
    );
}

#[tokio::test]
async fn a_replay_reads_exactly_the_parts_it_was_given() {
    // Review of #520: the first read counted one voucher on each of days 1-3,
    // read day 1, planned days 2-4 as one part, and divided it into 2-3 and 4
    // after Tally could not serve it. Before this, the replay re-planned from
    // its first measurement and sent the 2-4 request the first read had just
    // found unservable. It now reads the first read's parts, in order.
    let shape = VoucherReadShape::ImportVerification;
    let window = ("20260801", "20260804");
    let census = relabelled(
        &three_vouchers(),
        &[(1, "20260801"), (2, "20260802"), (3, "20260803")],
    );
    let d1 = xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")]));
    let d23 = xml_plan(relabelled(
        &vouchers_kept(2),
        &[(2, "20260802"), (3, "20260803")],
    ));
    let mut plans = paired(&xml_plan(census));
    plans.extend(paired(&d1));
    plans.extend(oversized());
    plans.extend(paired(&d23));
    plans.extend(paired(&xml_plan(empty_collection())));
    plans.extend(paired(&mark(3)));
    let (first, _) = read_window(
        plans,
        window,
        shape,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        three_a_read(),
    )
    .await;
    let first = first.unwrap();
    assert_eq!(
        first.reads,
        [
            part("20260801", "20260801", None),
            part("20260802", "20260803", None),
            part("20260804", "20260804", None),
        ]
    );
    let mut plans = paired(&d1);
    plans.extend(paired(&d23));
    plans.extend(paired(&xml_plan(empty_collection())));
    plans.extend(paired(&mark(3)));
    let (replay, observed) = read_window(
        plans,
        window,
        shape,
        WindowPlanSource::Replay {
            parts: first.reads.clone(),
            witness: first.witness.clone(),
        },
        three_a_read(),
    )
    .await;
    assert_eq!(replay.unwrap().reads, first.reads);
    assert_requests(
        &observed,
        &[1, 7, 13],
        &first
            .reads
            .iter()
            .map(|read| {
                shape
                    .render(&company(), &read.from, &read.to, read.span)
                    .unwrap()
            })
            .collect::<Vec<_>>(),
    );
}

#[tokio::test]
async fn a_replay_refuses_a_master_mark_that_moved_since_the_first_read() {
    // A ledger renamed between the first read and its replay changes the
    // replayed exports only through names; the master mark is what shows it.
    let first = first_read().await;
    let (_, parts) = divided_first_read();
    let mut plans = Vec::new();
    for part in &parts {
        plans.extend(paired(part));
    }
    plans.extend(paired(&marks_plan(3, 8)));
    let (outcome, _) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Replay {
            parts: first.reads,
            witness: first.witness,
        },
        three_a_read(),
    )
    .await;
    assert_eq!(
        outcome.err().map(|failure| failure.code).as_deref(),
        Some(WINDOW_CHANGED_DURING_READ)
    );
}

#[tokio::test]
async fn a_read_after_tally_refused_the_whole_window_does_not_admit_it_whole() {
    // Re-check of #520 review: verification planned the window whole, Tally
    // refused that request as too large, and the two days were read instead.
    // The parts summed well under the budget, and before this the pre-post
    // check admitted the very request Tally had just refused. A deadline would
    // be the same case with no size at all to sum.
    let shape = VoucherReadShape::ImportVerification;
    let mut plans = oversized();
    plans.extend(paired(&xml_plan(three_vouchers())));
    plans.extend(paired(&xml_plan(empty_collection())));
    plans.extend(paired(&mark(1)));
    let (outcome, _) = read_window(
        plans,
        ("20260801", "20260802"),
        shape,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(1)),
        },
        WindowReadLimits::for_shape(shape),
    )
    .await;
    let read = outcome.unwrap();
    assert!(read.refused_a_part);
    // #595: the refused request is timed as a part Tally did not serve, and
    // the read that stands reports no failure.
    assert!(!read.timings.parts[0].served);
    assert_eq!(read.timings.parts[0].bytes, None);
    assert!(read.timings.parts[1..].iter().all(|part| part.served));
    assert_eq!(read.timings.failed, None);
    let served = WindowServed::of(&read.reads, &read.evidence, read.refused_a_part);
    assert!(served.data_bytes <= WINDOW_READ_BUDGET_BYTES);
    assert!(!served.fits_one_request());
    // Control: the same parts, read without a refusal, would be admitted.
    assert!(WindowServed::of(&read.reads, &read.evidence, false).fits_one_request());
}

/// Three parts of one day-divided read, as in
/// `the_first_part_measures_the_book_and_the_rest_of_its_day_is_read_above_it`,
/// with the first part's report leg held so a withdrawal lands while it runs.
fn three_part_plans(hold_first_part: bool) -> Vec<ScenarioPlan> {
    let mut plans = paired(&xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")])));
    if hold_first_part {
        plans[1] = plans[1]
            .clone()
            .with_delivery(tally_protocol_simulator::Delivery::SlowHeaders(
                std::time::Duration::from_millis(900),
            ));
    }
    plans.extend(paired(&xml_plan(relabelled(
        &vouchers_kept(2),
        &[(2, "20260801"), (3, "20260801")],
    ))));
    plans.extend(paired(&xml_plan(relabelled(
        &vouchers_kept(2),
        &[(4, "20260802"), (5, "20260802")],
    ))));
    plans
}

fn three_part_census() -> WindowCensus {
    WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260801"), 2),
        (day("20260801"), 3),
        (day("20260802"), 4),
        (day("20260802"), 5),
    ])
}

async fn read_three_parts_under(
    cancellation: tokio_util::sync::CancellationToken,
    withdraw_after: Option<std::time::Duration>,
) -> (
    Result<WindowReadOutcome<Value>, ToolFailure>,
    Vec<tally_protocol_simulator::ObservedRequest>,
) {
    let simulator = SequenceSimulator::spawn(three_part_plans(withdraw_after.is_some())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let identity = identity();
    if let Some(delay) = withdraw_after {
        let withdraw = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            withdraw.cancel();
        });
    }
    let outcome = crate::tally::runtime::TOOL_CANCELLATION
        .scope(
            cancellation,
            server.read_voucher_window(
                &identity,
                identity.display_name(),
                "20260801",
                "20260802",
                VoucherReadShape::EntryWildcard,
                WindowPlanSource::Counted(three_part_census()),
                three_a_read(),
                |xml| parse_agent_rows(xml, GUID),
            ),
        )
        .await;
    simulator.cancel();
    // `cancel` wakes the simulator with an empty connection; keep only the
    // requests Bridge actually sent.
    let sent = simulator
        .finish()
        .unwrap()
        .into_iter()
        .filter(|request| !request.method.is_empty())
        .collect();
    (outcome, sent)
}

#[tokio::test]
async fn a_withdrawal_during_one_part_sends_no_further_part() {
    // #554. The withdrawal lands while the first part's report leg is held. That
    // part runs to completion (its six legs: abandoning a request does not stop
    // Tally), and the next part is never sent. The read is refused as withdrawn
    // with the first part's evidence kept, never returned as a partial window.
    let (outcome, observed) = read_three_parts_under(
        tokio_util::sync::CancellationToken::new(),
        Some(std::time::Duration::from_millis(200)),
    )
    .await;
    assert_eq!(observed.len(), 6, "only the part in flight was sent");
    let failure = outcome.err().expect("a withdrawn read is refused");
    assert_eq!(failure.code, "request_cancelled");
    // The part already read is accounted for; the response layer marks the
    // refusal's evidence partial (see the stdio test).
    let evidence = failure
        .evidence
        .expect("the part already read is accounted");
    assert!(!evidence.response_sha256.is_empty() && evidence.bytes > 0);
}

#[tokio::test]
async fn a_read_whose_withdrawal_never_comes_reads_every_part() {
    // Control: the same read under a cancellation that is never fired.
    let (outcome, observed) =
        read_three_parts_under(tokio_util::sync::CancellationToken::new(), None).await;
    assert_eq!(
        outcome.expect("an unwithdrawn read completes").rows.len(),
        5
    );
    assert_eq!(observed.len(), 18);
}

// --- Education date boundaries (#581) ---------------------------------------

/// The captured licensed company list with Tally's `EDUMODE` flag set, which
/// is how the list reports an instance in Education mode. A deliberate
/// synthetic mutation of the captured bytes; the flag is the only change.
fn education_company_plan() -> ScenarioPlan {
    let licensed = captured_utf16(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-licensed-companies.utf16le.xml"
    ));
    let education = licensed.replace(
        "<EDUMODE TYPE=\"Logical\">No</EDUMODE>",
        "<EDUMODE TYPE=\"Logical\">Yes</EDUMODE>",
    );
    assert_ne!(education, licensed);
    xml_plan(education)
}

/// Lane A's capture: what Education serves a movement or voucher read that
/// starts on a day other than the 1st, 2nd or 31st (an empty collection).
fn education_empty_part_plan() -> ScenarioPlan {
    xml_plan(captured_utf16(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-education-empty-movement-part.utf16le.xml"
    )))
}

/// One paired, identity-bracketed read with the Education company list.
fn education_paired(body: &ScenarioPlan) -> Vec<ScenarioPlan> {
    vec![
        education_company_plan(),
        body.clone(),
        status_plan(),
        body.clone(),
        status_plan(),
        education_company_plan(),
    ]
}

/// The vouchers tool over a window Education cannot serve: it starts on the
/// 5th. Before bridge#581 the part was sent, Education answered it with its
/// captured empty collection, and the tool reported an empty window marked
/// `partial / empty_uncorroborated` after 28 requests. Now the mark read's own
/// identity bracket reports Education, and the plan is refused by name before
/// anything of the part is sent.
#[tokio::test]
async fn education_refuses_a_window_starting_on_an_unaccepted_day_before_sending_it() {
    let mut plans = vec![
        education_company_plan(),
        status_plan(),
        education_company_plan(),
        status_plan(),
    ];
    plans.extend(education_paired(&mark(3)));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let response = server
        .call_tool(
            "vouchers",
            json!({"company_guid": GUID, "from": "20260405", "to": "20260405"}),
        )
        .await;
    simulator.cancel();
    let observed = simulator
        .finish()
        .unwrap()
        .into_iter()
        .filter(|request| !request.method.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        response["structuredContent"]["result"]["error"]["code"],
        "window_part_boundary_unsupported_in_education",
        "{response}"
    );
    assert_eq!(observed.len(), 10);
    let part = [
        render_agent_vouchers(&company(), "20260405", "20260405", None).unwrap(),
        render_agent_vouchers_in_span(&company(), "20260405", "20260405", None).unwrap(),
    ];
    assert!(observed.iter().all(|request| part
        .iter()
        .all(|xml| request.request_body_sha256 != request_sha(xml))));
}

/// The control: the same read on a licensed endpoint is sent as before, and
/// its empty window is corroborated as it always was.
#[tokio::test]
async fn a_licensed_endpoint_still_reads_a_window_starting_on_any_day() {
    let empty = education_empty_part_plan();
    let mut plans = vec![company_plan(), status_plan(), company_plan(), status_plan()];
    plans.extend(paired(&mark(3)));
    plans.extend(paired(&empty));
    plans.extend(paired(&empty));
    plans.extend(paired(&mark(3)));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let response = server
        .call_tool(
            "vouchers",
            json!({"company_guid": GUID, "from": "20260405", "to": "20260405"}),
        )
        .await;
    simulator.cancel();
    let sent = simulator
        .finish()
        .unwrap()
        .iter()
        .filter(|request| !request.method.is_empty())
        .count();
    assert_eq!(sent, 28);
    let result = &response["structuredContent"]["result"];
    assert_eq!(result["state"], "partial", "{response}");
    assert_eq!(result["reason"], "empty_uncorroborated");
}

/// A divided plan is refused whole, before its first part (bridge#581). The
/// census counts one voucher on each of the 1st, 2nd and 10th and one voucher
/// fits a read, so the plan is 1st, 2nd to 9th, 10th to 31st. Its first part
/// is one Education serves; its second is not. Checking only as each part is
/// sent would read the first part and then refuse; the census's own bracket
/// has already reported Education, so nothing is read.
#[tokio::test]
async fn an_education_plan_with_an_unaccepted_part_boundary_is_refused_before_any_part() {
    let census = relabelled(
        &three_vouchers(),
        &[(1, "20260801"), (2, "20260802"), (3, "20260810")],
    );
    let (outcome, observed) = read_window(
        education_paired(&xml_plan(census)),
        ("20260801", "20260831"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        three_a_read(),
    )
    .await;
    let failure = outcome.err().expect("the plan is refused");
    assert_eq!(failure.code, EDUCATION_BOUNDARY_UNSUPPORTED);
    assert!(failure.evidence.is_some(), "the census is accounted for");
    assert_eq!(observed.len(), 6);
    assert_requests(
        &observed,
        &[1],
        &[render_agent_voucher_census(&company(), "20260801", "20260831", None).unwrap()],
    );
}

/// The same census on a licensed endpoint plans the parts the Education test
/// refuses, and reads them: the control that the refusal is the mode's.
#[test]
fn the_refused_education_plan_divides_on_days_education_does_not_honour() {
    let limits = three_a_read();
    let plan = plan_window_reads(
        day("20260801"),
        day("20260831"),
        &census_of(&[("20260801", 1), ("20260802", 1), ("20260810", 1)]),
        None,
        3,
        limits.default_bytes_per_voucher,
        limits.budget_bytes,
        limits.max_reads,
    )
    .unwrap();
    assert_eq!(
        stack_of(&plan).into_iter().rev().collect::<Vec<_>>(),
        [
            part("20260801", "20260801", None),
            part("20260802", "20260809", None),
            part("20260810", "20260831", None),
        ]
    );
}

/// Without a preflight read the executor has not yet observed the mode, so the
/// runtime refuses the part itself: after its opening identity bracket reports
/// Education and before its data request is sent.
#[tokio::test]
async fn the_runtime_refuses_an_education_read_the_executor_could_not_check() {
    let (outcome, observed) = read_window(
        vec![education_company_plan()],
        ("20260805", "20260805"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census_of(&[("20260805", 1)])),
        three_a_read(),
    )
    .await;
    let failure = outcome.err().expect("the read is refused");
    assert_eq!(failure.code, EDUCATION_BOUNDARY_UNSUPPORTED);
    assert_eq!(
        observed
            .iter()
            .filter(|request| !request.method.is_empty())
            .count(),
        1
    );
}

/// Education seen on any read of a window holds for the rest of it; a later
/// licensed observation never relaxes it.
#[test]
fn an_observed_education_profile_is_never_relaxed_within_a_window() {
    use DateBoundaryProfile::{EducationRestricted, ModeAgnostic};
    for (observations, expected) in [
        (vec![], None),
        (vec![ModeAgnostic], Some(ModeAgnostic)),
        (vec![EducationRestricted], Some(EducationRestricted)),
        (
            vec![ModeAgnostic, EducationRestricted],
            Some(EducationRestricted),
        ),
        (
            vec![EducationRestricted, ModeAgnostic],
            Some(EducationRestricted),
        ),
    ] {
        let mut boundary = None;
        for observed in observations.iter().copied() {
            observe_boundary(&mut boundary, observed);
        }
        assert_eq!(boundary, expected, "{observations:?}");
    }
}

/// The end of a part is held to the rule as well as its start. Education's
/// treatment of an end day is not measured, so a whole-month window ending on
/// the 30th is refused: the accepted price of admitting only measured
/// boundaries (bridge#581).
#[tokio::test]
async fn an_education_window_ending_on_an_unaccepted_day_is_refused_before_it_is_sent() {
    let (outcome, observed) = read_window(
        education_paired(&mark(1)),
        ("20260901", "20260930"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate { known_marks: None },
        three_a_read(),
    )
    .await;
    let failure = outcome.err().expect("the window is refused");
    assert_eq!(failure.code, EDUCATION_BOUNDARY_UNSUPPORTED);
    assert_eq!(observed.len(), 6, "only the mark read is sent");
}

/// Each way a part fails admission is named beside `voucher_window_part_not_admitted`
/// (bridge#581), and a census disagreement carries its counts: an Education
/// part served empty reads as "returned 0 of 2", not as a bare refusal.
#[test]
fn a_part_not_admitted_names_its_cause_and_a_census_mismatch_its_counts() {
    // AlterIDs 1 and 2 on the 1st, 3 on the 2nd.
    let census = census_of(&[("20260801", 2), ("20260802", 1)]);
    let row = |alter_id: u64, date: &str| json!({"date": date, "alter_id": alter_id});
    let day_one = part("20260801", "20260801", None);
    let first_only = part(
        "20260801",
        "20260801",
        Some(AlterIdSpan {
            after: 0,
            through: 1,
        }),
    );
    let window = (day("20260801"), day("20260802"));
    for (part, rows, expected) in [
        (&day_one, vec![], Err((PART_CENSUS_MISMATCH, Some((0, 2))))),
        (
            &day_one,
            vec![row(1, "20260801"), row(2, "20260801")],
            Ok(()),
        ),
        (
            &day_one,
            vec![row(1, "20260801"), row(2, "20260801"), row(4, "20260801")],
            Err((PART_CENSUS_MISMATCH, Some((3, 2)))),
        ),
        (
            &day_one,
            vec![row(1, "20260801"), row(9, "20260801")],
            Err((PART_CENSUS_MISMATCH, Some((2, 2)))),
        ),
        (
            &day_one,
            vec![json!({"alter_id": 1})],
            Err((PART_ROW_UNREADABLE, None)),
        ),
        (
            &day_one,
            vec![json!({"date": "20260801"})],
            Err((PART_ROW_UNREADABLE, None)),
        ),
        (
            &day_one,
            vec![row(1, "20260801"), row(3, "20260802")],
            Err((PART_ROW_OUTSIDE_DATES, None)),
        ),
        (
            &day_one,
            vec![row(1, "20260801"), row(1, "20260801")],
            Err((PART_ROW_DUPLICATED, None)),
        ),
        (
            &first_only,
            vec![row(2, "20260801")],
            Err((PART_ROW_OUTSIDE_ALTER_ID_SPAN, None)),
        ),
    ] {
        let outcome = admit_part(part, window, &rows, Some(&census)).map_err(|failure| {
            assert_eq!(failure.code, PART_NOT_ADMITTED);
            (
                failure.cause.expect("a named cause"),
                failure
                    .counts
                    .map(|counts| (counts.returned, counts.counted)),
            )
        });
        assert_eq!(outcome, expected, "{rows:?}");
    }
}

/// The counts reach the caller beside the cause, through the tool's own error.
#[tokio::test]
async fn a_census_mismatch_reaches_the_caller_with_its_counts() {
    // A census of 50 vouchers on one day, more than one read carries at the
    // shipped limits, so the plan's first part is an AlterID span. Tally
    // answers it empty.
    let limits = WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard);
    let per_read = limits.budget_bytes / limits.default_bytes_per_voucher;
    let total = per_read + 8;
    let one = vouchers_kept(1);
    let start = one.find("<VOUCHER ").unwrap();
    let end = start + one[start..].find("</VOUCHER>").unwrap() + "</VOUCHER>".len();
    let many = format!(
        "{}{}{}",
        &one[..start],
        one[start..end].repeat(total as usize),
        &one[end..]
    );
    let ids = (1..=total).map(|id| (id, "20260801")).collect::<Vec<_>>();
    let census = relabelled(&many, &ids);
    let mut plans = vec![company_plan(), status_plan(), company_plan(), status_plan()];
    plans.extend(paired(&marks_plan(total, 7)));
    plans.extend(paired(&xml_plan(census)));
    plans.extend(paired(&xml_plan(empty_collection())));
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let response = server
        .call_tool(
            "vouchers",
            json!({"company_guid": GUID, "from": "20260801", "to": "20260801"}),
        )
        .await;
    simulator.cancel();
    let sent = simulator
        .finish()
        .unwrap()
        .iter()
        .filter(|request| !request.method.is_empty())
        .count();
    assert_eq!(sent, 22);
    let error = &response["structuredContent"]["result"]["error"];
    assert_eq!(error["code"], PART_NOT_ADMITTED, "{response}");
    assert_eq!(error["cause"], PART_CENSUS_MISMATCH);
    assert_eq!(error["counts"], json!({"returned": 0, "counted": per_read}));
}

/// A licence that drops to Education while a read is in flight: the opening
/// bracket said licensed, the closing one says Education. Which mode served
/// the read is unknown, so a read Education would have served empty is refused,
/// with the read it made accounted for (bridge#581).
#[tokio::test]
async fn education_reported_after_a_read_refuses_a_boundary_education_serves_empty() {
    let body = xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260805")]));
    let (outcome, observed) = read_window(
        vec![
            company_plan(),
            body.clone(),
            status_plan(),
            body,
            status_plan(),
            education_company_plan(),
        ],
        ("20260805", "20260805"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census_of(&[("20260805", 1)])),
        three_a_read(),
    )
    .await;
    let failure = outcome.err().expect("the read is refused");
    assert_eq!(failure.code, EDUCATION_BOUNDARY_UNSUPPORTED);
    assert!(failure.evidence.is_some(), "the data read is accounted for");
    assert_eq!(observed.len(), 6);
}

/// An Education list whose other capability fields do not parse still
/// restricts: an empty `SILVER` must not switch the guard off.
#[tokio::test]
async fn education_is_observed_when_another_capability_field_is_empty() {
    let mut plan = education_company_plan();
    let body = plan.fixture.body().into_owned();
    let broken = body.replace(
        "<SILVER TYPE=\"Logical\">Yes</SILVER>",
        "<SILVER TYPE=\"Logical\"/>",
    );
    assert_ne!(broken, body);
    assert!(bridge_tally_protocol::parse_company_gateway_capability_observation(&broken).is_err());
    plan = xml_plan(broken);
    let (outcome, observed) = read_window(
        vec![plan],
        ("20260805", "20260805"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census_of(&[("20260805", 1)])),
        three_a_read(),
    )
    .await;
    assert_eq!(
        outcome.err().map(|failure| failure.code).as_deref(),
        Some(EDUCATION_BOUNDARY_UNSUPPORTED)
    );
    assert_eq!(
        observed
            .iter()
            .filter(|request| !request.method.is_empty())
            .count(),
        1
    );
}

/// A census over a window Education cannot serve is refused by the runtime
/// under its own code, not relabelled as an unestimated window.
#[tokio::test]
async fn a_census_education_cannot_serve_is_refused_by_name() {
    let (outcome, observed) = read_window(
        vec![education_company_plan()],
        ("20260805", "20260831"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        three_a_read(),
    )
    .await;
    assert_eq!(
        outcome.err().map(|failure| failure.code).as_deref(),
        Some(EDUCATION_BOUNDARY_UNSUPPORTED)
    );
    assert_eq!(
        observed
            .iter()
            .filter(|request| !request.method.is_empty())
            .count(),
        1
    );
}

// --- Window readers (audit_read plan step 5b) ---

/// The three legs of one audit part: identity bracket, one unpaired read,
/// identity bracket.
fn single(body: &ScenarioPlan) -> Vec<ScenarioPlan> {
    vec![company_plan(), body.clone(), company_plan()]
}

/// The plans of `a_divided_read_is_bracketed_by_the_high_water_mark`, in the
/// order its reads are sent, with `legs` shaping each read.
fn divided_window_reads(closing: ScenarioPlan) -> Vec<ScenarioPlan> {
    vec![
        xml_plan(three_vouchers()),
        xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")])),
        xml_plan(relabelled(
            &vouchers_kept(2),
            &[(2, "20260801"), (3, "20260801")],
        )),
        xml_plan(empty_collection()),
        closing,
    ]
}

fn divided_parts() -> Vec<WindowPart> {
    vec![
        part(
            "20260801",
            "20260801",
            Some(AlterIdSpan {
                after: 0,
                through: 1,
            }),
        ),
        part(
            "20260801",
            "20260801",
            Some(AlterIdSpan {
                after: 1,
                through: 3,
            }),
        ),
        part("20260802", "20260802", None),
    ]
}

async fn read_audit_window(
    plans: Vec<ScenarioPlan>,
    runtime: TallyRuntime,
) -> (
    Result<WindowReadOutcome<Value>, ToolFailure>,
    Option<Vec<RetainedWindowRead>>,
    Option<AuditWindowFailure>,
    Vec<tally_protocol_simulator::ObservedRequest>,
    TallyConfig,
) {
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let config = TallyConfig {
        host: simulator.address().ip().to_string(),
        port: simulator.address().port(),
    };
    let identity = identity();
    // Through the owned entry point, as the orchestrator will read.
    let read = AuditWindowReader::new(&runtime, config.clone())
        .read_window(
            &identity,
            identity.display_name(),
            "20260801",
            "20260802",
            VoucherReadShape::EntryWildcard,
            three_a_read(),
            |xml| parse_agent_rows(xml, GUID),
        )
        .await;
    (
        read.outcome,
        read.retained,
        read.failure,
        simulator.finish().unwrap(),
        config,
    )
}

/// An undivided window for a sealed record is bracketed too: its record states
/// the marks it read, so it reads them again after its one part and refuses
/// if they moved, where an agent read of the same window would not look again.
#[tokio::test]
async fn an_undivided_audit_window_closes_its_bracket() {
    let one = || xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")]));
    let (outcome, retained, failure, observed, _) = read_audit_window(
        [marks_plan(1, 7), one(), marks_plan(1, 7)]
            .iter()
            .flat_map(single)
            .collect(),
        TallyRuntime::default(),
    )
    .await;
    let read = outcome.expect("an unchanged undivided window reads");
    assert!(failure.is_none());
    assert_eq!(read.reads, [part("20260801", "20260802", None)]);
    assert_eq!(observed.len(), 9);
    assert_eq!(
        retained
            .expect("its reads")
            .iter()
            .map(|r| r.kind.clone())
            .collect::<Vec<_>>(),
        [
            WindowReadKind::Marks,
            WindowReadKind::Part(part("20260801", "20260802", None)),
            WindowReadKind::Marks,
        ]
    );

    let (moved, retained, failure, _, _) = read_audit_window(
        [marks_plan(1, 7), one(), marks_plan(2, 7)]
            .iter()
            .flat_map(single)
            .collect(),
        TallyRuntime::default(),
    )
    .await;
    assert_eq!(
        moved.err().map(|f| f.code),
        Some(WINDOW_CHANGED_DURING_READ.to_string())
    );
    assert_eq!(failure, Some(AuditWindowFailure::WindowChanged));
    assert!(retained.is_none());
}

/// `Server::read_voucher_window` and an explicit [`AgentReader`] send the same
/// requests in the same order, so the two entry points cannot drift apart.
/// Both run the one executor, so this cannot show that the agent path is
/// unchanged from before readers existed. The pre-existing window tests show
/// that: they pass unmodified.
#[tokio::test]
async fn the_agent_reader_sends_exactly_the_requests_the_window_read_sent() {
    let plans = || {
        divided_window_reads(marks_plan(3, 7))
            .iter()
            .flat_map(paired)
            .collect::<Vec<_>>()
    };
    let (through_server, server_observed) = read_window(
        plans(),
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        three_a_read(),
    )
    .await;
    let simulator = SequenceSimulator::spawn(plans()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(simulator.address(), directory.path());
    let identity = identity();
    let through_reader = read_voucher_window_with(
        &AgentReader(&server),
        &identity,
        identity.display_name(),
        "20260801",
        "20260802",
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        three_a_read(),
        |xml| parse_agent_rows(xml, GUID),
    )
    .await;
    let reader_observed = simulator.finish().unwrap();
    assert_eq!(through_server.unwrap().reads, through_reader.unwrap().reads);
    let shas = |observed: &[tally_protocol_simulator::ObservedRequest]| {
        observed
            .iter()
            .map(|request| request.request_body_sha256.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(shas(&server_observed), shas(&reader_observed));
}

/// A divided window through the audit reader: the same plan, admission and
/// bracket as the agent path, each request one unpaired audit part, and every
/// admitted response kept byte for byte, in order.
#[tokio::test]
async fn an_audit_window_keeps_every_read_it_admitted_as_it_arrived() {
    // The audit window reads its own opening marks first.
    let reads = [
        vec![marks_plan(3, 7)],
        divided_window_reads(marks_plan(3, 7)),
    ]
    .concat();
    let wires = reads
        .iter()
        .map(|plan| tally_protocol_simulator::encode(&plan.fixture.body(), plan.encoding))
        .collect::<Vec<_>>();
    let (outcome, retained, failure, observed, _) = read_audit_window(
        reads.iter().flat_map(single).collect(),
        TallyRuntime::default(),
    )
    .await;
    let read = outcome.expect("the window reads");
    assert!(failure.is_none());
    let retained = retained.expect("a completed window yields its reads");
    assert_eq!(read.rows.len(), 3);
    assert_eq!(read.reads, divided_parts());
    // Opening marks, census, three parts, closing marks: six reads of three
    // legs each.
    assert_eq!(observed.len(), 18);
    let kinds = retained.iter().map(|r| r.kind.clone()).collect::<Vec<_>>();
    let mut expected = vec![WindowReadKind::Marks, WindowReadKind::Census];
    expected.extend(divided_parts().into_iter().map(WindowReadKind::Part));
    expected.push(WindowReadKind::Marks);
    assert_eq!(kinds, expected);
    for (index, (kept, wire)) in retained.iter().zip(&wires).enumerate() {
        assert_eq!(&kept.part.encoded_body, wire, "read {index}");
        assert_eq!(
            kept.request_sha256,
            observed[index * 3 + 1].request_body_sha256
        );
        assert_eq!(
            kept.part.boundary_profile,
            DateBoundaryProfile::ModeAgnostic
        );
    }
}

/// The reader never changes admission: a part that returns fewer vouchers than
/// the census counted is refused through the audit reader exactly as through
/// the agent path, and is not a transport failure of the part.
#[tokio::test]
async fn an_audit_window_refuses_a_part_the_census_disagrees_with() {
    let mut reads = divided_window_reads(marks_plan(3, 7));
    reads[1] = xml_plan(empty_collection());
    let (agent, _) = read_window(
        reads.iter().take(2).flat_map(paired).collect(),
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        three_a_read(),
    )
    .await;
    let agent = agent.err().expect("the agent path refuses");
    let (audit, retained, failure, observed, _) = read_audit_window(
        std::iter::once(marks_plan(3, 7))
            .chain(reads.iter().take(2).cloned())
            .flat_map(|plan| single(&plan))
            .collect(),
        TallyRuntime::default(),
    )
    .await;
    let audit = audit.err().expect("the audit path refuses");
    assert_eq!(audit.code, PART_NOT_ADMITTED);
    assert_eq!(audit.code, agent.code);
    assert_eq!(audit.cause, agent.cause);
    assert_eq!(audit.counts, agent.counts);
    assert_eq!(
        failure,
        Some(AuditWindowFailure::Refused(PART_NOT_ADMITTED.to_string()))
    );
    assert!(!failure.unwrap().retryable());
    // The opening marks, the census and the refused part were read, nothing
    // after, and a refused window yields no retained reads at all.
    assert!(retained.is_none());
    assert_eq!(observed.len(), 9);
}

/// Someone else writing is the normal case on a multi-user book: marks that
/// move across a divided window refuse it, typed as retryable.
#[tokio::test]
async fn an_audit_window_the_book_changed_under_is_retryable() {
    for closing in [marks_plan(4, 7), marks_plan(3, 8)] {
        let (outcome, _, failure, _, _) = read_audit_window(
            std::iter::once(marks_plan(3, 7))
                .chain(divided_window_reads(closing))
                .flat_map(|plan| single(&plan))
                .collect(),
            TallyRuntime::default(),
        )
        .await;
        assert_eq!(
            outcome.err().expect("changed").code,
            WINDOW_CHANGED_DURING_READ
        );
        assert_eq!(failure, Some(AuditWindowFailure::WindowChanged));
        assert!(failure.unwrap().retryable());
    }
}

/// A part whose connection drops stops the window at that part, typed with the
/// part's kind, and the endpoint then owes a drain: no later window reads.
#[tokio::test]
async fn an_audit_window_stops_at_a_dropped_part_and_owes_a_drain() {
    let mut reads = divided_window_reads(marks_plan(3, 7));
    reads[2] = reads[2].clone().with_delivery(
        tally_protocol_simulator::Delivery::ResetAfterRequestProcessed {
            delay: std::time::Duration::ZERO,
        },
    );
    let runtime = TallyRuntime::default();
    let (outcome, retained, failure, observed, config) = read_audit_window(
        single(&marks_plan(3, 7))
            .into_iter()
            .chain(reads.iter().take(3).flat_map(single).take(8))
            .collect(),
        runtime.clone(),
    )
    .await;
    assert!(outcome.is_err());
    assert_eq!(
        failure,
        Some(AuditWindowFailure::Part(
            crate::tally::runtime::AuditPartFailureKind::ConnectionDropped
        ))
    );
    assert!(failure.unwrap().retryable());
    // The marks, the census and the first part arrived before the drop, but a
    // failed window yields no retained reads.
    assert!(retained.is_none());
    assert_eq!(observed.len(), 11);
    // A second window to the same endpoint is refused before sending anything:
    // the drain debt is keyed by endpoint, and the check precedes any request,
    // so it holds even with nothing listening there any more.
    let reader = AuditWindowReader::new(&runtime, config);
    let identity = identity();
    let again = read_voucher_window_with(
        &reader,
        &identity,
        identity.display_name(),
        "20260801",
        "20260802",
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate { known_marks: None },
        three_a_read(),
        |xml| parse_agent_rows(xml, GUID),
    )
    .await;
    let again_failure = again
        .as_ref()
        .err()
        .map(|failure| audit_window_failure(failure, &reader));
    assert!(again.is_err());
    assert!(reader.into_retained(&again).is_none());
    assert_eq!(
        again_failure,
        Some(AuditWindowFailure::Part(
            crate::tally::runtime::AuditPartFailureKind::DrainRequired
        ))
    );
}

/// A later part is planned at the cost the first part measured, and the audit
/// reader measures it the same as the agent reader, although it reads each part
/// once where the agent reads it twice. Day two's four vouchers must go in two
/// parts through either reader: planned at half their real cost, the audit
/// reader would ask for all four in one part, twice the budget.
#[tokio::test]
async fn the_audit_reader_plans_later_parts_at_the_cost_the_agent_reader_measures() {
    let two = |ids: &[(u64, &str)]| xml_plan(relabelled(&vouchers_kept(2), ids));
    let day_one = two(&[(1, "20260801"), (2, "20260801")]);
    let first_half = two(&[(3, "20260802"), (4, "20260802")]);
    let second_half = two(&[(5, "20260802"), (6, "20260802")]);
    // One response of two vouchers is exactly the budget, so the measured cost
    // allows two vouchers a part, and so does the default.
    let per_voucher = wire_len(&day_one) / 2;
    let limits = WindowReadLimits {
        budget_bytes: 2 * per_voucher,
        default_bytes_per_voucher: per_voucher,
        max_reads: MAX_PLANNED_READS,
    };
    let census = || {
        WindowCensus::from_rows([
            (day("20260801"), 1),
            (day("20260801"), 2),
            (day("20260802"), 3),
            (day("20260802"), 4),
            (day("20260802"), 5),
            (day("20260802"), 6),
        ])
    };
    let bodies = [day_one, first_half, second_half];
    let expected = [
        part("20260801", "20260801", None),
        part(
            "20260802",
            "20260802",
            Some(AlterIdSpan {
                after: 0,
                through: 4,
            }),
        ),
        part(
            "20260802",
            "20260802",
            Some(AlterIdSpan {
                after: 4,
                through: 6,
            }),
        ),
    ];

    let (agent, _) = read_window(
        bodies.iter().flat_map(paired).collect(),
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census()),
        limits,
    )
    .await;
    assert_eq!(agent.unwrap().reads, expected);

    // The audit reader reads its own marks and census, then the same parts.
    let census_xml = {
        let first = relabelled(
            &three_vouchers(),
            &[(1, "20260801"), (2, "20260801"), (3, "20260802")],
        );
        let second = relabelled(
            &three_vouchers(),
            &[(4, "20260802"), (5, "20260802"), (6, "20260802")],
        );
        let start = second.find("<VOUCHER ").unwrap();
        let end = second.rfind("</VOUCHER>").unwrap() + "</VOUCHER>".len();
        first.replacen(
            "</COLLECTION>",
            &format!("{}</COLLECTION>", &second[start..end]),
            1,
        )
    };
    assert_eq!(census_xml.matches("<VOUCHER ").count(), 6);
    let audit_reads = [
        vec![marks_plan(6, 7), xml_plan(census_xml)],
        bodies.to_vec(),
        vec![marks_plan(6, 7)],
    ]
    .concat();
    let simulator =
        SequenceSimulator::spawn(audit_reads.iter().flat_map(single).collect()).unwrap();
    let runtime = TallyRuntime::default();
    let reader = AuditWindowReader::new(
        &runtime,
        TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        },
    );
    let identity = identity();
    let audit = read_voucher_window_with(
        &reader,
        &identity,
        identity.display_name(),
        "20260801",
        "20260802",
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate { known_marks: None },
        limits,
        |xml| parse_agent_rows(xml, GUID),
    )
    .await;
    let _ = simulator.finish();
    assert_eq!(audit.unwrap().reads, expected);
}

/// A reader reused after a failed window, as a retry after `WindowChanged`
/// would, starts the next window empty: the reads it yields are that window's
/// alone, never the rejected attempt's as well.
#[tokio::test]
async fn a_reused_audit_reader_yields_only_the_window_that_succeeded() {
    let attempt = |closing| {
        std::iter::once(marks_plan(3, 7))
            .chain(divided_window_reads(closing))
            .collect::<Vec<_>>()
    };
    let plans = [attempt(marks_plan(4, 7)), attempt(marks_plan(3, 7))]
        .concat()
        .iter()
        .flat_map(single)
        .collect();
    let simulator = SequenceSimulator::spawn(plans).unwrap();
    let runtime = TallyRuntime::default();
    let reader = AuditWindowReader::new(
        &runtime,
        TallyConfig {
            host: simulator.address().ip().to_string(),
            port: simulator.address().port(),
        },
    );
    let identity = identity();
    let window = || {
        read_voucher_window_with(
            &reader,
            &identity,
            identity.display_name(),
            "20260801",
            "20260802",
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Estimate { known_marks: None },
            three_a_read(),
            |xml| parse_agent_rows(xml, GUID),
        )
    };
    let first = window().await;
    assert_eq!(
        first.as_ref().err().unwrap().code,
        WINDOW_CHANGED_DURING_READ
    );
    let second = window().await;
    let _ = simulator.finish();
    assert!(second.is_ok());
    // Opening marks, census, three parts, closing marks: this window's six.
    assert_eq!(
        reader.into_retained(&second).map(|reads| reads.len()),
        Some(6)
    );
}

/// A window for a sealed record reads its own marks: a caller's count or
/// marks, or a replay, is refused before any request is sent.
#[tokio::test]
async fn an_audit_window_that_would_not_read_its_own_marks_is_refused_unread() {
    let runtime = TallyRuntime::default();
    // Nothing listens here: any request sent would fail differently.
    let reader = AuditWindowReader::new(
        &runtime,
        TallyConfig {
            host: "127.0.0.1".to_string(),
            port: 9,
        },
    );
    let identity = identity();
    for source in [
        WindowPlanSource::Estimate {
            known_marks: Some(marks_of(3)),
        },
        WindowPlanSource::Counted(census_of(&[("20260801", 1)])),
        WindowPlanSource::replay_of(divided_parts(), None),
    ] {
        let refused = read_voucher_window_with(
            &reader,
            &identity,
            identity.display_name(),
            "20260801",
            "20260802",
            VoucherReadShape::EntryWildcard,
            source,
            three_a_read(),
            |xml| parse_agent_rows(xml, GUID),
        )
        .await;
        assert_eq!(
            refused.err().map(|failure| failure.code),
            Some(AUDIT_WINDOW_NEEDS_ITS_OWN_MARKS.to_string())
        );
    }
}

#[test]
fn a_window_whose_part_ran_past_its_deadline_is_not_retried_as_it_is() {
    use crate::tally::runtime::AuditPartFailureKind;
    assert!(!AuditWindowFailure::Part(AuditPartFailureKind::Deadline).retryable());
    assert!(
        AuditPartFailureKind::Deadline.retryable(),
        "a part may be; the window is not"
    );
    assert!(AuditWindowFailure::Part(AuditPartFailureKind::ConnectionDropped).retryable());
    assert!(AuditWindowFailure::WindowChanged.retryable());
    assert!(!AuditWindowFailure::Refused("x".to_string()).retryable());
}

/// When a census was read, a sealed window admits its data against it, even
/// when the whole window fits one request: the census is sealed beside the
/// data, so a part holding a voucher the census did not count is refused.
#[tokio::test]
async fn an_audit_window_admits_its_data_against_the_census_it_read() {
    // A mark of 3 needs a census, which counts one voucher: the window fits.
    let counted = xml_plan(relabelled(&vouchers_kept(1), &[(1, "20260801")]));
    let (outcome, _, failure, _, _) = read_audit_window(
        [
            marks_plan(3, 7),
            counted.clone(),
            // The part returns a second voucher the census never counted.
            xml_plan(relabelled(
                &vouchers_kept(2),
                &[(1, "20260801"), (2, "20260801")],
            )),
            // Refused before the closing marks are read.
        ]
        .iter()
        .flat_map(single)
        .collect(),
        TallyRuntime::default(),
    )
    .await;
    assert_eq!(
        outcome.err().map(|f| f.code),
        Some(PART_NOT_ADMITTED.to_string())
    );
    assert_eq!(
        failure,
        Some(AuditWindowFailure::Refused(PART_NOT_ADMITTED.to_string()))
    );
    // Control: the part that returns exactly the counted voucher is admitted.
    let (outcome, retained, _, _, _) = read_audit_window(
        [marks_plan(3, 7), counted.clone(), counted, marks_plan(3, 7)]
            .iter()
            .flat_map(single)
            .collect(),
        TallyRuntime::default(),
    )
    .await;
    assert!(outcome.is_ok());
    assert_eq!(retained.map(|reads| reads.len()), Some(4));
}

/// #595: a refusal's timings give up their per-part list, and only it, when
/// the whole would not fit the share of the response budget allowed to them.
#[test]
fn window_timings_drop_only_their_parts_when_over_the_allowance() {
    let part = PartTiming {
        from: "20260801".into(),
        to: "20260801".into(),
        after: None,
        through: None,
        served: true,
        bytes: Some(10),
        rows: Some(1),
        ms: 5,
    };
    let timings = WindowReadTimings {
        from: "20260801".into(),
        to: "20260801".into(),
        marks: RequestTally { requests: 1, ms: 2 },
        census: RequestTally { requests: 3, ms: 4 },
        parts: vec![part; 3],
        failed: Some(FailedRequest {
            kind: "part",
            ms: 5,
        }),
    };
    let whole = serde_json::to_value(&timings).unwrap();
    let size = whole.to_string().len();
    assert_eq!(window_timings_within(&timings, size), whole);
    let trimmed = window_timings_within(&timings, size - 1);
    assert_eq!(
        trimmed,
        json!({
            "from": "20260801",
            "to": "20260801",
            "marks": {"requests": 1, "ms": 2},
            "census": {"requests": 3, "ms": 4},
            "failed": {"kind": "part", "ms": 5},
            "parts_omitted": 3,
        })
    );
}
