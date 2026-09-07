//! Shared scalar admission for native voucher readers.
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::agent) fn is_voucher_scalar(field: &str) -> bool {
    matches!(
        field,
        "DATE"
            | "VOUCHERTYPENAME"
            | "VOUCHERNUMBER"
            | "PARTYLEDGERNAME"
            | "NARRATION"
            | "GUID"
            | "ALTERID"
            | "MASTERID"
            | "ISCANCELLED"
            | "ISOPTIONAL"
    )
}

pub(in crate::agent) fn is_voucher_entry_scalar(field: &str) -> bool {
    matches!(field, "LEDGERNAME" | "AMOUNT" | "ISDEEMEDPOSITIVE")
}

/// Reserve one XML element, independently of text/entity event fragmentation.
pub(in crate::agent) fn claim_agent_scalar(
    row: &mut BTreeMap<String, String>,
    field: &str,
) -> Result<(), String> {
    if row.insert(field.into(), String::new()).is_some() {
        return Err("agent_read_protocol_invalid".into());
    }
    Ok(())
}

pub(in crate::agent) fn validate_tally_entry_polarity(
    amount: &bridge_tally_core::ExactDecimal,
    is_deemed_positive: bool,
) -> Result<(), String> {
    use bridge_tally_core::LedgerEntryPolarity;
    let polarity = if is_deemed_positive {
        LedgerEntryPolarity::Debit
    } else {
        LedgerEntryPolarity::Credit
    };
    // Core's polarity contract accepts zero with either observed flag: zero
    // cannot independently corroborate direction. Never invent a sign for it.
    if amount.is_zero()
        || matches!(
            (polarity, amount.is_negative()),
            (LedgerEntryPolarity::Debit, true) | (LedgerEntryPolarity::Credit, false)
        )
    {
        Ok(())
    } else {
        Err("voucher_entry_polarity_mismatch".into())
    }
}

pub(in crate::agent) fn parse_optional_tally_alter_id(
    value: Option<&str>,
) -> Result<Option<u64>, String> {
    parse_optional_tally_u64(value, "voucher_alter_id_invalid")
}

pub(in crate::agent) fn parse_optional_tally_u64(
    value: Option<&str>,
    error_code: &'static str,
) -> Result<Option<u64>, String> {
    value
        .map(|value| {
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(error_code.into());
            }
            value.parse::<u64>().map_err(|_| error_code.to_string())
        })
        .transpose()
}

/// Per-collection identity admission, before selectors or accounting arithmetic.
/// Missing identities retain each reader's policy; present identities are unique.
#[derive(Default)]
pub(in crate::agent) struct VoucherSourceIdentities {
    guids: BTreeSet<String>,
    master_ids: BTreeSet<u64>,
}

impl VoucherSourceIdentities {
    pub(in crate::agent) fn admit(
        &mut self,
        guid: Option<&str>,
        master_id: Option<u64>,
    ) -> Result<(), String> {
        if let Some(guid) = guid {
            let guid = guid.trim().to_ascii_lowercase();
            if guid.is_empty() || !self.guids.insert(guid) {
                return Err("voucher_source_identity_invalid".into());
            }
        }
        if master_id.is_some_and(|id| !self.master_ids.insert(id)) {
            return Err("voucher_source_identity_invalid".into());
        }
        Ok(())
    }
}
