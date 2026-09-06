//! Movement math for the local MCP adapter.
use super::*;

pub(super) fn ledger_movement_counts<T>(rows: &[Value], vouchers: &[T]) -> (usize, usize) {
    (rows.len(), vouchers.len())
}

pub(super) fn absent_movement_entry_policy(
    entry_ledger: &str,
    selected: Option<&str>,
) -> Result<(), String> {
    if selected.is_none_or(|ledger| ledger == entry_ledger) {
        Err("ledger_snapshot_drifted".to_string())
    } else {
        Ok(())
    }
}

pub(super) struct LedgerMovementRow {
    pub(super) name: String,
    pub(super) parent: Option<String>,
    pub(super) opening: Option<String>,
    pub(super) debit: String,
    pub(super) credit: String,
    pub(super) vouchers_touching: usize,
}

pub(super) fn ledger_movement_row(
    row: LedgerMovementRow,
    redaction: Redaction,
) -> Result<(Value, bool), String> {
    let LedgerMovementRow {
        name,
        parent,
        opening,
        debit,
        credit,
        vouchers_touching,
    } = row;
    let opening_unobserved = opening.is_none();
    let closing = opening
        .as_deref()
        .map(|opening| {
            let debited = add_decimal(opening, &debit)?;
            add_decimal(&debited, &credit)
        })
        .transpose()?;
    Ok((
        redact_value(
            json!({
                "ledger": party_name(name),
                "parent": parent,
                "opening": opening,
                "debit": debit,
                "credit": credit,
                "closing": closing,
                "vouchers_touching": vouchers_touching,
                "state": if opening_unobserved {"partial"} else {"complete"},
                "reason": opening_unobserved.then_some("opening_balance_not_observed"),
            }),
            redaction,
        ),
        opening_unobserved,
    ))
}
