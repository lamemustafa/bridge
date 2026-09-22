// SPDX-License-Identifier: Apache-2.0
//! The assessee's TRACES documents, as the reference implementation's adapters parse them: Form
//! 26AS rows (`Form26ASRow`), AIS entries (`AisRow`) and TIS category totals (`TisRow`). This crate
//! reads no document itself: the rows are caller data, in the JSON `parity/python_golden.py
//! --emit-traces-documents` writes from the reference's own adapters, so both sides of a parity
//! run see the same rows. A real client's rows are client data and never belong in this
//! repository; CI uses invented rows only.

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

fn bad(what: &str) -> AuditError {
    AuditError::Config(format!("traces documents: {what}"))
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
    }
}
