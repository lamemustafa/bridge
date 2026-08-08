import React from "react";
import { Cloud, KeyRound, RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

type DscCertificate = {
  label: string;
  common_name?: string | null;
  organization?: string | null;
  issuer_name?: string | null;
  serial_number?: string | null;
  valid_from?: string | null;
  valid_to?: string | null;
  fingerprint?: string | null;
  parse_error?: string | null;
};

type DscAttempt = {
  token_type: string;
  library_path: string;
  library_exists: boolean;
  loaded: boolean;
  initialized: boolean;
  slot_count: number;
  login_success: boolean;
  certificate_count?: number | null;
  certificates: DscCertificate[];
  error?: string | null;
};

type DscProbeReport = {
  platform: string;
  arch: string;
  force_load: boolean;
  detect_only: boolean;
  attempts: DscAttempt[];
};

type DscSyncResponse = {
  success: boolean;
  message: string;
  results?: {
    created: number;
    updated: number;
    skipped: number;
    errors: string[];
  } | null;
};

type AxalIntegration = "tally" | "documents" | "dsc";

const DSC_METADATA_RETENTION_MS = 5 * 60 * 1000;

type Props = {
  busy: boolean;
  setBusy: (busy: boolean) => void;
  // Owned by App() and shared with the AXAL view -- read here, never
  // duplicated locally.
  axalConnection: { workspace: { id: string } } | null;
  axalSession: { id: string; integration: AxalIntegration } | null;
};

// Owns: the DSC token view (view === "dsc"), its token-PIN/report/sync
// state, and the detect/extract/sync handlers.
//
// Deliberately does NOT own: `axalConnection` or `axalSession`. Those are
// AXAL workspace-session state shared with the AXAL and Documents views, so
// they stay in App() and are passed down read-only rather than duplicated
// here. `busy` is likewise a cross-view flag owned by App().
export function DscScreen({ busy, setBusy, axalConnection, axalSession }: Props) {
  // In App(), this state persisted across view changes, so a dedicated
  // `if (view !== "dsc") clearDscSensitiveState()` effect cleared it the
  // instant the operator navigated away. Now that this state lives inside
  // DscScreen, which is only mounted while view === "dsc", React unmounting
  // this component on navigation away already discards it -- that effect is
  // therefore redundant here and was intentionally not carried over. The
  // 5-minute idle-timeout effect below still applies while this view stays
  // mounted and active.
  const [dscReport, setDscReport] = React.useState<DscProbeReport | null>(null);
  const [dscDetectReport, setDscDetectReport] = React.useState<DscProbeReport | null>(null);
  const [dscPin, setDscPin] = React.useState("");
  const [dscError, setDscError] = React.useState<string | null>(null);
  const [dscAction, setDscAction] = React.useState<"detect" | "extract" | null>(null);
  const [dscSync, setDscSync] = React.useState<DscSyncResponse | null>(null);
  const [dscSyncing, setDscSyncing] = React.useState(false);
  const dscRequestVersion = React.useRef(0);

  const clearDscSensitiveState = React.useCallback(() => {
    dscRequestVersion.current += 1;
    setDscReport(null);
    setDscDetectReport(null);
    setDscPin("");
    setDscSync(null);
  }, []);

  React.useEffect(() => {
    if (!dscReport && !dscDetectReport && !dscPin && !dscSync) return;
    const expiry = window.setTimeout(clearDscSensitiveState, DSC_METADATA_RETENTION_MS);
    return () => window.clearTimeout(expiry);
  }, [clearDscSensitiveState, dscDetectReport, dscPin, dscReport, dscSync]);

  async function runDsc(detectOnly: boolean) {
    const pin = dscPin;
    if (!detectOnly && !pin) {
      setDscError("Enter the DSC token PIN before extracting certificates.");
      return;
    }

    const requestVersion = ++dscRequestVersion.current;
    setBusy(true);
    setDscAction(detectOnly ? "detect" : "extract");
    setDscError(null);
    setDscReport(null);
    setDscDetectReport(null);
    setDscSync(null);
    if (!detectOnly) {
      setDscPin("");
    }
    try {
      const result = detectOnly
        ? await invoke<DscProbeReport>("detect_dsc_token")
        : await invoke<DscProbeReport>("extract_dsc_certificates", { pins: [pin] });
      if (requestVersion === dscRequestVersion.current) {
        if (detectOnly) {
          setDscDetectReport(result);
        } else {
          setDscReport(result);
        }
      }
    } catch (error) {
      if (requestVersion === dscRequestVersion.current) {
        setDscError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      setBusy(false);
      setDscAction(null);
    }
  }

  const successfulDscAttempt = dscReport?.attempts.find(
    (attempt) => attempt.login_success && attempt.certificates.length > 0,
  );
  const detectedDscAttempt = dscDetectReport?.attempts.find(
    (attempt) => attempt.loaded && attempt.initialized && attempt.slot_count > 0 && !attempt.error,
  );
  const primaryCertificate =
    successfulDscAttempt?.certificates.find((certificate) => certificate.common_name) ??
    successfulDscAttempt?.certificates[0];

  async function syncDscCertificate() {
    if (!primaryCertificate || !successfulDscAttempt || !axalConnection || axalSession?.integration !== "dsc") {
      setDscError("Extract a certificate and check AXAL workspace status before syncing.");
      return;
    }

    setDscSyncing(true);
    setDscError(null);
    try {
      const holderName =
        primaryCertificate.common_name || primaryCertificate.organization || primaryCertificate.label;
      const result = await invoke<DscSyncResponse>("sync_dsc_certificates_to_axal", {
        request: {
          credentialSessionId: axalSession.id,
          workspaceExternalId: axalConnection.workspace.id,
          certificates: [
            {
              holderName,
              provider: primaryCertificate.issuer_name || "Unknown",
              serialNumber: primaryCertificate.serial_number || "",
              tokenType: successfulDscAttempt.token_type,
              class: "Unknown",
              purpose: "Digital Signature",
              issueDate: primaryCertificate.valid_from || "",
              expirationDate: primaryCertificate.valid_to || "",
              clientName: holderName,
              metadata: {
                organization: primaryCertificate.organization,
                issuer: primaryCertificate.issuer_name,
                fingerprint: primaryCertificate.fingerprint,
                tokenType: successfulDscAttempt.token_type,
              },
            },
          ],
        },
      });
      setDscSync(result);
    } catch (error) {
      setDscError(error instanceof Error ? error.message : String(error));
    } finally {
      setDscSyncing(false);
    }
  }

  return (
    <>
      <section className="toolbar">
        <label>
          Token PIN
          <input
            type="password"
            value={dscPin}
            autoComplete="off"
            onChange={(event) => setDscPin(event.target.value)}
          />
        </label>
        <button onClick={() => runDsc(true)} disabled={busy}>
          <RefreshCw size={18} className={dscAction === "detect" ? "spin" : ""} />
          {dscAction === "detect" ? "Detecting..." : "Detect Token"}
        </button>
        <button onClick={() => runDsc(false)} disabled={busy || !dscPin.trim()}>
          <KeyRound size={18} className={dscAction === "extract" ? "pulse-icon" : ""} />
          {dscAction === "extract" ? "Extracting..." : "Extract Certificates"}
        </button>
      </section>

      {dscError && <div className="error-banner">{dscError}</div>}

      <section className="grid single-panel-grid">
        <article className="panel certificate-panel">
          <h2>Certificate summary</h2>
          {dscAction ? (
            <div className="empty-state compact">
              <RefreshCw size={32} className="spin" />
              <strong>{dscAction === "detect" ? "Detecting token" : "Reading certificate"}</strong>
              <span>This can take a few seconds while the token library initializes.</span>
            </div>
          ) : primaryCertificate ? (
            <dl>
              <div><dt>Client</dt><dd>{primaryCertificate.common_name || primaryCertificate.organization || primaryCertificate.label}</dd></div>
              <div><dt>Expiry</dt><dd>{primaryCertificate.valid_to || "Unknown"}</dd></div>
              <div><dt>Serial</dt><dd>{primaryCertificate.serial_number || "Unknown"}</dd></div>
              <div><dt>Provider</dt><dd>{successfulDscAttempt?.token_type ?? "Unknown"}</dd></div>
              <div><dt>Certificates</dt><dd>{successfulDscAttempt?.certificate_count ?? successfulDscAttempt?.certificates.length ?? 0}</dd></div>
              <div><dt>AXAL sync</dt><dd>{dscSync?.message || "Not synced"}</dd></div>
            </dl>
          ) : detectedDscAttempt ? (
            <div className="empty-state compact success-state">
            <KeyRound size={32} />
            <strong>Token detected</strong>
            <span>{detectedDscAttempt.token_type} token is available. Extract certificates to show holder details.</span>
            </div>
          ) : (
            <div className="empty-state compact">
            <KeyRound size={32} />
            <strong>No certificate loaded</strong>
            <span>Detect the token or extract certificates to show DSC holder details.</span>
            </div>
          )}
          {primaryCertificate && (
            <div className="panel-actions">
              <button onClick={syncDscCertificate} disabled={busy || dscSyncing || !axalConnection || axalSession?.integration !== "dsc"}>
                <Cloud size={18} className={dscSyncing ? "pulse-icon" : ""} />
                {dscSyncing ? "Syncing..." : "Sync Certificate"}
              </button>
            </div>
          )}
          {(dscReport || dscDetectReport) && (
            <div className="panel-actions">
              <button onClick={clearDscSensitiveState} disabled={busy || dscSyncing}>
                Clear certificate details
              </button>
              <span>Certificate and token details clear automatically after five minutes.</span>
            </div>
          )}
        </article>
      </section>
    </>
  );
}
