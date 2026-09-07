import { Cable, Check } from "lucide-react";

export type TallyReadinessFlowProps = {
  config: { host: string; port: number };
  endpointReachable: boolean;
  passportObserved: boolean;
  companyReady: boolean;
  busy: boolean;
  settingsLocked: boolean;
  settingsLockMessage: string | null;
  onHostChange: (value: string) => void;
  onPortChange: (value: number) => void;
  onCheck: () => void;
  onOpenCompanies: () => void;
  onOpenOverview: () => void;
};

export function TallyReadinessFlow({
  config,
  endpointReachable,
  passportObserved,
  companyReady,
  busy,
  settingsLocked,
  settingsLockMessage,
  onHostChange,
  onPortChange,
  onCheck,
  onOpenCompanies,
  onOpenOverview,
}: TallyReadinessFlowProps) {
  const endpointComplete = endpointReachable && passportObserved;
  const guidance = companyReady
    ? "Tally matches your saved company. Open Overview to view outstandings."
    : endpointComplete
    ? "Connection checked. Choose a company on Companies to continue."
    : "Enter the address where Tally is running, then check the connection.";

  return (
    <section className="tally-readiness" aria-label="Connect Tally">
      <p className="tally-readiness-intro">Connect the company that is currently open in Tally. Bridge does not change data in Tally.</p>
      <div className="tally-readiness-action">
        <div className="tally-readiness-fields">
          <label>
            Host
            <input disabled={busy || settingsLocked} value={config.host} onChange={(event) => onHostChange(event.target.value)} />
          </label>
          <label>
            Port
            <input aria-describedby="tally-port-help" disabled={busy || settingsLocked} type="number" min="1" max="65535" value={config.port} onChange={(event) => onPortChange(Number(event.target.value))} />
            <small id="tally-port-help">Tally&rsquo;s HTTP port, usually 9000.</small>
          </label>
        </div>
        <div className="tally-readiness-action-copy">
          {settingsLockMessage && <p role="status">{settingsLockMessage}</p>}
          <p aria-live="polite">{guidance}</p>
        </div>
        <div className="tally-readiness-actions">
          <button className="primary" type="button" onClick={onCheck} disabled={busy || settingsLocked}>
            {endpointComplete && !busy ? <Check size={18} /> : <Cable size={18} />}
            {busy ? "Checking Tally…" : endpointComplete ? "Check Tally again" : "Check Tally"}
          </button>
          {endpointComplete && (
            <button className="secondary-action" type="button" onClick={companyReady ? onOpenOverview : onOpenCompanies} disabled={busy || settingsLocked}>
              {companyReady ? "Open Overview" : "Choose a company"}
            </button>
          )}
        </div>
      </div>
    </section>
  );
}
