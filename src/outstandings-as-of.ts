// SPDX-License-Identifier: Apache-2.0

import type { OutstandingsAgeingAnchor } from "./outstandings-copy";

/** Local calendar date for a native date input; UTC conversion would show a
 * different day for operators west/east of the UTC boundary. */
export function todayAsDateInput(now = new Date()) {
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export type OutstandingsAsOfSelection = {
  value: string;
  operatorSelected: boolean;
};

/** Starts an as-of field in automatic local-calendar mode. */
export function automaticOutstandingsAsOf(now = new Date()): OutstandingsAsOfSelection {
  return { value: todayAsDateInput(now), operatorSelected: false };
}

/** Records a deliberate operator date, which automatic refreshes must not replace. */
export function operatorSelectedOutstandingsAsOf(value: string): OutstandingsAsOfSelection {
  return { value, operatorSelected: true };
}

/** Refreshes only the automatic local-date default; an operator choice is durable. */
export function refreshAutomaticOutstandingsAsOf(
  selection: OutstandingsAsOfSelection,
  now = new Date(),
): OutstandingsAsOfSelection {
  return selection.operatorSelected ? selection : automaticOutstandingsAsOf(now);
}

/** Wait until the next local midnight, including daylight-saving calendar shifts. */
export function millisecondsUntilNextLocalMidnight(now = new Date()) {
  const nextMidnight = new Date(now);
  nextMidnight.setHours(24, 0, 0, 0);
  return Math.max(1, nextMidnight.getTime() - now.getTime());
}

/** A completed result is usable only for the date it requested. */
export type AsOfBoundValue<T> = {
  asOfYyyymmdd: string;
  value: T;
};

/** Discards a read that settled after the effective date changed. */
export function settleAsOfBoundValue<T>(
  currentAsOfYyyymmdd: string | null,
  requestedAsOfYyyymmdd: string,
  value: T,
): AsOfBoundValue<T> | null {
  if (currentAsOfYyyymmdd !== requestedAsOfYyyymmdd) return null;
  return { asOfYyyymmdd: requestedAsOfYyyymmdd, value };
}

/** Hides a completed result immediately when its request date is no longer current. */
export function asOfBoundValueForAsOf<T>(
  loaded: AsOfBoundValue<T> | null,
  currentAsOfYyyymmdd: string | null,
): T | null {
  return loaded?.asOfYyyymmdd === currentAsOfYyyymmdd ? loaded.value : null;
}

/** The Tauri contract accepts only canonical YYYYMMDD values. */
export function asOfYyyymmdd(value: string) {
  return /^\d{4}-\d{2}-\d{2}$/.test(value) ? value.replace(/-/g, "") : null;
}

export type OutstandingsConfig = { host: string; port: number };
export type OutstandingsCompany = { name: string; guid: string };

/** Builds the exact Tauri argument for one company's requested as-of date. */
export function singleCompanyOutstandingsInvokeArgument(
  config: OutstandingsConfig,
  company: OutstandingsCompany,
  asOf: string,
  ageingAnchor: OutstandingsAgeingAnchor,
) {
  const asOfYyyymmddValue = asOfYyyymmdd(asOf);
  if (!asOfYyyymmddValue) return null;
  return {
    request: {
      config,
      company: company.name,
      expected_company_guid: company.guid,
      currency_assertion: "INR" as const,
      as_of_yyyymmdd: asOfYyyymmddValue,
      ageing_anchor: ageingAnchor,
    },
  };
}

/** Builds the exact Tauri argument for the all-client comparison at one date. */
export function allCompaniesOutstandingsInvokeArgument(
  config: OutstandingsConfig,
  companies: OutstandingsCompany[],
  asOf: string,
  ageingAnchor: OutstandingsAgeingAnchor,
) {
  const asOfYyyymmddValue = asOfYyyymmdd(asOf);
  if (!asOfYyyymmddValue) return null;
  return {
    request: {
      config,
      companies: companies.map((company) => ({
        company: company.name,
        expected_company_guid: company.guid,
      })),
      currency_assertion: "INR" as const,
      as_of_yyyymmdd: asOfYyyymmddValue,
      ageing_anchor: ageingAnchor,
    },
  };
}

type StatementExportSource = {
  report: { company_name: string; as_of_yyyymmdd: string };
  ageing_anchor: OutstandingsAgeingAnchor;
  statement_open_bills?: unknown[];
  statement_unallocated_by_party?: Array<{ party: string; amount: string }>;
};

/** Keeps statement exports pinned to the returned report's actual as-of date. */
export function partyStatementInvokeArgument(
  result: StatementExportSource,
  party: string,
  format: "xlsx" | "pdf",
) {
  return {
    request: {
      company: result.report.company_name,
      as_of_yyyymmdd: result.report.as_of_yyyymmdd,
      party,
      format,
      ageing_anchor: result.ageing_anchor,
      open_bills: result.statement_open_bills ?? [],
      unallocated_by_party: result.statement_unallocated_by_party ?? [],
    },
  };
}

/** Keeps batch statement exports pinned to the returned report's actual date. */
export function bulkPartyStatementsInvokeArgument(
  result: StatementExportSource,
  destination: string,
  format: "xlsx" | "pdf",
) {
  return {
    request: {
      company: result.report.company_name,
      as_of_yyyymmdd: result.report.as_of_yyyymmdd,
      destination,
      format,
      ageing_anchor: result.ageing_anchor,
      open_bills: result.statement_open_bills ?? [],
      unallocated_by_party: result.statement_unallocated_by_party ?? [],
    },
  };
}
