//! LAB-ONLY write surface (audit-sprint 2026-09-14, Phase 3.4/3.5).
//!
//! Compiled only behind `lab-writes` (via `agent_lab.rs`'s `mod import`
//! declaration); every tool here additionally refuses at runtime unless
//! `BRIDGE_LAB_WRITES=1` -- checked by the caller in `agent.rs`, the same
//! gate `lab_read_inventory` uses. Every batch calls [`admit_lab_target`]
//! immediately before it is sent, so the loaded-company / deny-list /
//! target-identity guard is re-verified on every single write, not once per
//! tool call, matching the plan's "admits target before every batch"
//! requirement.
//!
//! Input is the book model documented in
//! `brain/50-projects/audit-sprint-2026-09-14/specs/book_schema.md`, built by
//! `SP/code/book/build_book.py`. This module never reads a snapshot itself.
//!
//! **What is reused, and what is not, and why:**
//! - [`bridge_tally_protocol::parse_import_outcome`] parses every
//!   `<RESPONSE>` (§9.1/§9.2) -- genuinely shared code, not duplicated.
//! - The master-name matching predicate ([`canonical_master_key`]) is the
//!   composed fold measured on **licensed TallyPrime 7.1** in §9.4d (space,
//!   `-` and `/` interchangeable, internal runs collapsed, surrounding
//!   whitespace ignored, ASCII case folded, otherwise exact codepoints; NFC/
//!   NFD is deliberately NOT folded -- §9.4d's own "canonical equivalence is
//!   still refused" finding). §9.4d's measurement scope is *ledgers, one
//!   company*; applying the same fold to every other master kind here is a
//!   deliberate conservative choice for a pre-*write* collision check (a
//!   false positive only makes this tool over-refuse, which is the safe
//!   failure direction for the Create-overwrite trap, §9.4) -- it is not a
//!   claim that Tally folds group/unit/godown/stock-item names the same way.
//! - The XML **shapes** for Payment/Receipt/Contra (§9.13: `EFFECTIVEDATE`,
//!   `PARTYLEDGERNAME` on the counterparty side, Dr-first ordering) and for
//!   invoice-mode Sales (§9.12a: `LEDGERENTRIES.LIST` + `ISINVOICE=Yes` +
//!   `ALLINVENTORYENTRIES.LIST`) are reused byte-for-byte against the
//!   documented captures. What is **not** reused is `agent_import.rs`'s
//!   `render_voucher_xml` function itself: its `ImportEntry` carries no
//!   `BILLALLOCATIONS.LIST`, and every voucher type this book model writes --
//!   Payment/Receipt/Contra included, per the rehearsal book -- can carry
//!   bill allocations against a bill-wise party. Reusing that function
//!   unmodified would silently drop them, which is exactly the class of
//!   defect §9.2/§12a.4 exist to catch. So this module renders its own
//!   entries from the proven wire shape rather than the Rust function.
//! - Group/Unit/Godown/StockGroup/StockItem **master** XML and
//!   Purchase/Credit-Note/Debit-Note **invoice** XML have no live capture
//!   anywhere in this repository's protocol reference. They are built from
//!   Tally's well-documented standard master schema and from §9.12a's
//!   invoice shape generalised across voucher types (a hypothesis §9.12
//!   explicitly says is untested for anything but Sales). **Both are
//!   UNVERIFIED for the gateway import path and need the one-voucher /
//!   one-master live probe §9.4/§9.12 themselves prescribe before a real
//!   batch** -- see this worker's final report.

use super::*;
use bridge_tally_core::ExactDecimal;
use bridge_tally_protocol::is_tally_reserved_root;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const MAX_MASTER_BATCH: usize = 200;
const MAX_VOUCHER_BATCH: usize = 100;

// ---------------------------------------------------------------------------
// Master-name matching (§9.4d, licensed TallyPrime 7.1) -- see module doc.
// ---------------------------------------------------------------------------

/// The composed fold §9.4d measured on licensed TallyPrime 7.1: ASCII case
/// folded, `-`/`/` treated as a space, internal whitespace runs collapsed to
/// one, surrounding whitespace trimmed. Deliberately does **not** apply
/// Unicode normalisation (NFC/NFD) -- §9.4d's own finding is that Tally
/// refuses that fold.
fn canonical_master_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_space = true; // trims leading whitespace for free
    for ch in name.chars() {
        let mapped = match ch {
            '-' | '/' => ' ',
            other => other,
        };
        if mapped.is_whitespace() {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.extend(mapped.to_lowercase());
            last_was_space = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The `company_guid` the caller supplied must be the admitted lab target's
/// own GUID -- `admit_lab_target` already proved *a* target is loaded and
/// unique; this closes the separate hole of a caller passing a different
/// (e.g. stale) GUID than the one that was just admitted.
fn identity_matches_requested_guid(identity: &VerifiedCompanyIdentity, guid: &str) -> bool {
    identity.company_guid().eq_ignore_ascii_case(guid)
}

fn amounts_equal(a: &str, b: &str) -> bool {
    match (ExactDecimal::parse(a), ExactDecimal::parse(b)) {
        // `numeric_eq`, not `==`: ExactDecimal's derived equality is on its
        // stored lexeme, so "0" and "0.00" would otherwise compare unequal.
        (Ok(a), Ok(b)) => a.numeric_eq(&b),
        _ => a.trim() == b.trim(),
    }
}

/// Whether `value` is numerically zero (or blank/unparseable, which a master
/// renderer treats the same as zero -- nothing to report). Used to gate
/// `OPENINGBALANCE`/`OPENINGVALUE`: the proven-good capture
/// (`babul-masters-complete.xml`) only ever emits an opening amount element
/// when it is non-zero -- a zero-balance ledger's `<LEDGER>` carries no
/// `OPENINGBALANCE` at all.
fn is_zero_amount(value: &str) -> bool {
    match ExactDecimal::parse(value) {
        Ok(amount) => amount.numeric_eq(&ExactDecimal::parse("0").expect("literal parses")),
        Err(_) => value.trim().is_empty(),
    }
}

/// Whether a ledger's recorded `tax_type` is a real GST/duty classification
/// worth sending, as opposed to an empty value or Tally's own inert default
/// `"Others"` -- the shape that reached the gateway request in the 2026-09-14
/// rehearsal sent `<TAXTYPE>Others</TAXTYPE>` on every ledger, including bank
/// and expense ledgers that are not duty heads at all.
fn is_real_gst_duty_type(tax_type: &str) -> bool {
    let trimmed = tax_type.trim();
    !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("others")
}

/// Whether `parent` resolves (via the §9.4d fold) to the `Duties & Taxes`
/// group -- `TAXTYPE` is only ever meaningful on a ledger actually parented
/// there.
fn is_duties_and_taxes_parent(parent: &str) -> bool {
    canonical_master_key(parent) == canonical_master_key("Duties & Taxes")
}

// ---------------------------------------------------------------------------
// Book model (input) -- see SP/specs/book_schema.md for the full schema.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
struct BookMasters {
    #[serde(default)]
    units: Vec<BookUnit>,
    #[serde(default)]
    godowns: Vec<BookNamedParent>,
    #[serde(default)]
    stock_groups: Vec<BookNamedParent>,
    #[serde(default)]
    groups: Vec<BookNamedParent>,
    #[serde(default)]
    ledgers: Vec<BookLedger>,
    #[serde(default)]
    stock_items: Vec<BookStockItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct BookUnit {
    name: String,
    #[serde(default)]
    is_simple_unit: Option<String>,
    #[serde(default)]
    decimal_places: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BookNamedParent {
    name: String,
    #[serde(default)]
    parent: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BookLedger {
    name: String,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    opening_balance: Option<String>,
    #[serde(default)]
    is_billwise_on: Option<bool>,
    #[serde(default)]
    party_gstin: Option<String>,
    #[serde(default)]
    tax_type: Option<String>,
    #[serde(default)]
    gst_duty_head: Option<String>,
    #[serde(default)]
    opening_bill_allocations: Vec<BookBillAllocation>,
}

#[derive(Debug, Clone, Deserialize)]
struct BookBillAllocation {
    #[serde(default)]
    name: Option<String>,
    bill_type: String,
    amount: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BookStockItem {
    name: String,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    base_unit: Option<String>,
    #[serde(default)]
    opening_qty: Option<String>,
    #[serde(default)]
    opening_rate: Option<String>,
    #[serde(default)]
    opening_value: Option<String>,
    #[serde(default)]
    gst_applicable: Option<String>,
    #[serde(default)]
    hsn_code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BookVoucher {
    source_guid: String,
    #[serde(rename = "type")]
    voucher_type: String,
    date: String,
    #[serde(default)]
    voucher_number: Option<String>,
    #[serde(default)]
    narration: Option<String>,
    #[serde(default)]
    party: Option<String>,
    #[serde(default)]
    is_invoice_mode: bool,
    ledger_lines: Vec<BookLedgerLine>,
    #[serde(default)]
    inventory_lines: Vec<BookInventoryLine>,
}

#[derive(Debug, Clone, Deserialize)]
struct BookLedgerLine {
    ledger: String,
    side: String,
    amount: String,
    #[serde(default)]
    bill_allocations: Vec<BookBillAllocation>,
}

#[derive(Debug, Clone, Deserialize)]
struct BookInventoryLine {
    #[serde(default)]
    stock_item: Option<String>,
    #[serde(default)]
    rate: Option<String>,
    #[serde(default)]
    qty: Option<String>,
    #[serde(default)]
    billed_qty: Option<String>,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    godown: Option<String>,
    #[serde(default)]
    accounting_allocations: Vec<BookAccountingAllocation>,
    #[serde(default)]
    batch_allocations: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct BookAccountingAllocation {
    ledger: String,
    amount: String,
}

fn parse_book_value<T: for<'de> Deserialize<'de>>(
    args: &Value,
    inline_key: &str,
    section_key: &str,
) -> Result<T, ToolFailure> {
    if let Some(inline) = args.get(inline_key) {
        return serde_json::from_value(inline.clone())
            .map_err(|_| ToolFailure::from(format!("{inline_key}_invalid")));
    }
    let path = args
        .get("book_path")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::from(format!("{inline_key}_or_book_path_required")))?;
    let text = fs::read_to_string(path)
        .map_err(|_| ToolFailure::from("book_path_unreadable".to_string()))?;
    let whole: Value = serde_json::from_str(&text)
        .map_err(|_| ToolFailure::from("book_path_invalid_json".to_string()))?;
    let section = whole.get(section_key).cloned().unwrap_or(whole);
    serde_json::from_value(section).map_err(|_| ToolFailure::from(format!("{inline_key}_invalid")))
}

// ---------------------------------------------------------------------------
// Master kinds, in the plan's required import order.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MasterKind {
    Unit,
    Godown,
    StockGroup,
    Group,
    Ledger,
    StockItem,
}

impl MasterKind {
    const IMPORT_ORDER: [Self; 6] = [
        Self::Unit,
        Self::Godown,
        Self::StockGroup,
        Self::Group,
        Self::Ledger,
        Self::StockItem,
    ];

    fn tally_type(self) -> &'static str {
        match self {
            Self::Unit => "Unit",
            Self::Godown => "Godown",
            Self::StockGroup => "StockGroup",
            Self::Group => "Group",
            Self::Ledger => "Ledger",
            Self::StockItem => "StockItem",
        }
    }

    fn readback_fetch_fields(self) -> &'static str {
        match self {
            Self::Unit => "NAME,ISSIMPLEUNIT,DECIMALPLACES,GUID,MASTERID,ALTERID",
            Self::Godown | Self::StockGroup | Self::Group => {
                "NAME,PARENT,RESERVEDNAME,GUID,MASTERID,ALTERID"
            }
            Self::Ledger => {
                "NAME,PARENT,OPENINGBALANCE,ISBILLWISEON,PARTYGSTIN,TAXTYPE,GSTDUTYHEAD,\
                GUID,MASTERID,ALTERID"
            }
            Self::StockItem => {
                "NAME,PARENT,BASEUNITS,OPENINGBALANCE,OPENINGRATE,OPENINGVALUE,\
                GSTAPPLICABLE,HSNCODE,GUID,MASTERID,ALTERID"
            }
        }
    }

    fn names(self, masters: &BookMasters) -> Vec<String> {
        match self {
            Self::Unit => masters.units.iter().map(|u| u.name.clone()).collect(),
            Self::Godown => masters.godowns.iter().map(|g| g.name.clone()).collect(),
            Self::StockGroup => masters
                .stock_groups
                .iter()
                .map(|g| g.name.clone())
                .collect(),
            Self::Group => masters.groups.iter().map(|g| g.name.clone()).collect(),
            Self::Ledger => masters.ledgers.iter().map(|l| l.name.clone()).collect(),
            Self::StockItem => masters.stock_items.iter().map(|s| s.name.clone()).collect(),
        }
    }

    fn count(self, masters: &BookMasters) -> usize {
        self.names(masters).len()
    }
}

fn render_master_collection_request(company: &str, kind: MasterKind) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    let object_type = kind.tally_type();
    let name = format!("Bridge Lab Write {object_type}s");
    Ok(format!(
        r#"<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>{name}</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES><TDL><TDLMESSAGE><COLLECTION NAME="{name}" ISMODIFY="No"><TYPE>{object_type}</TYPE><FETCH>{}</FETCH></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>"#,
        xml_escape(company.as_str()),
        kind.readback_fetch_fields()
    ))
}

fn find_readback_row<'a>(
    rows: &'a [BTreeMap<String, String>],
    name: &str,
) -> Option<&'a BTreeMap<String, String>> {
    let key = canonical_master_key(name);
    rows.iter()
        .find(|row| canonical_master_key(row.get("NAME").map(String::as_str).unwrap_or("")) == key)
}

/// The raw control character Tally's reserved-root marker begins with when
/// an XML numeric character reference (`&#4;`) has been decoded to its
/// literal Unicode scalar value -- exactly what this module's own
/// `decoded_agent_reference`-based read-back parsers now produce, since the
/// 2026-09-14 entity-decoding fix (`extract_line_error_texts`'s sibling
/// arms in `agent_lab.rs`/`agent_lab_import.rs`).
const RESERVED_ROOT_RAW_MARKER: char = '\u{4}';

/// The literal, undecoded XML numeric-character-reference text for the same
/// marker -- observed verbatim in `book.json` (built by a separate Python
/// codebase that does not always XML-unescape a captured value before this
/// point; see `build_book.py`'s own widened `is_tally_reserved_root`).
const RESERVED_ROOT_UNDECODED_MARKER: &str = "&#4;";

/// Whether `value` names Tally's reserved top-level root under *any*
/// spelling this codebase has been observed to produce for it (found live,
/// second 2026-09-14 rehearsal: the target's `Profit & Loss A/c` read back
/// with `PARENT` as the raw control character, and `book.json` separately
/// carries the sanitized placeholder, causing a false
/// `lab_master_already_exists` refusal on a ledger that is in fact Tally's
/// own recognised default). Deliberately wider than
/// [`bridge_tally_protocol::is_tally_reserved_root`] itself: that function's
/// narrower definition (only the sanitized placeholder) is a considered,
/// tested choice for the production group-ancestry walk -- an unrecognised
/// raw marker there safely resolves as an absent group, pinned by
/// `group_ancestry.rs`'s own
/// `every_refusal_is_distinguishable_and_none_is_an_answer` test -- and must
/// not be widened for every one of that function's other consumers just to
/// fix this lab-only default-master detection gap (widening a shared
/// function changes behaviour for every consumer silently). This wrapper
/// strips the two extra spellings first and falls through to the shared
/// function for everything else, so the two stay in agreement on whatever
/// the shared function already recognises.
fn is_reserved_root_any_spelling(value: &str) -> bool {
    let trimmed = value.trim();
    let stripped = trimmed
        .strip_prefix(RESERVED_ROOT_RAW_MARKER)
        .or_else(|| trimmed.strip_prefix(RESERVED_ROOT_UNDECODED_MARKER));
    match stripped {
        Some(rest) => rest.trim().eq_ignore_ascii_case("primary"),
        None => is_tally_reserved_root(trimmed),
    }
}

// ---------------------------------------------------------------------------
// Tally default masters -- every new company has these before this tool ever
// runs, so a same-name row is not the Create-overwrite collision §9.4 exists
// to catch. Distinguished from a true collision so a default is (a) never
// sent as a Create, and (b) still diffed against the book and, for Ledger,
// partially Altered if a writable field differs. Any other same-name master
// remains an ordinary refusal.
// ---------------------------------------------------------------------------

/// Where a *default* ledger's `PARENT` is expected to resolve. Compared with
/// [`canonical_master_key`] / [`is_tally_reserved_root`] -- never written:
/// [`render_ledger_xml`] still defaults a missing parent to the plain word
/// `Primary`, never to the reserved-root marker.
enum DefaultLedgerParent {
    /// A named reserved group (matched by the §9.4d fold), e.g. `Cash-in-Hand`.
    ReservedGroup(&'static str),
    /// Tally's reserved primary root itself (`Profit & Loss A/c`'s parent).
    ReservedPrimary,
}

/// Tally auto-creates both of these in every new company: ledger `Cash`
/// under the reserved group `Cash-in-Hand`, and `Profit & Loss A/c` under
/// the reserved primary root. Matched by name only here -- the caller must
/// additionally confirm the *observed* parent before treating a same-name
/// row as this default; see [`is_default_ledger`].
fn default_ledger_parent(name: &str) -> Option<DefaultLedgerParent> {
    let key = canonical_master_key(name);
    if key == canonical_master_key("Cash") {
        Some(DefaultLedgerParent::ReservedGroup("Cash-in-Hand"))
    } else if key == canonical_master_key("Profit & Loss A/c") {
        Some(DefaultLedgerParent::ReservedPrimary)
    } else {
        None
    }
}

/// Whether an *existing* ledger row is Tally's own default for `name`, not a
/// same-name collision. Requires the observed parent to match the default's
/// expected parent too -- a ledger named "Cash" moved under a different
/// group, or a user-created "Profit & Loss A/c" that is not actually under
/// the reserved root, is a true collision and must still fall through to the
/// ordinary refusal, not be silently treated as the default.
fn is_default_ledger(name: &str, observed_parent: &str) -> bool {
    match default_ledger_parent(name) {
        Some(DefaultLedgerParent::ReservedGroup(expected)) => {
            canonical_master_key(observed_parent) == canonical_master_key(expected)
        }
        Some(DefaultLedgerParent::ReservedPrimary) => {
            is_reserved_root_any_spelling(observed_parent)
        }
        None => false,
    }
}

/// Whether an existing Group row is one of Tally's own predefined/reserved
/// groups (every new company has all of them), signalled by a non-empty
/// `RESERVEDNAME` -- the same signal `group_ancestry.rs`'s `GroupIndex`
/// already uses to recognise a predefined group identity.
fn is_default_group(row: &BTreeMap<String, String>) -> bool {
    row.get("RESERVEDNAME")
        .is_some_and(|value| !value.trim().is_empty())
}

/// Whether an existing ledger row's `PARENT` differs from the book (via the
/// §9.4d fold). A hard mismatch -- coordinator instruction, 2026-09-14
/// (second live rehearsal): "parent/name differences still refuse". Split
/// out from the writable-field comparison below because only THIS mismatch
/// makes an existing ledger a true collision; any other difference is a
/// partial-Alter reconcile candidate. `None` when the book does not specify
/// a parent at all (nothing to compare, so nothing to refuse on).
fn ledger_parent_mismatch(book: &BookLedger, row: &BTreeMap<String, String>) -> Option<String> {
    let expected = book.parent.as_deref()?;
    let observed = row.get("PARENT").map(String::as_str).unwrap_or("");
    if canonical_master_key(expected) != canonical_master_key(observed) {
        Some(format!(
            "ledger {}: parent expected {expected:?}, observed {observed:?}",
            book.name
        ))
    } else {
        None
    }
}

/// Which writable field(s) on an existing ledger differ from the book and
/// need a partial `Alter` (Brain trap: `Create` on an existing ledger
/// overwrites its opening balance instead of merging; a partial `Alter`
/// carrying only the changed field(s) is the safe write here). Used for
/// Tally's own default ledgers (Cash/Profit & Loss A/c) and, since
/// 2026-09-14 (coordinator instruction, second live rehearsal), for any
/// other pre-existing ledger whose `PARENT` already matches the book (see
/// `ledger_parent_mismatch` -- a parent difference is never offered here,
/// it must refuse instead).
///
/// Every writable field is offered EXCEPT `GSTDUTYHEAD`: §8.3 of
/// `TALLY_PROTOCOL_REFERENCE.md` measured that field specifically as
/// settable at Create but silently *not* updated at Alter -- "Measured both
/// ways... `ALTERED=1`... and the field stays empty" -- so offering it here
/// would only ever produce a write that reports success but never lands,
/// which the mandatory post-Alter read-back this feeds into would then
/// correctly report as a failed reconcile even when every *other* field
/// genuinely changed. `ISBILLWISEON`/`PARTYGSTIN`/`TAXTYPE` have no
/// equivalent citation establishing Alter-inertness -- an earlier version of
/// this function excluded them anyway, generalising the one measured field
/// to three unmeasured ones (a "private allowance" this module's own
/// discipline exists to catch). They are attempted here; the mandatory
/// read-back this feeds into is what actually proves whether Tally applied
/// them, exactly like every other write in this module.
fn ledger_alter_fields(
    book: &BookLedger,
    row: &BTreeMap<String, String>,
) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    let expected_opening = book.opening_balance.as_deref().unwrap_or("0.00");
    let observed_opening = row.get("OPENINGBALANCE").map(String::as_str).unwrap_or("");
    if !amounts_equal(expected_opening, observed_opening) {
        fields.push(("OPENINGBALANCE", expected_opening.to_string()));
    }
    let expected_billwise = if book.is_billwise_on.unwrap_or(false) {
        "Yes"
    } else {
        "No"
    };
    let observed_billwise = row.get("ISBILLWISEON").map(String::as_str).unwrap_or("");
    if !observed_billwise.eq_ignore_ascii_case(expected_billwise) {
        fields.push(("ISBILLWISEON", expected_billwise.to_string()));
    }
    if let Some(expected) = book.party_gstin.as_deref() {
        let observed = row.get("PARTYGSTIN").map(String::as_str).unwrap_or("");
        if expected != observed {
            fields.push(("PARTYGSTIN", expected.to_string()));
        }
    }
    // Only when the book actually carries a real GST/duty classification
    // (not empty, not Tally's own inert default "Others") AND the ledger is
    // parented under Duties & Taxes -- the same gate `render_ledger_xml`
    // uses at Create.
    let parent = book.parent.as_deref().unwrap_or("Primary");
    if let Some(expected) = book.tax_type.as_deref() {
        if is_real_gst_duty_type(expected) && is_duties_and_taxes_parent(parent) {
            let observed = row.get("TAXTYPE").map(String::as_str).unwrap_or("");
            if expected != observed {
                fields.push(("TAXTYPE", expected.to_string()));
            }
        }
    }
    fields
}

/// Renders a partial `Alter`: only the given fields, never the full ledger
/// (an Alter that omitted a field would leave it unchanged, but resending
/// every field would also silently re-assert ones §8.3 already says are
/// Alter-inert -- so this renders exactly, and only, `fields`).
fn render_ledger_alter_xml(name: &str, fields: &[(&'static str, String)]) -> String {
    let body: String = fields
        .iter()
        .map(|(tag, value)| format!("<{tag}>{}</{tag}>", xml_escape(value)))
        .collect();
    // No `xmlns:UDF`: this partial Alter carries only plain writable fields
    // (currently `OPENINGBALANCE`), never a `UDF:`-namespaced element, so the
    // namespace declaration has nothing to bind to -- see module doc / proven
    // shape (`babul-masters-complete.xml`) for the same convention on Create.
    format!(
        "<TALLYMESSAGE><LEDGER NAME=\"{name}\" ACTION=\"Alter\">{body}</LEDGER></TALLYMESSAGE>",
        name = xml_escape(name)
    )
}

// ---------------------------------------------------------------------------
// Master XML renderers (Create). See module doc: UNVERIFIED for the gateway
// on every kind except the fields §8.3/§9.4a already qualify for Ledger.
// ---------------------------------------------------------------------------

fn render_import_envelope(company: &str, report_name: &str, messages: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ENVELOPE><HEADER><TALLYREQUEST>Import Data</TALLYREQUEST></HEADER><BODY><IMPORTDATA><REQUESTDESC><REPORTNAME>{report_name}</REPORTNAME><STATICVARIABLES><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY></STATICVARIABLES></REQUESTDESC><REQUESTDATA>{messages}</REQUESTDATA></IMPORTDATA></BODY></ENVELOPE>",
        xml_escape(company)
    )
}

// No renderer below declares `xmlns:UDF="TallyUDF"`: none of them emit a
// `UDF:`-namespaced element (that would require a genuine User Defined
// Field, which this book model never carries), so the earlier blanket
// declaration bound to nothing. The proven-good capture
// (`babul-masters-complete.xml`) confirms an ordinary ledger Create carries
// no such attribute at all -- only one incidental `TALLYMESSAGE` in that
// capture (for a UDF-bearing ledger the source system emitted) has it.

fn render_unit_xml(u: &BookUnit) -> String {
    let simple = u.is_simple_unit.as_deref().unwrap_or("Yes");
    let decimals = u.decimal_places.as_deref().unwrap_or("2");
    let name = xml_escape(&u.name);
    format!(
        "<TALLYMESSAGE><UNIT NAME=\"{name}\" ACTION=\"Create\"><NAME>{name}</NAME>\
<ISSIMPLEUNIT>{simple}</ISSIMPLEUNIT><DECIMALPLACES>{decimals}</DECIMALPLACES></UNIT></TALLYMESSAGE>",
        simple = xml_escape(simple),
        decimals = xml_escape(decimals)
    )
}

fn render_parented_xml(tag: &str, item: &BookNamedParent) -> String {
    let parent = item.parent.as_deref().unwrap_or("Primary");
    let name = xml_escape(&item.name);
    format!(
        "<TALLYMESSAGE><{tag} NAME=\"{name}\" ACTION=\"Create\"><NAME>{name}</NAME><PARENT>{parent}</PARENT></{tag}></TALLYMESSAGE>",
        tag = tag,
        parent = xml_escape(parent)
    )
}

fn render_ledger_xml(l: &BookLedger) -> String {
    let parent = l.parent.as_deref().unwrap_or("Primary");
    let name = xml_escape(&l.name);
    // Explicit on every Create, defaulting the unspecified case to `No` --
    // the proven capture never omits it.
    let billwise = format!(
        "<ISBILLWISEON>{}</ISBILLWISEON>",
        if l.is_billwise_on.unwrap_or(false) {
            "Yes"
        } else {
            "No"
        }
    );
    // Only when non-zero: the proven capture's zero-balance ledgers (e.g.
    // "Sales", "Wages and Salary") carry no `OPENINGBALANCE` element at all.
    let opening = l.opening_balance.as_deref().unwrap_or("0.00");
    let opening_balance = if is_zero_amount(opening) {
        String::new()
    } else {
        format!("<OPENINGBALANCE>{}</OPENINGBALANCE>", xml_escape(opening))
    };
    // GST fields are passed through exactly as observed on the source ledger
    // (§8.3: `GSTDUTYHEAD` vocabulary is irregular, `State Tax` not `SGST`);
    // never synthesised. §8.3: settable at Create, silently not at Alter --
    // this renderer only ever builds a Create.
    let gstin = l
        .party_gstin
        .as_deref()
        .map(|g| format!("<PARTYGSTIN>{}</PARTYGSTIN>", xml_escape(g)))
        .unwrap_or_default();
    // Only when the book actually carries a real GST/duty classification
    // (not empty, not Tally's own inert default "Others") AND the ledger is
    // parented under Duties & Taxes -- the 2026-09-14 rehearsal sent
    // `<TAXTYPE>Others</TAXTYPE>` on every ledger, including "HDFC Bank
    // 1649" and "Wages and Salary", which is not a duty head at all.
    let tax_type = l
        .tax_type
        .as_deref()
        .filter(|t| is_real_gst_duty_type(t) && is_duties_and_taxes_parent(parent))
        .map(|t| format!("<TAXTYPE>{}</TAXTYPE>", xml_escape(t)))
        .unwrap_or_default();
    let duty_head = l
        .gst_duty_head
        .as_deref()
        .map(|d| format!("<GSTDUTYHEAD>{}</GSTDUTYHEAD>", xml_escape(d)))
        .unwrap_or_default();
    // Opening bill-wise allocations, when the source captured them, nested
    // under the ledger master the same way an accounting voucher's
    // BILLALLOCATIONS.LIST nests under its ledger entry (§9.4a family) --
    // this specific master-level placement has no live capture in this
    // repository and is UNVERIFIED for the gateway; see module doc.
    let opening_bills = l
        .opening_bill_allocations
        .iter()
        .map(|b| {
            let name = b.name.clone().unwrap_or_default();
            format!(
                "<BILLALLOCATIONS.LIST><NAME>{}</NAME><BILLTYPE>{}</BILLTYPE><AMOUNT>{}</AMOUNT></BILLALLOCATIONS.LIST>",
                xml_escape(&name), xml_escape(&b.bill_type), xml_escape(&b.amount)
            )
        })
        .collect::<String>();
    format!(
        "<TALLYMESSAGE><LEDGER NAME=\"{name}\" ACTION=\"Create\"><NAME>{name}</NAME>\
<PARENT>{parent}</PARENT>{billwise}{opening_balance}{gstin}{tax_type}{duty_head}{opening_bills}</LEDGER></TALLYMESSAGE>",
        parent = xml_escape(parent)
    )
}

fn render_stock_item_xml(s: &BookStockItem) -> String {
    let parent = s.parent.as_deref().unwrap_or("Primary");
    let name = xml_escape(&s.name);
    let base_units = s
        .base_unit
        .as_deref()
        .map(|u| format!("<BASEUNITS>{}</BASEUNITS>", xml_escape(u)))
        .unwrap_or_default();
    // Only when there is a genuinely non-zero opening value -- same
    // zero-suppression convention as the ledger's `OPENINGBALANCE`.
    let opening = match (
        s.opening_qty.as_deref(),
        s.opening_rate.as_deref(),
        s.opening_value.as_deref(),
    ) {
        (Some(qty), Some(rate), Some(value)) if !is_zero_amount(value) => format!(
            "<OPENINGBALANCE>{}</OPENINGBALANCE><OPENINGRATE>{}</OPENINGRATE><OPENINGVALUE>{}</OPENINGVALUE>",
            xml_escape(qty), xml_escape(rate), xml_escape(value)
        ),
        _ => String::new(),
    };
    let gst = s
        .gst_applicable
        .as_deref()
        .map(|g| format!("<GSTAPPLICABLE>{}</GSTAPPLICABLE>", xml_escape(g)))
        .unwrap_or_default();
    let hsn = s
        .hsn_code
        .as_deref()
        .map(|h| format!("<HSNCODE>{}</HSNCODE>", xml_escape(h)))
        .unwrap_or_default();
    format!(
        "<TALLYMESSAGE><STOCKITEM NAME=\"{name}\" ACTION=\"Create\"><NAME>{name}</NAME>\
<PARENT>{parent}</PARENT>{base_units}{opening}{gst}{hsn}</STOCKITEM></TALLYMESSAGE>",
        parent = xml_escape(parent)
    )
}

fn render_master_batch_xml(company: &str, kind: MasterKind, masters: &BookMasters) -> String {
    let messages = match kind {
        MasterKind::Unit => masters
            .units
            .iter()
            .map(render_unit_xml)
            .collect::<String>(),
        MasterKind::Godown => masters
            .godowns
            .iter()
            .map(|g| render_parented_xml("GODOWN", g))
            .collect::<String>(),
        MasterKind::StockGroup => masters
            .stock_groups
            .iter()
            .map(|g| render_parented_xml("STOCKGROUP", g))
            .collect::<String>(),
        MasterKind::Group => masters
            .groups
            .iter()
            .map(|g| render_parented_xml("GROUP", g))
            .collect::<String>(),
        MasterKind::Ledger => masters
            .ledgers
            .iter()
            .map(render_ledger_xml)
            .collect::<String>(),
        MasterKind::StockItem => masters
            .stock_items
            .iter()
            .map(render_stock_item_xml)
            .collect::<String>(),
    };
    render_import_envelope(company, "All Masters", &messages)
}

// ---------------------------------------------------------------------------
// Master read-back diff
// ---------------------------------------------------------------------------

fn diff_unit(u: &BookUnit, row: &BTreeMap<String, String>) -> Vec<String> {
    let mut mismatches = Vec::new();
    if let Some(expected) = u.decimal_places.as_deref() {
        let observed = row.get("DECIMALPLACES").map(String::as_str).unwrap_or("");
        if expected != observed {
            mismatches.push(format!(
                "unit {}: decimal_places expected {expected}, observed {observed}",
                u.name
            ));
        }
    }
    mismatches
}

fn diff_parented(tag: &str, item: &BookNamedParent, row: &BTreeMap<String, String>) -> Vec<String> {
    let mut mismatches = Vec::new();
    if let Some(expected) = item.parent.as_deref() {
        let observed = row.get("PARENT").map(String::as_str).unwrap_or("");
        if canonical_master_key(expected) != canonical_master_key(observed) {
            mismatches.push(format!(
                "{tag} {}: parent expected {expected:?}, observed {observed:?}",
                item.name
            ));
        }
    }
    mismatches
}

fn diff_ledger(l: &BookLedger, row: &BTreeMap<String, String>) -> Vec<String> {
    let mut mismatches = Vec::new();
    if let Some(expected) = l.parent.as_deref() {
        let observed = row.get("PARENT").map(String::as_str).unwrap_or("");
        if canonical_master_key(expected) != canonical_master_key(observed) {
            mismatches.push(format!(
                "ledger {}: parent expected {expected:?}, observed {observed:?}",
                l.name
            ));
        }
    }
    let expected_opening = l.opening_balance.as_deref().unwrap_or("0.00");
    let observed_opening = row.get("OPENINGBALANCE").map(String::as_str).unwrap_or("");
    if !amounts_equal(expected_opening, observed_opening) {
        mismatches.push(format!(
            "ledger {}: opening_balance expected {expected_opening}, observed {observed_opening:?}",
            l.name
        ));
    }
    if let Some(expected) = l.party_gstin.as_deref() {
        let observed = row.get("PARTYGSTIN").map(String::as_str).unwrap_or("");
        if expected != observed {
            mismatches.push(format!(
                "ledger {}: party_gstin expected {expected:?}, observed {observed:?}",
                l.name
            ));
        }
    }
    mismatches
}

fn diff_stock_item(s: &BookStockItem, row: &BTreeMap<String, String>) -> Vec<String> {
    let mut mismatches = Vec::new();
    if let Some(expected) = s.parent.as_deref() {
        let observed = row.get("PARENT").map(String::as_str).unwrap_or("");
        if canonical_master_key(expected) != canonical_master_key(observed) {
            mismatches.push(format!(
                "stock item {}: parent expected {expected:?}, observed {observed:?}",
                s.name
            ));
        }
    }
    if let (Some(qty), Some(observed)) = (s.opening_qty.as_deref(), row.get("OPENINGBALANCE")) {
        if !amounts_equal(qty, observed) {
            mismatches.push(format!(
                "stock item {}: opening_qty expected {qty}, observed {observed}",
                s.name
            ));
        }
    }
    mismatches
}

// ---------------------------------------------------------------------------
// Idempotent-resume precheck (coordinator instruction, 2026-09-14): a
// same-name, non-default master already in the target is not automatically
// a collision. If every field this tool would itself have written already
// matches the book, it is evidence of a prior successful (or partially
// successful) write -- skip it (`already_present_verified`), do not refuse
// and do not re-Create. Any field difference still falls through to the
// ordinary `lab_master_already_exists` refusal.
// ---------------------------------------------------------------------------

/// Ledger-specific comparison for the idempotent-resume precheck: parent,
/// bill-wise flag, opening balance, and GST fields -- every field
/// `render_ledger_xml` would itself have sent, compared exactly as that
/// renderer computes the value, so a genuinely-identical prior write reads
/// back as equal rather than as a false mismatch. Extends `diff_ledger`
/// (parent/opening/GSTIN) rather than replacing it, so the ordinary
/// post-Create read-back check (which never looks at bill-wise/TAXTYPE/
/// GSTDUTYHEAD) is untouched by this addition.
fn ledger_already_present_mismatches(
    l: &BookLedger,
    row: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut mismatches = diff_ledger(l, row);
    let expected_billwise = if l.is_billwise_on.unwrap_or(false) {
        "Yes"
    } else {
        "No"
    };
    let observed_billwise = row.get("ISBILLWISEON").map(String::as_str).unwrap_or("");
    if !observed_billwise.eq_ignore_ascii_case(expected_billwise) {
        mismatches.push(format!(
            "ledger {}: is_billwise_on expected {expected_billwise}, observed {observed_billwise:?}",
            l.name
        ));
    }
    // Only compared when the renderer would actually have sent it (real
    // GST/duty type, parent resolves to Duties & Taxes) -- see
    // `render_ledger_xml`'s identical gate. Tally's own inert default
    // (`Others`) is never a mismatch source here.
    let parent = l.parent.as_deref().unwrap_or("Primary");
    if let Some(expected) = l.tax_type.as_deref() {
        if is_real_gst_duty_type(expected) && is_duties_and_taxes_parent(parent) {
            let observed = row.get("TAXTYPE").map(String::as_str).unwrap_or("");
            if expected != observed {
                mismatches.push(format!(
                    "ledger {}: tax_type expected {expected:?}, observed {observed:?}",
                    l.name
                ));
            }
        }
    }
    if let Some(expected) = l.gst_duty_head.as_deref() {
        let observed = row.get("GSTDUTYHEAD").map(String::as_str).unwrap_or("");
        if expected != observed {
            mismatches.push(format!(
                "ledger {}: gst_duty_head expected {expected:?}, observed {observed:?}",
                l.name
            ));
        }
    }
    mismatches
}

/// Dispatches to the right per-kind comparison for the idempotent-resume
/// precheck. Reuses the same diff functions the post-Create read-back check
/// uses (`diff_unit`/`diff_parented`/`diff_stock_item`) for every kind
/// except Ledger, which needs the wider `ledger_already_present_mismatches`
/// above. An empty result means "safe to skip"; any entry means "still a
/// real collision, refuse".
fn already_present_verified_mismatches(
    kind: MasterKind,
    masters: &BookMasters,
    name: &str,
    row: &BTreeMap<String, String>,
) -> Vec<String> {
    let key = canonical_master_key(name);
    match kind {
        MasterKind::Unit => masters
            .units
            .iter()
            .find(|u| canonical_master_key(&u.name) == key)
            .map(|u| diff_unit(u, row))
            .unwrap_or_default(),
        MasterKind::Godown => masters
            .godowns
            .iter()
            .find(|g| canonical_master_key(&g.name) == key)
            .map(|g| diff_parented("godown", g, row))
            .unwrap_or_default(),
        MasterKind::StockGroup => masters
            .stock_groups
            .iter()
            .find(|g| canonical_master_key(&g.name) == key)
            .map(|g| diff_parented("stock group", g, row))
            .unwrap_or_default(),
        MasterKind::Group => masters
            .groups
            .iter()
            .find(|g| canonical_master_key(&g.name) == key)
            .map(|g| diff_parented("group", g, row))
            .unwrap_or_default(),
        MasterKind::Ledger => masters
            .ledgers
            .iter()
            .find(|l| canonical_master_key(&l.name) == key)
            .map(|l| ledger_already_present_mismatches(l, row))
            .unwrap_or_default(),
        MasterKind::StockItem => masters
            .stock_items
            .iter()
            .find(|s| canonical_master_key(&s.name) == key)
            .map(|s| diff_stock_item(s, row))
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// lab_import_masters
// ---------------------------------------------------------------------------

pub(in crate::agent) async fn lab_import_masters(
    server: &Server,
    args: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let masters: BookMasters = parse_book_value(args, "masters", "masters")?;
    let guid = required_string(args, "company_guid")?;

    let (_company, identity, mut evidence) = admit_lab_target(server).await?;
    if !identity_matches_requested_guid(&identity, guid) {
        return Err(ToolFailure::from("lab_target_company_mismatch".to_string())
            .with_prior_evidence(evidence));
    }

    // ---- Create-overwrite pre-check (§9.4): refuse before any write if the
    // target already carries a same-name master under ANY kind requested --
    // except a Tally *default* (ledger `Cash`/`Profit & Loss A/c`, or any
    // reserved Group), which every new company already has before this tool
    // ever runs and so is never a collision. A default is excluded from the
    // Create batch below and, for Ledger, scheduled for a partial Alter if a
    // writable field differs. Any other same-name master is still refused,
    // *unless* (coordinator instruction, 2026-09-14, second live rehearsal)
    // its only differences from the book are in writable fields (parent
    // matches) -- see `reconcile_ledger_alters` below.
    let mut collisions: Vec<String> = Vec::new();
    let mut default_ledger_alters: Vec<(BookLedger, BTreeMap<String, String>)> = Vec::new();
    // Ordinary (non-default) pre-existing ledgers whose PARENT matches the
    // book but some other writable field does not -- reconciled via the same
    // partial-Alter-then-verify mechanism as `default_ledger_alters` below,
    // merged with it before that mechanism runs. Never populated when the
    // parent itself differs (`ledger_parent_mismatch`), or when book.json
    // names the ledger as the reserved root (excluded above): those are
    // real collisions, not reconcile candidates.
    let mut reconcile_ledger_alters: Vec<(BookLedger, BTreeMap<String, String>)> = Vec::new();
    let mut default_group_keys: BTreeSet<String> = BTreeSet::new();
    // Idempotent resume (coordinator instruction, 2026-09-14): a same-name
    // master already in the target, verified equal to the book on every
    // field this tool would itself write, is not a collision -- see
    // `already_present_verified_mismatches` above. Also covers a Tally
    // default ledger (Cash/Profit & Loss A/c) that already matches: those
    // are classified below, not pushed to `default_ledger_alters` at all.
    let mut already_present_verified: Vec<String> = Vec::new();
    let mut already_present_keys: BTreeSet<(&'static str, String)> = BTreeSet::new();
    for kind in MasterKind::IMPORT_ORDER {
        let requested = kind.names(&masters);
        if requested.is_empty() {
            continue;
        }
        let request = render_master_collection_request(identity.display_name(), kind)
            .map_err(ToolFailure::from)?;
        let (xml, read_evidence) =
            lab_post_read(server, &identity, "lab_import_masters.precheck", request).await?;
        evidence = combine_evidence(evidence.clone(), read_evidence);
        let rows = parse_lab_master_rows(&xml, kind.tally_type())
            .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;
        for name in &requested {
            if kind == MasterKind::Group && is_reserved_root_any_spelling(name) {
                // A requested Group named as Tally's own reserved-primary
                // marker (raw or sanitized `\u{4}`/`\u{fffd}#4;` prefix) is
                // never a real master to create: it *is* the root every
                // company already has. It also never collision-matches by
                // name -- Tally's own row is plainly "Primary", not the
                // marker-decorated form a book may carry -- so without this
                // check it would fall through as "new" and get Created with
                // a garbled name. Refuse explicitly instead.
                collisions.push(format!("{}:{name}:reserved_root", kind.tally_type()));
                continue;
            }
            let Some(existing) = find_readback_row(&rows, name) else {
                continue;
            };
            match kind {
                MasterKind::Ledger => {
                    let observed_parent = existing.get("PARENT").map(String::as_str).unwrap_or("");
                    if is_default_ledger(name, observed_parent) {
                        let book_ledger = masters
                            .ledgers
                            .iter()
                            .find(|l| canonical_master_key(&l.name) == canonical_master_key(name))
                            .expect("name was drawn from kind.names(&masters)")
                            .clone();
                        // A default that already matches the book on every
                        // writable field is `already_present_verified`, not
                        // scheduled for an Alter that would carry no fields.
                        if ledger_alter_fields(&book_ledger, existing).is_empty() {
                            already_present_verified.push(format!("{}:{name}", kind.tally_type()));
                            already_present_keys
                                .insert((kind.tally_type(), canonical_master_key(name)));
                        } else {
                            default_ledger_alters.push((book_ledger, existing.clone()));
                        }
                        continue;
                    }
                }
                MasterKind::Group if is_default_group(existing) => {
                    default_group_keys.insert(canonical_master_key(name));
                    continue;
                }
                _ => {}
            }
            // Not a Tally default -- a genuine same-name master. Before
            // refusing, check whether it already matches the book on every
            // field this tool would itself have written: if so, a prior run
            // already created it (successfully, or up to this point before
            // stopping elsewhere), and resuming must not refuse or re-Create
            // it. Any field difference still falls through to the ordinary
            // refusal below.
            let already_mismatches =
                already_present_verified_mismatches(kind, &masters, name, existing);
            if already_mismatches.is_empty() {
                already_present_verified.push(format!("{}:{name}", kind.tally_type()));
                already_present_keys.insert((kind.tally_type(), canonical_master_key(name)));
                continue;
            }
            // A genuine difference from the book. For Ledger only
            // (coordinator instruction, 2026-09-14): if the difference is
            // confined to writable fields -- the parent itself matches --
            // reconcile it with a partial Alter instead of refusing. A
            // parent difference, or a difference `ledger_alter_fields`
            // deliberately never offers (GSTDUTYHEAD -- see its doc
            // comment), still falls through to the ordinary refusal.
            if kind == MasterKind::Ledger {
                let book_ledger = masters
                    .ledgers
                    .iter()
                    .find(|l| canonical_master_key(&l.name) == canonical_master_key(name))
                    .expect("name was drawn from kind.names(&masters)")
                    .clone();
                if ledger_parent_mismatch(&book_ledger, existing).is_none() {
                    let alter_fields = ledger_alter_fields(&book_ledger, existing);
                    if !alter_fields.is_empty() {
                        reconcile_ledger_alters.push((book_ledger, existing.clone()));
                        continue;
                    }
                    // Parent matches and nothing `ledger_alter_fields` can
                    // reconcile is offered, yet `already_mismatches` was
                    // non-empty -- the only way that happens is a GSTDUTYHEAD
                    // difference (the sole field excluded from that
                    // function, per its own doc comment). Not reconcilable:
                    // fall through to the ordinary refusal below.
                }
            }
            collisions.push(format!("{}:{name}", kind.tally_type()));
        }
    }
    if !collisions.is_empty() {
        persist_lab_precheck_collisions(server, &collisions);
        return Err(ToolFailure::from("lab_master_already_exists".to_string())
            .with_prior_evidence(evidence));
    }

    // Masters actually sent as Create: every requested master minus the
    // defaults just identified above (Creating an existing default would hit
    // the very overwrite trap the pre-check exists to avoid) and minus
    // whatever the idempotent-resume check above already verified present.
    let default_ledger_keys: BTreeSet<String> = default_ledger_alters
        .iter()
        .chain(reconcile_ledger_alters.iter())
        .map(|(ledger, _)| canonical_master_key(&ledger.name))
        .collect();
    let already_present = |kind: MasterKind, name: &str| {
        already_present_keys.contains(&(kind.tally_type(), canonical_master_key(name)))
    };
    let mut creatable = masters.clone();
    creatable
        .units
        .retain(|u| !already_present(MasterKind::Unit, &u.name));
    creatable
        .godowns
        .retain(|g| !already_present(MasterKind::Godown, &g.name));
    creatable
        .stock_groups
        .retain(|g| !already_present(MasterKind::StockGroup, &g.name));
    creatable.groups.retain(|g| {
        !default_group_keys.contains(&canonical_master_key(&g.name))
            && !already_present(MasterKind::Group, &g.name)
    });
    creatable.ledgers.retain(|l| {
        !default_ledger_keys.contains(&canonical_master_key(&l.name))
            && !already_present(MasterKind::Ledger, &l.name)
    });
    creatable
        .stock_items
        .retain(|s| !already_present(MasterKind::StockItem, &s.name));

    let mut batches = Vec::new();
    let mut mismatches: Vec<String> = Vec::new();
    let mut counts = serde_json::Map::new();
    // Per-master report (coordinator instruction, 2026-09-14): every master
    // actually Created in this call, "Kind:Name" -- alongside
    // `already_present_verified`/`altered_verified` below, this is the
    // `created` quarter of "created / already_present_verified /
    // altered_verified / refused".
    let mut created_masters: Vec<String> = Vec::new();

    'kinds: for kind in MasterKind::IMPORT_ORDER {
        let total = kind.count(&creatable);
        if total == 0 {
            continue;
        }
        let mut created = 0usize;
        let mut chunk_start = 0usize;
        while chunk_start < total {
            // Re-admit before every batch, not just once per tool call.
            let (_company, identity, admit_evidence) = admit_lab_target(server).await?;
            evidence = combine_evidence(evidence.clone(), admit_evidence);

            let chunk_masters = chunked_masters(&creatable, kind, chunk_start, MAX_MASTER_BATCH);
            let chunk_len = kind.count(&chunk_masters);
            let xml = render_master_batch_xml(identity.display_name(), kind, &chunk_masters);
            let (response, post_evidence) =
                post_lab_batch(server, &identity, "lab_import_masters.write", xml).await?;
            evidence = combine_evidence(evidence.clone(), post_evidence);
            let outcome = bridge_tally_protocol::parse_import_outcome(&response)
                .map_err(|_| ToolFailure::from("lab_import_response_invalid".to_string()))?;
            let counters = outcome.counters();

            if tally_rejected(counters) {
                // Tally refused the whole batch (e.g. the 2026-09-14
                // rehearsal's CREATED=0 ERRORS=0 EXCEPTIONS=17) -- report
                // that explicitly, with any LINEERROR text, before the
                // mandatory read-back rather than after it: a read-back can
                // only ever say "not found", which does not distinguish a
                // rejected write from one that was never sent.
                let line_errors = extract_line_error_texts(&response);
                batches.push(json!({
                    "kind": kind.tally_type(),
                    "requested": chunk_len,
                    "state": "tally_rejected",
                    "counters": tally_import_counters_json(counters),
                    "line_errors": line_errors,
                    "ok": false,
                }));
                mismatches.push(tally_rejection_message(
                    kind.tally_type(),
                    counters,
                    &line_errors,
                ));
                break 'kinds;
            }

            let clean = counters.is_clean_success_for(chunk_len as u64, 0, 0);

            // Mandatory read-back, regardless of the counters (§9.2: never
            // trust CREATED/ERRORS alone).
            let read_request = render_master_collection_request(identity.display_name(), kind)
                .map_err(ToolFailure::from)?;
            let (read_xml, read_evidence) = lab_post_read(
                server,
                &identity,
                "lab_import_masters.readback",
                read_request,
            )
            .await?;
            evidence = combine_evidence(evidence.clone(), read_evidence);
            let rows = parse_lab_master_rows(&read_xml, kind.tally_type())
                .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;

            let batch_mismatches = readback_mismatches(kind, &chunk_masters, &rows);
            let batch_ok = clean && batch_mismatches.is_empty();
            batches.push(json!({
                "kind": kind.tally_type(),
                "requested": chunk_len,
                "counters_clean": clean,
                "mismatches": batch_mismatches,
                "ok": batch_ok,
            }));
            if !batch_ok {
                mismatches.extend(batch_mismatches);
                break 'kinds; // stop on first mismatch, per the plan
            }
            created_masters.extend(
                kind.names(&chunk_masters)
                    .into_iter()
                    .map(|name| format!("{}:{name}", kind.tally_type())),
            );
            created += chunk_len;
            chunk_start += MAX_MASTER_BATCH;
        }
        counts.insert(kind.tally_type().to_string(), json!(created));
    }

    // ---- Ledger reconcile: partial Alter for every pre-existing ledger --
    // Tally default (Cash/Profit & Loss A/c) or ordinary (coordinator
    // instruction, 2026-09-14) -- whose only differences from the book are
    // in writable fields (Brain trap: Create on an existing ledger
    // overwrites its opening balance; a partial Alter carrying only the
    // changed field(s) is the safe write here). Only runs if nothing above
    // already stopped on a mismatch, and only sends an Alter for ledgers
    // whose book value actually differs from the target -- every entry here
    // was already confirmed non-empty-fields at precheck time (see the loop
    // above), so no further filtering is needed. ----
    let ledger_alter_candidates: Vec<(BookLedger, BTreeMap<String, String>)> =
        default_ledger_alters
            .into_iter()
            .chain(reconcile_ledger_alters)
            .collect();
    let mut altered_verified: Vec<String> = Vec::new();
    if mismatches.is_empty() && !ledger_alter_candidates.is_empty() {
        let to_alter: Vec<(&BookLedger, Vec<(&'static str, String)>)> = ledger_alter_candidates
            .iter()
            .map(|(ledger, row)| (ledger, ledger_alter_fields(ledger, row)))
            .collect();
        let (_company, identity, admit_evidence) = admit_lab_target(server).await?;
        evidence = combine_evidence(evidence.clone(), admit_evidence);
        let messages: String = to_alter
            .iter()
            .map(|(ledger, fields)| render_ledger_alter_xml(&ledger.name, fields))
            .collect();
        let xml = render_import_envelope(identity.display_name(), "All Masters", &messages);
        let (response, post_evidence) =
            post_lab_batch(server, &identity, "lab_import_masters.write.reconcile", xml).await?;
        evidence = combine_evidence(evidence.clone(), post_evidence);
        let outcome = bridge_tally_protocol::parse_import_outcome(&response)
            .map_err(|_| ToolFailure::from("lab_import_response_invalid".to_string()))?;
        let counters = outcome.counters();
        counts.insert("LedgerReconcileAlter".to_string(), json!(to_alter.len()));
        if tally_rejected(counters) {
            let line_errors = extract_line_error_texts(&response);
            batches.push(json!({
                "kind": "LedgerReconcileAlter",
                "requested": to_alter.len(),
                "state": "tally_rejected",
                "counters": tally_import_counters_json(counters),
                "line_errors": line_errors,
                "ok": false,
            }));
            mismatches.push(tally_rejection_message(
                "LedgerReconcileAlter",
                counters,
                &line_errors,
            ));
        } else {
            let clean = counters.is_clean_success_for(0, to_alter.len() as u64, 0);
            batches.push(json!({
                "kind": "LedgerReconcileAlter",
                "requested": to_alter.len(),
                "counters_clean": clean,
                "ok": clean,
            }));
            if !clean {
                mismatches.push(
                    "ledger reconcile alter: import counters not a clean altered-only success"
                        .to_string(),
                );
            }
        }

        // Mandatory read-back over every reconciled ledger, requiring full
        // equality with the book -- not just the fields this batch tried to
        // change: `ledger_already_present_mismatches` is the same "does this
        // now match the book" check the idempotent-resume precheck uses, so
        // "altered_verified" means exactly what "already_present_verified"
        // means, just reached by a write instead of by finding it already
        // so. This is also what actually proves whether Tally applied
        // ISBILLWISEON/PARTYGSTIN/TAXTYPE (§8.3 measured only GSTDUTYHEAD as
        // Alter-inert; this settles the other fields empirically rather
        // than assuming).
        if mismatches.is_empty() {
            let (_company, identity, admit_evidence) = admit_lab_target(server).await?;
            evidence = combine_evidence(evidence.clone(), admit_evidence);
            let read_request =
                render_master_collection_request(identity.display_name(), MasterKind::Ledger)
                    .map_err(ToolFailure::from)?;
            let (read_xml, read_evidence) = lab_post_read(
                server,
                &identity,
                "lab_import_masters.readback.reconcile",
                read_request,
            )
            .await?;
            evidence = combine_evidence(evidence.clone(), read_evidence);
            let rows = parse_lab_master_rows(&read_xml, MasterKind::Ledger.tally_type())
                .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;
            let mut reconcile_mismatches: Vec<String> = Vec::new();
            for (ledger, _) in &ledger_alter_candidates {
                match find_readback_row(&rows, &ledger.name) {
                    None => reconcile_mismatches
                        .push(format!("ledger {} not found on readback", ledger.name)),
                    Some(row) => {
                        let per_ledger = ledger_already_present_mismatches(ledger, row);
                        if per_ledger.is_empty() {
                            altered_verified.push(format!("Ledger:{}", ledger.name));
                        } else {
                            reconcile_mismatches.extend(per_ledger);
                        }
                    }
                }
            }
            let reconcile_ok = reconcile_mismatches.is_empty();
            batches.push(json!({
                "kind": "LedgerReconcileReadback",
                "requested": ledger_alter_candidates.len(),
                "counters_clean": true,
                "mismatches": reconcile_mismatches,
                "ok": reconcile_ok,
            }));
            if !reconcile_ok {
                mismatches.extend(reconcile_mismatches);
            }
        }
    }

    let ok = mismatches.is_empty();
    Ok(ToolOutcome {
        payload: json!({"result": {
            "ok": ok,
            "counts": counts,
            "batches": batches,
            "mismatches": mismatches,
            // Per-master reconcile report (coordinator instruction,
            // 2026-09-14): every requested master resolves to exactly one
            // of these four states -- `created` (this call's own Create
            // batches, "Kind:Name"), `already_present_verified` (existing,
            // matched the book, nothing written), `altered_verified`
            // (existing, a partial Alter reconciled the differing writable
            // field(s), and the mandatory read-back confirmed equality), or
            // refused (an unrecoverable collision -- reported via the
            // `lab_master_already_exists` error path instead, since any
            // such collision stops the whole call before any write; see
            // `persist_lab_precheck_collisions`).
            //
            // Idempotent-resume precheck: same-name masters already present
            // in the target and verified equal to the book, so skipped
            // rather than refused or re-Created. Non-empty even on a run
            // that creates nothing new -- `ok` is still true in that case
            // (an all-already_present_verified masters result is success,
            // not a no-op failure), so a caller resuming after a prior
            // successful write proceeds straight to vouchers.
            "created": created_masters,
            "already_present_verified": already_present_verified,
            "altered_verified": altered_verified,
        }}),
        evidence,
        company_guid: Some(guid.to_string()),
        truncated: false,
    })
}

fn readback_mismatches(
    kind: MasterKind,
    chunk: &BookMasters,
    rows: &[BTreeMap<String, String>],
) -> Vec<String> {
    let mut mismatches = Vec::new();
    match kind {
        MasterKind::Unit => {
            for item in &chunk.units {
                match find_readback_row(rows, &item.name) {
                    None => mismatches.push(format!("unit {} not found on readback", item.name)),
                    Some(row) => mismatches.extend(diff_unit(item, row)),
                }
            }
        }
        MasterKind::Godown => {
            for item in &chunk.godowns {
                match find_readback_row(rows, &item.name) {
                    None => mismatches.push(format!("godown {} not found on readback", item.name)),
                    Some(row) => mismatches.extend(diff_parented("godown", item, row)),
                }
            }
        }
        MasterKind::StockGroup => {
            for item in &chunk.stock_groups {
                match find_readback_row(rows, &item.name) {
                    None => {
                        mismatches.push(format!("stock group {} not found on readback", item.name))
                    }
                    Some(row) => mismatches.extend(diff_parented("stock group", item, row)),
                }
            }
        }
        MasterKind::Group => {
            for item in &chunk.groups {
                match find_readback_row(rows, &item.name) {
                    None => mismatches.push(format!("group {} not found on readback", item.name)),
                    Some(row) => mismatches.extend(diff_parented("group", item, row)),
                }
            }
        }
        MasterKind::Ledger => {
            for item in &chunk.ledgers {
                match find_readback_row(rows, &item.name) {
                    None => mismatches.push(format!("ledger {} not found on readback", item.name)),
                    Some(row) => mismatches.extend(diff_ledger(item, row)),
                }
            }
        }
        MasterKind::StockItem => {
            for item in &chunk.stock_items {
                match find_readback_row(rows, &item.name) {
                    None => {
                        mismatches.push(format!("stock item {} not found on readback", item.name))
                    }
                    Some(row) => mismatches.extend(diff_stock_item(item, row)),
                }
            }
        }
    }
    mismatches
}

fn chunked_masters(
    masters: &BookMasters,
    kind: MasterKind,
    start: usize,
    len: usize,
) -> BookMasters {
    let end_of = |n: usize| (start + len).min(n);
    match kind {
        MasterKind::Unit => BookMasters {
            units: masters.units[start.min(masters.units.len())..end_of(masters.units.len())]
                .to_vec(),
            ..Default::default()
        },
        MasterKind::Godown => BookMasters {
            godowns: masters.godowns
                [start.min(masters.godowns.len())..end_of(masters.godowns.len())]
                .to_vec(),
            ..Default::default()
        },
        MasterKind::StockGroup => BookMasters {
            stock_groups: masters.stock_groups
                [start.min(masters.stock_groups.len())..end_of(masters.stock_groups.len())]
                .to_vec(),
            ..Default::default()
        },
        MasterKind::Group => BookMasters {
            groups: masters.groups[start.min(masters.groups.len())..end_of(masters.groups.len())]
                .to_vec(),
            ..Default::default()
        },
        MasterKind::Ledger => BookMasters {
            ledgers: masters.ledgers
                [start.min(masters.ledgers.len())..end_of(masters.ledgers.len())]
                .to_vec(),
            ..Default::default()
        },
        MasterKind::StockItem => BookMasters {
            stock_items: masters.stock_items
                [start.min(masters.stock_items.len())..end_of(masters.stock_items.len())]
                .to_vec(),
            ..Default::default()
        },
    }
}

// ---------------------------------------------------------------------------
// Explicit Tally-rejection reporting (2026-09-14 rehearsal: 17 ledgers sent,
// Tally answered CREATED=0 ERRORS=0 EXCEPTIONS=17, no mutation). A batch
// whose response reports ERRORS or EXCEPTIONS was rejected outright -- that
// must be reported as such, with whatever LINEERROR text Tally attached,
// instead of proceeding to the mandatory read-back and reporting only
// "not found on readback" (indistinguishable from a request that was never
// sent at all).
// ---------------------------------------------------------------------------

/// Best-effort extraction of every `<LINEERROR>` element's text from a raw
/// import response. Deliberately separate from
/// `bridge_tally_protocol::ParsedImportEvidence`, which redacts this text by
/// design (retaining only a sha256 digest) for persisted evidence -- this is
/// a one-shot diagnostic surfaced directly in the tool's own JSON result, not
/// persisted evidence, so the raw text is exactly what a caller needs to act
/// on a rejection.
fn extract_line_error_texts(xml: &str) -> Vec<String> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut errors = Vec::new();
    let mut in_line_error = false;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event))
                if event.name().as_ref().eq_ignore_ascii_case(b"LINEERROR") =>
            {
                in_line_error = true;
            }
            Ok(quick_xml::events::Event::End(event))
                if event.name().as_ref().eq_ignore_ascii_case(b"LINEERROR") =>
            {
                in_line_error = false;
            }
            Ok(quick_xml::events::Event::Text(text)) if in_line_error => {
                if let Ok(value) = decoded_agent_text(text) {
                    let trimmed = value.trim();
                    if !trimmed.is_empty() {
                        errors.push(trimmed.to_string());
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    errors
}

fn tally_import_counters_json(counters: &bridge_tally_protocol::TallyImportResult) -> Value {
    json!({
        "created": counters.created,
        "altered": counters.altered,
        "deleted": counters.deleted,
        "ignored": counters.ignored,
        "errors": counters.errors,
        "cancelled": counters.cancelled,
        "exceptions": counters.exceptions,
    })
}

fn tally_rejection_message(
    label: &str,
    counters: &bridge_tally_protocol::TallyImportResult,
    line_errors: &[String],
) -> String {
    let suffix = if line_errors.is_empty() {
        String::new()
    } else {
        format!(" LINEERROR: {}", line_errors.join("; "))
    };
    format!(
        "{label} rejected by Tally: CREATED={} ALTERED={} ERRORS={} EXCEPTIONS={}{suffix}",
        counters.created, counters.altered, counters.errors, counters.exceptions
    )
}

/// Whether the response counters signal an outright rejection: any `ERRORS`
/// or `EXCEPTIONS` -- the exact shape the 2026-09-14 rehearsal produced
/// (`CREATED=0 ERRORS=0 EXCEPTIONS=17`). Checked ahead of, and independently
/// of, `is_clean_success_for`'s exact-count comparison so a rejection is
/// reported as `tally_rejected` rather than folded into an ordinary
/// mismatch.
fn tally_rejected(counters: &bridge_tally_protocol::TallyImportResult) -> bool {
    counters.errors > 0 || counters.exceptions > 0
}

fn persist_lab_precheck_collisions(server: &Server, collisions: &[String]) {
    if let Ok(dir) = lab_evidence_dir(server) {
        let record = json!({
            "tool": "lab_import_masters.precheck",
            "at": Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            "collisions": collisions,
        });
        let _ = append_egress_line(
            &dir.join("lab-precheck-collisions.jsonl"),
            &record.to_string(),
        );
    }
}

// ---------------------------------------------------------------------------
// Voucher rendering
// ---------------------------------------------------------------------------

const BANK_SHAPE_TYPES: &[&str] = &["Payment", "Receipt", "Contra"];

fn is_bank_shape(voucher_type: &str) -> bool {
    BANK_SHAPE_TYPES.contains(&voucher_type)
}

/// `None` for Contra (moves between two of the company's own accounts, so it
/// has no counterparty leg -- §9.13).
fn bank_party_side(voucher_type: &str) -> Option<&'static str> {
    match voucher_type {
        "Payment" => Some("Dr"),
        "Receipt" => Some("Cr"),
        _ => None,
    }
}

fn signed_wire_amount(side: &str, amount: &str) -> String {
    if side == "Dr" {
        format!("-{amount}")
    } else {
        amount.to_string()
    }
}

fn render_bill_allocations_xml(side: &str, allocations: &[BookBillAllocation]) -> String {
    allocations
        .iter()
        .map(|a| {
            let name = a.name.clone().unwrap_or_default();
            format!(
                "<BILLALLOCATIONS.LIST><NAME>{}</NAME><BILLTYPE>{}</BILLTYPE><AMOUNT>{}</AMOUNT></BILLALLOCATIONS.LIST>",
                xml_escape(&name),
                xml_escape(&a.bill_type),
                signed_wire_amount(side, &a.amount)
            )
        })
        .collect()
}

fn render_ledger_entry_xml(tag: &str, line: &BookLedgerLine) -> String {
    format!(
        "<{tag}><LEDGERNAME>{}</LEDGERNAME><ISDEEMEDPOSITIVE>{}</ISDEEMEDPOSITIVE><AMOUNT>{}</AMOUNT>{}</{tag}>",
        xml_escape(&line.ledger),
        if line.side == "Dr" { "Yes" } else { "No" },
        signed_wire_amount(&line.side, &line.amount),
        render_bill_allocations_xml(&line.side, &line.bill_allocations),
    )
}

fn narration_with_marker(narration: Option<&str>, attribution_id: Uuid) -> String {
    format!(
        "<NARRATION>{}</NARRATION>",
        xml_escape(
            format!(
                "{} [BRIDGE-LAB:{attribution_id}]",
                narration.unwrap_or("").trim()
            )
            .trim()
        )
    )
}

/// Journal, Payment/Receipt/Contra, and any accounting-mode (non-invoice)
/// Sales/Purchase/Credit-Note/Debit-Note -- every one of them renders with
/// `ALLLEDGERENTRIES.LIST` (the element every non-invoice write in
/// `TALLY_PROTOCOL_REFERENCE.md` uses). Bank-shape types additionally carry
/// `EFFECTIVEDATE` and `PARTYLEDGERNAME` and are sorted debit-first, per
/// §9.13; a Journal and an accounting-mode Sales keep the book's own line
/// order and carry neither, matching the observed accounting-mode Sales wire
/// shape (no `PARTYLEDGERNAME` element).
fn render_accounting_voucher_xml(
    voucher: &BookVoucher,
    remote_id: Uuid,
    attribution_id: Uuid,
) -> Result<String, String> {
    let date = normalized_date(&voucher.date)?;
    let bank = is_bank_shape(&voucher.voucher_type);
    let mut lines: Vec<&BookLedgerLine> = voucher.ledger_lines.iter().collect();
    if bank {
        lines.sort_by_key(|line| if line.side == "Dr" { 0 } else { 1 });
    }
    let entries = lines
        .iter()
        .map(|line| render_ledger_entry_xml("ALLLEDGERENTRIES.LIST", line))
        .collect::<String>();
    let effective_date = if bank {
        format!("<EFFECTIVEDATE>{date}</EFFECTIVEDATE>")
    } else {
        String::new()
    };
    let party = bank_party_side(&voucher.voucher_type)
        .and_then(|side| lines.iter().find(|line| line.side == side))
        .map(|line| {
            format!(
                "<PARTYLEDGERNAME>{}</PARTYLEDGERNAME>",
                xml_escape(&line.ledger)
            )
        })
        .unwrap_or_default();
    let voucher_number = voucher
        .voucher_number
        .as_deref()
        .map(|v| format!("<VOUCHERNUMBER>{}</VOUCHERNUMBER>", xml_escape(v)))
        .unwrap_or_default();
    let narration = narration_with_marker(voucher.narration.as_deref(), attribution_id);
    let vt = xml_escape(&voucher.voucher_type);
    Ok(format!(
        "<TALLYMESSAGE xmlns:UDF=\"TallyUDF\"><VOUCHER REMOTEID=\"{remote_id}\" VCHTYPE=\"{vt}\" ACTION=\"Create\" OBJVIEW=\"Accounting Voucher View\"><DATE>{date}</DATE>{effective_date}<VOUCHERTYPENAME>{vt}</VOUCHERTYPENAME>{party}{voucher_number}{narration}{entries}</VOUCHER></TALLYMESSAGE>"
    ))
}

fn render_inventory_entry_xml(line: &BookInventoryLine) -> Result<String, String> {
    let stock_item = line
        .stock_item
        .as_deref()
        .ok_or_else(|| "lab_inventory_stock_item_required".to_string())?;
    let amount = line
        .amount
        .as_deref()
        .ok_or_else(|| "lab_inventory_amount_required".to_string())?;
    // §9.12a note 2: a service line carries an amount and no quantity --
    // omit RATE/ACTUALQTY/BILLEDQTY entirely rather than send empties.
    let rate = line
        .rate
        .as_deref()
        .map(|r| format!("<RATE>{}</RATE>", xml_escape(r)))
        .unwrap_or_default();
    let qty = line
        .qty
        .as_deref()
        .map(|qty| {
            let billed = line.billed_qty.as_deref().unwrap_or(qty);
            format!(
                "<ACTUALQTY>{}</ACTUALQTY><BILLEDQTY>{}</BILLEDQTY>",
                xml_escape(qty),
                xml_escape(billed)
            )
        })
        .unwrap_or_default();
    let godown = line
        .godown
        .as_deref()
        .map(|g| format!("<GODOWNNAME>{}</GODOWNNAME>", xml_escape(g)))
        .unwrap_or_default();
    // §9.12a note 1: each inventory line carried its own
    // ACCOUNTINGALLOCATIONS.LIST naming the sales/purchase ledger -- the
    // *observed* working shape, not a proven requirement for every line.
    let allocations = line
        .accounting_allocations
        .iter()
        .map(|a| {
            format!(
                "<ACCOUNTINGALLOCATIONS.LIST><LEDGERNAME>{}</LEDGERNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE><AMOUNT>{}</AMOUNT></ACCOUNTINGALLOCATIONS.LIST>",
                xml_escape(&a.ledger), xml_escape(&a.amount)
            )
        })
        .collect::<String>();
    // Batch/godown allocations, in the shape `lab_read_inventory` (Phase 3.2,
    // agent_lab.rs) already reads back: BATCHNAME/GODOWNNAME/ACTUALQTY/
    // BILLEDQTY/AMOUNT. UNVERIFIED for import -- see module doc.
    let batches = line
        .batch_allocations
        .iter()
        .map(|b| {
            let field = |key: &str| b.get(key).and_then(Value::as_str).unwrap_or("");
            format!(
                "<BATCHALLOCATIONS.LIST><BATCHNAME>{}</BATCHNAME><GODOWNNAME>{}</GODOWNNAME><ACTUALQTY>{}</ACTUALQTY><BILLEDQTY>{}</BILLEDQTY><AMOUNT>{}</AMOUNT></BATCHALLOCATIONS.LIST>",
                xml_escape(field("batch")),
                xml_escape(field("godown")),
                xml_escape(field("actual_qty")),
                xml_escape(field("billed_qty")),
                xml_escape(field("amount")),
            )
        })
        .collect::<String>();
    Ok(format!(
        "<ALLINVENTORYENTRIES.LIST><STOCKITEMNAME>{}</STOCKITEMNAME><ISDEEMEDPOSITIVE>No</ISDEEMEDPOSITIVE>{rate}{qty}<AMOUNT>{}</AMOUNT>{godown}{allocations}{batches}</ALLINVENTORYENTRIES.LIST>",
        xml_escape(stock_item),
        xml_escape(amount)
    ))
}

/// Sales/Purchase/Credit-Note/Debit-Note carrying `inventory_lines`. Follows
/// §9.12a's "the shape that works" byte-for-byte: `LEDGERENTRIES.LIST` (never
/// `ALLLEDGERENTRIES.LIST` -- §9.12's TRAP), `ISINVOICE=Yes`,
/// `OBJVIEW="Invoice Voucher View"`. **UNVERIFIED for the gateway** -- see
/// module doc; §9.12a itself is a UI-import capture, not a gateway one, and
/// only for Sales.
fn render_invoice_voucher_xml(
    voucher: &BookVoucher,
    remote_id: Uuid,
    attribution_id: Uuid,
) -> Result<String, String> {
    let date = normalized_date(&voucher.date)?;
    let party = voucher
        .party
        .as_deref()
        .ok_or_else(|| "lab_invoice_party_required".to_string())?;
    let voucher_number = voucher
        .voucher_number
        .as_deref()
        .map(|v| format!("<VOUCHERNUMBER>{}</VOUCHERNUMBER>", xml_escape(v)))
        .unwrap_or_default();
    let narration = narration_with_marker(voucher.narration.as_deref(), attribution_id);
    let ledger_entries = voucher
        .ledger_lines
        .iter()
        .map(|line| render_ledger_entry_xml("LEDGERENTRIES.LIST", line))
        .collect::<String>();
    let inventory_entries = voucher
        .inventory_lines
        .iter()
        .map(render_inventory_entry_xml)
        .collect::<Result<Vec<_>, _>>()?
        .concat();
    let vt = xml_escape(&voucher.voucher_type);
    let party_escaped = xml_escape(party);
    Ok(format!(
        "<TALLYMESSAGE xmlns:UDF=\"TallyUDF\"><VOUCHER REMOTEID=\"{remote_id}\" VCHTYPE=\"{vt}\" ACTION=\"Create\" OBJVIEW=\"Invoice Voucher View\"><DATE>{date}</DATE><EFFECTIVEDATE>{date}</EFFECTIVEDATE><VOUCHERTYPENAME>{vt}</VOUCHERTYPENAME>{voucher_number}<PARTYLEDGERNAME>{party_escaped}</PARTYLEDGERNAME><BASICBASEPARTYNAME>{party_escaped}</BASICBASEPARTYNAME><PERSISTEDVIEW>Invoice Voucher View</PERSISTEDVIEW><ISINVOICE>Yes</ISINVOICE>{narration}{ledger_entries}{inventory_entries}</VOUCHER></TALLYMESSAGE>"
    ))
}

fn render_voucher_message(
    voucher: &BookVoucher,
    remote_id: Uuid,
    attribution_id: Uuid,
) -> Result<String, String> {
    if voucher.is_invoice_mode {
        render_invoice_voucher_xml(voucher, remote_id, attribution_id)
    } else {
        render_accounting_voucher_xml(voucher, remote_id, attribution_id)
    }
}

fn render_voucher_batch_xml(
    company: &str,
    vouchers: &[BookVoucher],
    attribution_ids: &[Uuid],
) -> Result<String, String> {
    let messages = vouchers
        .iter()
        .zip(attribution_ids)
        .map(|(voucher, id)| render_voucher_message(voucher, Uuid::new_v4(), *id))
        .collect::<Result<Vec<_>, _>>()?
        .concat();
    Ok(render_import_envelope(company, "Vouchers", &messages))
}

// ---------------------------------------------------------------------------
// Voucher read-back / resume pre-check
// ---------------------------------------------------------------------------

const ACCOUNTING_VOUCHER_FETCH: &str =
    "DATE,VOUCHERNUMBER,VOUCHERTYPENAME,NARRATION,PARTYLEDGERNAME,\
GUID,ISCANCELLED,ALLLEDGERENTRIES.LEDGERNAME,ALLLEDGERENTRIES.AMOUNT,\
ALLLEDGERENTRIES.ISDEEMEDPOSITIVE,ALLLEDGERENTRIES.BILLALLOCATIONS.NAME,\
ALLLEDGERENTRIES.BILLALLOCATIONS.BILLTYPE,ALLLEDGERENTRIES.BILLALLOCATIONS.AMOUNT";

fn render_voucher_window_request(company: &str, from: &str, to: &str) -> Result<String, String> {
    let company = ValidatedCompanyName::new(company.to_string())
        .map_err(|_| "company_name_invalid".to_string())?;
    Ok(format!(
        "<ENVELOPE><HEADER><VERSION>1</VERSION><TALLYREQUEST>Export</TALLYREQUEST><TYPE>Collection</TYPE><ID>Bridge Lab Voucher Readback</ID></HEADER><BODY><DESC><STATICVARIABLES><SVEXPORTFORMAT>$$SysName:XML</SVEXPORTFORMAT><SVCURRENTCOMPANY>{}</SVCURRENTCOMPANY><SVFROMDATE TYPE=\"Date\">{from}</SVFROMDATE><SVTODATE TYPE=\"Date\">{to}</SVTODATE></STATICVARIABLES><TDL><TDLMESSAGE><SYSTEM TYPE=\"Formulae\" NAME=\"BridgeLabWindow\">$Date &gt;= $$Date:\"{from}\" AND $Date &lt;= $$Date:\"{to}\"</SYSTEM><COLLECTION NAME=\"Bridge Lab Voucher Readback\" ISMODIFY=\"No\"><TYPE>Voucher</TYPE><FETCH>{}</FETCH><FILTERS>BridgeLabWindow</FILTERS></COLLECTION></TDLMESSAGE></TDL></DESC></BODY></ENVELOPE>",
        xml_escape(company.as_str()), ACCOUNTING_VOUCHER_FETCH
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedVoucher {
    date: String,
    voucher_number: Option<String>,
    voucher_type: Option<String>,
    narration: Option<String>,
    is_cancelled: bool,
    ledger_entries: Vec<(String, String, String)>, // (ledger, is_deemed_positive, amount)
}

/// Parses `ALLLEDGERENTRIES.LIST` per voucher, mirroring the nested-list
/// approach `parse_lab_inventory_vouchers` (agent_lab.rs) already established
/// for `ALLINVENTORYENTRIES.LIST` -- generalised here to the ledger-entry
/// list every non-invoice write in this document uses.
fn parse_voucher_readback_nested(xml: &str) -> Result<Vec<ObservedVoucher>, String> {
    validate_agent_envelope(xml)?;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut path: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    let mut voucher: Option<BTreeMap<String, String>> = None;
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut entries: Vec<(String, String, String)> = Vec::new();
    let mut current_tag = String::new();
    const VOUCHER_PREFIX: [&str; 5] = ["ENVELOPE", "BODY", "DATA", "COLLECTION", "VOUCHER"];
    const ENTRY_PREFIX: [&str; 6] = [
        "ENVELOPE",
        "BODY",
        "DATA",
        "COLLECTION",
        "VOUCHER",
        "ALLLEDGERENTRIES.LIST",
    ];
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if path.len() == 4 && path == ["ENVELOPE", "BODY", "DATA", "COLLECTION"] {
                    if tag != "VOUCHER" {
                        return Err("agent_read_protocol_invalid".to_string());
                    }
                    voucher = Some(BTreeMap::new());
                    entries.clear();
                } else if path.as_slice() == VOUCHER_PREFIX && tag == "ALLLEDGERENTRIES.LIST" {
                    entry = Some(BTreeMap::new());
                }
                path.push(tag.clone());
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                let value = decoded_agent_text(text)?;
                let parent = &path[..path.len().saturating_sub(1)];
                if parent == ENTRY_PREFIX {
                    if let Some(row) = entry.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                } else if parent == VOUCHER_PREFIX {
                    if let Some(row) = voucher.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                }
            }
            // quick_xml delivers a general entity/character reference
            // (`&amp;`, `&#4;`, ...) as its own `GeneralRef` event, separate
            // from the surrounding `Text` events -- NOT inline within them.
            // Without this arm the reference is silently dropped by the
            // catch-all below, which is exactly the 2026-09-14 rehearsal
            // read-back bug: "Duties &amp; Taxes" arrived as two Text events
            // ("Duties " and " Taxes") with the entity between them
            // discarded, producing the false mismatch "Duties  Taxes". Every
            // other native-collection parser in this crate
            // (agent_voucher_parse.rs, agent_change_parse.rs,
            // agent_company_checkpoint.rs, source_draft_xml.rs) already
            // handles this event; the lab read-back path did not.
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                let value = decoded_agent_reference(reference)?;
                let parent = &path[..path.len().saturating_sub(1)];
                if parent == ENTRY_PREFIX {
                    if let Some(row) = entry.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                } else if parent == VOUCHER_PREFIX {
                    if let Some(row) = voucher.as_mut() {
                        append_agent_text(row, &current_tag, value);
                    }
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if end == "ALLLEDGERENTRIES.LIST" && path.as_slice() == ENTRY_PREFIX {
                    if let Some(row) = entry.take() {
                        entries.push((
                            row.get("LEDGERNAME").cloned().unwrap_or_default(),
                            row.get("ISDEEMEDPOSITIVE").cloned().unwrap_or_default(),
                            row.get("AMOUNT").cloned().unwrap_or_default(),
                        ));
                    }
                }
                if end == "VOUCHER" && path.as_slice() == VOUCHER_PREFIX {
                    if let Some(row) = voucher.take() {
                        rows.push(ObservedVoucher {
                            date: row.get("DATE").cloned().unwrap_or_default(),
                            voucher_number: row.get("VOUCHERNUMBER").cloned(),
                            voucher_type: row.get("VOUCHERTYPENAME").cloned(),
                            narration: row.get("NARRATION").cloned(),
                            is_cancelled: row.get("ISCANCELLED").map(String::as_str) == Some("Yes"),
                            ledger_entries: entries.clone(),
                        });
                        entries.clear();
                    }
                }
                if path.pop().as_deref() != Some(end.as_str()) {
                    return Err("agent_read_protocol_invalid".to_string());
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
        }
    }
    Ok(rows)
}

fn narration_marker(narration: Option<&str>) -> Option<String> {
    let text = narration?;
    let start = text.rfind("[BRIDGE-LAB:")?;
    let rest = &text[start + "[BRIDGE-LAB:".len()..];
    let end = rest.find(']')?;
    Some(rest[..end].to_string())
}

/// A voucher counts as already-posted-and-verified for a resume pre-check
/// only on (type, date, total-debit-amount, narration marker OR voucher
/// number) -- content alone (date/ledger/amount) is not an attribution key,
/// per §9.3: a book with a recurring same-day payment can already contain a
/// voucher with that tuple. Matching on the marker this module stamps into
/// every write closes that hole the same way the production path's
/// narration tag does.
fn voucher_already_verified(expected: &BookVoucher, observed: &[ObservedVoucher]) -> bool {
    // Compare in Tally's own wire form: `normalized_date` accepts the book
    // model's date (which may or may not already be YYYYMMDD) and the
    // observed row is always already in that form; the ledger amount must be
    // the *signed* wire amount (§9.13's Dr-negative convention), since
    // book.json stores an unsigned magnitude plus a side.
    let expected_date = normalized_date(&expected.date).unwrap_or_else(|_| expected.date.clone());
    observed.iter().any(|row| {
        if row.is_cancelled {
            return false;
        }
        if row.voucher_type.as_deref() != Some(expected.voucher_type.as_str()) {
            return false;
        }
        if row.date != expected_date {
            return false;
        }
        let number_matches =
            expected.voucher_number.is_some() && row.voucher_number == expected.voucher_number;
        let marker_matches = narration_marker(row.narration.as_deref()).is_some()
            && narration_marker(row.narration.as_deref())
                == narration_marker(expected.narration.as_deref());
        if !number_matches && !marker_matches {
            return false;
        }
        expected.ledger_lines.iter().all(|line| {
            let expected_signed = signed_wire_amount(&line.side, &line.amount);
            row.ledger_entries.iter().any(|(ledger, is_dr, amount)| {
                ledger == &line.ledger
                    && ((line.side == "Dr") == (is_dr == "Yes"))
                    && amounts_equal(amount, &expected_signed)
            })
        })
    })
}

// ---------------------------------------------------------------------------
// lab_import_vouchers
// ---------------------------------------------------------------------------

pub(in crate::agent) async fn lab_import_vouchers(
    server: &Server,
    args: &Value,
) -> Result<ToolOutcome, ToolFailure> {
    let mut vouchers: Vec<BookVoucher> = parse_book_value(args, "vouchers", "vouchers")?;
    vouchers.sort_by(|a, b| {
        (a.date.as_str(), a.voucher_number.as_deref().unwrap_or(""))
            .cmp(&(b.date.as_str(), b.voucher_number.as_deref().unwrap_or("")))
    });
    let guid = required_string(args, "company_guid")?;
    let start_batch = arg_usize(args, "start_batch", 0)?;

    let (_company, identity, mut evidence) = admit_lab_target(server).await?;
    if !identity_matches_requested_guid(&identity, guid) {
        return Err(ToolFailure::from("lab_target_company_mismatch".to_string())
            .with_prior_evidence(evidence));
    }

    let mut batch_reports = Vec::new();
    let batch_count = vouchers.len().div_ceil(MAX_VOUCHER_BATCH);
    let mut stopped_at: Option<usize> = None;

    for batch_index in start_batch..batch_count {
        let (_company, identity, admit_evidence) = admit_lab_target(server).await?;
        evidence = combine_evidence(evidence.clone(), admit_evidence);

        let start = batch_index * MAX_VOUCHER_BATCH;
        let end = (start + MAX_VOUCHER_BATCH).min(vouchers.len());
        let batch = &vouchers[start..end];
        let from = batch.first().map(|v| v.date.clone()).unwrap_or_default();
        let to = batch.last().map(|v| v.date.clone()).unwrap_or_default();

        // Resume pre-check (§9.3/§12a's discipline): never blind-retry. Read
        // the window this batch would occupy and check every voucher against
        // it before sending anything.
        let probe_request = render_voucher_window_request(identity.display_name(), &from, &to)
            .map_err(ToolFailure::from)?;
        let (probe_xml, probe_evidence) = lab_post_read(
            server,
            &identity,
            "lab_import_vouchers.precheck",
            probe_request,
        )
        .await?;
        evidence = combine_evidence(evidence.clone(), probe_evidence);
        let observed = parse_voucher_readback_nested(&probe_xml)
            .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;

        let source_guids: Vec<&str> = batch.iter().map(|v| v.source_guid.as_str()).collect();
        let verified_count = batch
            .iter()
            .filter(|v| voucher_already_verified(v, &observed))
            .count();
        if verified_count == batch.len() {
            batch_reports.push(json!({
                "batch": batch_index, "count": batch.len(), "state": "already_verified", "posted": false,
                "source_guids": source_guids,
            }));
            continue;
        }
        if verified_count > 0 {
            // Partial match on an uncertain prior attempt: stop rather than
            // guess which subset is safe to resend. Returned immediately
            // below, so this batch never reaches `stopped_at`'s summary use.
            batch_reports.push(json!({
                "batch": batch_index, "count": batch.len(), "state": "partially_verified_uncertain",
                "verified": verified_count, "posted": false,
            }));
            return Err(
                ToolFailure::from("lab_batch_partially_verified_uncertain".to_string())
                    .with_prior_evidence(evidence),
            );
        }

        let attribution_ids: Vec<Uuid> = batch.iter().map(|_| Uuid::new_v4()).collect();
        let xml = render_voucher_batch_xml(identity.display_name(), batch, &attribution_ids)
            .map_err(ToolFailure::from)?;
        let (response, post_evidence) =
            post_lab_batch(server, &identity, "lab_import_vouchers.write", xml).await?;
        evidence = combine_evidence(evidence.clone(), post_evidence);
        let outcome = bridge_tally_protocol::parse_import_outcome(&response)
            .map_err(|_| ToolFailure::from("lab_import_response_invalid".to_string()))?;
        let counters = outcome.counters();

        if tally_rejected(counters) {
            // Same explicit-rejection reporting as lab_import_masters: a
            // read-back after this can only ever say "not found", so report
            // the rejection itself, with any LINEERROR text, and stop.
            let line_errors = extract_line_error_texts(&response);
            batch_reports.push(json!({
                "batch": batch_index,
                "count": batch.len(),
                "state": "tally_rejected",
                "counters": tally_import_counters_json(counters),
                "line_errors": line_errors,
                "posted": true,
                "source_guids": source_guids,
            }));
            stopped_at = Some(batch_index);
            break;
        }

        let clean = counters.is_clean_success_for(batch.len() as u64, 0, 0);

        // Mandatory read-back.
        let (readback_xml, readback_evidence) = lab_post_read(
            server,
            &identity,
            "lab_import_vouchers.readback",
            render_voucher_window_request(identity.display_name(), &from, &to)
                .map_err(ToolFailure::from)?,
        )
        .await?;
        evidence = combine_evidence(evidence.clone(), readback_evidence);
        let readback = parse_voucher_readback_nested(&readback_xml)
            .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;
        let posted_count = batch
            .iter()
            .filter(|v| voucher_already_verified(v, &readback))
            .count();
        let batch_ok = clean && posted_count == batch.len();

        batch_reports.push(json!({
            "batch": batch_index,
            "count": batch.len(),
            "counters_clean": clean,
            "verified_on_readback": posted_count,
            "state": if batch_ok { "posted_verified" } else { "readback_mismatch" },
            "posted": true,
            "source_guids": source_guids,
        }));
        if !batch_ok {
            stopped_at = Some(batch_index);
            break;
        }
    }

    let ok = stopped_at.is_none();
    Ok(ToolOutcome {
        payload: json!({"result": {
            "ok": ok,
            "total_vouchers": vouchers.len(),
            "batch_count": batch_count,
            "batches": batch_reports,
            "stopped_at_batch": stopped_at,
        }}),
        evidence,
        company_guid: Some(guid.to_string()),
        truncated: false,
    })
}

// ---------------------------------------------------------------------------
// Shared write-path helper
// ---------------------------------------------------------------------------

async fn post_lab_batch(
    server: &Server,
    identity: &VerifiedCompanyIdentity,
    tool: &str,
    xml: String,
) -> Result<(String, Evidence), ToolFailure> {
    let _ = identity;
    let (body, runtime_evidence) = server
        .runtime
        .post_lab_import(server.tally_config(), xml.clone())
        .await
        .map_err(|error| ToolFailure::from_runtime("lab_import_post_failed", error))?;
    let evidence = evidence_from_runtime_read(runtime_evidence);
    persist_lab_exchange(server, tool, &xml, &body)
        .map_err(|code| ToolFailure::from(code).with_prior_evidence(evidence.clone()))?;
    Ok((body, evidence))
}

#[cfg(test)]
#[path = "agent_lab_import_tests.rs"]
mod tests;
