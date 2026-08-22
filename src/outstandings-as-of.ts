// SPDX-License-Identifier: Apache-2.0

/** Local calendar date for a native date input; UTC conversion would show a
 * different day for operators west/east of the UTC boundary. */
export function todayAsDateInput(now = new Date()) {
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
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
    },
  };
}

/** Builds the exact Tauri argument for the all-client comparison at one date. */
export function allCompaniesOutstandingsInvokeArgument(
  config: OutstandingsConfig,
  companies: OutstandingsCompany[],
  asOf: string,
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
    },
  };
}

type StatementExportSource = {
  report: { company_name: string; as_of_yyyymmdd: string };
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
      open_bills: result.statement_open_bills ?? [],
      unallocated_by_party: result.statement_unallocated_by_party ?? [],
    },
  };
}
