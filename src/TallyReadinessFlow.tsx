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
    ? "Company setup is complete"
    : endpointComplete
      ? "Choose the intended company"
      : "Check the local Tally connection";

  return (
    <section className="tally-readiness" aria-labelledby="tally-readiness-title">
      <div className="tally-readiness-intro">
        <div>
          <h2 id="tally-readiness-title">Connect your Tally company</h2>
          <p>Check Tally, choose the company you need, then save it. Setup never changes data in Tally.</p>
        </div>
        <div className="tally-readiness-next" aria-live="polite">
          <span>Next</span>
          <strong>{nextStep}</strong>
        </div>
      </div>

      <ol className="tally-readiness-steps" aria-label="Tally readiness steps">
        <ReadinessStep complete={endpointComplete} number={1} title="Check Tally" detail={endpointComplete ? "Connection found" : "Use the local Tally address"} />
        <ReadinessStep complete={Boolean(companyName)} number={2} title="Choose company" detail={companyName ? "Company selected" : "Choose an open company"} />
        <ReadinessStep complete={readiness.companyReady} number={3} title="Save company" detail={readiness.companyReady ? "Ready to use" : "Save after selection"} />
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
          {!endpointComplete ? <p>Enter the address where Tally is running, then check the connection.</p> : null}
          {endpointComplete && !companyName ? <p>Connection found. Choose the company you want to use.</p> : null}
          {companyName && !companySaved ? <p>Save this company to use it in Bridge.</p> : null}
          {companySaved ? <p>This company is saved for the checked Tally connection.</p> : null}
        </div>
        <div className="tally-readiness-actions">
          {readiness.showCheck ? (
            <button className="primary" type="button" onClick={onCheck} disabled={busy}>
              <Cable size={18} /> {busy ? "Checking local Tally…" : endpointComplete ? "Refresh local Tally" : "Check local Tally"}
            </button>
          ) : (
            <span className="readiness-complete"><Check size={18} /> Company setup complete</span>
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
