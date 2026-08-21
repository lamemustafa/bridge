// SPDX-License-Identifier: Apache-2.0

export type OpenBill = {
  party: string;
  reference: string;
  bill_date: string;
  due_date: string;
  amount: string;
  age_days: number | null;
  kind: "receivable" | "payable";
};

// A party absent from the grouped map (`.get(party) === undefined`) genuinely
// has zero bill rows in the complete, uncapped source Bridge received from
// Tally -- its balance, if any, is unallocated. That is a real fact and reads
// as one.
//
// A party PRESENT in the map is split into two states so a display cap can
// never again collapse into the same "no bills" rendering as a true zero:
//
// - "loaded": every one of this party's bill rows is included in `bills`.
// - "not_loaded": this party alone has more open bill rows than Bridge
//   renders by default. `shown` is the capped subset for the default render;
//   `bills` is the complete set already held in memory (no further Tally read
//   is needed to see the rest -- Bridge already fetched it).
//
// This state is typed, not a rendered string, so a future surface cannot
// re-collapse "not loaded" into "zero" by accident.
export type PartyBillsState =
  | { status: "loaded"; bills: Array<OpenBill> }
  | { status: "not_loaded"; shown: Array<OpenBill>; bills: Array<OpenBill> };

/// Groups open bills by exact party name for the drill-down, capping each
/// PARTY's own bill count independently -- never the flattened cross-party
/// list. Capping the flattened list before grouping (the previous behaviour)
/// meant a party's bills could be pushed past the cutoff by other parties'
/// rows earlier in the source order, so a party with real, non-zero exposure
/// could render with an empty bill list -- indistinguishable from, and
/// mislabeled as, a party that genuinely carries no bill reference at all.
/// Capping per party after grouping means only a party whose OWN bill count
/// exceeds the limit is ever capped, and that party is marked `not_loaded`
/// rather than silently rendered as zero.
///
/// Returns null when statement rows are absent entirely (the voucher-scan
/// path never sends them) -- that null is what tells a row it must not be
/// expandable at all, distinct from a present map that simply has no entry
/// for a given party.
export function groupOpenBillsByParty(
  openBills: Array<OpenBill> | undefined,
  displayLimitPerParty: number,
): Map<string, PartyBillsState> | null {
  if (openBills === undefined) return null;
  const byParty = new Map<string, Array<OpenBill>>();
  for (const bill of openBills) {
    const list = byParty.get(bill.party);
    if (list) list.push(bill);
    else byParty.set(bill.party, [bill]);
  }
  const result = new Map<string, PartyBillsState>();
  for (const [party, bills] of byParty) {
    result.set(
      party,
      bills.length <= displayLimitPerParty
        ? { status: "loaded", bills }
        : { status: "not_loaded", shown: bills.slice(0, displayLimitPerParty), bills },
    );
  }
  return result;
}
