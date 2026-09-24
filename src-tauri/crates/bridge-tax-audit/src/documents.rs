// SPDX-License-Identifier: Apache-2.0
//! The assessee's documents, as the reference implementation's adapters parse them: the TRACES
//! documents -- Form 26AS rows (`Form26ASRow`), AIS entries (`AisRow`) and TIS category totals
//! (`TisRow`) -- and a bank statement (`BankStatementDoc`). This crate reads no document itself:
//! the rows are caller data, in the JSON `parity/python_golden.py --emit-traces-documents` or
//! `--emit-bank-statement` writes from the reference's own adapters, so both sides of a parity run
//! see the same rows. A real client's rows are client data and never belong in this repository; CI
//! uses invented rows only.

use bridge_tally_primitives::TallyDate;
use serde_json::Value;

use crate::error::{AuditError, Result};

/// One Form 26AS row: Part I is TDS, Part VI is TCS; the reference keeps other parts too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form26asRow {
    pub doc: String,
    pub row: i64,
    pub part: String,
    pub deductor_tan: String,
    pub section: String,
    pub txn_date: TallyDate,
    pub amount_paise: i64,
    pub tax_paise: i64,
}

/// One AIS entry: `category` is "gst_turnover", "gst_purchases", "advance_tax" or "refund".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AisRow {
    pub doc: String,
    pub row: i64,
    pub category: String,
    pub source_name: String,
    pub source_id: String,
    pub txn_date: Option<TallyDate>,
    pub amount_paise: i64,
}

/// One TIS category total, under TIS's own category text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TisRow {
    pub doc: String,
    pub row: i64,
    pub category: String,
    pub reported_paise: i64,
    pub processed_paise: i64,
    pub accepted_paise: i64,
}

/// Every TRACES document row a run was given. Empty is "none supplied".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TracesDocuments {
    pub form26as: Vec<Form26asRow>,
    pub ais: Vec<AisRow>,
    pub tis: Vec<TisRow>,
}

/// A bank statement, as the reference's `adapters/bank_documents.py` reads one extraction: the
/// document-level facts it carries once, and its transaction rows in the source's own order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankStatementDoc {
    pub doc_id: String,
    pub source_sha256: String,
    /// The masked account number, as the extraction masks it.
    pub account_ref: String,
    pub bank: String,
    /// The calendar window the statement covers, both ends inclusive.
    pub start: TallyDate,
    pub end: TallyDate,
    pub opening_balance_paise: i64,
    pub closing_balance_paise: i64,
    pub rows: Vec<BankStatementRow>,
}

/// One statement transaction. `row` is its 0-based position in the source's list; `balance_paise`
/// is `None` where the source carries no running balance for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BankStatementRow {
    pub doc: String,
    pub row: i64,
    pub account_ref: String,
    pub txn_date: TallyDate,
    pub narration: String,
    pub debit_paise: i64,
    pub credit_paise: i64,
    pub balance_paise: Option<i64>,
}

/// A malformed field; the public reader it came through adds which document it was.
fn bad(what: &str) -> AuditError {
    AuditError::Config(what.to_string())
}

fn document(name: &str) -> impl Fn(AuditError) -> AuditError + '_ {
    move |e| match e {
        AuditError::Config(m) => AuditError::Config(format!("{name}: {m}")),
        other => other,
    }
}

fn text(v: &Value, key: &str, what: &str) -> Result<String> {
    v[key]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| bad(&format!("{what}.{key} is not a string")))
}

fn int(v: &Value, key: &str, what: &str) -> Result<i64> {
    v[key]
        .as_i64()
        .ok_or_else(|| bad(&format!("{what}.{key} is not an integer")))
}

/// An ISO date, `YYYY-MM-DD`, as the emitter writes `date.isoformat()`.
fn iso_date(v: &Value, key: &str, what: &str) -> Result<TallyDate> {
    let s = text(v, key, what)?;
    let b = s.as_bytes();
    let shaped = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    shaped
        .then(|| TallyDate::parse(s.replace('-', "")).ok())
        .flatten()
        .ok_or_else(|| bad(&format!("{what}.{key} {s:?} is not YYYY-MM-DD")))
}

fn rows<'a>(v: &'a Value, key: &str) -> Result<&'a [Value]> {
    match &v[key] {
        Value::Null => Ok(&[]),
        Value::Array(a) => Ok(a),
        _ => Err(bad(&format!("{key} is not a list"))),
    }
}

/// The JSON `parity/python_golden.py --emit-traces-documents` writes: `form26as`, `ais` and `tis`
/// lists (each optional, empty when absent).
pub fn traces_documents_from_json(v: &Value) -> Result<TracesDocuments> {
    traces_documents(v).map_err(document("traces documents"))
}

fn traces_documents(v: &Value) -> Result<TracesDocuments> {
    if !v.is_object() {
        return Err(bad("not an object"));
    }
    let form26as = rows(v, "form26as")?
        .iter()
        .map(|r| {
            Ok(Form26asRow {
                doc: text(r, "doc", "form26as")?,
                row: int(r, "row", "form26as")?,
                part: text(r, "part", "form26as")?,
                deductor_tan: text(r, "deductor_tan", "form26as")?,
                section: text(r, "section", "form26as")?,
                txn_date: iso_date(r, "txn_date", "form26as")?,
                amount_paise: int(r, "amount_paise", "form26as")?,
                tax_paise: int(r, "tax_paise", "form26as")?,
            })
        })
        .collect::<Result<_>>()?;
    let ais = rows(v, "ais")?
        .iter()
        .map(|r| {
            Ok(AisRow {
                doc: text(r, "doc", "ais")?,
                row: int(r, "row", "ais")?,
                category: text(r, "category", "ais")?,
                source_name: text(r, "source_name", "ais")?,
                source_id: text(r, "source_id", "ais")?,
                txn_date: match &r["txn_date"] {
                    Value::Null => None,
                    _ => Some(iso_date(r, "txn_date", "ais")?),
                },
                amount_paise: int(r, "amount_paise", "ais")?,
            })
        })
        .collect::<Result<_>>()?;
    let tis = rows(v, "tis")?
        .iter()
        .map(|r| {
            Ok(TisRow {
                doc: text(r, "doc", "tis")?,
                row: int(r, "row", "tis")?,
                category: text(r, "category", "tis")?,
                reported_paise: int(r, "reported_paise", "tis")?,
                processed_paise: int(r, "processed_paise", "tis")?,
                accepted_paise: int(r, "accepted_paise", "tis")?,
            })
        })
        .collect::<Result<_>>()?;
    Ok(TracesDocuments { form26as, ais, tis })
}

/// The JSON `parity/python_golden.py --emit-bank-statement` writes: the document's own facts, a
/// `period` of `start`/`end`, and its `rows` (each `balance_paise` an integer or null).
pub fn bank_statement_from_json(v: &Value) -> Result<BankStatementDoc> {
    bank_statement(v).map_err(document("bank statement"))
}

fn bank_statement(v: &Value) -> Result<BankStatementDoc> {
    if !v.is_object() {
        return Err(bad("not an object"));
    }
    let period = &v["period"];
    if !period.is_object() {
        return Err(bad("period is not an object"));
    }
    let rows = rows(v, "rows")?
        .iter()
        .map(|r| {
            Ok(BankStatementRow {
                doc: text(r, "doc", "rows")?,
                row: int(r, "row", "rows")?,
                account_ref: text(r, "account_ref", "rows")?,
                txn_date: iso_date(r, "txn_date", "rows")?,
                narration: text(r, "narration", "rows")?,
                debit_paise: int(r, "debit_paise", "rows")?,
                credit_paise: int(r, "credit_paise", "rows")?,
                balance_paise: match &r["balance_paise"] {
                    Value::Null => None,
                    _ => Some(int(r, "balance_paise", "rows")?),
                },
            })
        })
        .collect::<Result<_>>()?;
    Ok(BankStatementDoc {
        doc_id: text(v, "doc_id", "statement")?,
        source_sha256: text(v, "source_sha256", "statement")?,
        account_ref: text(v, "account_ref", "statement")?,
        bank: text(v, "bank", "statement")?,
        start: iso_date(period, "start", "period")?,
        end: iso_date(period, "end", "period")?,
        opening_balance_paise: int(v, "opening_balance_paise", "statement")?,
        closing_balance_paise: int(v, "closing_balance_paise", "statement")?,
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn documents_are_read_as_the_emitter_writes_them_and_bad_shapes_refuse() {
        let d = traces_documents_from_json(&json!({
            "form26as": [{"doc": "form26as:x", "row": 1, "part": "I", "deductor_tan": "TAN-A",
                          "section": "194C", "txn_date": "2025-06-30", "amount_paise": 100,
                          "tax_paise": 1}],
            "ais": [{"doc": "ais:x", "row": 2, "category": "refund", "source_name": "s",
                     "source_id": "i", "txn_date": null, "amount_paise": 5}],
            "tis": [{"doc": "tis:x", "row": 3, "category": "GST turnover", "reported_paise": 1,
                     "processed_paise": 2, "accepted_paise": 3}]
        }))
        .unwrap();
        assert_eq!(d.form26as[0].txn_date.as_str(), "20250630");
        assert_eq!(d.ais[0].txn_date, None);
        assert_eq!(d.tis[0].accepted_paise, 3);
        assert_eq!(
            traces_documents_from_json(&json!({})).unwrap(),
            TracesDocuments::default()
        );
        for bad in [
            json!([]),
            json!({"form26as": {}}),
            json!({"form26as": [{"doc": "d", "row": 1, "part": "I", "deductor_tan": "T",
                                 "section": "194C", "txn_date": "30-06-2025",
                                 "amount_paise": 1, "tax_paise": 1}]}),
            json!({"tis": [{"doc": "d", "row": "1", "category": "c", "reported_paise": 1,
                            "processed_paise": 1, "accepted_paise": 1}]}),
        ] {
            assert!(traces_documents_from_json(&bad).is_err(), "{bad}");
        }
        let err = traces_documents_from_json(&json!([])).unwrap_err();
        assert_eq!(format!("{err}"), format!("{}", AuditError::Config("traces documents: not an object".into())));
    }

    #[test]
    fn a_bank_statement_is_read_as_the_emitter_writes_it_and_bad_shapes_refuse() {
        let row = json!({"doc": "bank:x:statement", "row": 0, "account_ref": "XX12", "txn_date":
                         "2026-03-02", "narration": "NEFT", "debit_paise": 0, "credit_paise": 500,
                         "balance_paise": 1500});
        let doc = json!({"doc_id": "bank:x:statement", "source_sha256": "ab", "account_ref": "XX12",
                         "bank": "B", "period": {"start": "2026-03-01", "end": "2026-03-31"},
                         "opening_balance_paise": 1000, "closing_balance_paise": 1500,
                         "rows": [row, {"doc": "bank:x:statement", "row": 1, "account_ref": "XX12",
                                        "txn_date": "2026-03-03", "narration": "", "debit_paise": 1,
                                        "credit_paise": 0, "balance_paise": null}]});
        let d = bank_statement_from_json(&doc).unwrap();
        assert_eq!((d.start.as_str(), d.end.as_str()), ("20260301", "20260331"));
        assert_eq!((d.rows.len(), d.rows[0].balance_paise, d.rows[1].balance_paise), (2, Some(1500), None));
        assert_eq!(d.opening_balance_paise, 1000);
        for (key, value) in [("period", json!("2026-03")), ("opening_balance_paise", json!(1.5)),
                             ("rows", json!({})), ("bank", json!(null))] {
            let mut broken = doc.clone();
            broken[key] = value;
            let err = bank_statement_from_json(&broken).unwrap_err();
            assert!(format!("{err}").contains("bank statement: "), "{key}: {err}");
        }
        let mut broken = doc.clone();
        broken["rows"][0]["debit_paise"] = json!("0");
        assert!(bank_statement_from_json(&broken).is_err());
    }
}
