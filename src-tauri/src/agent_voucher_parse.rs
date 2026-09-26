//! Voucher parse for the local MCP adapter.
use super::*;

#[path = "agent_voucher_scalars.rs"]
mod scalars;
pub(super) use scalars::*;

/// Shares the native collection boundary used by the protocol crate: CMPINFO
/// contains identically named counters outside BODY/DATA/COLLECTION.
#[derive(Default)]
pub(super) struct NativeCollectionScope {
    path: Vec<String>,
    collection_seen: bool,
    repeated_collection: bool,
}

impl NativeCollectionScope {
    pub(super) fn collection(&self) -> bool {
        self.path == ["ENVELOPE", "BODY", "DATA", "COLLECTION"]
    }
    pub(super) fn row(&self, row: &str) -> bool {
        self.path.len() == 5
            && self.path[..4] == ["ENVELOPE", "BODY", "DATA", "COLLECTION"]
            && self.path[4] == row
    }
    pub(super) fn field(&self, row: &str) -> bool {
        self.path.len() == 6
            && self.path[..4] == ["ENVELOPE", "BODY", "DATA", "COLLECTION"]
            && self.path[4] == row
    }
    pub(super) fn child(&self, row: &str, child: &str) -> bool {
        self.field(row) && self.path[5] == child
    }
    pub(super) fn entry_field(&self) -> bool {
        self.path.len() == 7
            && self.path[..5] == ["ENVELOPE", "BODY", "DATA", "COLLECTION", "VOUCHER"]
            && self.path[5] == "ALLLEDGERENTRIES.LIST"
    }
    pub(super) fn bill_allocation(&self) -> bool {
        self.path.len() == 7
            && self.path[..5] == ["ENVELOPE", "BODY", "DATA", "COLLECTION", "VOUCHER"]
            && self.path[5] == "ALLLEDGERENTRIES.LIST"
            && self.path[6] == "BILLALLOCATIONS.LIST"
    }
    pub(super) fn bill_allocation_field(&self) -> bool {
        self.path.len() == 8
            && self.path[..5] == ["ENVELOPE", "BODY", "DATA", "COLLECTION", "VOUCHER"]
            && self.path[5] == "ALLLEDGERENTRIES.LIST"
            && self.path[6] == "BILLALLOCATIONS.LIST"
    }
    pub(super) fn voucher_scalar(&self) -> bool {
        self.path.last().is_some_and(|field| {
            (self.field("VOUCHER") && is_voucher_scalar(field))
                || (self.entry_field() && is_voucher_entry_scalar(field))
                || (self.bill_allocation_field() && is_voucher_bill_allocation_scalar(field))
        })
    }
    pub(super) fn start(&mut self, name: String) {
        if self.path == ["ENVELOPE", "BODY", "DATA"] && name == "COLLECTION" {
            self.repeated_collection |= self.collection_seen;
            self.collection_seen = true;
        }
        self.path.push(name);
    }
    pub(super) fn end(&mut self, name: &str) -> Result<(), String> {
        if self.path.pop().as_deref() != Some(name) {
            return Err("agent_read_protocol_invalid".to_string());
        }
        Ok(())
    }
    pub(super) fn finish(&self) -> Result<(), String> {
        if self.collection_seen && !self.repeated_collection && self.path.is_empty() {
            Ok(())
        } else {
            Err("agent_read_protocol_invalid".to_string())
        }
    }
}

pub(super) fn parse_agent_rows(xml: &str, company_guid: &str) -> Result<Vec<Value>, String> {
    parse_agent_rows_with_accounting_state(xml, false, company_guid)
}

pub(super) fn parse_agent_changed_rows(
    xml: &str,
    company_guid: &str,
) -> Result<Vec<Value>, String> {
    parse_agent_rows_with_accounting_state(xml, true, company_guid)
}

/// Changed rows plus `effective_date`, which only import verification reads.
/// The public voucher tools do not fetch it and their rows do not carry it.
pub(super) fn parse_import_verification_rows(
    xml: &str,
    company_guid: &str,
) -> Result<Vec<Value>, String> {
    refused_composites(parse_voucher_rows(
        xml,
        true,
        true,
        company_guid,
        CompositePolicy::Refuse,
    )?)
}

pub(super) fn parse_agent_rows_with_accounting_state(
    xml: &str,
    require_change_identity: bool,
    company_guid: &str,
) -> Result<Vec<Value>, String> {
    refused_composites(parse_voucher_rows(
        xml,
        require_change_identity,
        false,
        company_guid,
        CompositePolicy::Refuse,
    )?)
}

/// The `vouchers` rows (#674): a voucher whose amount Tally stored as a
/// foreign-currency composite is withheld whole, as a [`VoucherRow::Withheld`],
/// instead of failing the window. Every other check still applies to it, and
/// any other bad amount still fails the window. Nothing that sums, matches or
/// verifies amounts reads through here.
pub(super) fn parse_agent_rows_withholding(
    xml: &str,
    company_guid: &str,
) -> Result<Vec<VoucherRow>, String> {
    Ok(
        parse_voucher_rows(xml, false, false, company_guid, CompositePolicy::Withhold)?
            .into_iter()
            .map(|(row, withheld)| {
                if withheld {
                    VoucherRow::Withheld(WithheldVoucher::from_parsed(row))
                } else {
                    VoucherRow::Read(row)
                }
            })
            .collect(),
    )
}

/// What an amount that is not a plain decimal does to its voucher.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CompositePolicy {
    /// The window fails, as it always has: every caller that uses amounts.
    Refuse,
    /// A Tally composite withholds its voucher; anything else still fails.
    Withhold,
}

impl CompositePolicy {
    fn withholds(self, amount: &str) -> bool {
        self == Self::Withhold
            && bridge_tally_protocol::currency_composite::is_currency_composite(amount)
    }
}

/// Under [`CompositePolicy::Refuse`] no row is ever withheld: a composite has
/// already failed the parse with its amount code.
fn refused_composites(rows: Vec<(Value, bool)>) -> Result<Vec<Value>, String> {
    rows.into_iter()
        .map(|(row, withheld)| {
            if withheld {
                Err("agent_read_protocol_invalid".to_string())
            } else {
                Ok(row)
            }
        })
        .collect()
}

/// A `vouchers` row: read in full, or withheld because an amount was stored as
/// a foreign-currency composite (#674). A withheld voucher carries no amount,
/// so no path that sums, matches or verifies amounts can take one.
#[derive(Debug, Clone)]
pub(super) enum VoucherRow {
    Read(Value),
    Withheld(WithheldVoucher),
}

/// A withheld voucher: its identity, type fields and entry ledgers, never an
/// amount. The field is private; [`Self::filter_view`] is the only way out.
#[derive(Debug, Clone)]
pub(super) struct WithheldVoucher(Value);

/// Why a voucher is withheld; the only cause so far.
pub(super) const WITHHELD_FOREIGN_CURRENCY: &str = "foreign_currency_amount_unparsed";

/// Marks a row in [`WithheldVoucher::filter_view`]; the parser never emits it.
pub(super) const WITHHELD_MARKER: &str = "withheld_cause";

impl WithheldVoucher {
    fn from_parsed(mut row: Value) -> Self {
        // Keep each entry's ledger (the ledger filter reads it) and drop
        // everything that carries an amount.
        let ledgers = row["amounts"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| json!({"ledger": entry["ledger"]}))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        row["amounts"] = Value::Array(ledgers);
        row[WITHHELD_MARKER] = json!(WITHHELD_FOREIGN_CURRENCY);
        Self(row)
    }

    /// The row as the date, ledger and voucher-type filters read it: identity,
    /// type fields and entry ledgers, marked withheld, with no amount.
    pub(super) fn filter_view(&self) -> Value {
        self.0.clone()
    }
}

impl super::voucher_window::WindowRow for VoucherRow {
    fn window_date(&self) -> Option<&str> {
        self.row().window_date()
    }
    fn window_alter_id(&self) -> Option<u64> {
        self.row().window_alter_id()
    }
    fn window_guid(&self) -> Option<&str> {
        self.row().window_guid()
    }
    fn window_master_id(&self) -> Result<Option<u64>, String> {
        self.row().window_master_id()
    }
}

impl VoucherRow {
    /// The row the date, ledger and voucher-type filters read: a read voucher
    /// whole, a withheld one as its [`WithheldVoucher::filter_view`].
    pub(super) fn into_filter_row(self) -> Value {
        match self {
            Self::Read(row) => row,
            Self::Withheld(withheld) => withheld.filter_view(),
        }
    }

    fn row(&self) -> &Value {
        match self {
            Self::Read(row) => row,
            Self::Withheld(WithheldVoucher(row)) => row,
        }
    }
}

fn parse_voucher_rows(
    xml: &str,
    require_change_identity: bool,
    include_effective_date: bool,
    company_guid: &str,
    composites: CompositePolicy,
) -> Result<Vec<(Value, bool)>, String> {
    // Tally's collection XML varies by release; use a deliberately conservative
    // extractor and never infer a missing field. Malformed rows fail before
    // optional selectors can hide them as an apparently complete empty result.
    let marked = mark_agent_xml(xml);
    let xml = marked.as_ref();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut rows = Vec::new();
    let mut identities = VoucherSourceIdentities::default();
    validate_agent_envelope(xml)?;
    let mut current: Option<BTreeMap<String, String>> = None;
    let mut entry: Option<BTreeMap<String, String>> = None;
    let mut allocation: Option<BTreeMap<String, String>> = None;
    let mut entries = Vec::<Value>::new();
    let mut allocations = Vec::<Value>::new();
    let mut current_tag = String::new();
    let mut scope = NativeCollectionScope::default();
    // Set by a composite amount under `CompositePolicy::Withhold`, per voucher.
    let mut withheld = false;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(event)) => {
                let tag = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.voucher_scalar() || (scope.collection() && tag != "VOUCHER") {
                    return Err("agent_read_protocol_invalid".into());
                }
                if tag == "VOUCHER" && scope.collection() {
                    let mut row = BTreeMap::new();
                    for attribute in event.attributes() {
                        let attribute =
                            attribute.map_err(|_| "agent_read_protocol_invalid".to_string())?;
                        if attribute.key.as_ref().eq_ignore_ascii_case(b"REMOTEID") {
                            claim_agent_scalar(&mut row, "REMOTEID")?;
                            row.insert(
                                "REMOTEID".into(),
                                attribute
                                    .decoded_and_normalized_value(
                                        quick_xml::XmlVersion::Implicit1_0,
                                        reader.decoder(),
                                    )
                                    .map_err(|_| "agent_read_protocol_invalid".to_string())?
                                    .into_owned(),
                            );
                        }
                    }
                    current = Some(row);
                    entries.clear();
                    withheld = false;
                }
                if tag == "ALLLEDGERENTRIES.LIST" && scope.row("VOUCHER") {
                    entry = Some(BTreeMap::new());
                    allocations.clear();
                }
                if tag == "BILLALLOCATIONS.LIST" && scope.child("VOUCHER", "ALLLEDGERENTRIES.LIST")
                {
                    allocation = Some(BTreeMap::new());
                }
                claim_voucher_scalar(
                    &scope,
                    &tag,
                    current.as_mut(),
                    entry.as_mut(),
                    allocation.as_mut(),
                )?;
                scope.start(tag.clone());
                if scope.repeated_collection {
                    return Err("agent_read_protocol_invalid".into());
                }
                current_tag = tag;
            }
            Ok(quick_xml::events::Event::Text(text)) => {
                if let Some(row) = allocation
                    .as_mut()
                    .filter(|_| scope.bill_allocation_field())
                {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                } else if let Some(row) = entry.as_mut().filter(|_| scope.entry_field()) {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                } else if let Some(row) = current.as_mut().filter(|_| scope.field("VOUCHER")) {
                    append_agent_text(row, &current_tag, decoded_agent_text(text)?);
                }
            }
            Ok(quick_xml::events::Event::CData(text)) => {
                let value = text
                    .decode()
                    .map_err(|_| "agent_read_protocol_invalid".to_string())?
                    .into_owned();
                if let Some(row) = allocation
                    .as_mut()
                    .filter(|_| scope.bill_allocation_field())
                {
                    append_agent_text(row, &current_tag, value);
                } else if let Some(row) = entry.as_mut().filter(|_| scope.entry_field()) {
                    append_agent_text(row, &current_tag, value);
                } else if let Some(row) = current.as_mut().filter(|_| scope.field("VOUCHER")) {
                    append_agent_text(row, &current_tag, value);
                }
            }
            Ok(quick_xml::events::Event::GeneralRef(reference)) => {
                if let Some(row) = allocation
                    .as_mut()
                    .filter(|_| scope.bill_allocation_field())
                {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                } else if let Some(row) = entry.as_mut().filter(|_| scope.entry_field()) {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                } else if let Some(row) = current.as_mut().filter(|_| scope.field("VOUCHER")) {
                    append_agent_text(row, &current_tag, decoded_agent_reference(reference)?);
                }
            }
            Ok(quick_xml::events::Event::End(event)) => {
                let end = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.bill_allocation() {
                    if let Some(allocation_row) = allocation.take().filter(|row| !row.is_empty()) {
                        // Tally emits an amount-only container for a ledger entry with no
                        // typed allocation. It is NOT empty, so the filter above admits
                        // it, and requiring BILLTYPE unconditionally aborted the entire
                        // read over a row that carries nothing to record. The rule for
                        // which untyped rows are placeholders is shared with the
                        // voucher-scan boundary rather than restated here.
                        //
                        // A skipped row must still fall through to `scope.end` below, so
                        // this is an `if let` and not a `continue`: continuing the event
                        // loop would leave the scope stack unbalanced and mis-attribute
                        // every element after it.
                        let bill_type = allocation_row
                            .get("BILLTYPE")
                            .filter(|value| !value.trim().is_empty());
                        if let Some(bill_type) = bill_type {
                            let amount = allocation_row
                                .get("AMOUNT")
                                .filter(|value| !value.trim().is_empty())
                                .ok_or_else(|| "bill_allocation_field_missing".to_string())?;
                            if bridge_tally_core::ExactDecimal::parse(amount.clone()).is_err() {
                                if composites.withholds(amount) {
                                    withheld = true;
                                } else {
                                    return Err("bill_allocation_amount_invalid".to_string());
                                }
                            }
                            let name = allocation_row
                                .get("NAME")
                                .filter(|value| !value.trim().is_empty());
                            let reference = if bill_type.trim() == "On Account" {
                                // On Account is the one bill type with no bill identity.
                                // Keep that absence explicit instead of representing it
                                // as an empty name.
                                //
                                // A NAME here is a contradiction, not a value to drop.
                                // Silently discarding it loses a supplier reference that
                                // a malformed response -- or a request-shape regression
                                // -- is trying to tell us about. The typed boundary in
                                // outstandings/parser.rs refuses the same state as
                                // `bill_reference_forbidden`; refuse it here too.
                                if name.is_some() {
                                    return Err("bill_reference_forbidden".to_string());
                                }
                                json!({"kind": "on_account"})
                            } else {
                                let name = name.ok_or_else(|| {
                                    "bill_allocation_field_missing".to_string()
                                })?;
                                json!({"kind": "named", "name": name})
                            };
                            allocations.push(json!({
                                "reference": reference,
                                "bill_type": bill_type,
                                "amount": amount,
                            }));
                        } else if !bridge_tally_protocol::outstandings_shared::bill_allocation_without_type_is_placeholder(
                            allocation_row.get("NAME").map(String::as_str),
                        ) {
                            // A named bill with no type is partially populated, not a
                            // placeholder. Guessing the type would invent an allocation.
                            return Err("bill_allocation_field_missing".to_string());
                        }
                    }
                } else if scope.child("VOUCHER", "ALLLEDGERENTRIES.LIST") {
                    if allocation.is_some() {
                        return Err("agent_read_protocol_invalid".to_string());
                    }
                    // A Stock Journal moves inventory and has no accounting effect, so
                    // Tally returns an ALLLEDGERENTRIES.LIST with no children at all.
                    // That is an empty entry list, not a malformed entry, and refusing
                    // it lost the whole window over a voucher that is exactly right.
                    // The same distinction is already drawn one level down for
                    // BILLALLOCATIONS.LIST. A row carrying *some* of the three fields
                    // stays refused below: that is what a truncated response or a
                    // request-shape regression looks like, and it must stay loud.
                    if let (Some(_), Some(entry_row)) =
                        (current.as_mut(), entry.take().filter(|row| !row.is_empty()))
                    {
                        let ledger = entry_row
                            .get("LEDGERNAME")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let amount = entry_row
                            .get("AMOUNT")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let parsed_amount =
                            match bridge_tally_core::ExactDecimal::parse(amount.clone()) {
                                Ok(parsed) => Some(parsed),
                                Err(_) if composites.withholds(amount) => {
                                    withheld = true;
                                    None
                                }
                                Err(_) => return Err("voucher_amount_invalid".to_string()),
                            };
                        let polarity = entry_row
                            .get("ISDEEMEDPOSITIVE")
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "agent_read_protocol_invalid".to_string())?;
                        let is_deemed_positive = required_tally_bool(Some(polarity))?;
                        // AMOUNT carries the sign; ISDEEMEDPOSITIVE records the column the
                        // entry was made in, and the two legitimately disagree. A rounding
                        // ledger is flagged with the voucher's default side while the
                        // rounding itself goes either way. Refusing the disagreement threw
                        // away the whole window over entries whose arithmetic is right --
                        // summing AMOUNT alone reproduces Tally's own closing balance for
                        // the ledger, and honouring the flag does not.
                        //
                        // The disagreement is still worth saying out loud, so it is carried
                        // on the entry rather than dropped. It is only present when the two
                        // observations conflict, so an ordinary entry is unchanged.
                        let mut parsed_entry = json!({
                            "ledger": ledger,
                            "amount": amount,
                            "is_deemed_positive": if is_deemed_positive { "Yes" } else { "No" },
                            "bill_allocations": std::mem::take(&mut allocations),
                        });
                        if parsed_amount.as_ref().is_some_and(|parsed| {
                            !tally_entry_polarity_agrees(parsed, is_deemed_positive)
                        }) {
                            parsed_entry["polarity_disagrees_with_amount"] = Value::Bool(true);
                        }
                        entries.push(parsed_entry);
                    }
                } else if scope.row("VOUCHER") {
                    if let Some(row) = current.take() {
                        if ["DATE", "VOUCHERTYPENAME"].iter().any(|field| {
                            row.get(*field).is_none_or(|value| value.trim().is_empty())
                        }) {
                            return Err(if require_change_identity {
                                "change_row_core_field_invalid"
                            } else {
                                "agent_read_protocol_invalid"
                            }
                            .to_string());
                        }
                        if !row.get("GUID").is_some_and(|guid| {
                            bridge_tally_protocol::master_guid_belongs_to_company(
                                guid,
                                company_guid,
                            )
                        }) {
                            return Err("voucher_company_identity_invalid".to_string());
                        }
                        bridge_tally_core::TallyDate::parse(row["DATE"].clone())
                            .map_err(|_| "voucher_date_invalid".to_string())?;
                        let master_id = parse_optional_tally_u64(
                            row.get("MASTERID").map(String::as_str),
                            "voucher_master_id_invalid",
                        )?;
                        identities.admit(row.get("GUID").map(String::as_str), master_id)?;
                        let amounts = std::mem::take(&mut entries);
                        let mut parsed = json!({"date": row.get("DATE"), "voucher_number": row.get("VOUCHERNUMBER"), "voucher_type": row.get("VOUCHERTYPENAME"), "party": row.get("PARTYLEDGERNAME"), "narration": row.get("NARRATION"), "guid": row.get("GUID"), "alter_id": parse_optional_tally_alter_id(row.get("ALTERID").map(String::as_str))?, "master_id": row.get("MASTERID"), "amounts": amounts});
                        // Present only when the read asked Tally to resolve the
                        // row's voucher type (bridge#625).
                        if let Some(resolved) = resolve_row_voucher_type(&row, company_guid)? {
                            parsed["voucher_type_guid"] = json!(resolved.guid);
                            parsed["voucher_type_reserved_name"] = json!(resolved.reserved_name);
                            parsed["voucher_class"] =
                                json!(resolved.class.map(ReservedVoucherClass::name));
                        }
                        if require_change_identity {
                            parsed["remote_id"] = json!(row.get("REMOTEID"));
                        }
                        if include_effective_date {
                            // An empty element is treated like an absent one: not observed.
                            // A present value must be a date, as DATE must: a malformed
                            // one refuses the read rather than passing as unobserved.
                            if let Some(effective) = row
                                .get("EFFECTIVEDATE")
                                .map(|value| value.trim())
                                .filter(|value| !value.is_empty())
                            {
                                bridge_tally_core::TallyDate::parse(effective.to_string())
                                    .map_err(|_| "voucher_effective_date_invalid".to_string())?;
                                parsed["effective_date"] = json!(effective);
                            }
                        }
                        parsed["cancelled"] =
                            Value::Bool(required_tally_bool(row.get("ISCANCELLED"))?);
                        parsed["optional"] =
                            Value::Bool(required_tally_bool(row.get("ISOPTIONAL"))?);
                        // Unlike ISCANCELLED/ISOPTIONAL, a real capture has shown Tally
                        // omitting ISPOSTDATED entirely rather than asserting "No" on every
                        // voucher. required_tally_bool would refuse the whole read on that
                        // shape; that is right for a tag known to always be present, but
                        // wrong here; it would turn "Tally did not say" into a hard failure
                        // instead of a legible unknown. So this field is optional like
                        // EFFECTIVEDATE above: an absent or empty element is not observed and
                        // the key is omitted, never invented as `false`. A present value must
                        // be Yes/No: an unrecognised value refuses the read rather than
                        // guessing, exactly as a malformed EFFECTIVEDATE does.
                        if let Some(post_dated) = row
                            .get("ISPOSTDATED")
                            .map(|value| value.trim())
                            .filter(|value| !value.is_empty())
                        {
                            parsed["post_dated"] = Value::Bool(match post_dated {
                                "Yes" => true,
                                "No" => false,
                                _ => return Err("voucher_post_dated_invalid".to_string()),
                            });
                        }
                        // REFERENCE is TYPE="String", the same shape NARRATION uses, but
                        // unlike NARRATION a blank reference carries no information worth
                        // returning: protocol reference §8.2c observed it empty on most
                        // vouchers and populated with a manual reference number on the
                        // rest. An empty or absent element is not observed and the key is
                        // omitted, never emitted as "".
                        if let Some(reference) = row
                            .get("REFERENCE")
                            .map(|value| value.trim())
                            .filter(|value| !value.is_empty())
                        {
                            parsed["reference"] = json!(reference);
                        }
                        // ISINVOICE follows ISPOSTDATED's optional-boolean idiom exactly:
                        // absent or empty is "Tally did not say" and the key is omitted,
                        // never invented as false; a present value must be Yes/No. §8.2c's
                        // capture asserted it (with one of those two values) on every
                        // voucher observed, unlike ISPOSTDATED, but that is one instance on
                        // one release and is not grounds to promote it to
                        // required_tally_bool. §8.2c also notes ISINVOICE is the one
                        // logical here Tally emits without a TYPE="Logical" attribute;
                        // parsing here matches on tag name only, so that is not visible to
                        // this code and changes nothing about it.
                        if let Some(is_invoice) = row
                            .get("ISINVOICE")
                            .map(|value| value.trim())
                            .filter(|value| !value.is_empty())
                        {
                            parsed["is_invoice"] = Value::Bool(match is_invoice {
                                "Yes" => true,
                                "No" => false,
                                _ => return Err("voucher_is_invoice_invalid".to_string()),
                            });
                        }
                        // PARTYGSTIN is TYPE="String", handled like REFERENCE above: an
                        // empty or absent element is not observed. §8.2c's capture proved
                        // the tag round-trips through this FETCH list but never observed a
                        // populated value (this synthetic company's parties carry no
                        // GSTIN) -- presence is verified, population is not, and this
                        // parsing makes no claim about what a populated value looks like.
                        if let Some(party_gstin) = row
                            .get("PARTYGSTIN")
                            .map(|value| value.trim())
                            .filter(|value| !value.is_empty())
                        {
                            parsed["party_gstin"] = json!(party_gstin);
                        }
                        rows.push((parsed, withheld));
                    }
                }
                scope.end(&end)?;
                current_tag.clear();
            }
            Ok(quick_xml::events::Event::Empty(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).to_ascii_uppercase();
                if scope.voucher_scalar() {
                    return Err("agent_read_protocol_invalid".into());
                }
                if scope.collection() {
                    return Err("agent_read_protocol_invalid".to_string());
                }
                claim_voucher_scalar(
                    &scope,
                    &name,
                    current.as_mut(),
                    entry.as_mut(),
                    allocation.as_mut(),
                )?;
                scope.start(name.clone());
                if scope.repeated_collection {
                    return Err("agent_read_protocol_invalid".into());
                }
                scope.end(&name)?;
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(_) => return Err("agent_read_protocol_invalid".to_string()),
            _ => {}
        }
    }
    scope.finish()?;
    Ok(rows)
}

pub(super) fn append_agent_text(row: &mut BTreeMap<String, String>, tag: &str, value: String) {
    row.entry(tag.to_string()).or_default().push_str(&value);
}

fn claim_voucher_scalar(
    scope: &NativeCollectionScope,
    field: &str,
    current: Option<&mut BTreeMap<String, String>>,
    entry: Option<&mut BTreeMap<String, String>>,
    allocation: Option<&mut BTreeMap<String, String>>,
) -> Result<(), String> {
    let row = if scope.row("VOUCHER")
        && (is_voucher_scalar(field) || is_voucher_type_class_scalar(field))
    {
        current
    } else if scope.child("VOUCHER", "ALLLEDGERENTRIES.LIST") && is_voucher_entry_scalar(field) {
        entry
    } else if scope.bill_allocation() && is_voucher_bill_allocation_scalar(field) {
        allocation
    } else {
        None
    };
    if let Some(row) = row {
        claim_agent_scalar(row, field)?;
    }
    Ok(())
}

/// The one rule every agent-facing parser applies to a Tally response before
/// reading it: forbidden numeric references are marked, then the XML reader
/// unescapes what is left (`docs/tally/TALLY_PROTOCOL_REFERENCE.md` §1.1(d)).
/// `&#4; Primary` therefore reads as `U+FFFD#4; Primary` here exactly as it
/// does in `bridge-tally-protocol`'s native parsers, never as a raw U+0004,
/// and [`decoded_agent_reference`] only sees references the rule leaves
/// alone. A literal U+FFFD followed by `#`, digits and `;` reads as
/// `U+FFFD#65533;` and the rest, which keeps the rewrite reversible.
pub(super) fn mark_agent_xml(xml: &str) -> std::borrow::Cow<'_, str> {
    bridge_tally_protocol::mark_forbidden_numeric_references(xml)
}

pub(super) fn decoded_agent_text(text: quick_xml::events::BytesText<'_>) -> Result<String, String> {
    let decoded = text
        .decode()
        .map_err(|_| "agent_read_protocol_invalid".to_string())?;
    quick_xml::escape::unescape(&decoded)
        .map(|value| value.into_owned())
        .map_err(|_| "agent_read_protocol_invalid".to_string())
}

pub(super) fn decoded_agent_reference(
    reference: quick_xml::events::BytesRef<'_>,
) -> Result<String, String> {
    let reference = reference
        .decode()
        .map_err(|_| "agent_read_protocol_invalid".to_string())?;
    quick_xml::escape::unescape(&format!("&{reference};"))
        .map(|value| value.into_owned())
        .map_err(|_| "agent_read_protocol_invalid".to_string())
}

pub(super) fn required_tally_bool(value: Option<&String>) -> Result<bool, String> {
    match value.map(String::as_str).map(str::trim) {
        Some("Yes") => Ok(true),
        Some("No") => Ok(false),
        _ => Err("voucher_accounting_state_not_observed".to_string()),
    }
}

/// Synthetic mutation for tests (#674): the captured three-voucher window
/// (`native-three-vouchers`) with the first `count` vouchers' amounts replaced
/// by the composites a live read of the several-currency book captured
/// (`vouchers-forex-composite-20260915`): each negative amount by the captured
/// negative composite, each positive one by the positive composite.
#[cfg(test)]
pub(super) fn window_with_composite_vouchers(count: usize) -> String {
    let decode = |bytes: &[u8]| {
        String::from_utf16(
            &bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap()
    };
    let forex = decode(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/vouchers-forex-composite-20260915.utf16le.xml"
    ));
    let amounts = |xml: &str| -> Vec<String> {
        xml.split("<AMOUNT")
            .skip(1)
            .filter_map(|tail| Some(tail[tail.find('>')? + 1..tail.find("</AMOUNT>")?].to_string()))
            .collect()
    };
    let composites = amounts(&forex);
    let negative = composites
        .iter()
        .find(|value| value.starts_with('-'))
        .unwrap()
        .clone();
    let positive = composites
        .iter()
        .find(|value| !value.starts_with('-'))
        .unwrap()
        .clone();
    let mut window = decode(include_bytes!(
        "../crates/bridge-tally-protocol/tests/fixtures/agent/native-three-vouchers.utf16le.xml"
    ));
    // The first `</VOUCHER>` closes CMPINFO's counter, not a voucher.
    let mut from = 0;
    for _ in 0..count {
        let start = from + window[from..].find("<VOUCHER ").unwrap();
        let end = start + window[start..].find("</VOUCHER>").unwrap();
        let mut voucher = window[start..end].to_string();
        for plain in amounts(&voucher) {
            let at = voucher.find(&format!(">{plain}</AMOUNT>")).unwrap();
            let composite = if plain.starts_with('-') {
                &negative
            } else {
                &positive
            };
            voucher.replace_range(at + 1..at + 1 + plain.len(), composite);
        }
        window.replace_range(start..end, &voucher);
        from = start + voucher.len();
    }
    window
}

#[cfg(test)]
#[path = "agent_voucher_parse_tests.rs"]
mod boundary_tests;
