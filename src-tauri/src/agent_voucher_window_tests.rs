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

#[test]
fn the_census_reads_a_captured_voucher_row_and_ignores_cmpinfo() {
    let rows = parse_voucher_census(&three_vouchers(), ("20260801", "20260802"), None).unwrap();
    assert_eq!(
        rows,
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
}

#[test]
fn an_unobservable_high_water_mark_is_not_read_as_an_empty_book() {
    let guid = "61c6de69-1748-461c-ad3f-162cb949df9f";
    assert_eq!(
        voucher_high_water(&mark_xml(guid, "<ALTVCHID>42</ALTVCHID>"), guid),
        Some(42)
    );
    // Tally omits ALTVCHID for a company that has never held a voucher.
    assert_eq!(voucher_high_water(&mark_xml(guid, ""), guid), Some(0));
    assert_eq!(
        voucher_high_water(&mark_xml(guid, "<ALTVCHID>x</ALTVCHID>"), guid),
        None
    );
    assert_eq!(
        voucher_high_water(&mark_xml(guid, "<ALTVCHID>42</ALTVCHID>"), "another-guid"),
        None
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
    // first span measures one voucher, capacity becomes 3, and the rest of that
    // day is re-planned from above the AlterID already read, then day two whole.
    let limits = three_a_read();
    let census = WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260801"), 2),
        (day("20260801"), 3),
        (day("20260802"), 4),
        (day("20260802"), 5),
        (day("20260802"), 6),
    ]);
    let mut plans = paired(&xml_plan(vouchers_kept(1)));
    plans.extend(paired(&xml_plan(vouchers_kept(2))));
    plans.extend(paired(&xml_plan(
        three_vouchers().replace("20260801", "20260802"),
    )));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census),
        limits,
    )
    .await;
    let outcome = outcome.unwrap();
    assert_eq!(outcome.rows.len(), 6);
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
                through: 6,
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
    // capacity to 3, and the re-plan must begin on day two — not re-read day one.
    let limits = three_a_read();
    let census = WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260802"), 2),
        (day("20260802"), 3),
        (day("20260802"), 4),
    ]);
    let mut plans = paired(&xml_plan(vouchers_kept(1)));
    plans.extend(paired(&xml_plan(
        three_vouchers().replace("20260801", "20260802"),
    )));
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
    assert_eq!(outcome.rows.len(), 4);
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
async fn an_unmeasured_book_is_planned_at_the_conservative_default_never_unbounded() {
    // Nothing comes back to measure (the first span is empty), so every part is
    // planned at the default: one voucher a read, three reads for three
    // vouchers — never one read of all of them on the strength of nothing.
    let limits = three_a_read();
    let census = WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260801"), 2),
        (day("20260801"), 3),
    ]);
    let mut plans = paired(&xml_plan(empty_collection()));
    plans.extend(paired(&xml_plan(empty_collection())));
    plans.extend(paired(&xml_plan(empty_collection())));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260801"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Counted(census),
        limits,
    )
    .await;
    assert_eq!(outcome.unwrap().reads.len(), 3);
    assert_eq!(observed.len(), 18);
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
        WindowPlanSource::Estimate {
            known_high_water: None,
        },
        three_a_read(),
    )
    .await;
    let failure = outcome.err().expect("an unestimated window is refused");
    assert_eq!(failure.code, VOLUME_UNESTIMATED);
    assert!(
        failure.evidence.is_some(),
        "the high-water read is accounted for"
    );
    assert_eq!(observed.len(), 6);
}

#[tokio::test]
async fn a_short_window_on_a_big_book_is_counted_in_one_date_census() {
    // A mark of a quarter of a million puts the book far over the shortcut, but
    // the census is narrowed by date, not by the book's AlterID range: one
    // census of the window, which here proves the window small enough to read
    // whole — the request it was before the bound existed.
    // Production limits: a census of up to 4,096 rows, and the book's average
    // density since its books began (2026-04-01) is about 2,000 a day.
    let limits = WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard);
    let mut plans = paired(&xml_plan(three_vouchers()));
    plans.extend(paired(&xml_plan(three_vouchers())));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_high_water: Some(250_000),
        },
        limits,
    )
    .await;
    let outcome = outcome.unwrap();
    assert_eq!(outcome.reads, [part("20260801", "20260802", None)]);
    assert_eq!(observed.len(), 12);
    assert_requests(
        &observed,
        &[1, 7],
        &[
            render_agent_voucher_census(&company(), "20260801", "20260802", None).unwrap(),
            render_agent_vouchers(&company(), "20260801", "20260802", None).unwrap(),
        ],
    );
}

#[tokio::test]
async fn a_divided_read_is_bracketed_by_the_high_water_mark() {
    // A census of three vouchers on day one; the default fits one a read, so the
    // read divides. After the last part the mark is read again: unchanged, the
    // read stands; moved, it is refused, because the parts no longer describe
    // one state of the book.
    for (closing, expect_ok) in [(3, true), (4, false)] {
        let limits = three_a_read();
        let mut plans = paired(&xml_plan(three_vouchers()));
        plans.extend(paired(&xml_plan(vouchers_kept(1))));
        plans.extend(paired(&xml_plan(vouchers_kept(2))));
        plans.extend(paired(&xml_plan(empty_collection())));
        plans.extend(paired(&mark(closing)));
        let (outcome, observed) = read_window(
            plans,
            ("20260801", "20260802"),
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Estimate {
                known_high_water: Some(3),
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

#[tokio::test]
async fn a_census_day_too_dense_for_a_date_census_is_counted_in_alterid_spans() {
    // The date census of one day comes back over the transport cap. The day is
    // counted again in AlterID spans of the book, each bounded by construction.
    let limits = WindowReadLimits {
        budget_bytes: 3 * wire_len(&xml_plan(vouchers_kept(1))),
        default_bytes_per_voucher: wire_len(&xml_plan(vouchers_kept(1))),
    };
    let mut plans = oversized();
    plans.extend(paired(&xml_plan(three_vouchers())));
    plans.extend(paired(&xml_plan(three_vouchers())));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260801"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_high_water: Some(4),
        },
        limits,
    )
    .await;
    assert_eq!(outcome.unwrap().rows.len(), 3);
    assert_eq!(observed.len(), 14);
    assert_requests(
        &observed,
        &[1, 3],
        &[
            render_agent_voucher_census(&company(), "20260801", "20260801", None).unwrap(),
            render_agent_voucher_census(
                &company(),
                "20260801",
                "20260801",
                Some(AlterIdSpan {
                    after: 0,
                    through: 4,
                }),
            )
            .unwrap(),
        ],
    );
}

#[tokio::test]
async fn a_verification_part_tally_cannot_serve_is_halved_left_first() {
    // #485 inside the shared reader: the undivided window is refused by the
    // transport, so it is read again in date halves — the earlier half first,
    // so rows arrive in the order one undivided read would have produced.
    let shape = VoucherReadShape::ImportVerification;
    let mut plans = oversized();
    plans.extend(paired(&xml_plan(three_vouchers())));
    plans.extend(paired(&xml_plan(empty_collection())));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260802"),
        shape,
        WindowPlanSource::Estimate {
            known_high_water: Some(1),
        },
        WindowReadLimits::for_shape(shape),
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
    assert_requests(
        &observed,
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
    // A shape that does not halve fails on the same response instead.
    let (outcome, _) = read_window(
        oversized(),
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_high_water: Some(1),
        },
        WindowReadLimits::for_shape(VoucherReadShape::EntryWildcard),
    )
    .await;
    assert_eq!(
        outcome.err().map(|failure| failure.code).as_deref(),
        Some("response_size_limit_exceeded")
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
            known_high_water: Some(1),
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
    let budget = 3 * wire_len(&xml_plan(heavy.clone()));
    let limits = WindowReadLimits {
        budget_bytes: budget,
        default_bytes_per_voucher: budget,
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
    let third_day = vouchers_kept(3).replace("20260801", "20260803");
    let mut plans = paired(&xml_plan(heavy));
    plans.extend(paired(&xml_plan(
        three_vouchers().replace("20260801", "20260802"),
    )));
    plans.extend(paired(&xml_plan(third_day.clone())));
    plans.extend(paired(&xml_plan(third_day)));
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

#[tokio::test]
async fn a_census_the_transport_refuses_is_halved_by_date_earlier_half_first() {
    // Budget small enough that a census carries six rows; the book's average
    // density puts both days in one census, which comes back over the cap. It is
    // counted again as two single days, the earlier first, and the three
    // vouchers counted then fit one data read of the whole window.
    let one = wire_len(&xml_plan(vouchers_kept(1)));
    let limits = WindowReadLimits {
        budget_bytes: 3 * one,
        default_bytes_per_voucher: one,
    };
    let mut plans = oversized();
    plans.extend(paired(&xml_plan(three_vouchers())));
    plans.extend(paired(&xml_plan(empty_collection())));
    plans.extend(paired(&xml_plan(three_vouchers())));
    let (outcome, observed) = read_window(
        plans,
        ("20260801", "20260802"),
        VoucherReadShape::EntryWildcard,
        WindowPlanSource::Estimate {
            known_high_water: Some(4),
        },
        limits,
    )
    .await;
    assert_eq!(outcome.unwrap().reads, [part("20260801", "20260802", None)]);
    assert_eq!(observed.len(), 20);
    assert_requests(
        &observed,
        &[1, 3, 9, 15],
        &[
            render_agent_voucher_census(&company(), "20260801", "20260802", None).unwrap(),
            render_agent_voucher_census(&company(), "20260801", "20260801", None).unwrap(),
            render_agent_voucher_census(&company(), "20260802", "20260802", None).unwrap(),
            render_agent_vouchers(&company(), "20260801", "20260802", None).unwrap(),
        ],
    );
}
