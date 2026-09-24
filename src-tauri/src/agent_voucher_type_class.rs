//! Which vouchers a `vouchers` type filter selects (bridge#625).
//!
//! A voucher-type name is user-editable in Tally: a reserved type can be
//! renamed, and another type can carry a reserved type's display name under a
//! different class. Matching `VOUCHERTYPENAME` as a string therefore returned
//! a confident zero on a book whose Purchase type had been renamed.
//!
//! The class is read from Tally in the same response as the vouchers, one
//! COMPUTE per row, so the class and the rows describe one state of the book.
//! Measured on licensed TallyPrime 7.1 (2026-09-24, synthetic company):
//! - `$$Is<Class>:$VoucherTypeName` is `Yes` for a renamed reserved type and
//!   for a child of it, and `No` for a type of another class that carries the
//!   reserved display name;
//! - `$$Is<Class>` on a name no type carries is also `No`, so it cannot prove
//!   that a type exists: `$GUID:VoucherType:$VoucherTypeName` does;
//! - an unknown `$$` function omits its element entirely, so a missing element
//!   is refused, never read as `No`.
//!
//! Child-type resolution was measured for Purchase only; the other classes were
//! measured on their own reserved type.
use super::*;
use std::collections::{BTreeMap as Map, BTreeSet};

/// A reserved voucher class whose `$$Is<Class>` function has been measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ReservedVoucherClass {
    Sales,
    Purchase,
    Payment,
    Receipt,
    Contra,
    Journal,
    DebitNote,
    CreditNote,
}

impl ReservedVoucherClass {
    pub(super) const ALL: [Self; 8] = [
        Self::Sales,
        Self::Purchase,
        Self::Payment,
        Self::Receipt,
        Self::Contra,
        Self::Journal,
        Self::DebitNote,
        Self::CreditNote,
    ];

    /// Tally's `RESERVEDNAME` for the class, which is also the tool's input
    /// spelling.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Sales => "Sales",
            Self::Purchase => "Purchase",
            Self::Payment => "Payment",
            Self::Receipt => "Receipt",
            Self::Contra => "Contra",
            Self::Journal => "Journal",
            Self::DebitNote => "Debit Note",
            Self::CreditNote => "Credit Note",
        }
    }

    const fn function(self) -> &'static str {
        match self {
            Self::Sales => "$$IsSales",
            Self::Purchase => "$$IsPurchase",
            Self::Payment => "$$IsPayment",
            Self::Receipt => "$$IsReceipt",
            Self::Contra => "$$IsContra",
            Self::Journal => "$$IsJournal",
            Self::DebitNote => "$$IsDebitNote",
            Self::CreditNote => "$$IsCreditNote",
        }
    }

    /// The response element carrying this class's answer for a row.
    pub(super) const fn tag(self) -> &'static str {
        match self {
            Self::Sales => "BRIDGEVCHISSALES",
            Self::Purchase => "BRIDGEVCHISPURCHASE",
            Self::Payment => "BRIDGEVCHISPAYMENT",
            Self::Receipt => "BRIDGEVCHISRECEIPT",
            Self::Contra => "BRIDGEVCHISCONTRA",
            Self::Journal => "BRIDGEVCHISJOURNAL",
            Self::DebitNote => "BRIDGEVCHISDEBITNOTE",
            Self::CreditNote => "BRIDGEVCHISCREDITNOTE",
        }
    }

    pub(super) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.name() == value)
    }
}

/// The voucher type's GUID, looked up by the row's own type name.
pub(super) const VOUCHER_TYPE_GUID_TAG: &str = "BRIDGEVCHTYPEGUID";
/// The voucher type's own `RESERVEDNAME`: empty for a child type.
pub(super) const VOUCHER_TYPE_RESERVED_NAME_TAG: &str = "BRIDGEVCHRESERVEDNAME";

fn voucher_type_class_tags() -> impl Iterator<Item = &'static str> + Clone {
    [VOUCHER_TYPE_GUID_TAG, VOUCHER_TYPE_RESERVED_NAME_TAG]
        .into_iter()
        .chain(ReservedVoucherClass::ALL.into_iter().map(ReservedVoucherClass::tag))
}

/// The COMPUTE elements the class-resolving read adds after its FETCH.
pub(super) fn voucher_type_class_computes() -> String {
    let mut computes = format!(
        "<COMPUTE>{VOUCHER_TYPE_GUID_TAG}:$GUID:VoucherType:$VoucherTypeName</COMPUTE>\
         <COMPUTE>{VOUCHER_TYPE_RESERVED_NAME_TAG}:$ReservedName:VoucherType:$VoucherTypeName</COMPUTE>"
    );
    for class in ReservedVoucherClass::ALL {
        computes.push_str(&format!(
            "<COMPUTE>{}:{}:$VoucherTypeName</COMPUTE>",
            class.tag(),
            class.function()
        ));
    }
    computes
}

/// Whether `field` is one of the elements [`voucher_type_class_computes`] asks for.
pub(super) fn is_voucher_type_class_scalar(field: &str) -> bool {
    voucher_type_class_tags().any(|tag| tag == field)
}

/// A row's voucher type as Tally resolved it in the same response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResolvedVoucherType {
    pub(super) guid: String,
    /// `None` for a type outside the measured classes (for example Attendance).
    pub(super) class: Option<ReservedVoucherClass>,
}

/// Reads the class elements of one row. `Ok(None)` when the row carries none
/// of them, which is every read that did not ask for them. A row that carries
/// some but not all, or an answer that is not `Yes`/`No`, is refused: an
/// unknown function omits its element, so a gap is never a `No`.
pub(super) fn resolve_row_voucher_type(
    row: &Map<String, String>,
    company_guid: &str,
) -> Result<Option<ResolvedVoucherType>, String> {
    let present = voucher_type_class_tags()
        .filter(|tag| row.contains_key(*tag))
        .count();
    if present == 0 {
        return Ok(None);
    }
    if present != voucher_type_class_tags().count() {
        return Err("voucher_type_class_missing".to_string());
    }
    let guid = row[VOUCHER_TYPE_GUID_TAG].trim();
    if guid.is_empty()
        || !bridge_tally_protocol::master_guid_belongs_to_company(guid, company_guid)
    {
        return Err("voucher_type_unresolved".to_string());
    }
    let mut classes = Vec::new();
    for class in ReservedVoucherClass::ALL {
        match row[class.tag()].trim() {
            "Yes" => classes.push(class),
            "No" => {}
            _ => return Err("voucher_type_class_invalid".to_string()),
        }
    }
    if classes.len() > 1 {
        return Err("voucher_type_class_contradictory".to_string());
    }
    let class = classes.first().copied();
    // A type's own reserved name is a direct observation of its class; the
    // class function must agree with it.
    if let Some(direct) =
        ReservedVoucherClass::parse(row[VOUCHER_TYPE_RESERVED_NAME_TAG].trim())
    {
        if class != Some(direct) {
            return Err("voucher_type_class_contradictory".to_string());
        }
    }
    Ok(Some(ResolvedVoucherType {
        guid: guid.to_string(),
        class,
    }))
}

/// What a `vouchers` call asked for, parsed at the boundary: exactly one
/// reading of the request, never guessed from the string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum VoucherTypeSelector {
    /// Every type whose class is this one, whatever it is named.
    Class(ReservedVoucherClass),
    /// Exactly the type with this GUID.
    Guid(String),
    /// Exactly the type with this display name.
    Name(String),
}

impl VoucherTypeSelector {
    pub(super) fn from_args(args: &Value) -> Result<Option<Self>, ToolFailure> {
        let class = optional_string(args, "voucher_class")?;
        let guid = optional_string(args, "voucher_type_guid")?;
        let name = optional_string(args, "voucher_type")?;
        match (class, guid, name) {
            (None, None, None) => Ok(None),
            (Some(class), None, None) => ReservedVoucherClass::parse(&class)
                .map(|class| Some(Self::Class(class)))
                .ok_or_else(|| ToolFailure::from("voucher_class_unsupported".to_string())),
            (None, Some(guid), None) => Ok(Some(Self::Guid(guid))),
            (None, None, Some(name)) if !name.is_empty() => Ok(Some(Self::Name(name))),
            (None, None, Some(_)) => Err(ToolFailure::from("voucher_type_invalid".to_string())),
            _ => Err(ToolFailure::from(
                "voucher_type_selectors_conflict".to_string(),
            )),
        }
    }
}

/// One voucher type seen in the window, with how many of its rows were there.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct WindowVoucherType {
    pub(super) guid: String,
    pub(super) name: String,
    pub(super) class: Option<ReservedVoucherClass>,
    pub(super) rows: usize,
}

impl WindowVoucherType {
    pub(super) fn json(&self) -> Value {
        json!({
            "name": self.name,
            "guid": self.guid,
            "class": self.class.map(ReservedVoucherClass::name),
            "rows": self.rows,
        })
    }
}

/// The rows a selector keeps, and the types it kept them from.
#[derive(Debug)]
pub(super) struct VoucherTypeSelection {
    pub(super) rows: Vec<Value>,
    pub(super) included: Vec<WindowVoucherType>,
    pub(super) window_types: Vec<WindowVoucherType>,
}

/// A refusal that names the types involved, so a caller can choose one.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct VoucherTypeRefusal {
    pub(super) code: &'static str,
    pub(super) candidates: Vec<WindowVoucherType>,
}

impl VoucherTypeRefusal {
    fn bare(code: &'static str) -> Self {
        Self {
            code,
            candidates: Vec::new(),
        }
    }
}

fn row_type(row: &Value) -> Result<(String, String, Option<ReservedVoucherClass>), &'static str> {
    let guid = row["voucher_type_guid"]
        .as_str()
        .ok_or("voucher_type_class_missing")?;
    let name = row["voucher_type"]
        .as_str()
        .ok_or("voucher_type_class_missing")?;
    let class = match &row["voucher_class"] {
        Value::Null => None,
        Value::String(class) => {
            Some(ReservedVoucherClass::parse(class).ok_or("voucher_type_class_invalid")?)
        }
        _ => return Err("voucher_type_class_invalid"),
    };
    Ok((guid.to_string(), name.to_string(), class))
}

/// Applies `selector` to rows that carry their resolved type. Judged over this
/// window's rows only: a type with no voucher in the window is not seen.
///
/// A name is refused as ambiguous when it is also a class name (ignoring case)
/// and the types carrying that name are not exactly the types of that class
/// in the window: on a book whose Purchase type was renamed, or has child
/// types, "Purchase" by name would otherwise return a silent part, or none, of
/// the purchases.
pub(super) fn select_voucher_rows(
    rows: Vec<Value>,
    selector: &VoucherTypeSelector,
) -> Result<VoucherTypeSelection, VoucherTypeRefusal> {
    let mut types: Map<String, WindowVoucherType> = Map::new();
    let mut keyed = Vec::with_capacity(rows.len());
    for row in rows {
        let (guid, name, class) = row_type(&row).map_err(VoucherTypeRefusal::bare)?;
        let seen = types
            .entry(guid.clone())
            .or_insert_with(|| WindowVoucherType {
                guid: guid.clone(),
                name: name.clone(),
                class,
                rows: 0,
            });
        // One GUID is one type: a second name or class for it is not one state.
        if seen.name != name || seen.class != class {
            return Err(VoucherTypeRefusal::bare(
                "voucher_type_snapshot_inconsistent",
            ));
        }
        seen.rows += 1;
        keyed.push((guid, row));
    }
    let window_types = types.into_values().collect::<Vec<_>>();
    let guids_where = |selected: &dyn Fn(&WindowVoucherType) -> bool| {
        window_types
            .iter()
            .filter(|kind| selected(kind))
            .map(|kind| kind.guid.clone())
            .collect::<BTreeSet<_>>()
    };
    let chosen = match selector {
        VoucherTypeSelector::Class(class) => guids_where(&|kind| kind.class == Some(*class)),
        VoucherTypeSelector::Guid(guid) => guids_where(&|kind| &kind.guid == guid),
        VoucherTypeSelector::Name(name) => {
            let named = guids_where(&|kind| &kind.name == name);
            if let Some(class) = ReservedVoucherClass::ALL
                .into_iter()
                .find(|class| class.name().eq_ignore_ascii_case(name))
            {
                let of_class = guids_where(&|kind| kind.class == Some(class));
                if named != of_class {
                    return Err(VoucherTypeRefusal {
                        code: "voucher_type_ambiguous",
                        candidates: window_types
                            .iter()
                            .filter(|kind| {
                                named.contains(&kind.guid) || of_class.contains(&kind.guid)
                            })
                            .cloned()
                            .collect(),
                    });
                }
            }
            named
        }
    };
    let included = window_types
        .iter()
        .filter(|kind| chosen.contains(&kind.guid))
        .cloned()
        .collect();
    Ok(VoucherTypeSelection {
        rows: keyed
            .into_iter()
            .filter(|(guid, _)| chosen.contains(guid))
            .map(|(_, row)| row)
            .collect(),
        included,
        window_types,
    })
}

#[cfg(test)]
#[path = "agent_voucher_type_class_tests.rs"]
mod tests;
