use bridge_tally_core::{ExactDecimal, TallyDate};
use std::collections::BTreeSet;

use crate::xml_read_profiles::ValidatedCompanyName;

use super::{
    tolerant_xml::sanitize_invalid_numeric_references,
    wire::{
        CompanyCollection, Envelope, Header, RawBillAllocation, RawLedgerEntry, RawVoucher,
        VoucherCollection,
    },
    AlterIdRange, BillAllocation, CompanyBookExtent, DateWindow, LedgerEntry, MoneyValue,
    OutstandingsError, PinnedCompany, Voucher, VoucherAlterId, VoucherAlterIdHighWater,
};

pub(super) struct ParsedSegment {
    pub(super) vouchers: Vec<Voucher>,
    pub(super) raw_row_count: usize,
}

pub fn parse_company_book_extent(
    xml: &str,
    expected_name: &str,
    expected_guid: &str,
) -> Result<CompanyBookExtent, OutstandingsError> {
    require_complete_envelope(xml)?;
    let sanitized = sanitize_invalid_numeric_references(xml);
    let parsed: Envelope<CompanyCollection> = quick_xml::de::from_str(&sanitized)
        .map_err(|_| OutstandingsError::InvalidResponse("company_extent_xml_invalid"))?;
    require_success(&parsed.header)?;
    let mut matching = parsed
        .body
        .data
        .collection
        .companies
        .into_iter()
        .filter(|raw| raw.guid.text.trim().eq_ignore_ascii_case(expected_guid));
    let raw = matching
        .next()
        .ok_or(OutstandingsError::CompanyIdentityMismatch)?;
    if matching.next().is_some() {
        return Err(OutstandingsError::InvalidResponse(
            "company_identity_ambiguous",
        ));
    }
    let name = required(raw.name.text, "company_name_missing")?;
    let guid = required(raw.guid.text, "company_guid_missing")?;
    if raw.attribute_name != name
        || name != expected_name
        || !guid.eq_ignore_ascii_case(expected_guid)
    {
        return Err(OutstandingsError::CompanyIdentityMismatch);
    }
    let name =
        ValidatedCompanyName::new(name).map_err(|_| OutstandingsError::InvalidCompanyIdentity)?;
    let company = PinnedCompany::verified(name, guid)?;
    let books_from = parse_date(raw.books_from.text)?;
    let last_voucher_date = parse_date(raw.last_voucher_date.text)?;
    let voucher_alter_id_high_water = raw
        .alter_voucher_id
        .map(|value| VoucherAlterIdHighWater::parse(&value.text))
        .transpose()?;
    if books_from > last_voucher_date {
        return Err(OutstandingsError::InvalidResponse(
            "company_extent_reversed",
        ));
    }
    Ok(CompanyBookExtent::new(
        company,
        books_from,
        last_voucher_date,
        voucher_alter_id_high_water,
    ))
}

pub(super) fn parse_segment(
    xml: &str,
    _company: &PinnedCompany,
    reporting_window: &DateWindow,
    alter_id_range: AlterIdRange,
) -> Result<ParsedSegment, OutstandingsError> {
    require_complete_envelope(xml)?;
    let raw_row_count = xml.match_indices("<VOUCHER ").count();
    let sanitized = sanitize_invalid_numeric_references(xml);
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
    Ok(Some(BillAllocation {
        name,
        bill_type: required(bill_type.text, "bill_type_missing")?,
        amount: parse_money(
            raw.amount
                .ok_or(OutstandingsError::InvalidResponse("bill_amount_missing"))?
                .text,
        )?,
    }))
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
