use std::collections::BTreeMap;

use bridge_tally_primitives::{ExactDecimal, TallyDate};

use crate::outstandings_shared::{
    AgeingBillCounts, AgeingBuckets, OutstandingsReport, PartyOutstanding,
};
use crate::{is_tally_reserved_root, TallyNamedMaster};

use super::model::{
    AgeingAnchor, LedgerSnapshotEntry, NativeBillRow, NativeOutstandingsError,
    NativeOutstandingsResult, NativeOverdueCrosscheck, PartyResidual,
};

#[derive(Default)]
struct PartyAccumulator {
    receivable: Option<ExactDecimal>,
    payable: Option<ExactDecimal>,
    oldest_bill_age: Option<u32>,
}

/// Group ancestry evidence available to the native outstandings computation.
///
/// Production callers must use [`Self::Complete`]. An empty complete snapshot
/// is an invalid read, not evidence that the book has no groups. The legacy
/// variant exists only for offline fixtures captured before group ancestry was
/// part of the evidence set, and callers must opt into that degraded behavior
/// by name.
pub enum NativeGroupSnapshot<'a> {
    Complete(&'a [TallyNamedMaster]),
    LegacyFixtureWithoutGroups,
}

/// Ledger and group masters captured for the same native outstandings read.
/// Group ancestry is required to classify nested party ledgers correctly.
pub struct NativeMasterSnapshot<'a> {
    pub ledgers: &'a [LedgerSnapshotEntry],
    pub groups: NativeGroupSnapshot<'a>,
}

/// Computes a drop-in [`OutstandingsReport`] plus on-account residual
/// evidence from the native Bills Receivable/Payable rows and the ledger
/// snapshot, per TALLY_PROTOCOL_REFERENCE ground truth captured 2026-08-07.
///
/// `source_bytes` is the caller's real encoded byte count for the responses
/// consumed; this path reads no vouchers, so `source_voucher_count` is
/// always `0`.
pub fn compute_native_outstandings(
    company_name: &str,
    receivable_rows: &[NativeBillRow],
    payable_rows: &[NativeBillRow],
    masters: NativeMasterSnapshot<'_>,
    anchor: AgeingAnchor,
    as_of: &TallyDate,
    source_bytes: usize,
) -> Result<NativeOutstandingsResult, NativeOutstandingsError> {
    let mut receivable_total = ExactDecimal::zero();
    let mut payable_total = ExactDecimal::zero();
    let mut ageing = AgeingBuckets {
        days_0_30: ExactDecimal::zero(),
        days_31_60: ExactDecimal::zero(),
        days_61_90: ExactDecimal::zero(),
        days_90_plus: ExactDecimal::zero(),
    };
    let mut ageing_bill_counts = AgeingBillCounts {
        days_0_30: 0,
        days_31_60: 0,
        days_61_90: 0,
        days_90_plus: 0,
    };
    let mut parties = BTreeMap::<String, PartyAccumulator>::new();

    for row in receivable_rows
        .iter()
        .filter(|row| !row.closing_balance.is_zero())
    {
        if !row.closing_balance.is_negative() {
            return Err(NativeOutstandingsError::InvalidResponse(
                "receivable_bill_sign_contradiction",
            ));
        }
        let amount = row
            .closing_balance
            .abs()
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
        receivable_total = add(&receivable_total, &amount)?;

        let age = overdue_days(bill_anchor_date(row, anchor), as_of)?;
        let totals = parties.entry(row.party.clone()).or_default();
        totals.receivable = Some(add(
            totals.receivable.as_ref().unwrap_or(&ExactDecimal::zero()),
            &amount,
        )?);
        // Tally keeps a future-due open bill in its first ageing bucket even
        // though BILLOVERDUE is empty and no overdue age can truthfully be
        // claimed. Bucket membership and bill age are therefore distinct:
        // count the bill and its amount, but retain `None` for oldest age.
        let (bucket, count) = match age {
            None | Some(0..=30) => (&mut ageing.days_0_30, &mut ageing_bill_counts.days_0_30),
            Some(31..=60) => (&mut ageing.days_31_60, &mut ageing_bill_counts.days_31_60),
            Some(61..=90) => (&mut ageing.days_61_90, &mut ageing_bill_counts.days_61_90),
            Some(_) => (
                &mut ageing.days_90_plus,
                &mut ageing_bill_counts.days_90_plus,
            ),
        };
        *bucket = add(bucket, &amount)?;
        *count = count
            .checked_add(1)
            .ok_or(NativeOutstandingsError::ArithmeticOverflow)?;
        if let Some(age) = age {
            totals.oldest_bill_age =
                Some(totals.oldest_bill_age.map_or(age, |oldest| oldest.max(age)));
        }
    }

    for row in payable_rows
        .iter()
        .filter(|row| !row.closing_balance.is_zero())
    {
        if row.closing_balance.is_negative() {
            return Err(NativeOutstandingsError::InvalidResponse(
                "payable_bill_sign_contradiction",
            ));
        }
        let amount = row
            .closing_balance
            .abs()
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
        payable_total = add(&payable_total, &amount)?;

        let age = overdue_days(bill_anchor_date(row, anchor), as_of)?;

        let totals = parties.entry(row.party.clone()).or_default();
        totals.payable = Some(add(
            totals.payable.as_ref().unwrap_or(&ExactDecimal::zero()),
            &amount,
        )?);
        if let Some(age) = age {
            totals.oldest_bill_age =
                Some(totals.oldest_bill_age.map_or(age, |oldest| oldest.max(age)));
        }
    }

    let mut top_parties = parties
        .into_iter()
        .map(|(party, totals)| {
            let receivable = totals.receivable.unwrap_or_else(ExactDecimal::zero);
            let payable = totals.payable.unwrap_or_else(ExactDecimal::zero);
            let outstanding_total = add(&receivable, &payable)?;
            Ok(PartyOutstanding {
                party,
                receivable,
                payable,
                outstanding_total,
                oldest_bill_age_days: totals.oldest_bill_age,
            })
        })
        .collect::<Result<Vec<_>, NativeOutstandingsError>>()?;
    top_parties.sort_by(|left, right| {
        right
            .outstanding_total
            .cmp_magnitude(&left.outstanding_total)
            .then_with(|| left.party.cmp(&right.party))
    });
    top_parties.truncate(10);

    let open_receivable_bill_count = ageing_bill_counts
        .days_0_30
        .checked_add(ageing_bill_counts.days_31_60)
        .and_then(|value| value.checked_add(ageing_bill_counts.days_61_90))
        .and_then(|value| value.checked_add(ageing_bill_counts.days_90_plus))
        .ok_or(NativeOutstandingsError::ArithmeticOverflow)?;

    let (residuals, residual_total, has_unaged_receivable) = compute_residuals(
        receivable_rows,
        payable_rows,
        masters.ledgers,
        masters.groups,
    )?;
    let overdue_crosscheck =
        classify_overdue_crosscheck(receivable_rows, payable_rows, &residual_total, as_of)?;

    let report = OutstandingsReport {
        company_name: company_name.to_string(),
        as_of_yyyymmdd: as_of.as_str().to_string(),
        receivable_total,
        payable_total,
        has_unaged_receivable,
        ageing,
        open_receivable_bill_count,
        ageing_bill_counts,
        top_parties,
        source_voucher_count: 0,
        source_bytes,
    };

    Ok(NativeOutstandingsResult {
        report,
        residuals,
        residual_total,
        overdue_crosscheck,
    })
}

/// Classifies the returned bill counters with the following exhaustive
/// decision table. **Honored requires positive evidence that the requested
/// date was used**: empty and zero counters can constrain a proven candidate,
/// but never create one. `positive` means `BILLOVERDUE > 0`, whose implied date
/// is `BILLDUE + BILLOVERDUE`. The substituted-date rows are policy cases; a
/// refusal is not currently reproducible on the licensed local instance.
///
/// | rows | residual | counters and due date | implied dates | verdict | why |
/// | --- | --- | --- | --- | --- | --- |
/// | none | zero | none; due n/a | absent | unconfirmed effective date | settled historical rows can be absent under a substituted date |
/// | none | non-zero | none; due n/a | absent | unconfirmed without bill references | ledger money has no bill evidence at all |
/// | some | either | only empty; due earlier/equal/later | absent | unconfirmed effective date | empty counters identify no date |
/// | some | either | only zero; due earlier/equal/later | absent | unconfirmed effective date | zero counters identify no date, including equality |
/// | some | either | empty and zero only; due earlier/equal/later | absent | unconfirmed effective date | combining non-evidence does not create evidence |
/// | some | either | any negative or unrepresentable counter | absent | inconsistent | the counter cannot imply a valid civil date |
/// | some | either | positive counters disagree | disagree | inconsistent | one response cannot have two effective dates |
/// | some | either | positives agree on requested; no companions | all agree | honored | positive counters independently identify the request |
/// | some | either | positives agree on requested; empty companions later | all agree | honored | an empty counter is compatible only for a future-due bill |
/// | some | either | positives agree on requested; empty companion earlier/equal | all agree | inconsistent | an empty counter contradicts the proven candidate |
/// | some | either | positives agree on requested; zero companions equal/later | all agree | honored | zero is compatible but not relied on as proof |
/// | some | either | positives agree on requested; zero companion earlier | all agree | inconsistent | zero contradicts the proven candidate |
/// | some | either | one positive agrees on another date | all agree | inconsistent | one row cannot establish a refused period |
/// | some | either | at least two positives agree on another date; every empty is later and every zero is equal/later at that date | all agree | refused | independent counters identify one substituted date |
/// | some | either | at least two positives agree on another date; any incompatible companion | all agree | inconsistent | the claimed common date is contradicted |
///
/// `residual_total` distinguishes only the no-row cases; once bill rows exist,
/// date evidence comes exclusively from their counters. Unreachable combinations
/// are: rows=`none` with any counter, due relation, or non-absent implied date;
/// implied=`all agree`/`disagree` without a positive counter; and a missing
/// `BILLOVERDUE` field, which the wire parser rejects before this function.
/// See `TALLY_PROTOCOL_REFERENCE.md` §5.3 for the observed product-specific
/// boundary facts; this table deliberately makes no claim that future-due
/// counters are always empty or always zero.
fn classify_overdue_crosscheck(
    receivable_rows: &[NativeBillRow],
    payable_rows: &[NativeBillRow],
    residual_total: &ExactDecimal,
    requested_as_of: &TallyDate,
) -> Result<NativeOverdueCrosscheck, NativeOutstandingsError> {
    let rows = receivable_rows.iter().chain(payable_rows.iter());
    let mut any_row = false;
    let mut informative_dates = Vec::new();
    let mut counterless_rows = Vec::new();
    let mut zero_rows = Vec::new();

    for row in rows {
        any_row = true;
        let Some(tally_overdue) = row.tally_overdue_days else {
            counterless_rows.push(row);
            continue;
        };
        if tally_overdue == 0 {
            zero_rows.push(row);
            continue;
        }

        let Some(implied) = implied_as_of_date(&row.due_date, tally_overdue)? else {
            return Ok(NativeOverdueCrosscheck::Inconsistent);
        };
        informative_dates.push(implied);
    }

    if !any_row {
        return Ok(if residual_total.is_zero() {
            NativeOverdueCrosscheck::UnconfirmedAsOfWithoutEffectiveDateEvidence
        } else {
            NativeOverdueCrosscheck::UnconfirmedAsOfWithoutBillReferences
        });
    }
    let Some(tally_as_of) = informative_dates.first() else {
        return Ok(NativeOverdueCrosscheck::UnconfirmedAsOfWithoutEffectiveDateEvidence);
    };
    if informative_dates
        .iter()
        .any(|implied_as_of| implied_as_of != tally_as_of)
    {
        return Ok(NativeOverdueCrosscheck::Inconsistent);
    }
    if counterless_rows
        .iter()
        .any(|row| row.due_date <= *tally_as_of)
        || zero_rows.iter().any(|row| row.due_date < *tally_as_of)
    {
        return Ok(NativeOverdueCrosscheck::Inconsistent);
    }
    if *tally_as_of == *requested_as_of {
        return Ok(NativeOverdueCrosscheck::Honored);
    }
    if informative_dates.len() >= 2 {
        Ok(NativeOverdueCrosscheck::RefusedAsOf {
            tally_as_of: tally_as_of.clone(),
        })
    } else {
        Ok(NativeOverdueCrosscheck::Inconsistent)
    }
}

fn implied_as_of_date(
    due_date: &TallyDate,
    tally_overdue_days: i64,
) -> Result<Option<TallyDate>, NativeOutstandingsError> {
    let Ok(overdue_days) = u32::try_from(tally_overdue_days) else {
        return Ok(None);
    };
    let day = civil_day(due_date)?
        .checked_add(i64::from(overdue_days))
        .ok_or(NativeOutstandingsError::ArithmeticOverflow)?;
    let (year, month, day) = civil_from_day(day);
    if !(1..=9999).contains(&year) {
        return Ok(None);
    }
    TallyDate::parse(format!("{year:04}{month:02}{day:02}"))
        .map(Some)
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_implied_as_of_invalid"))
}

/// Per-party residual: `ledger CLOSINGBALANCE - sum(receivable BILLCL) -
/// sum(payable BILLCL)`. The native Bills Receivable/Payable reports only
/// ever list NAMED bills, so any non-zero residual on a party ledger is
/// exactly that party's on-account exposure — present in the ledger balance
/// but invisible to (and therefore unaged by) the bill-level reports. A
/// Sundry Debtor/Creditor with bill-wise tracking disabled has no bill rows
/// by construction, so its entire balance is such a residual.
fn compute_residuals(
    receivable_rows: &[NativeBillRow],
    payable_rows: &[NativeBillRow],
    ledgers: &[LedgerSnapshotEntry],
    group_snapshot: NativeGroupSnapshot<'_>,
) -> Result<(Vec<PartyResidual>, ExactDecimal, bool), NativeOutstandingsError> {
    let (groups, legacy_tolerances) = match group_snapshot {
        NativeGroupSnapshot::Complete([]) => {
            return Err(NativeOutstandingsError::InvalidResponse(
                "group_snapshot_empty",
            ))
        }
        NativeGroupSnapshot::Complete(groups) => (groups, false),
        NativeGroupSnapshot::LegacyFixtureWithoutGroups => (&[] as &[TallyNamedMaster], true),
    };
    let mut receivable_sums = BTreeMap::<&str, ExactDecimal>::new();
    for row in receivable_rows {
        let entry = receivable_sums
            .entry(row.party.as_str())
            .or_insert_with(ExactDecimal::zero);
        *entry = entry
            .checked_add(&row.closing_balance)
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
    }
    let mut payable_sums = BTreeMap::<&str, ExactDecimal>::new();
    for row in payable_rows {
        let entry = payable_sums
            .entry(row.party.as_str())
            .or_insert_with(ExactDecimal::zero);
        *entry = entry
            .checked_add(&row.closing_balance)
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
    }

    let mut residuals = Vec::new();
    let mut residual_total = ExactDecimal::zero();
    let mut has_unaged_receivable = false;
    let group_parents = group_parent_map(groups, legacy_tolerances)?;
    for ledger in ledgers {
        if !is_party_ledger(ledger, &group_parents, legacy_tolerances)? {
            continue;
        }
        let zero = ExactDecimal::zero();
        let receivable_sum = receivable_sums.get(ledger.name.as_str()).unwrap_or(&zero);
        let payable_sum = payable_sums.get(ledger.name.as_str()).unwrap_or(&zero);
        // Preserve the shipped Outstandings interpretation of Tally's
        // observed empty ledger closing-balance encoding as zero. Other
        // consumers (notably the party master workbook) receive the optional
        // value and can fail closed instead of inheriting this behaviour.
        let established_zero = ExactDecimal::zero();
        let closing_balance = ledger.closing_balance.as_ref().unwrap_or(&established_zero);
        let residual = closing_balance
            .checked_subtract(receivable_sum)
            .and_then(|value| value.checked_subtract(payable_sum))
            .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
        if !residual.is_zero() {
            let magnitude = residual
                .abs()
                .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)?;
            residual_total = add(&residual_total, &magnitude)?;
            // A receivable-side (debtor) ledger reports a negative closing
            // balance in this data; a non-zero residual there is exposure
            // the Bills Receivable report cannot see and therefore cannot
            // age.
            has_unaged_receivable |= residual.is_negative();
        }
        residuals.push(PartyResidual {
            party: ledger.name.clone(),
            amount: residual,
        });
    }
    Ok((residuals, residual_total, has_unaged_receivable))
}

/// A group's resolved ancestry link plus the identity key predefined-party
/// classification matches against. See [`group_identity_key`].
struct GroupAncestry {
    parent: Option<String>,
    identity: String,
}

/// The identity a group is classified by: Tally's own immutable
/// `RESERVEDNAME` when a complete reader captured it. The explicit legacy
/// fixture mode is the only mode allowed to fall back to the group's mutable
/// `NAME` when no `RESERVEDNAME` evidence exists at all.
///
/// `RESERVEDNAME` is Tally's field for exactly this purpose: a predefined
/// group carries its original identity there for life, impervious to the
/// group later being renamed over `Import Data`. Matching `NAME` instead is
/// the defect this classification used to have -- rename "Sundry Debtors"
/// and every one of its ledgers silently vanishes from the outstandings
/// report with no error.
///
/// The three states of `reserved_name`, and why each is handled the way it
/// is:
/// - `Some(non-empty)`: a trustworthy predefined identity. Used outright,
///   even where it disagrees with the group's current `NAME` -- that
///   disagreement is exactly the renamed-group case this function exists to
///   still get right.
/// - `Some("")`: Tally's own explicit signal that this specific group is
///   user-created, not predefined. This is deliberately NOT treated as "no
///   evidence, fall back to NAME": a custom group a user happens to name
///   "Sundry Debtors" is not the predefined group Tally ships (predefined
///   groups cannot be deleted, only renamed, so the real one -- wherever its
///   `NAME` now points -- is a different row carrying the non-empty
///   `RESERVEDNAME`). Falling back to `NAME` here would let that lookalike
///   masquerade as the real predefined group, reintroducing a mirror image
///   of the very bug being fixed. A ledger under such a group can still
///   reach the report through `is_party_ledger`'s other trigger
///   (`bill_wise_on`); only a bill-wise-off ledger there is affected.
/// - `None`: a complete snapshot promised group rows with this identity
///   field, so its absence is a contradiction and fails closed. Only
///   [`NativeGroupSnapshot::LegacyFixtureWithoutGroups`] can use the old
///   NAME-only tolerance; that variant has no group rows, so this branch
///   exists to make the evidence boundary explicit rather than to smooth a
///   complete read.
fn group_identity_key(
    group: &TallyNamedMaster,
    legacy_tolerances: bool,
) -> Result<String, NativeOutstandingsError> {
    match group.reserved_name.as_deref() {
        Some(reserved) => Ok(normalized_group_name(reserved)),
        None if legacy_tolerances => Ok(normalized_group_name(&group.name)),
        None => Err(NativeOutstandingsError::InvalidResponse(
            "group_reserved_name_missing",
        )),
    }
}

fn group_parent_map(
    groups: &[TallyNamedMaster],
    legacy_tolerances: bool,
) -> Result<BTreeMap<String, GroupAncestry>, NativeOutstandingsError> {
    let mut parents = BTreeMap::new();
    for group in groups {
        let name = normalized_group_name(&group.name);
        if name.is_empty() || parents.contains_key(&name) {
            return Err(NativeOutstandingsError::InvalidResponse(
                "group_name_missing_or_duplicate",
            ));
        }
        parents.insert(
            name,
            GroupAncestry {
                parent: group
                    .parent
                    .nonempty_returned_text()
                    .map(normalized_group_name)
                    .filter(|parent| !parent.is_empty()),
                identity: group_identity_key(group, legacy_tolerances)?,
            },
        );
    }
    Ok(parents)
}

fn is_party_ledger(
    ledger: &LedgerSnapshotEntry,
    group_parents: &BTreeMap<String, GroupAncestry>,
    legacy_tolerances: bool,
) -> Result<bool, NativeOutstandingsError> {
    if ledger.bill_wise_on {
        return Ok(true);
    }
    let Some(parent) = ledger.parent.as_deref() else {
        return Ok(false);
    };
    let mut current = normalized_group_name(parent);
    for _ in 0..=group_parents.len() {
        if is_tally_reserved_root(&current) || current.is_empty() {
            return Ok(false);
        }
        let Some(ancestry) = group_parents.get(&current) else {
            // A missing group is a contradiction for a read that claims
            // complete ancestry. Only the deliberately labelled historical
            // no-group fixture path may retain the direct NAME comparison.
            if !legacy_tolerances {
                return Err(NativeOutstandingsError::InvalidResponse(
                    "ledger_group_parent_unresolved",
                ));
            }
            return Ok(matches!(
                current.as_str(),
                "sundry debtors" | "sundry creditors"
            ));
        };
        let identity = ancestry.identity.as_str();
        if matches!(identity, "sundry debtors" | "sundry creditors") {
            return Ok(true);
        }
        match ancestry.parent.as_ref() {
            Some(parent) => current = parent.clone(),
            None => return Ok(false),
        }
    }
    Err(NativeOutstandingsError::InvalidResponse(
        "group_parent_cycle",
    ))
}

fn normalized_group_name(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn bill_anchor_date(row: &NativeBillRow, anchor: AgeingAnchor) -> &TallyDate {
    match anchor {
        AgeingAnchor::BillDate => &row.bill_date,
        AgeingAnchor::DueDate => &row.due_date,
    }
}

fn add(left: &ExactDecimal, right: &ExactDecimal) -> Result<ExactDecimal, NativeOutstandingsError> {
    left.checked_add(right)
        .map_err(|_| NativeOutstandingsError::ArithmeticOverflow)
}

/// Public for tests and callers that want to cross-check or display a raw
/// bill age without going through the full report computation.
pub fn age_in_days(from: &TallyDate, to: &TallyDate) -> Result<u32, NativeOutstandingsError> {
    days_between(from, to)
}

/// A bill whose due date has not arrived has zero overdue days in Tally's
/// `BILLOVERDUE` column, but no bill age to place into the ageing buckets.
/// Keep that state distinct from a bill due today: the latter is aged zero,
/// while the former is absent from ageing and from `oldest_bill_age_days`.
fn overdue_days(from: &TallyDate, to: &TallyDate) -> Result<Option<u32>, NativeOutstandingsError> {
    if from > to {
        return Ok(None);
    }
    days_between(from, to).map(Some)
}

fn days_between(from: &TallyDate, to: &TallyDate) -> Result<u32, NativeOutstandingsError> {
    let from = civil_day(from)?;
    let to = civil_day(to)?;
    u32::try_from(to - from)
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_after_as_of"))
}

/// Days-since-epoch via Howard Hinnant's `days_from_civil` algorithm — the
/// same computation `outstandings::compute` uses, duplicated here because
/// that module's helper is private to its own subtree.
fn civil_day(date: &TallyDate) -> Result<i64, NativeOutstandingsError> {
    let value = date.as_str();
    let year = value[0..4]
        .parse::<i64>()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_malformed"))?;
    let month = value[4..6]
        .parse::<i64>()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_malformed"))?;
    let day = value[6..8]
        .parse::<i64>()
        .map_err(|_| NativeOutstandingsError::InvalidDate("native_date_malformed"))?;
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Ok(era * 146_097 + day_of_era)
}

fn civil_from_day(day: i64) -> (i64, i64, i64) {
    let era = day.div_euclid(146_097);
    let day_of_era = day - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use crate::TALLY_SANITIZED_ROOT_MARKER;

    use super::*;

    #[test]
    fn reserved_root_policy_matches_canonical_window_for_marker_carrying_parents() {
        for root in [
            format!("{TALLY_SANITIZED_ROOT_MARKER} Primary"),
            "Primary".to_string(),
        ] {
            let groups = [TallyNamedMaster {
                name: "Sundry Debtors".to_string(),
                parent: crate::PartyLedgerMasterFieldObservation::Returned(root),
                reserved_name: Some("Sundry Debtors".to_string()),
            }];
            let ledgers = [LedgerSnapshotEntry {
                name: "Synthetic Ledger".to_string(),
                parent: Some("Sundry Debtors".to_string()),
                closing_balance: Some(ExactDecimal::zero()),
                opening_balance: ExactDecimal::zero(),
                bill_wise_on: false,
            }];
            compute_residuals(&[], &[], &ledgers, NativeGroupSnapshot::Complete(&groups))
                .expect("shared reserved-root forms must terminate group ancestry");
        }

        let groups = [TallyNamedMaster {
            name: "Sundry Debtors".to_string(),
            parent: crate::PartyLedgerMasterFieldObservation::Returned("Primary".to_string()),
            reserved_name: Some("Sundry Debtors".to_string()),
        }];
        for parent in [
            format!("{TALLY_SANITIZED_ROOT_MARKER} Resave"),
            format!("{TALLY_SANITIZED_ROOT_MARKER}{TALLY_SANITIZED_ROOT_MARKER} Primary"),
        ] {
            let ledgers = [LedgerSnapshotEntry {
                name: "Synthetic Ledger".to_string(),
                parent: Some(parent),
                closing_balance: Some(ExactDecimal::zero()),
                opening_balance: ExactDecimal::zero(),
                bill_wise_on: false,
            }];

            assert!(matches!(
                compute_residuals(&[], &[], &ledgers, NativeGroupSnapshot::Complete(&groups)),
                Err(NativeOutstandingsError::InvalidResponse(
                    "ledger_group_parent_unresolved"
                ))
            ));
        }
    }
}
