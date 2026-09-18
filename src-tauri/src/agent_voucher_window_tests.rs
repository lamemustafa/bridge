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

fn counts(days: &[(&str, u64)]) -> BTreeMap<NaiveDate, u64> {
    days.iter().map(|(date, n)| (day(date), *n)).collect()
}

/// Every plan must tile its window exactly: a gap drops vouchers from a read
/// and an overlap reads them twice.
fn assert_tiles(plan: &[PlannedRead], from: &str, to: &str) {
    assert_eq!(plan.first().unwrap().from, day(from), "plan starts late");
    assert_eq!(plan.last().unwrap().to, day(to), "plan ends early");
    for pair in plan.windows(2) {
        assert_eq!(
            pair[1].from,
            pair[0].to.succ_opt().unwrap(),
            "gap or overlap between {:?} and {:?}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn a_window_predicted_within_budget_is_planned_as_itself() {
    // 30 vouchers at 1 KiB against a 64 KiB budget: one read, and that read is
    // the caller's own window — which is how a small book keeps its request.
    let days = counts(&[("20260402", 10), ("20260415", 20)]);
    let plan =
        plan_window_reads(day("20260401"), day("20260430"), &days, 1024, 64 * 1024, 8).unwrap();
    assert_eq!(
        plan,
        [PlannedRead {
            from: day("20260401"),
            to: day("20260430"),
            vouchers: 30
        }]
    );
    // An empty window is one read of zero vouchers, not no read: emptiness
    // still has to be observed.
    let plan = plan_window_reads(
        day("20260401"),
        day("20260430"),
        &BTreeMap::new(),
        1024,
        64 * 1024,
        8,
    )
    .unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].vouchers, 0);
}

#[test]
fn a_window_over_budget_is_divided_greedily_and_exactly() {
    // Capacity is 10 vouchers a read. Days of 6, 4, 7, 2, 9 pack as
    // [6+4], [7+2], [9]; the empty days between them ride along.
    let days = counts(&[
        ("20260403", 6),
        ("20260404", 4),
        ("20260410", 7),
        ("20260411", 2),
        ("20260420", 9),
    ]);
    let plan = plan_window_reads(day("20260401"), day("20260430"), &days, 100, 1000, 8).unwrap();
    assert_tiles(&plan, "20260401", "20260430");
    assert_eq!(
        plan.iter().map(|read| read.vouchers).collect::<Vec<_>>(),
        [10, 9, 9]
    );
    assert_eq!(plan[0].to, day("20260409"));
    assert_eq!(plan[1].to, day("20260419"));
    for read in &plan {
        assert!(
            read.vouchers * 100 <= 1000,
            "{read:?} predicted over budget"
        );
    }
    // Exactly at capacity still fits; one more voucher does not.
    let exact = counts(&[("20260401", 5), ("20260402", 5)]);
    assert_eq!(
        plan_window_reads(day("20260401"), day("20260402"), &exact, 100, 1000, 8)
            .unwrap()
            .len(),
        1
    );
    let over = counts(&[("20260401", 5), ("20260402", 6)]);
    let plan = plan_window_reads(day("20260401"), day("20260402"), &over, 100, 1000, 8).unwrap();
    assert_tiles(&plan, "20260401", "20260402");
    assert_eq!(plan.len(), 2);
}

#[test]
fn a_day_alone_over_budget_is_refused_by_name_not_sent() {
    // Capacity 10; the 11-voucher day cannot be divided by date, so there is no
    // plan at all — not a plan that reads the days around it.
    let days = counts(&[("20260401", 3), ("20260415", 11), ("20260420", 2)]);
    let refusal =
        plan_window_reads(day("20260401"), day("20260430"), &days, 100, 1000, 8).unwrap_err();
    assert_eq!(
        refusal,
        PlanRefusal::DayOverBudget {
            day: day("20260415"),
            vouchers: 11
        }
    );
    assert_eq!(refusal.code(), "voucher_window_day_over_budget");
    // A per-voucher cost larger than the whole budget refuses any voucher.
    let one = counts(&[("20260401", 1)]);
    assert!(matches!(
        plan_window_reads(day("20260401"), day("20260401"), &one, 2000, 1000, 8),
        Err(PlanRefusal::DayOverBudget { .. })
    ));
}

#[test]
fn a_window_needing_more_reads_than_allowed_is_refused() {
    let days = counts(&[("20260401", 10), ("20260402", 10), ("20260403", 10)]);
    let refusal =
        plan_window_reads(day("20260401"), day("20260403"), &days, 100, 1000, 2).unwrap_err();
    assert_eq!(refusal, PlanRefusal::TooManyReads { reads: 3 });
    assert_eq!(refusal.code(), "voucher_window_too_many_reads");
}

#[test]
fn the_measured_heavy_book_is_divided_below_the_cap_at_production_limits() {
    // The heaviest book measured (§11c): 16,367 vouchers over a year at 35 KB of
    // UTF-8 each with named entry fields. Spread evenly, about 45 a day. At the
    // production budget and default, every read is predicted within budget and
    // the year stays within the read allowance.
    let mut days = BTreeMap::new();
    let mut date = day("20250401");
    let mut left = 16_367_u64;
    while left > 0 {
        let today = left.min(45);
        days.insert(date, today);
        left -= today;
        date = date.succ_opt().unwrap();
    }
    let shape = VoucherReadShape::ImportVerification;
    let plan = plan_window_reads(
        day("20250401"),
        day("20260331"),
        &days,
        shape.default_wire_bytes_per_voucher(),
        WINDOW_READ_BUDGET_BYTES,
        MAX_PLANNED_READS,
    )
    .unwrap();
    assert_tiles(&plan, "20250401", "20260331");
    assert!(plan.len() > 1);
    for read in &plan {
        let predicted = read.vouchers * shape.default_wire_bytes_per_voucher();
        assert!(predicted <= WINDOW_READ_BUDGET_BYTES, "{read:?}");
        // What the book actually costs on Bridge's UTF-16 wire stays under the
        // transport cap with room: 35 KB of UTF-8 is 70 KB on the wire.
        assert!(
            read.vouchers * 70_000 < bridge_tally_transport::XML_RESPONSE_MAX_BYTES as u64,
            "{read:?}"
        );
    }
}

#[test]
fn the_conservative_defaults_stay_above_every_measured_per_voucher_cost() {
    // §11c: named entry fields averaged 35 KB of UTF-8 per voucher over a year of
    // the heaviest book, and the entry wildcard ran about 128 KB on a one-day
    // probe of the same book. Doubled for UTF-16, the defaults must exceed both,
    // or "conservative" is not true.
    for shape in [
        VoucherReadShape::ImportVerification,
        VoucherReadShape::Movement,
    ] {
        assert!(shape.default_wire_bytes_per_voucher() > 2 * 35_000);
    }
    assert!(VoucherReadShape::EntryWildcard.default_wire_bytes_per_voucher() > 2 * 128_000);
    // And the budget sits well below the cap it exists to keep clear of.
    assert!(WINDOW_READ_BUDGET_BYTES * 2 <= bridge_tally_transport::XML_RESPONSE_MAX_BYTES as u64);
}

#[test]
fn a_measured_figure_is_planned_with_its_margin() {
    assert_eq!(with_headroom(1000), 1500);
    // Rounded up, never down: a margin that rounds away is not a margin.
    assert_eq!(with_headroom(1001), 1502);
    assert_eq!(with_headroom(1), 2);
}

#[test]
fn census_spans_cover_the_book_exactly_and_are_each_bounded() {
    // 40 KiB / 4 KiB = 10 AlterIDs per census read.
    let spans = census_spans(25, 40 * 1024).unwrap();
    assert_eq!(
        spans,
        [
            AlterIdSpan {
                after: 0,
                through: 10
            },
            AlterIdSpan {
                after: 10,
                through: 20
            },
            AlterIdSpan {
                after: 20,
                through: 25
            },
        ]
    );
    // No span can return more rows than fit the budget at the census cost.
    for span in &spans {
        assert!((span.through - span.after) * CENSUS_WIRE_BYTES_PER_VOUCHER <= 40 * 1024);
    }
    assert_eq!(census_spans(0, 40 * 1024), Some(vec![]));
    // A book needing more census reads than one call may spend is unestimated,
    // never censused partially.
    assert_eq!(census_spans(10 * (MAX_CENSUS_READS + 1), 40 * 1024), None);
    assert!(census_spans(10 * MAX_CENSUS_READS, 40 * 1024).is_some());
}

#[test]
fn a_calibration_sample_is_spread_bounded_and_never_overlaps() {
    let ids = (1..=30).collect::<Vec<u64>>();
    let selected = |spans: &[AlterIdSpan]| {
        ids.iter()
            .filter(|id| {
                spans
                    .iter()
                    .any(|span| **id > span.after && **id <= span.through)
            })
            .count()
    };
    let spans = calibration_spans(&ids, 6);
    // Three places, two vouchers each, from the top of each third.
    assert_eq!(
        spans,
        [
            AlterIdSpan {
                after: 8,
                through: 10
            },
            AlterIdSpan {
                after: 18,
                through: 20
            },
            AlterIdSpan {
                after: 28,
                through: 30
            },
        ]
    );
    assert_eq!(selected(&spans), 6);
    for sample in 1..=31 {
        let spans = calibration_spans(&ids, sample);
        assert!(
            selected(&spans) as u64 <= sample,
            "sample {sample} selected {}",
            selected(&spans)
        );
        assert!(selected(&spans) >= 1);
        for pair in spans.windows(2) {
            assert!(pair[0].through <= pair[1].after, "overlap at {sample}");
        }
    }
    // A sample as large as the window takes all of it, in one span.
    assert_eq!(
        calibration_spans(&ids, 30),
        [AlterIdSpan {
            after: 0,
            through: 30
        }]
    );
    assert!(calibration_spans(&[], 6).is_empty());
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

/// The captured three-voucher response reduced to its last voucher, by removing
/// the first two `VOUCHER` elements from the captured bytes in memory.
fn one_voucher() -> String {
    let mut xml = three_vouchers();
    for _ in 0..2 {
        let start = xml.find("<VOUCHER ").unwrap();
        let end = start + xml[start..].find("</VOUCHER>").unwrap() + "</VOUCHER>".len();
        xml.replace_range(start..end, "");
    }
    assert_eq!(xml.matches("<VOUCHER ").count(), 1);
    xml
}

#[test]
fn the_census_reads_a_captured_voucher_row_and_ignores_cmpinfo() {
    // A live capture carries far more than the census asks for; only the
    // voucher's own DATE and ALTERID are taken, from inside DATA/COLLECTION.
    let rows = parse_voucher_census(
        &three_vouchers(),
        ("20260801", "20260802"),
        AlterIdSpan {
            after: 0,
            through: 3,
        },
    )
    .unwrap();
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
        parse_voucher_census(
            &empty,
            ("20260801", "20260802"),
            AlterIdSpan {
                after: 0,
                through: 3
            }
        ),
        Ok(vec![])
    );
}

#[test]
fn a_census_that_does_not_describe_what_was_asked_is_refused() {
    let xml = three_vouchers();
    // A row outside the window.
    assert_eq!(
        parse_voucher_census(
            &xml,
            ("20260802", "20260803"),
            AlterIdSpan {
                after: 0,
                through: 3
            }
        ),
        Err("window_not_honoured".to_string())
    );
    // A row outside the AlterID span.
    assert_eq!(
        parse_voucher_census(
            &xml,
            ("20260801", "20260801"),
            AlterIdSpan {
                after: 1,
                through: 3
            }
        ),
        Err("window_not_honoured".to_string())
    );
    // A row without its own DATE cannot be counted on any day.
    let undated = xml.replacen("<DATE TYPE=\"Date\">20260801</DATE>", "", 1);
    assert_ne!(undated, xml);
    assert_eq!(
        parse_voucher_census(
            &undated,
            ("20260801", "20260801"),
            AlterIdSpan {
                after: 0,
                through: 3
            }
        ),
        Err("agent_read_protocol_invalid".to_string())
    );
}

#[test]
fn an_unobservable_high_water_mark_is_not_read_as_an_empty_book() {
    let guid = "61c6de69-1748-461c-ad3f-162cb949df9f";
    let mark = |fields: &str| {
        format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>{guid}</GUID>{fields}</COMPANY></COLLECTION></DATA></BODY></ENVELOPE>")
    };
    assert_eq!(
        voucher_high_water(&mark("<ALTVCHID>42</ALTVCHID><ALTMSTID>7</ALTMSTID>"), guid),
        Some(42)
    );
    // Tally omits ALTVCHID for a company that has never held a voucher.
    assert_eq!(
        voucher_high_water(&mark("<ALTMSTID>7</ALTMSTID>"), guid),
        Some(0)
    );
    // Anything else is unobserved, which the caller refuses as unestimated.
    assert_eq!(
        voucher_high_water(&mark("<ALTVCHID>x</ALTVCHID><ALTMSTID>7</ALTMSTID>"), guid),
        None
    );
    assert_eq!(voucher_high_water(&mark(""), guid), None);
    assert_eq!(
        voucher_high_water(
            &mark("<ALTVCHID>42</ALTVCHID><ALTMSTID>7</ALTMSTID>"),
            "another-guid"
        ),
        None
    );
}

#[test]
fn an_unsampled_read_is_byte_identical_to_the_request_before_the_bound() {
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
    // The sample renderers with no span are the same bytes too, so neither
    // path can drift from the other.
    assert_eq!(
        render_agent_vouchers_sample(company, from, to, None).unwrap(),
        render_agent_vouchers(company, from, to, None).unwrap()
    );
    // A sample differs by exactly its span clause, inside the date formula.
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
        let sample = shape.render(company, from, to, Some(span)).unwrap();
        let clause = " AND $AlterID &gt; 7 AND $AlterID &lt;= 9";
        assert_eq!(sample.replacen(clause, "", 1), whole, "{shape:?}");
        let formula_end = sample.find("</SYSTEM>").unwrap();
        assert!(sample[..formula_end].ends_with(clause), "{shape:?}");
    }
}

#[test]
fn the_census_request_is_an_admitted_minimal_collection_export() {
    let request = render_agent_voucher_census(
        "Synthetic & Book",
        "20260401",
        "20260430",
        AlterIdSpan {
            after: 0,
            through: 4096,
        },
    )
    .unwrap();
    assert!(crate::tally::agent_read_request::AgentReadRequest::parse(request.clone()).is_ok());
    assert!(request.contains("<FETCH>GUID,ALTERID,DATE</FETCH>"));
    assert!(request.contains("Synthetic &amp; Book"));
    assert!(request.contains(
        "$Date &gt;= $$Date:\"20260401\" AND $Date &lt;= $$Date:\"20260430\" AND $AlterID &gt; 0 AND $AlterID &lt;= 4096</SYSTEM>"
    ));
}

// --- Through the simulator --------------------------------------------------

const GUID: &str = "61c6de69-1748-461c-ad3f-162cb949df9f";

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

/// The six legs of one paired, identity-bracketed agent read.
fn paired(body: &ScenarioPlan) -> Vec<ScenarioPlan> {
    let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
        .with_framing(ResponseFraming::ContentLength);
    vec![
        company_plan(),
        body.clone(),
        status.clone(),
        body.clone(),
        status,
        company_plan(),
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

/// Limits under which the captured one-voucher response, measured and given
/// its margin, fits exactly three vouchers a read — and under which, before
/// anything is measured, the default fits only one.
fn three_a_read() -> WindowReadLimits {
    let measured = wire_len(&xml_plan(one_voucher()));
    let budget = 3 * with_headroom(measured);
    WindowReadLimits {
        budget_bytes: budget,
        default_bytes_per_voucher: budget,
    }
}

/// A caller-held count: three vouchers on each of two days.
fn two_days_of_three() -> WindowCensus {
    WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260801"), 2),
        (day("20260801"), 3),
        (day("20260802"), 4),
        (day("20260802"), 5),
        (day("20260802"), 6),
    ])
}

async fn read_window(
    plans: Vec<ScenarioPlan>,
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
            "20260801",
            "20260802",
            VoucherReadShape::EntryWildcard,
            source,
            limits,
            |xml| parse_agent_rows(xml, GUID),
        )
        .await;
    (outcome, simulator.finish().unwrap())
}

#[tokio::test]
async fn a_window_over_budget_is_sampled_then_read_in_planned_parts() {
    let limits = three_a_read();
    let company = identity().display_name().to_string();
    let second_day = three_vouchers().replace("20260801", "20260802");
    let mut plans = paired(&xml_plan(one_voucher()));
    plans.extend(paired(&xml_plan(three_vouchers())));
    plans.extend(paired(&xml_plan(second_day)));
    let (outcome, observed) = read_window(
        plans,
        WindowPlanSource::Counted(two_days_of_three()),
        limits,
    )
    .await;
    let outcome = outcome.unwrap();
    assert_eq!(outcome.rows.len(), 6);
    assert_eq!(
        outcome.reads,
        [
            ("20260801".to_string(), "20260801".to_string()),
            ("20260802".to_string(), "20260802".to_string())
        ]
    );
    assert_eq!(observed.len(), 18);
    // The sample selects only the window's highest AlterID; the two parts are
    // exactly the unsampled renderer's own requests for each day.
    let shape = VoucherReadShape::EntryWildcard;
    let expected = [
        shape
            .render(
                &company,
                "20260801",
                "20260802",
                Some(AlterIdSpan {
                    after: 5,
                    through: 6,
                }),
            )
            .unwrap(),
        shape
            .render(&company, "20260801", "20260801", None)
            .unwrap(),
        shape
            .render(&company, "20260802", "20260802", None)
            .unwrap(),
    ];
    for (index, request) in expected.iter().enumerate() {
        let leg = 6 * index + 1;
        assert_eq!(
            observed[leg].request_body_sha256,
            request_sha(request),
            "read {index}"
        );
        assert_eq!(
            observed[leg + 2].request_body_sha256,
            request_sha(request),
            "pair {index}"
        );
    }
    // The sample is pre-flight, not data.
    assert!(outcome.preflight_evidence.is_some());
}

#[tokio::test]
async fn a_day_over_budget_is_refused_after_sampling_and_before_any_data_read() {
    let limits = three_a_read();
    let census = WindowCensus::from_rows([
        (day("20260801"), 1),
        (day("20260801"), 2),
        (day("20260801"), 3),
        (day("20260801"), 4),
        (day("20260802"), 5),
    ]);
    let (outcome, observed) = read_window(
        paired(&xml_plan(one_voucher())),
        WindowPlanSource::Counted(census),
        limits,
    )
    .await;
    let failure = outcome.err().expect("an over-budget day is refused");
    assert_eq!(failure.code, "voucher_window_day_over_budget");
    // Only the bounded sample was sent. The refused day never was.
    assert_eq!(observed.len(), 6);
    // The refusal still accounts for the read it did make.
    assert_eq!(
        failure.evidence.expect("sample evidence").bytes,
        2 * wire_len(&xml_plan(one_voucher())) as usize
    );
}

#[tokio::test]
async fn an_unmeasured_book_is_planned_at_the_conservative_default_never_unbounded() {
    // The sample comes back empty, so nothing is measured. The plan falls back
    // to the default, which fits one voucher a read, and a day of three is
    // refused rather than sent on the strength of a figure nobody measured.
    let limits = three_a_read();
    let (outcome, observed) = read_window(
        paired(&xml_plan(empty_collection())),
        WindowPlanSource::Counted(two_days_of_three()),
        limits,
    )
    .await;
    assert_eq!(
        outcome.err().map(|failure| failure.code).as_deref(),
        Some("voucher_window_day_over_budget")
    );
    assert_eq!(observed.len(), 6);
}

#[tokio::test]
async fn an_unobservable_high_water_mark_refuses_the_window_unread() {
    let limits = three_a_read();
    let foreign = "<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>00000000-0000-4000-8000-000000000009</GUID><ALTVCHID>3</ALTVCHID><ALTMSTID>7</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>";
    let (outcome, observed) = read_window(
        paired(&xml_plan(foreign.to_string())),
        WindowPlanSource::Estimate {
            known_high_water: None,
        },
        limits,
    )
    .await;
    let failure = outcome.err().expect("an unestimated window is refused");
    assert_eq!(failure.code, VOLUME_UNESTIMATED);
    assert!(
        failure.evidence.is_some(),
        "the high-water read is accounted for"
    );
    // The high-water read only; no census, no window.
    assert_eq!(observed.len(), 6);
}

#[tokio::test]
async fn a_book_too_large_to_census_is_refused_without_a_request() {
    // Nothing may reach this port: the refusal is decided from the known mark.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let server = server_at(listener.local_addr().unwrap(), directory.path());
    let identity = identity();
    let limits = three_a_read();
    let width = limits.budget_bytes / CENSUS_WIRE_BYTES_PER_VOUCHER;
    let failure = server
        .read_voucher_window(
            &identity,
            identity.display_name(),
            "20260801",
            "20260802",
            VoucherReadShape::EntryWildcard,
            WindowPlanSource::Estimate {
                known_high_water: Some(width * (MAX_CENSUS_READS + 1)),
            },
            limits,
            |xml| parse_agent_rows(xml, GUID),
        )
        .await
        .err()
        .expect("refused");
    assert_eq!(failure.code, VOLUME_UNESTIMATED);
    assert!(matches!(
        listener.accept(),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
    ));
}

#[tokio::test]
async fn a_census_that_proves_the_window_fits_keeps_the_original_request() {
    // A known mark too large for the shortcut sends a census; the captured rows
    // it returns, once measured, fit one read — so the window goes out whole,
    // as exactly the request it was before the bound existed.
    let limits = three_a_read();
    let company = identity().display_name().to_string();
    let mut plans = paired(&xml_plan(three_vouchers()));
    plans.extend(paired(&xml_plan(one_voucher())));
    plans.extend(paired(&xml_plan(three_vouchers())));
    let (outcome, observed) = read_window(
        plans,
        WindowPlanSource::Estimate {
            known_high_water: Some(3),
        },
        limits,
    )
    .await;
    let outcome = outcome.unwrap();
    assert_eq!(outcome.rows.len(), 3);
    assert_eq!(
        outcome.reads,
        [("20260801".to_string(), "20260802".to_string())]
    );
    assert_eq!(observed.len(), 18);
    assert_eq!(
        observed[1].request_body_sha256,
        request_sha(
            &render_agent_voucher_census(
                &company,
                "20260801",
                "20260802",
                AlterIdSpan {
                    after: 0,
                    through: 3
                }
            )
            .unwrap()
        )
    );
    assert_eq!(
        observed[13].request_body_sha256,
        request_sha(&render_agent_vouchers(&company, "20260801", "20260802", None).unwrap())
    );
}

#[tokio::test]
async fn a_small_book_sends_the_same_voucher_request_as_before_the_bound() {
    // Through the vouchers tool: the only new read is the high-water mark, and
    // the window read that follows is the unsampled request for the caller's
    // whole window — the request the tool sent before this bound existed.
    let mark = format!("<ENVELOPE><HEADER><STATUS>1</STATUS></HEADER><BODY><DATA><COLLECTION><COMPANY><GUID>{GUID}</GUID><ALTVCHID>3</ALTVCHID><ALTMSTID>7</ALTMSTID></COMPANY></COLLECTION></DATA></BODY></ENVELOPE>");
    let status = ScenarioPlan::new(Fixture::ProductStatus(ProductStatus::TallyPrime))
        .with_framing(ResponseFraming::ContentLength);
    let mut plans = vec![company_plan(), status.clone(), company_plan(), status];
    plans.extend(paired(&xml_plan(mark.clone())));
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
    let company = identity().display_name().to_string();
    assert_eq!(
        observed[5].request_body_sha256,
        request_sha(&render_agent_company_high_water(&company))
    );
    for leg in [11, 13] {
        assert_eq!(
            observed[leg].request_body_sha256,
            request_sha(&render_agent_vouchers(&company, "20260801", "20260831", None).unwrap())
        );
    }
}

#[tokio::test]
async fn a_part_heavier_than_the_sample_raises_the_estimate_for_the_rest() {
    // The sample measures one ordinary voucher, so the plan fits three a read.
    // The first day then comes back far heavier per voucher than the sample —
    // its narrations padded in memory — and the rest of the window is planned
    // again at that figure. At it the second day no longer fits, so it is
    // refused rather than sent on the sample's word.
    let limits = three_a_read();
    let captured = three_vouchers();
    let padding = "N".repeat(20_000);
    let heavy = captured.replace(
        "<NARRATION TYPE=\"String\">",
        &format!("<NARRATION TYPE=\"String\">{padding}"),
    );
    assert_ne!(heavy, captured);
    let mut plans = paired(&xml_plan(one_voucher()));
    plans.extend(paired(&xml_plan(heavy)));
    let (outcome, observed) = read_window(
        plans,
        WindowPlanSource::Counted(two_days_of_three()),
        limits,
    )
    .await;
    let failure = outcome.err().expect("the rest no longer fits");
    assert_eq!(failure.code, "voucher_window_day_over_budget");
    // The sample and the first day were read; the second day never was.
    assert_eq!(observed.len(), 12);
}
