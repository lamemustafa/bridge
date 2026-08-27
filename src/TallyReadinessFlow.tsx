import { Cable, Check } from "lucide-react";

type Props = {
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
}: Props) {
  const endpointComplete = endpointReachable && passportObserved;
  const guidance = companyReady
    ? "Tally matches your saved company. You can open outstandings."
    : endpointComplete
    ? "Tally is connected. Choose the company you want to use below."
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
            <input disabled={busy || settingsLocked} type="number" min="1" max="65535" value={config.port} onChange={(event) => onPortChange(Number(event.target.value))} />
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
        </div>
      </div>
    </section>
  );
}
