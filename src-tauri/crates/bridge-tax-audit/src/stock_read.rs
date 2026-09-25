// SPDX-License-Identifier: Apache-2.0
//! The stock inputs the reference's pack reads for `stock` from a read (`tae/adapters/tally_stock.py`,
//! `read_format.stock_inputs` and `read_format.company_isintegrated`): the stock item masters, the
//! opening and closing Stock Summaries `[stock]` names, and the company's ISINTEGRATED flag.
//!
//! `load_book` keeps the verified stock parts on the book ([`StockReadParts`]) and parses none of
//! them; [`stock_inputs`] parses them when `stock` runs, so a missing or malformed stock part refuses
//! that test alone, with a typed error, as the reference fails only where it reads them.
//!
//! Divergence, deliberate: a `[stock].*_date` must be `YYYY-MM-DD`. The reference's
//! `date.fromisoformat` also admits other ISO 8601 spellings (`20250401`, week dates); those refuse
//! here rather than being read.

use std::collections::BTreeMap;

use bridge_tally_primitives::TallyDate;

use crate::book::{flip, quantity, rate_paise};
use crate::error::{AuditError, Result};
use crate::read::Part;
use crate::xml::{self, Element};

/// Tally's own reserved BASEUNITS marker for a value-only stock item, not client data.
pub const NOT_APPLICABLE_BASEUNITS: &str = "Not Applicable";

/// The read's stock parts as `load_book` found them, unparsed.
#[derive(Debug, Clone, Default)]
pub struct StockReadParts {
    /// The first `stock_items` part, as the reference's `one("stock_items")` takes it.
    pub items: Option<Part>,
    /// Every `stock_summary` part, in manifest order.
    pub summaries: Vec<Part>,
    /// The company part's ISINTEGRATED on its first COMPANY carrying a GUID: `None` when that
    /// element has no such tag or an empty one, else whether it reads "yes" in any case.
    pub is_integrated: Option<bool>,
}

/// One stock item's own master fields (the reference's `StockItemMaster`).
#[derive(Debug, Clone, PartialEq)]
pub struct StockItemMaster {
    pub name: String,
    pub guid: String,
    pub parent: String,
    pub base_unit: String,
    pub opening_qty: Option<f64>,
    pub opening_value_paise: Option<i64>,
    pub closing_qty: Option<f64>,
    pub closing_value_paise: Option<i64>,
}

impl StockItemMaster {
    /// Goods unless BASEUNITS is Tally's reserved "Not Applicable" (a unit a user named that way
    /// is still a unit).
    pub fn is_goods(&self) -> bool {
        xml::reserved_value(&self.base_unit) != Some(NOT_APPLICABLE_BASEUNITS)
    }
}

/// One item's value and quantity as of one date.
#[derive(Debug, Clone, PartialEq)]
pub struct StockSnapshotRow {
    pub name: String,
    pub guid: String,
    pub qty: Option<f64>,
    pub value_paise: Option<i64>,
    pub rate_paise: Option<i64>,
}

/// A Stock Summary as of one date, keyed by item name (a repeated name keeps its last row, as the
/// reference's dict does).
#[derive(Debug, Clone, PartialEq)]
pub struct StockSnapshot {
    pub as_of: TallyDate,
    pub rows: BTreeMap<String, StockSnapshotRow>,
}

fn overflow() -> AuditError {
    AuditError::Config("stock: a Stock Summary total overflows".to_string())
}

impl StockSnapshot {
    fn sum(&self, keep: impl Fn(i64) -> bool) -> Result<i64> {
        self.rows
            .values()
            .filter_map(|r| r.value_paise.filter(|v| keep(*v)))
            .try_fold(0_i64, |acc, v| acc.checked_add(v).ok_or_else(overflow))
    }

    /// Every row's value, a missing one as nil.
    pub fn total_value_paise(&self) -> Result<i64> {
        self.sum(|_| true)
    }

    pub fn positive_value_paise(&self) -> Result<i64> {
        self.sum(|v| v > 0)
    }

    pub fn negative_value_paise(&self) -> Result<i64> {
        self.sum(|v| v < 0)
    }
}

/// Everything `stock` reads besides the book.
#[derive(Debug, Clone, PartialEq)]
pub struct StockInputs {
    pub items: BTreeMap<String, StockItemMaster>,
    pub opening: StockSnapshot,
    pub closing: StockSnapshot,
    pub is_integrated: Option<bool>,
}

/// Every named STOCKITEM in document order: (element, its NAME). An empty or absent NAME is
/// skipped, as the reference's `if not name: continue` skips it.
fn named_items(root: &Element) -> Vec<(&Element, &str)> {
    root.descendants_named("STOCKITEM")
        .into_iter()
        .filter_map(|s| s.attr("NAME").filter(|n| !n.is_empty()).map(|n| (s, n)))
        .collect()
}

/// The reference's `load_stock_item_masters`.
pub fn stock_item_masters(part: &Part) -> Result<BTreeMap<String, StockItemMaster>> {
    let root = xml::read(&part.content, &part.id)?;
    let mut out = BTreeMap::new();
    for (s, name) in named_items(&root) {
        let master = StockItemMaster {
            name: name.to_string(),
            guid: s.child_text("GUID").to_string(),
            parent: s.child_text("PARENT").to_string(),
            base_unit: s.child_text("BASEUNITS").to_string(),
            opening_qty: quantity(s.child_text("OPENINGBALANCE"), &part.id)?,
            opening_value_paise: flip(s.child_text("OPENINGVALUE"), &part.id)?,
            closing_qty: quantity(s.child_text("CLOSINGBALANCE"), &part.id)?,
            closing_value_paise: flip(s.child_text("CLOSINGVALUE"), &part.id)?,
        };
        out.insert(name.to_string(), master);
    }
    Ok(out)
}

/// The reference's `load_stock_snapshot`: CLOSINGBALANCE/CLOSINGVALUE/CLOSINGRATE as of `as_of`.
pub fn stock_snapshot(part: &Part, as_of: TallyDate) -> Result<StockSnapshot> {
    let root = xml::read(&part.content, &part.id)?;
    let mut rows = BTreeMap::new();
    for (s, name) in named_items(&root) {
        let row = StockSnapshotRow {
            name: name.to_string(),
            guid: s.child_text("GUID").to_string(),
            qty: quantity(s.child_text("CLOSINGBALANCE"), &part.id)?,
            value_paise: flip(s.child_text("CLOSINGVALUE"), &part.id)?,
            rate_paise: rate_paise(s.child_text("CLOSINGRATE"), &part.id)?,
        };
        rows.insert(name.to_string(), row);
    }
    Ok(StockSnapshot { as_of, rows })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Opening,
    Closing,
}

impl Side {
    fn as_str(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Closing => "closing",
        }
    }
}

/// The reference's `snapshot_from_masters`: every master, with its own opening or closing fields.
fn snapshot_from_masters(
    items: &BTreeMap<String, StockItemMaster>,
    as_of: TallyDate,
    side: Side,
) -> StockSnapshot {
    let rows = items
        .iter()
        .map(|(name, it)| {
            let (qty, value_paise) = match side {
                Side::Opening => (it.opening_qty, it.opening_value_paise),
                Side::Closing => (it.closing_qty, it.closing_value_paise),
            };
            let row = StockSnapshotRow {
                name: name.clone(),
                guid: it.guid.clone(),
                qty,
                value_paise,
                rate_paise: None,
            };
            (name.clone(), row)
        })
        .collect();
    StockSnapshot { as_of, rows }
}

/// The reference's `read_format.stock_inputs` plus `company_isintegrated`, in its order: `[stock]`
/// is checked for a mixed configuration, the masters are read, then the opening and the closing
/// summary. Refuses without the stock parts, without a summary's date, or on a book not built
/// from a read.
pub fn stock_inputs(
    stock_cfg: Option<&toml::Value>,
    parts: Option<&StockReadParts>,
) -> Result<StockInputs> {
    let Some(parts) = parts else {
        return Err(AuditError::Config(
            "stock: this book was not built from a read, so it carries no stock parts".to_string(),
        ));
    };
    // No [stock] reads as an empty table, as the reference's `cfg.get("stock", {})` does: it then
    // refuses on the parts or on the first missing date.
    let empty = toml::Table::new();
    let st = match stock_cfg {
        None => &empty,
        Some(v) => v
            .as_table()
            .ok_or_else(|| AuditError::Config("stock: [stock] is not a table".to_string()))?,
    };
    if st.contains_key("items") {
        return Err(AuditError::refused(
            "CFG-mixed",
            "[stock].items names a file but [snapshot] names a read; use the read",
        ));
    }
    let items_part = parts
        .items
        .as_ref()
        .ok_or_else(|| AuditError::refused("C4-required", "no part of kind \"stock_items\""))?;
    let items = stock_item_masters(items_part)?;
    let snap = |side: Side| -> Result<StockSnapshot> {
        let kind = side.as_str();
        let src = st.get(&format!("{kind}_summary"));
        let date_key = format!("[stock].{kind}_date");
        let date_text = st
            .get(&format!("{kind}_date"))
            .ok_or_else(|| {
                AuditError::Config(format!(
                    "stock: client config missing required key '{date_key}'"
                ))
            })?
            .as_str()
            .ok_or_else(|| AuditError::Config(format!("stock: {date_key} is not a string")))?;
        let as_of = crate::read::iso_date(date_text, &date_key)?;
        match src.and_then(toml::Value::as_str) {
            Some("from_masters") => Ok(snapshot_from_masters(&items, as_of, side)),
            Some("from_read") => {
                let matching: Vec<&Part> = parts
                    .summaries
                    .iter()
                    .filter(|p| p.as_of.as_ref() == Some(&as_of))
                    .collect();
                match matching.as_slice() {
                    [part] => stock_snapshot(part, as_of),
                    _ => Err(AuditError::refused(
                        "C4-stock-summary",
                        format!(
                            "{} stock_summary parts as of {}",
                            matching.len(),
                            crate::read::iso(&as_of)
                        ),
                    )),
                }
            }
            _ => Err(AuditError::refused(
                "CFG-stock",
                format!(
                    "[stock].{kind}_summary = {}; with a read use from_read or from_masters",
                    match src {
                        // The reference formats the value with `!r`.
                        None => "None".to_string(),
                        Some(toml::Value::String(s)) => crate::support::py_repr_str(s),
                        Some(v) => v.to_string(),
                    }
                ),
            )),
        }
    };
    let opening = snap(Side::Opening)?;
    let closing = snap(Side::Closing)?;
    Ok(StockInputs {
        items,
        opening,
        closing,
        is_integrated: parts.is_integrated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(id: &str, kind: &str, as_of: Option<&str>, body: &str) -> Part {
        Part {
            id: id.to_string(),
            kind: kind.to_string(),
            name: format!("{id}.xml"),
            window: None,
            as_of: as_of.map(|d| TallyDate::parse(d.replace('-', "")).unwrap()),
            rows: None,
            alter_id_max: None,
            scope: None,
            stored_name: format!("{id}.xml"),
            response_sha256: String::new(),
            content: format!(
                "<ENVELOPE><BODY><DATA><COLLECTION>{body}</COLLECTION></DATA></BODY></ENVELOPE>"
            )
            .into_bytes(),
        }
    }

    fn cfg(text: &str) -> toml::Value {
        toml::Value::Table(text.parse::<toml::Table>().unwrap())
    }

    /// The masters and a summary are read as the reference reads them (NAME attribute, a repeated
    /// name keeping its last entry, a nameless one skipped, Tally's reserved "Not Applicable" unit
    /// marking a value-only item), and every refusal is typed.
    #[test]
    fn the_reader_reads_what_the_reference_reads_and_refuses_by_code() {
        let items = part(
            "stock-items",
            "stock_items",
            None,
            "<STOCKITEM NAME=\"Hinge\"><BASEUNITS>Nos</BASEUNITS><OPENINGBALANCE> 9 Nos</OPENINGBALANCE>\
             <OPENINGVALUE>-900.00</OPENINGVALUE></STOCKITEM>\
             <STOCKITEM NAME=\"Hinge\"><BASEUNITS>Nos</BASEUNITS><OPENINGBALANCE> 12 Nos</OPENINGBALANCE>\
             <OPENINGVALUE>-1200.00</OPENINGVALUE></STOCKITEM>\
             <STOCKITEM NAME=\"\"><OPENINGBALANCE> 1 Nos</OPENINGBALANCE></STOCKITEM>\
             <STOCKITEM NAME=\"Placeholder\"><BASEUNITS>&#4; Not Applicable</BASEUNITS></STOCKITEM>",
        );
        let close = part(
            "close",
            "stock_summary",
            Some("2026-03-31"),
            "<STOCKITEM NAME=\"Hinge\"><CLOSINGBALANCE> -5 Nos</CLOSINGBALANCE>\
             <CLOSINGVALUE>500.00</CLOSINGVALUE></STOCKITEM>\
             <STOCKITEM NAME=\"Hinge\"><CLOSINGBALANCE> -2 Nos</CLOSINGBALANCE>\
             <CLOSINGVALUE>200.00</CLOSINGVALUE><CLOSINGRATE>100.00/Nos</CLOSINGRATE></STOCKITEM>\
             <STOCKITEM NAME=\"\"><CLOSINGVALUE>-999.00</CLOSINGVALUE></STOCKITEM>",
        );
        let parts = StockReadParts {
            items: Some(items),
            summaries: vec![close.clone()],
            is_integrated: Some(false),
        };
        let ok = cfg(
            "opening_summary = \"from_masters\"\nopening_date = \"2025-04-01\"\n\
                      closing_summary = \"from_read\"\nclosing_date = \"2026-03-31\"",
        );
        let got = stock_inputs(Some(&ok), Some(&parts)).unwrap();
        assert_eq!(got.items.len(), 2);
        assert_eq!(got.items["Hinge"].opening_qty, Some(12.0));
        assert_eq!(got.items["Hinge"].opening_value_paise, Some(120_000));
        assert!(got.items["Hinge"].is_goods() && !got.items["Placeholder"].is_goods());
        assert_eq!(got.opening.rows["Hinge"].qty, Some(12.0));
        assert_eq!(got.opening.rows["Placeholder"].qty, None);
        let row = &got.closing.rows["Hinge"];
        assert_eq!(
            (row.qty, row.value_paise, row.rate_paise),
            (Some(-2.0), Some(-20_000), Some(10_000))
        );
        // A repeated summary name keeps its last row and a nameless one is dropped, as for masters.
        assert_eq!(got.closing.rows.len(), 1);
        assert_eq!(got.closing.total_value_paise().unwrap(), -20_000);
        assert_eq!(got.is_integrated, Some(false));

        let code = |c: &toml::Value, p: &StockReadParts| {
            stock_inputs(Some(c), Some(p)).unwrap_err().code()
        };
        let two = StockReadParts {
            summaries: vec![close.clone(), close],
            ..parts.clone()
        };
        assert_eq!(code(&ok, &two), Some("C4-stock-summary"));
        let none = StockReadParts {
            summaries: Vec::new(),
            ..parts.clone()
        };
        assert_eq!(code(&ok, &none), Some("C4-stock-summary"));
        let other_day = cfg(
            "opening_summary = \"from_masters\"\nopening_date = \"2025-04-01\"\n\
             closing_summary = \"from_read\"\nclosing_date = \"2026-03-30\"",
        );
        assert_eq!(code(&other_day, &parts), Some("C4-stock-summary"));
        let no_items = StockReadParts {
            items: None,
            ..parts.clone()
        };
        assert_eq!(code(&ok, &no_items), Some("C4-required"));
        assert_eq!(
            code(&cfg("items = \"masters.xml\""), &parts),
            Some("CFG-mixed")
        );
        let odd = cfg("opening_summary = \"from_file\"\nopening_date = \"2025-04-01\"");
        assert_eq!(code(&odd, &parts), Some("CFG-stock"));
        assert!(stock_inputs(
            Some(&cfg("opening_summary = \"from_masters\"")),
            Some(&parts)
        )
        .is_err());
        assert!(stock_inputs(Some(&ok), None).is_err());
    }
}
