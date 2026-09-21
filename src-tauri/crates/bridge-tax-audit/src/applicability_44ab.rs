//! s.44AB(a) tax-audit applicability: a port of the reference Python implementation's
//! `applicability_44ab` test module, version 1.
//!
//! Like the reference, this test computes nothing another test computes. It combines inputs decided
//! elsewhere into one applicability call:
//!   * books turnover -- `financial_statements`' own `sales` figure (see that module for the
//!     definition, and its note that the Guidance Note's s.44AB turnover can differ at the edges);
//!   * the cash share -- `cash_44ab`'s own `cash_share_receipts`/`cash_share_payments` values and its
//!     finding's limits, reused verbatim;
//!   * GSTR-1 / GSTR-3B / AIS turnover, for comparison only -- caller data (this crate reads no GST
//!     document), each with the coverage the client configuration states;
//!   * presumptive-taxation history -- client configuration, never inferred from the books.
//!
//! The decision is asymmetric, as the reference's: turnover above the highest threshold s.44AB(a)
//! can apply is "yes", at or below the lowest is "no", whatever the cash share; in between, a books
//! cash share already over the limit on either leg is "yes" (a books figure can only understate
//! cash), and within the limit on both legs is "undetermined", never "no". Due dates are republished
//! for both outcomes. The s.44AB(e) history and s.44ADA are questions for the CA, never conclusions.

use crate::error::{AuditError, Result};
use crate::findings::{Confidence, EvidenceRef, Finding, TestResult, Unit, Value};
use crate::rules::Rules;

pub const TEST_ID: &str = "applicability_44ab";
pub const VERSION: &str = "1";

const COMPARISON_SOURCES: [&str; 3] = ["gstr1", "gstr3b", "ais"];

const TURNOVER_DEFINITION: &str = "Sales net of returns and trade discounts, excluding GST \
collected (ICAI Guidance Note on Tax Audit u/s 44AB; confirm). Taken from the books' Sales \
Accounts: GST excluded, with sales returns and credit notes already netted because they are posted \
to the same sales ledgers.";

/// One comparison source's turnover, caller data: the value and what part of the return it covers
/// ("full" when the whole return was read). The reference defaults a missing coverage to "full";
/// a caller with no stated coverage must pass "full" itself -- anything else is never differenced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonTurnover {
    pub turnover_paise: i64,
    pub coverage: String,
}

/// The reference's `turnover_inputs`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnoverInputs {
    pub books_turnover_paise: Option<i64>,
    pub gstr1: Option<ComparisonTurnover>,
    pub gstr3b: Option<ComparisonTurnover>,
    pub ais: Option<ComparisonTurnover>,
}

/// The reference's `cash_share`: `cash_44ab`'s own figures (`None` where undefined) and limits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CashShare {
    pub receipts_bp: Option<i64>,
    pub payments_bp: Option<i64>,
    pub limits: Vec<String>,
}

/// Python's `str()` of a TOML scalar, for the values the reference interpolates with f"{x}".
fn py_str(v: &toml::Value) -> Result<String> {
    match v {
        toml::Value::String(s) => Ok(s.clone()),
        toml::Value::Integer(n) => Ok(n.to_string()),
        toml::Value::Boolean(b) => Ok(if *b { "True" } else { "False" }.to_string()),
        other => Err(AuditError::Config(format!(
            "[presumptive_history]: a {} value is not supported here (string, integer or \
boolean only)",
            other.type_str()
        ))),
    }
}

/// Python's `repr()` of a TOML scalar or a missing key, for the reference's f"{x!r}".
fn py_repr(v: Option<&toml::Value>) -> Result<String> {
    match v {
        None => Ok("None".to_string()),
        Some(toml::Value::String(s)) => {
            let quote = if s.contains('\'') && !s.contains('"') {
                '"'
            } else {
                '\''
            };
            let mut out = String::from(quote);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                        out.push_str(&format!("\\x{:02x}", c as u32));
                    }
                    c => out.push(c),
                }
            }
            out.push(quote);
            Ok(out)
        }
        Some(other) => py_str(other),
    }
}

/// `limit_bp / 100` as Python's `:g` prints it for a whole or two-decimal percentage.
fn percent_g(bp: i64) -> String {
    let (q, r) = (bp / 100, (bp % 100).abs());
    if r == 0 {
        q.to_string()
    } else {
        format!("{q}.{r:02}").trim_end_matches('0').to_string()
    }
}

pub fn run(
    rules: &Rules,
    entity_type: &str,
    turnover_inputs: &TurnoverInputs,
    cash_share: &CashShare,
    presumptive_history: Option<&toml::Table>,
) -> Result<TestResult> {
    let mut r = TestResult::new(TEST_ID, VERSION, &rules.version);
    let limit_bp = rules.cash_share_limit_bp;
    let lo_thr = rules.turnover_threshold_paise;
    let hi_thr = rules.turnover_threshold_low_cash_paise;

    let (rec_bp, pay_bp) = (cash_share.receipts_bp, cash_share.payments_bp);
    let cash_known = rec_bp.is_some() && pay_bp.is_some();
    let within_5pct =
        matches!((rec_bp, pay_bp), (Some(a), Some(b)) if a <= limit_bp && b <= limit_bp);

    let thr = if within_5pct { hi_thr } else { lo_thr };
    let mut facts: Vec<(String, String)> = Vec::new();
    let f_thr = r.fig(
        "applicable_threshold",
        Value::Int(thr),
        Unit::Paise,
        &format!(
            "₹10 crore if cash receipts and cash payments are each within {}% of the total, else \
₹1 crore (s.44AB(a) and its proviso).",
            percent_g(limit_bp)
        ),
        vec![EvidenceRef::new("rule", "s44ab")],
    );
    facts.push(("applicable_threshold".to_string(), f_thr));
    let f_def = r.fig(
        "turnover_definition",
        Value::Text(TURNOVER_DEFINITION.to_string()),
        Unit::Text,
        "This test's own turnover definition, stated once and cited by every turnover figure \
below.",
        Vec::new(),
    );
    facts.push(("turnover_definition".to_string(), f_def));

    let turnover = turnover_inputs.books_turnover_paise;
    let f_turnover = match turnover {
        Some(t) => r.fig(
            "turnover",
            Value::Int(t),
            Unit::Paise,
            "Books turnover, per the definition above, supplied by the caller from \
financial_statements.py's own 'sales' figure -- never recomputed here.",
            Vec::new(),
        ),
        None => r.fig(
            "turnover",
            Value::Text("not supplied".to_string()),
            Unit::Text,
            "Books turnover was not supplied to this test (turnover_inputs has no \
'books_turnover_paise').",
            Vec::new(),
        ),
    };
    facts.push(("turnover".to_string(), f_turnover));

    let mut comparison_limits: Vec<String> = Vec::new();
    for source in COMPARISON_SOURCES {
        let input = match source {
            "gstr1" => &turnover_inputs.gstr1,
            "gstr3b" => &turnover_inputs.gstr3b,
            _ => &turnover_inputs.ais,
        };
        let upper = crate::support::py_upper(source);
        let Some(input) = input else {
            comparison_limits.push(format!(
                "{upper} turnover was not supplied for this engagement; not compared to books \
turnover."
            ));
            continue;
        };
        let f_src = r.fig(
            &format!("{source}_turnover"),
            Value::Int(input.turnover_paise),
            Unit::Paise,
            &format!(
                "Turnover per {upper}, as supplied by the caller -- never recomputed by this test; \
may not use the same population or section coverage as the books definition above (see the \
source test's own report)."
            ),
            Vec::new(),
        );
        facts.push((format!("{source}_turnover"), f_src));
        if input.coverage != "full" {
            comparison_limits.push(format!(
                "{upper} figure covers only '{}', so it is not compared to books turnover.",
                input.coverage
            ));
            continue;
        }
        if let Some(t) = turnover {
            let diff = t
                .checked_sub(input.turnover_paise)
                .ok_or_else(|| AuditError::Config("applicability_44ab: overflow".to_string()))?;
            let f_diff = r.fig(
                &format!("{source}_turnover_diff_from_books"),
                Value::Int(diff),
                Unit::Paise,
                &format!("turnover - {source}_turnover."),
                Vec::new(),
            );
            facts.push((format!("{source}_turnover_diff_from_books"), f_diff));
        }
    }

    let (audit_required, reason) = match turnover {
        None => ("undetermined", "turnover not supplied"),
        Some(t) if t > hi_thr => (
            "yes",
            "turnover exceeds ₹10 crore, the highest threshold s.44AB(a) can ever apply, so the \
cash-share breakdown does not matter",
        ),
        Some(t) if t <= lo_thr => (
            "no",
            "turnover does not exceed ₹1 crore, the lowest threshold s.44AB(a) can ever apply, so \
the cash-share breakdown does not matter",
        ),
        Some(_) if !cash_known => (
            "undetermined",
            "turnover is between the two possible thresholds and cash_share receipts/payments were \
not supplied",
        ),
        Some(_) if !within_5pct => (
            "yes",
            "turnover is between the two possible thresholds; books cash share is already over 5% \
on at least one leg, and an understated books figure can only be over limit by MORE, never bring \
it back within limit, so the ₹1 crore threshold and this 'yes' are robust to that understatement \
risk",
        ),
        Some(_) => (
            "undetermined",
            "turnover is between the two possible thresholds; books cash share is within 5% on \
both legs, but a books-only breakdown can UNDERSTATE the true cash share (a non-account-payee \
cheque/draft counts as cash but is not visible in Tally as such -- cash_44ab.py's own limit); if \
the true share is not within 5%, the threshold reverts to ₹1 crore and turnover already exceeds \
it, so this cannot be asserted as a confident 'no'",
        ),
    };
    let f_req = r.fig(
        "audit_required_44ab_a",
        Value::Text(audit_required.to_string()),
        Unit::Text,
        &format!(
            "s.44AB(a): 'yes' when turnover certainly exceeds the applicable threshold however \
cash_share resolves, 'no' when it certainly does not, else 'undetermined'. This call: {reason}."
        ),
        Vec::new(),
    );
    facts.push(("audit_required_44ab_a".to_string(), f_req));

    let profession = crate::support::py_lower(entity_type).contains("profession");
    let f_44ada = r.fig(
        "s44ada_professions_in_scope",
        Value::Text(if profession { "yes" } else { "no" }.to_string()),
        Unit::Text,
        "s.44ADA (presumptive taxation for professionals) and its correlated audit trigger are \
out of scope of this test for every entity; this figure only flags whether eng.entity_type text \
suggests a profession, in which case s.44ADA needs separate CA analysis this test does not provide.",
        Vec::new(),
    );
    facts.push(("s44ada_professions_in_scope".to_string(), f_44ada));

    for (name, value, definition) in [
        (
            "due_date_audit_report",
            &rules.due_date_audit_report,
            "rules[due_dates].audit_report: s.44AB audit report due date, subject to CBDT \
extension, if audit is required.",
        ),
        (
            "due_date_return_audit_case",
            &rules.due_date_return_audit_case,
            "rules[due_dates].return_audit_case: return due date if audit is required.",
        ),
        (
            "due_date_return_non_audit_case",
            &rules.due_date_return_non_audit_firm,
            "rules[due_dates].return_non_audit_firm: return due date if audit is NOT required \
(Explanation 2 to s.139(1)).",
        ),
    ] {
        let id = r.fig(
            name,
            Value::Text(value.clone()),
            Unit::Text,
            definition,
            Vec::new(),
        );
        facts.push((name.to_string(), id));
    }

    let mut limits = vec![
        if rules.due_dates_status == "verified" {
            "The due dates above are checked against the Finance Act 2026 text.".to_string()
        } else {
            "The due dates above are taken from a practitioner source and have not yet been \
checked against the Finance Act 2026 text; confirm them before relying on them for filing."
                .to_string()
        },
        "s.44ADA (presumptive taxation for professionals) and its correlated audit trigger are \
out of scope of this test regardless of the flag above; a profession needs separate analysis this \
test does not provide."
            .to_string(),
        "A voluntary s.44AB audit report does not extend the return due date under s.139(1); the \
applicability conclusion above does not itself confirm whether or when the return was actually \
filed."
            .to_string(),
    ];
    limits.extend(comparison_limits);
    limits.extend(cash_share.limits.iter().cloned());

    let mut ask_client = vec![
        "If audit is found not required under s.44AB(a) (or remains \
undetermined), has the return already been filed by the non-audit-case due date above -- and if \
that date has already passed without a filing, is it now a belated return under s.139(4) (s.234F \
fee, s.234A interest, and no carry-forward of business loss under s.80)?"
            .to_string(),
    ];
    if within_5pct {
        ask_client.push(
            "Full-year bank statements for every account, to confirm the cash-share breakdown \
above is not understated by a non-account-payee cheque or draft booked as bank."
                .to_string(),
        );
    }

    r.findings.push(Finding {
        id: format!("{TEST_ID}/applicability"),
        clauses: vec!["s.44AB(a)".to_string(), "3CD-8".to_string()],
        title: "s.44AB(a) tax-audit applicability on turnover and the books cash-share breakdown"
            .to_string(),
        facts,
        evidence: vec![
            EvidenceRef::new("rule", "s44ab"),
            EvidenceRef::new("rule", "due_dates"),
        ],
        confidence: Confidence::NeedsDocument,
        limits,
        ask_client,
    });

    let clauses = vec![
        "s.44AB(e)".to_string(),
        "s.44AD(4)".to_string(),
        "s.44AD(5)".to_string(),
    ];
    match presumptive_history {
        None => {
            let f_hist = r.fig(
                "presumptive_history_status",
                Value::Text("not supplied".to_string()),
                Unit::Text,
                "presumptive_history was not passed to this test.",
                Vec::new(),
            );
            r.findings.push(Finding {
                id: format!("{TEST_ID}/presumptive_history"),
                clauses,
                title: "s.44AB(e)/s.44AD(4)/(5) presumptive-taxation opt-out history not supplied"
                    .to_string(),
                facts: vec![("presumptive_history_status".to_string(), f_hist)],
                evidence: Vec::new(),
                confidence: Confidence::JudgementRequired,
                limits: vec![
                    "Presumptive-taxation history was not supplied for this engagement; \
this test never assumes no opt-out occurred merely because the history is absent."
                        .to_string(),
                ],
                ask_client: vec![
                    "For every year in the applicable s.44AD(4)/(5) lookback window: \
did the assessee declare income under s.44AD, and did it opt out (declare below the presumptive \
rate, or not under s.44AD at all) in any of those years while otherwise eligible? An opt-out can \
require an audit under s.44AB(e) independently of the turnover threshold above."
                        .to_string(),
                ],
            });
        }
        Some(history) => {
            let ay = match history.get("ay") {
                None => "unspecified".to_string(),
                Some(v) => py_str(v)?,
            };
            let opted = py_repr(history.get("opted_44ad"))?;
            let f_hist = r.fig(
                "presumptive_history_status",
                Value::Text("supplied".to_string()),
                Unit::Text,
                &format!(
                    "presumptive_history supplied for AY {ay}: opted_44ad={opted} (client config, \
verbatim; this test does not interpret it beyond restating it)."
                ),
                Vec::new(),
            );
            r.findings.push(Finding {
                id: format!("{TEST_ID}/presumptive_history"),
                clauses,
                title: "s.44AB(e)/s.44AD(4)/(5) presumptive-taxation history supplied for one \
year; full lookback needs CA judgement"
                    .to_string(),
                facts: vec![("presumptive_history_status".to_string(), f_hist)],
                evidence: vec![EvidenceRef::new("config", "presumptive_history")],
                confidence: Confidence::JudgementRequired,
                limits: vec![format!(
                    "presumptive_history covers AY {ay} only, as supplied; s.44AD(4)/(5) looks at \
every year in the lookback window, not one data point, so this test never computes a yes/no \
s.44AB(e) conclusion from it."
                )],
                ask_client: vec![
                    "Confirm the presumptive-taxation status for every remaining \
year in the s.44AD(4)/(5) lookback window, and whether an opt-out in any of them requires an audit \
under s.44AB(e) independently of the turnover threshold above."
                        .to_string(),
                ],
            });
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRORE: i64 = 1_000_000_000; // Rs 1 crore, in paise

    fn rules() -> Rules {
        Rules::vendored().unwrap()
    }

    fn cash(rec: Option<i64>, pay: Option<i64>) -> CashShare {
        CashShare {
            receipts_bp: rec,
            payments_bp: pay,
            limits: vec!["cash limit".to_string()],
        }
    }

    fn turnover(t: Option<i64>) -> TurnoverInputs {
        TurnoverInputs {
            books_turnover_paise: t,
            ..TurnoverInputs::default()
        }
    }

    fn fig(r: &TestResult, name: &str) -> Value {
        r.figures
            .iter()
            .find(|f| f.id == format!("{TEST_ID}.{name}"))
            .unwrap_or_else(|| panic!("no figure {name}"))
            .value
            .clone()
    }

    fn call(t: Option<i64>, c: &CashShare) -> String {
        let r = run(&rules(), "individual", &turnover(t), c, None).unwrap();
        match fig(&r, "audit_required_44ab_a") {
            Value::Text(s) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_rules_are_the_ones_the_boundaries_below_assume() {
        let r = rules();
        assert_eq!(r.turnover_threshold_paise, CRORE);
        assert_eq!(r.turnover_threshold_low_cash_paise, 10 * CRORE);
        assert_eq!(r.cash_share_limit_bp, 500);
    }

    /// "Exceeds" is strict at both thresholds: exactly Rs 1 crore is "no" whatever the cash
    /// share, exactly Rs 10 crore is still in the band, one paisa more is "yes".
    #[test]
    fn the_turnover_thresholds_are_strict() {
        let over = cash(Some(600), Some(100));
        let within = cash(Some(500), Some(500));
        assert_eq!(call(Some(CRORE), &over), "no");
        assert_eq!(call(Some(CRORE + 1), &over), "yes");
        assert_eq!(call(Some(CRORE + 1), &within), "undetermined");
        assert_eq!(call(Some(10 * CRORE), &within), "undetermined");
        assert_eq!(call(Some(10 * CRORE + 1), &within), "yes");
    }

    /// The cash-share limit is inclusive ("within 5%"): exactly 500 bp on both legs is within, 501
    /// on either is not.
    #[test]
    fn the_cash_share_limit_is_inclusive_on_each_leg() {
        let t = Some(5 * CRORE);
        assert_eq!(call(t, &cash(Some(500), Some(500))), "undetermined");
        assert_eq!(call(t, &cash(Some(501), Some(500))), "yes");
        assert_eq!(call(t, &cash(Some(500), Some(501))), "yes");
        let r = run(
            &rules(),
            "individual",
            &turnover(t),
            &cash(Some(500), Some(500)),
            None,
        )
        .unwrap();
        assert_eq!(fig(&r, "applicable_threshold"), Value::Int(10 * CRORE));
        assert_eq!(r.findings[0].ask_client.len(), 2); // bank statements asked only when within
        let r = run(
            &rules(),
            "individual",
            &turnover(t),
            &cash(Some(501), Some(0)),
            None,
        )
        .unwrap();
        assert_eq!(fig(&r, "applicable_threshold"), Value::Int(CRORE));
        assert_eq!(r.findings[0].ask_client.len(), 1);
    }

    #[test]
    fn missing_inputs_are_undetermined_never_no() {
        assert_eq!(call(None, &cash(Some(0), Some(0))), "undetermined");
        assert_eq!(call(Some(5 * CRORE), &cash(None, Some(0))), "undetermined");
        // An unknown cash share does not matter outside the band.
        assert_eq!(call(Some(CRORE), &cash(None, None)), "no");
        let r = run(
            &rules(),
            "individual",
            &turnover(None),
            &cash(None, None),
            None,
        )
        .unwrap();
        assert_eq!(fig(&r, "turnover"), Value::Text("not supplied".to_string()));
    }

    #[test]
    fn a_partial_comparison_source_is_never_differenced() {
        let inputs = TurnoverInputs {
            books_turnover_paise: Some(5_000),
            gstr1: Some(ComparisonTurnover {
                turnover_paise: 4_000,
                coverage: "full".to_string(),
            }),
            gstr3b: Some(ComparisonTurnover {
                turnover_paise: 1_000,
                coverage: "B2B section only".to_string(),
            }),
            ais: None,
        };
        let r = run(
            &rules(),
            "individual",
            &inputs,
            &cash(Some(0), Some(0)),
            None,
        )
        .unwrap();
        assert_eq!(fig(&r, "gstr1_turnover_diff_from_books"), Value::Int(1_000));
        assert_eq!(fig(&r, "gstr3b_turnover"), Value::Int(1_000));
        assert!(!r
            .figures
            .iter()
            .any(|f| f.id.ends_with("gstr3b_turnover_diff_from_books")));
        let limits = &r.findings[0].limits;
        assert!(limits
            .iter()
            .any(|l| l == "GSTR3B figure covers only 'B2B section only', so it is not compared to books turnover."));
        assert!(limits
            .iter()
            .any(|l| l.starts_with("AIS turnover was not supplied")));
        assert_eq!(limits.last().unwrap(), "cash limit"); // cash_44ab's limits come last
    }

    #[test]
    fn a_profession_is_flagged_by_entity_type_text() {
        let r = run(
            &rules(),
            "Profession (individual)",
            &turnover(None),
            &cash(None, None),
            None,
        )
        .unwrap();
        assert_eq!(
            fig(&r, "s44ada_professions_in_scope"),
            Value::Text("yes".to_string())
        );
        let r = run(&rules(), "firm", &turnover(None), &cash(None, None), None).unwrap();
        assert_eq!(
            fig(&r, "s44ada_professions_in_scope"),
            Value::Text("no".to_string())
        );
    }

    #[test]
    fn due_dates_carry_their_status() {
        let r = run(&rules(), "firm", &turnover(None), &cash(None, None), None).unwrap();
        assert_eq!(
            fig(&r, "due_date_audit_report"),
            Value::Text("2026-09-30".to_string())
        );
        assert!(r.findings[0].limits[0].contains("not yet been checked"));
        let mut verified = rules();
        verified.due_dates_status = "verified".to_string();
        let r = run(&verified, "firm", &turnover(None), &cash(None, None), None).unwrap();
        assert_eq!(
            r.findings[0].limits[0],
            "The due dates above are checked against the Finance Act 2026 text."
        );
    }

    fn history(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    fn history_definition(text: &str) -> String {
        let h = history(text);
        let r = run(
            &rules(),
            "firm",
            &turnover(None),
            &cash(None, None),
            Some(&h),
        )
        .unwrap();
        r.figures
            .iter()
            .find(|f| f.id.ends_with(".presumptive_history_status"))
            .unwrap()
            .definition
            .clone()
    }

    /// The history is restated as Python's f"...{ay}...{opted!r}" writes it, never interpreted.
    #[test]
    fn presumptive_history_is_restated_as_the_reference_writes_it() {
        assert!(history_definition("ay = \"2024-25\"\nopted_44ad = true\n")
            .starts_with("presumptive_history supplied for AY 2024-25: opted_44ad=True "));
        assert!(history_definition("ay = 2024\n")
            .starts_with("presumptive_history supplied for AY 2024: opted_44ad=None "));
        assert!(history_definition("opted_44ad = \"it's\"\n")
            .starts_with("presumptive_history supplied for AY unspecified: opted_44ad=\"it's\" "));
        assert!(history_definition("opted_44ad = \"no\"\n").contains("opted_44ad='no' "));
        let h = history("opted_44ad = 1.5\n");
        assert!(run(
            &rules(),
            "firm",
            &turnover(None),
            &cash(None, None),
            Some(&h)
        )
        .is_err());
    }

    #[test]
    fn absent_history_is_a_question_never_assumed() {
        let r = run(&rules(), "firm", &turnover(None), &cash(None, None), None).unwrap();
        assert_eq!(
            fig(&r, "presumptive_history_status"),
            Value::Text("not supplied".to_string())
        );
        assert_eq!(r.findings[1].confidence, Confidence::JudgementRequired);
    }
}
