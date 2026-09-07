import { Cable, CircleHelp } from "lucide-react";
import { TallyReadinessFlow } from "./TallyReadinessFlow";

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

export function SettingsScreen(props: Props) {
  return (
    <>
      <TallyReadinessFlow {...props} />
      <section className="panel wide settings-help" aria-labelledby="settings-help-heading">
        <div className="panel-heading">
          <div>
            <h2 id="settings-help-heading">Connection settings</h2>
            <p className="panel-description">
              Bridge connects to the Tally HTTP server on this computer. Use the same port that Tally exposes, then check the connection before choosing a company.
            </p>
          </div>
          <Cable size={22} aria-hidden="true" />
        </div>
        <div className="settings-help-grid">
          <div>
            <strong>Host</strong>
            <span>{props.config.host || "Not set"}</span>
            <small>Usually localhost when Tally is on this computer.</small>
          </div>
          <div>
            <strong>Port</strong>
            <span>{props.config.port}</span>
            <small>This is Tally&rsquo;s HTTP port, not a Bridge account setting.</small>
          </div>
          <div>
            <strong>What happens next</strong>
            <span>{props.companyReady ? "Your selected company is ready." : "Choose a company after the check succeeds."}</span>
            <small>Bridge reads from Tally; it does not change Tally data.</small>
          </div>
        </div>
        <p className="settings-help-note"><CircleHelp size={16} aria-hidden="true" /> If the check fails, the message below explains whether Tally is closed, the port is wrong, or the responder was not recognized.</p>
      </section>
    </>
  );
}
