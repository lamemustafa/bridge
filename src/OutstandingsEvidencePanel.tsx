import { outstandingsAgeingAnchorLabel } from "./outstandings-copy";
import type { EvidenceDrawerEntry } from "./evidence-drawer-entry";
import { companyIdentityLabel } from "./company-identity";
import { readProvenance } from "./outstandings-provenance";

type Props = {
  entry: EvidenceDrawerEntry;
};

function displayDate(value?: string) {
  if (!value || !/^\d{8}$/.test(value)) return "Not reported";
  return `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6, 8)}`;
}

function displayTime(value: number) {
  return new Date(value).toLocaleString();
}

function displayBytes(value: number) {
  return `${new Intl.NumberFormat().format(value)} bytes`;
}

export function OutstandingsEvidencePanel({ entry }: Props) {
  if (entry.kind === "local-only") {
    return (
      <section className="panel wide report-evidence-panel" aria-labelledby="report-evidence-heading">
        <h2 id="report-evidence-heading">No Outstandings read attached</h2>
        <p className="panel-description">This drawer was opened for local evidence review, not from an Outstandings report. It does not describe a current report read.</p>
      </section>
    );
  }

  if (entry.kind === "report-not-read") {
    return (
      <section className="panel wide report-evidence-panel" aria-labelledby="report-evidence-heading">
        <h2 id="report-evidence-heading">No Outstandings read available</h2>
        <p className="panel-description">No report-bound Outstandings read has completed for this view. Run or refresh the read before relying on report evidence.</p>
      </section>
    );
  }

  if (entry.kind === "report-read-failed") {
    return (
      <section className="panel wide report-evidence-panel" aria-labelledby="report-evidence-heading" role="alert">
        <h2 id="report-evidence-heading">Outstandings read failed</h2>
        <p className="panel-description">Bridge could not complete the report-bound read: {entry.message}</p>
      </section>
    );
  }

  const { evidence } = entry;

  return (
    <section className="panel wide report-evidence-panel" aria-labelledby="report-evidence-heading">
      <div className="panel-heading">
        <div>
          <h2 id="report-evidence-heading">Report-bound Outstandings read</h2>
          <p className="panel-description">This is the exact read that produced the report behind this drawer. It is separate from the Core Accounting history below.</p>
        </div>
        <span>{evidence.state === "complete" ? "Complete result" : evidence.state === "partial" ? "Partial result" : evidence.title}</span>
      </div>
      <dl className="report-evidence-facts">
        <div><dt>Company</dt><dd>{companyIdentityLabel(evidence.companyIdentity)}</dd></div>
        <div><dt>Read recorded</dt><dd>{displayTime(evidence.syncedAt)}</dd></div>
        {evidence.state === "complete" ? (
          <>
            <div><dt>As of</dt><dd>{displayDate(evidence.asOfYyyymmdd)}</dd></div>
            <div><dt>Ageing basis</dt><dd>{outstandingsAgeingAnchorLabel(evidence.ageingAnchor)}</dd></div>
            <div><dt>Currency assertion</dt><dd>{evidence.currencyAssertion}</dd></div>
            <div><dt>Read scope</dt><dd>{readProvenance(evidence.readProvenance)} · {displayBytes(evidence.sourceBytes)}</dd></div>
            <div><dt>Receivable</dt><dd>{evidence.receivableTotal}</dd></div>
            <div><dt>Payable</dt><dd>{evidence.payableTotal}</dd></div>
          </>
        ) : evidence.state === "partial" ? (
          <>
            <div><dt>Requested as of</dt><dd>{displayDate(evidence.requestedAsOfYyyymmdd)}</dd></div>
            <div><dt>Tally as of</dt><dd>{displayDate(evidence.tallyAsOfYyyymmdd)}</dd></div>
            <div><dt>Read attempted</dt><dd>{evidence.tallyReadAttempted ? "Yes" : "No"}</dd></div>
            <div><dt>Reason code</dt><dd><code>{evidence.reasonCode}</code></dd></div>
          </>
        ) : (
          <div><dt>Result</dt><dd>{evidence.message}</dd></div>
        )}
      </dl>
      {evidence.state === "partial" && (
        <p className="report-evidence-partial" role="status"><strong>{evidence.title}</strong> {evidence.message}</p>
      )}
    </section>
  );
}
