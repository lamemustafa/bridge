type TallyConfig = { host: string; port: number };
type TallyCompany = { name: string; guid: string };

export type TrialBalanceExportSummary = {
  path: string;
  company: string;
  from_yyyymmdd: string;
  to_yyyymmdd: string;
  ledger_count: number;
};

export function trialBalanceInvokeArgument(
  config: TallyConfig,
  company: TallyCompany,
  asOfYyyymmdd: string | null,
) {
  if (!asOfYyyymmdd) return null;
  return {
    request: {
      config,
      company: company.name,
      expected_company_guid: company.guid,
      as_of_yyyymmdd: asOfYyyymmdd,
    },
  };
}

export function trialBalanceExportMessage(summary: TrialBalanceExportSummary) {
  return `Book-to-date Trial Balance · ${summary.ledger_count} ledgers · ${displayDate(summary.from_yyyymmdd)} to ${displayDate(summary.to_yyyymmdd)}`;
}

function displayDate(value: string) {
  if (!/^\d{8}$/.test(value)) return value;
  return `${value.slice(6, 8)}-${value.slice(4, 6)}-${value.slice(0, 4)}`;
}
