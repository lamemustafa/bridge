import { Building2, Cable, Check, Circle } from "lucide-react";
import { tallyReadinessState } from "./tally-company-selection";

type Props = {
  config: { host: string; port: number };
  endpointReachable: boolean;
  passportObserved: boolean;
  companySaved: boolean;
  companyLive: boolean;
  companyName?: string;
  busy: boolean;
  settingsLocked: boolean;
  onHostChange: (value: string) => void;
  onPortChange: (value: number) => void;
  onCheck: () => void;
};

type ReadinessStepProps = {
  complete: boolean;
  number: number;
  title: string;
  detail: string;
};

export function TallyReadinessFlow({
  config,
  endpointReachable,
  passportObserved,
  companySaved,
  companyLive,
  companyName,
  busy,
  settingsLocked,
  onHostChange,
  onPortChange,
  onCheck,
}: Props) {
  const endpointComplete = endpointReachable && passportObserved;
  const readiness = tallyReadinessState({
    endpointComplete,
    companySelected: Boolean(companyName),
    companyCurrent: companyLive,
    companySaved,
  });
  const nextStep = readiness.companyReady
    ? "Company evidence is saved"
    : endpointComplete
      ? "Choose the intended company"
      : "Check the local Tally connection";

  return (
    <section className="tally-readiness" aria-labelledby="tally-readiness-title">
      <div className="tally-readiness-intro">
        <div>
          <p className="eyebrow">Local Tally readiness</p>
          <h2 id="tally-readiness-title">Get one company ready for evidence</h2>
          <p>Bridge checks the local endpoint, observes the company identity, then saves the reviewed scope. Each step is read-only; none establishes accounting completeness.</p>
        </div>
        <div className="tally-readiness-next" aria-live="polite">
          <span>Next</span>
          <strong>{nextStep}</strong>
        </div>
      </div>

      <ol className="tally-readiness-steps" aria-label="Tally readiness steps">
        <ReadinessStep complete={endpointComplete} number={1} title="Check local Tally" detail={endpointComplete ? "Endpoint and capability observation available" : "Use the configured host and port"} />
        <ReadinessStep complete={Boolean(companyName)} number={2} title="Choose company" detail={companyName ? `${companyName} selected for review` : "Select a company with an observed GUID"} />
        <ReadinessStep complete={readiness.companyReady} number={3} title="Save reviewed scope" detail={readiness.companyReady ? "Observed company identity saved for this endpoint" : "Save only after review"} />
      </ol>

      <div className="tally-readiness-action">
        <div className="tally-readiness-fields">
          <label>
            Host
            <input disabled={busy || settingsLocked} value={config.host} onChange={(event) => onHostChange(event.target.value)} />
          </label>
          <label>
            Port
            <input disabled={busy || settingsLocked} type="number" min="1" max="65535" value={config.port} onChange={(event) => onPortChange(Number(event.target.value))} />
          </label>
        </div>
        <div className="tally-readiness-action-copy">
          {settingsLocked ? <p>Endpoint settings are locked while the active snapshot continues against its reviewed source.</p> : null}
          {!endpointComplete ? <p>Start here. Bridge will only inspect the local Tally endpoint and its capability response.</p> : null}
          {endpointComplete && !companyName ? <p>Connection evidence is ready. Choose the intended company in the reviewed list below.</p> : null}
          {companyName && !companySaved ? <p>Review the selected company and save its scope below before relying on it elsewhere in Bridge.</p> : null}
          {companySaved ? <p>The saved identity remains evidence for its observed endpoint; re-check before any new live read.</p> : null}
        </div>
        <div className="tally-readiness-actions">
          {readiness.showCheck ? (
            <button className="primary" type="button" onClick={onCheck} disabled={busy}>
              <Cable size={18} /> {busy ? "Checking local Tally…" : endpointComplete ? "Refresh local Tally" : "Check local Tally"}
            </button>
          ) : (
            <span className="readiness-complete"><Check size={18} /> Company evidence saved</span>
          )}
          {readiness.showCompanyLink && <a className="secondary-action readiness-link" href="#company-profile"><Building2 size={18} /> {companyName ? "Review company" : "Choose company"}</a>}
        </div>
      </div>
    </section>
  );
}

function ReadinessStep({ complete, number, title, detail }: ReadinessStepProps) {
  return (
    <li className={complete ? "is-complete" : undefined}>
      <span className="tally-readiness-step-icon" aria-hidden="true">{complete ? <Check size={16} /> : <Circle size={16} />}</span>
      <div>
        <span>Step {number}</span>
        <strong>{title}</strong>
        <small>{detail}</small>
      </div>
    </li>
  );
}
