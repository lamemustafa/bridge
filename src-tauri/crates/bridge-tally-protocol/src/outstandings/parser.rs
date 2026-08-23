use bridge_tally_primitives::{ExactDecimal, TallyDate};
use std::collections::{BTreeMap, BTreeSet};

use crate::outstandings_shared::{OutstandingsError, PinnedCompany};
use crate::tolerant_xml::sanitize_invalid_numeric_references;

use super::{
    wire::{
        Envelope, Header, LedgerCollection, RawBillAllocation, RawLedgerEntry, RawVoucher,
        RawWitnessVoucher, VoucherCollection, WitnessVoucherCollection,
    },
    AlterIdRange, BillAllocation, BillReferenceKind, CreditPeriod, DateWindow, LedgerEntry,
    LedgerOpeningCoverage, MoneyValue, Voucher, VoucherAlterId, WitnessVoucher,
};

pub(super) struct ParsedSegment {
    pub(super) vouchers: Vec<Voucher>,
    pub(super) raw_row_count: usize,
}

pub(super) struct ParsedWitnessSegment {
    pub(super) vouchers: Vec<WitnessVoucher>,
    pub(super) raw_row_count: usize,
}

/// Detect bill-wise OPENING balances on ledger masters.
///
/// **Known limitation — offsetting opening bills are not detected.** This works
/// from the ledger-level `OPENINGBALANCE`, so a bill-wise ledger holding, say, a
/// 100 debit opening bill and a 100 credit opening bill nets to zero and is
/// classified as fully covered, while both bills exist with no voucher. The scan
/// can then complete while omitting both receivable and payable exposure.
///
/// Closing this needs evidence about the opening *allocations* rather than the
/// ledger balance, which is the same read that reconciling opening bills would
/// require — Unit B work, tracked in issue #108. Recorded here so the cheap
/// detector is not mistaken for a complete one.
///
/// A ledger with bill-wise tracking on and a non-zero opening balance carries
/// bills that exist without any voucher. A voucher-only scan cannot observe
/// them, so their presence must block a `Complete` claim rather than silently
/// under-report.
pub fn parse_ledger_opening_coverage(
    xml: &str,
    company: &PinnedCompany,
) -> Result<LedgerOpeningCoverage, OutstandingsError> {
    require_complete_envelope(xml)?;
    let sanitized = sanitize_invalid_numeric_references(xml);
    let parsed: Envelope<LedgerCollection> = quick_xml::de::from_str(&sanitized)
        .map_err(|_| OutstandingsError::InvalidResponse("ledger_collection_xml_invalid"))?;
    require_success(&parsed.header)?;
    let ledgers = parsed.body.data.collection.ledgers;
    let mut ledger_identities = BTreeMap::new();
    let mut openings = 0usize;
    for ledger in &ledgers {
        let guid = required(
            ledger
                .guid
                .as_ref()
                .ok_or(OutstandingsError::InvalidResponse("ledger_guid_missing"))?
                .text
                .clone(),
            "ledger_guid_missing",
        )?
        .to_ascii_lowercase();
        if !master_guid_belongs_to_company(&guid, company.guid()) {
            return Err(OutstandingsError::InvalidResponse(
                "ledger_belongs_to_another_company",
            ));
        }
        let name = required(
            ledger
                .attribute_name
                .clone()
                .ok_or(OutstandingsError::InvalidResponse("ledger_name_missing"))?,
            "ledger_name_missing",
        )?;
        if ledger_identities.insert(guid, name).is_some() {
            return Err(OutstandingsError::InvalidResponse("ledger_guid_duplicate"));
        }
        // Fail closed. `ISBILLWISEON` is in this profile's FETCH list, so an
        // absent or unrecognised value means the response does not match the
        // request. Defaulting to "not bill-wise" would classify a ledger with a
        // non-zero opening as fully covered and let a voucher-only scan report
        // Complete while omitting that ledger's opening bills.
        let bill_wise = parse_bool(
            &ledger
                .bill_wise_on
                .as_ref()
                .ok_or(OutstandingsError::InvalidResponse(
                    "ledger_bill_wise_state_missing",
                ))?
                .text,
        )?;
        if !bill_wise {
            continue;
        }
        // Fail closed, like ISBILLWISEON. OPENINGBALANCE is in this profile's
        // FETCH list, so an absent field means the response does not match the
        // request -- and treating absence as "no opening" would classify a
        // ledger carrying a non-zero opening as fully covered, which is exactly
        // the omission this probe exists to prevent.
        let opening = ledger
            .opening_balance
            .as_ref()
            .ok_or(OutstandingsError::InvalidResponse(
                "ledger_opening_balance_missing",
            ))?
            .text
            .trim()
            .to_string();
        // A present-but-EMPTY amount is not zero (§6.4). Skipping it would count
        // the ledger as carrying no opening, so a response that drops the value
        // could let the scan complete while omitting that ledger's opening bills.
        if opening.is_empty() {
            return Err(OutstandingsError::InvalidResponse(
                "ledger_opening_balance_empty",
            ));
        }
        if !matches!(parse_money(opening)?, MoneyValue::Exact(ref v) if v.is_zero()) {
            openings += 1;
        }
    }
    // A successful-but-EMPTY collection is the false-empty route of §2.8 applied
    // to masters: every company that has vouchers also has ledgers, so zero rows
    // here means the response is not describing the book we asked about. Counting
    // it as "no openings" would publish a Complete voucher-only report while
    // omitting every ledger-opening bill.
    if ledgers.is_empty() {
        return Err(OutstandingsError::InvalidResponse(
            "ledger_coverage_response_empty",
        ));
    }
    Ok(LedgerOpeningCoverage::new(ledger_identities, openings))
}

pub(super) fn parse_segment(
    xml: &str,
    company: &PinnedCompany,
    reporting_window: &DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<ParsedSegment, OutstandingsError> {
    require_complete_envelope(xml)?;
    let sanitized = sanitize_invalid_numeric_references(xml);
    let raw_row_count = count_voucher_start_elements(&sanitized)?;
    let parsed: Envelope<VoucherCollection> = quick_xml::de::from_str(&sanitized)
        .map_err(|_| OutstandingsError::InvalidResponse("voucher_collection_xml_invalid"))?;
    require_success(&parsed.header)?;
    let vouchers = parsed
        .body
        .data
        .collection
        .vouchers
        .into_iter()
        .map(|raw| convert_voucher(raw, reporting_window, alter_id_range))
        .collect::<Result<Vec<_>, _>>()?;
    for voucher in &vouchers {
        if !master_guid_belongs_to_company(&voucher.guid, company.guid()) {
            return Err(OutstandingsError::InvalidResponse(
                "voucher_belongs_to_another_company",
            ));
        }
    }

    let mut guids = BTreeSet::new();
    let mut alter_ids = BTreeSet::new();
    for voucher in &vouchers {
        if !guids.insert(voucher.guid.as_str()) {
            return Err(OutstandingsError::InvalidResponse(
                "duplicate_voucher_guid_within_segment",
            ));
        }
        if !alter_ids.insert(voucher.alter_id) {
            return Err(OutstandingsError::InvalidResponse(
                "duplicate_voucher_alter_id_within_segment",
            ));
        }
    }
    Ok(ParsedSegment {
        vouchers,
        raw_row_count,
    })
}

fn master_guid_belongs_to_company(master_guid: &str, company_guid: &str) -> bool {
    // Bind the response to the pinned company, not just the request.
    //
    // `SVCURRENTCOMPANY` selects by NAME. If a second loaded company shares the
    // selected name, or the name binding shifts mid-scan, Tally can return that
    // other company's vouchers while the paired company collection still finds
    // the expected GUID among all loaded companies -- so date checks, AlterID
    // range checks, and the closing extent all pass, and another company's
    // financial data is published under the pinned name.
    //
    // TALLY_PROTOCOL_REFERENCE.md:632 records that every master GUID begins
    // with its company GUID; require the documented `-<master-id>` delimiter
    // as response identity evidence instead of accepting the bare company GUID.
    let Some(prefix) = master_guid.get(..company_guid.len()) else {
        return false;
    };
    let Some(suffix) = master_guid.get(company_guid.len()..) else {
        return false;
    };
    prefix.eq_ignore_ascii_case(company_guid)
        && suffix
            .strip_prefix('-')
            .is_some_and(|master_id| !master_id.is_empty())
}

pub(super) fn parse_witness_segment(
    xml: &str,
    company: &PinnedCompany,
    window: &DateWindow,
) -> Result<ParsedWitnessSegment, OutstandingsError> {
    require_complete_envelope(xml)?;
    let sanitized = sanitize_invalid_numeric_references(xml);
    let raw_row_count = count_voucher_start_elements(&sanitized)?;
    let parsed: Envelope<WitnessVoucherCollection> =
        quick_xml::de::from_str(&sanitized).map_err(|_| {
            OutstandingsError::InvalidResponse("empty_date_witness_collection_xml_invalid")
        })?;
    require_success(&parsed.header)?;
    let vouchers = parsed
        .body
        .data
        .collection
        .vouchers
        .into_iter()
        .map(|raw| convert_witness_voucher(raw, window))
        .collect::<Result<Vec<_>, _>>()?;
    let mut guids = BTreeSet::new();
    let mut alter_ids = BTreeSet::new();
    for voucher in &vouchers {
        if !master_guid_belongs_to_company(&voucher.guid, company.guid()) {
            return Err(OutstandingsError::InvalidResponse(
                "witness_voucher_belongs_to_another_company",
            ));
        }
        if !guids.insert(voucher.guid.as_str()) {
            return Err(OutstandingsError::InvalidResponse(
                "duplicate_witness_voucher_guid",
            ));
        }
        if !alter_ids.insert(voucher.alter_id) {
            return Err(OutstandingsError::InvalidResponse(
                "duplicate_witness_voucher_alter_id",
            ));
        }
    }
    Ok(ParsedWitnessSegment {
        vouchers,
        raw_row_count,
    })
}

fn convert_witness_voucher(
    raw: RawWitnessVoucher,
    window: &DateWindow,
) -> Result<WitnessVoucher, OutstandingsError> {
    let date = parse_date(raw.date.text)?;
    if &date < window.from() || &date > window.to() {
        return Err(OutstandingsError::InvalidResponse(
            "witness_voucher_outside_requested_window",
        ));
    }
    Ok(WitnessVoucher {
        guid: required(raw.guid, "witness_voucher_guid_missing")?,
        alter_id: VoucherAlterId::parse(&required(
            raw.alter_id.text,
            "witness_voucher_alter_id_missing",
        )?)?,
        date,
    })
}

fn convert_voucher(
    raw: RawVoucher,
    reporting_window: &DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<Voucher, OutstandingsError> {
    let date = parse_date(raw.date.text)?;
    if &date < reporting_window.from() || &date > reporting_window.to() {
        return Err(OutstandingsError::InvalidResponse(
            "voucher_outside_requested_window",
        ));
    }
    let alter_id =
        VoucherAlterId::parse(&required(raw.alter_id.text, "voucher_alter_id_missing")?)?;
    if !alter_id_range.contains(alter_id) {
        return Err(OutstandingsError::InvalidResponse(
            "voucher_outside_requested_alter_id_range",
        ));
    }
    Ok(Voucher {
        guid: required(raw.guid, "voucher_guid_missing")?,
        master_id: required(raw.master_id.text, "voucher_master_id_missing")?,
        alter_id,
        date,
        voucher_type: required(raw.voucher_type, "voucher_type_missing")?,
        voucher_number: trimmed_optional(raw.voucher_number),
        party_ledger_name: raw
            .party_ledger_name
            .and_then(|value| trimmed_optional(Some(value.text))),
        cancelled: parse_bool(&raw.cancelled.text)?,
        deleted: parse_bool(&raw.deleted.text)?,
        // Fail closed. `ISOPTIONAL` is in the sealed request's FETCH list and
        // Tally returns it on every row (live-verified: both `No` and `Yes`
        // observed). If it is absent the response does not match the request we
        // sent, and defaulting to "posting" would re-admit exactly the
        // non-posting allocations this field exists to exclude.
        optional: parse_bool(
            &raw.optional
                .ok_or(OutstandingsError::InvalidResponse(
                    "voucher_optional_state_missing",
                ))?
                .text,
        )?,
        ledger_entries: raw
            .ledger_entries
            .into_iter()
            .map(convert_ledger_entry)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect(),
    })
}

fn convert_ledger_entry(raw: RawLedgerEntry) -> Result<Option<LedgerEntry>, OutstandingsError> {
    let bill_allocations = raw
        .bill_allocations
        .into_iter()
        .map(convert_bill_allocation)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if bill_allocations.is_empty() {
        return Ok(None);
    }
    Ok(Some(LedgerEntry {
        ledger_name: required(
            raw.ledger_name
                .ok_or(OutstandingsError::InvalidResponse("ledger_name_missing"))?
                .text,
            "ledger_name_missing",
        )?,
        bill_allocations,
    }))
}

fn convert_bill_allocation(
    raw: RawBillAllocation,
) -> Result<Option<BillAllocation>, OutstandingsError> {
    let name = raw
        .name
        .and_then(|value| trimmed_optional(Some(value.text)));
    let Some(bill_type) = raw.bill_type else {
        if name.is_some() {
            return Err(OutstandingsError::InvalidResponse("bill_type_missing"));
        }
        // Tally emits both empty and amount-only placeholder containers for
        // ledger entries that have no typed bill allocation.
        return Ok(None);
    };
    if raw.amount.is_none() {
        return Err(OutstandingsError::InvalidResponse("bill_amount_missing"));
    }
    let bill_type = BillReferenceKind::parse(&bill_type.text)?;
    match (bill_type, &name) {
        (BillReferenceKind::OnAccount, Some(_)) => {
            return Err(OutstandingsError::InvalidResponse(
                "bill_reference_forbidden",
            ))
        }
        (kind, None) if kind.requires_named_reference() => {
            return Err(OutstandingsError::InvalidResponse("bill_reference_missing"))
        }
        _ => {}
    }
    let bill_date = match raw.bill_date {
        Some(value) if !value.text.trim().is_empty() => Some(parse_date(value.text)?),
        // New Ref and Agst Ref ageing depends on BILLDATE. The wildcard fetch
        // returns it for both, so its absence means the response does not match
        // the request. Reject it at the parser boundary so a Complete scan
        // cannot reach computation with an allocation it will reject.
        _ if matches!(
            bill_type,
            BillReferenceKind::NewRef | BillReferenceKind::AgstRef
        ) =>
        {
            return Err(OutstandingsError::InvalidResponse("bill_date_missing"))
        }
        _ => None,
    };
    let credit_period = match raw.bill_credit_period {
        Some(value) => parse_credit_period(&value.text)?,
        None if bill_type.requires_named_reference() => {
            return Err(OutstandingsError::InvalidResponse(
                "bill_credit_period_missing",
            ))
        }
        None => CreditPeriod::Days(0),
    };
    Ok(Some(BillAllocation {
        name,
        bill_type,
        amount: parse_money(
            raw.amount
                .ok_or(OutstandingsError::InvalidResponse("bill_amount_missing"))?
                .text,
        )?,
        bill_date,
        credit_period,
    }))
}

// Licensed TallyPrime 7.1 read-back on 2026-08-23 retained up to 9999 days,
// but silently discarded 10000 days to an empty credit period. This is a
// measured wire-format ceiling, not a business-term policy. Weeks and months
// have no equivalent measured ceiling, so their checked resulting date is the
// bound instead.
const MAX_TALLY_CREDIT_PERIOD_DAYS: u32 = 9999;

fn parse_credit_period(value: &str) -> Result<CreditPeriod, OutstandingsError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(CreditPeriod::Days(0));
    }
    let Some((magnitude, period, maximum)) = [
        (
            " Months",
            CreditPeriod::Months as fn(u32) -> CreditPeriod,
            None,
        ),
        (
            " Month",
            CreditPeriod::Months as fn(u32) -> CreditPeriod,
            None,
        ),
        (
            " Weeks",
            CreditPeriod::Weeks as fn(u32) -> CreditPeriod,
            None,
        ),
        (
            " Week",
            CreditPeriod::Weeks as fn(u32) -> CreditPeriod,
            None,
        ),
        (
            " Days",
            CreditPeriod::Days as fn(u32) -> CreditPeriod,
            Some(MAX_TALLY_CREDIT_PERIOD_DAYS),
        ),
        (
            " Day",
            CreditPeriod::Days as fn(u32) -> CreditPeriod,
            Some(MAX_TALLY_CREDIT_PERIOD_DAYS),
        ),
    ]
    .into_iter()
    .find_map(|(suffix, period, maximum)| {
        value
            .strip_suffix(suffix)
            .map(|magnitude| (magnitude, period, maximum))
    }) else {
        return Err(OutstandingsError::InvalidResponse(
            "bill_credit_period_invalid",
        ));
    };
    if magnitude.is_empty() || !magnitude.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(OutstandingsError::InvalidResponse(
            "bill_credit_period_invalid",
        ));
    }
    let magnitude = magnitude
        .parse::<u32>()
        .map_err(|_| OutstandingsError::InvalidResponse("bill_credit_period_invalid"))?;
    if let Some(maximum) = maximum {
        if magnitude > maximum {
            return Err(OutstandingsError::InvalidResponse(
                "bill_credit_period_invalid",
            ));
        }
    }
    Ok(period(magnitude))
}

fn parse_money(value: String) -> Result<MoneyValue, OutstandingsError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(MoneyValue::Absent);
    }
    ExactDecimal::parse(value.to_string())
        .map(MoneyValue::Exact)
        .map_err(|_| OutstandingsError::InvalidAmount)
}

fn parse_date(value: String) -> Result<TallyDate, OutstandingsError> {
    TallyDate::parse(value.trim().to_string())
        .map_err(|_| OutstandingsError::InvalidResponse("tally_date_invalid"))
}

fn parse_bool(value: &str) -> Result<bool, OutstandingsError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => Err(OutstandingsError::InvalidResponse("tally_boolean_invalid")),
    }
}

/// Count `<VOUCHER>` **start elements inside `<DATA>`** structurally.
///
/// Two traps, both live-observed, make the obvious implementations wrong:
///
/// 1. A textual `"<VOUCHER "` scan under-reports. Tally may legitimately
///    serialize a row as `<VOUCHER>` with no attributes, or place a newline
///    between the name and its first attribute. `quick_xml` deserializes those
///    rows, so two identical complete replies would fail the raw-vs-parsed row
///    agreement check and withhold a correct report.
/// 2. Counting every `VOUCHER` element over-reports. An empty live witness
///    response carries `CMPINFO` (under `DESC`, not `DATA`) with a bare
///    `<VOUCHER>0</VOUCHER>` counter. A whole-document count reports one row
///    and inverts the empty-partition verdict. This appeared three times in the
///    supervised qualification session. Scoping to `DATA` excludes counters by
///    structure rather than by guessing at attributes.
fn count_voucher_start_elements(xml: &str) -> Result<usize, OutstandingsError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().check_end_names = false;
    let mut count = 0usize;
    let mut data_depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = element.name();
                let name = name.as_ref();
                if name.eq_ignore_ascii_case(b"DATA") {
                    data_depth += 1;
                } else if data_depth > 0 && name.eq_ignore_ascii_case(b"VOUCHER") {
                    count += 1;
                }
            }
            Ok(Event::Empty(element)) => {
                if data_depth > 0 && element.name().as_ref().eq_ignore_ascii_case(b"VOUCHER") {
                    count += 1;
                }
            }
            Ok(Event::End(element)) => {
                if element.name().as_ref().eq_ignore_ascii_case(b"DATA") {
                    data_depth = data_depth.saturating_sub(1);
                }
            }
            Ok(Event::Eof) => return Ok(count),
            Ok(_) => {}
            Err(_) => {
                return Err(OutstandingsError::InvalidResponse(
                    "voucher_collection_xml_invalid",
                ))
            }
        }
    }
}

fn require_complete_envelope(xml: &str) -> Result<(), OutstandingsError> {
    if !xml.trim_end().ends_with("</ENVELOPE>") {
        return Err(OutstandingsError::InvalidResponse("response_truncated"));
    }
    Ok(())
}

fn require_success(header: &Header) -> Result<(), OutstandingsError> {
    if header.status.trim() == "1" {
        Ok(())
    } else {
        Err(OutstandingsError::InvalidResponse(
            "tally_status_not_success",
        ))
    }
}

fn required(value: String, code: &'static str) -> Result<String, OutstandingsError> {
    let value = value.trim().to_string();
    if value.is_empty() {
        Err(OutstandingsError::InvalidResponse(code))
    } else {
        Ok(value)
    }
}

fn trimmed_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{count_voucher_start_elements, parse_credit_period};
    use crate::outstandings::{CreditPeriod, OutstandingsError};

    #[test]
    fn credit_period_accepts_verified_units_and_rejects_unknown_ones() {
        assert_eq!(
            parse_credit_period("45 Days").unwrap(),
            CreditPeriod::Days(45)
        );
        assert_eq!(parse_credit_period("1 Day").unwrap(), CreditPeriod::Days(1));
        assert_eq!(
            parse_credit_period("3 Weeks").unwrap(),
            CreditPeriod::Weeks(3)
        );
        assert_eq!(
            parse_credit_period("2 Months").unwrap(),
            CreditPeriod::Months(2)
        );
        assert_eq!(parse_credit_period(" ").unwrap(), CreditPeriod::Days(0));
        // Licensed TallyPrime 7.1 read-back on 2026-08-23 retained all four
        // values. Only the day-unit ceiling is measured: 10000 Days comes
        // back empty from Tally rather than as an over-limit period.
        assert_eq!(
            parse_credit_period("9999 Days").unwrap(),
            CreditPeriod::Days(9999)
        );
        assert_eq!(
            parse_credit_period("3650 Days").unwrap(),
            CreditPeriod::Days(3650)
        );
        assert_eq!(
            parse_credit_period("100 Months").unwrap(),
            CreditPeriod::Months(100)
        );
        assert_eq!(
            parse_credit_period("1000 Months").unwrap(),
            CreditPeriod::Months(1000)
        );
        assert_eq!(
            parse_credit_period("2 Fortnights"),
            Err(OutstandingsError::InvalidResponse(
                "bill_credit_period_invalid"
            ))
        );
        assert_eq!(
            parse_credit_period("10000 Days"),
            Err(OutstandingsError::InvalidResponse(
                "bill_credit_period_invalid"
            ))
        );
    }

    #[test]
    fn voucher_rows_are_counted_structurally_not_by_one_textual_spelling() {
        // Three serializations quick_xml deserializes identically. A
        // `"<VOUCHER "` substring scan sees only the first.
        let xml = concat!(
            "<ENVELOPE><BODY><DATA><COLLECTION>",
            "<VOUCHER REMOTEID=\"a\"></VOUCHER>",
            "<VOUCHER></VOUCHER>",
            "<VOUCHER\n  REMOTEID=\"c\"></VOUCHER>",
            "</COLLECTION></DATA></BODY></ENVELOPE>"
        );
        assert_eq!(count_voucher_start_elements(xml).unwrap(), 3);
        assert_eq!(xml.match_indices("<VOUCHER ").count(), 1);
    }

    #[test]
    fn cmpinfo_counter_elements_are_not_counted_as_rows() {
        // CMPINFO sits under DESC and carries bare `<VOUCHER>0</VOUCHER>`
        // counters. The retained live capture has exactly one against 75 real
        // rows, so counting every VOUCHER element would over-report and fail
        // the raw-vs-parsed agreement check.
        let xml = concat!(
            "<ENVELOPE><BODY>",
            "<DESC><CMPINFO><VOUCHER>0</VOUCHER><VOUCHERTYPE>0</VOUCHERTYPE></CMPINFO></DESC>",
            "<DATA><COLLECTION><VOUCHER REMOTEID=\"a\"></VOUCHER></COLLECTION></DATA>",
            "</BODY></ENVELOPE>"
        );
        assert_eq!(count_voucher_start_elements(xml).unwrap(), 1);
    }

    #[test]
    fn a_response_with_no_rows_counts_zero_rather_than_failing() {
        let xml = "<ENVELOPE><BODY><DATA><COLLECTION></COLLECTION></DATA></BODY></ENVELOPE>";
        assert_eq!(count_voucher_start_elements(xml).unwrap(), 0);
    }
}
