import React from "react";
import { Cloud, RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

type AxalIntegration = "tally" | "documents" | "dsc";

type AxalValidationResponse = {
  valid: boolean;
  status?: string | null;
  last_synced?: string | null;
  error?: string | null;
};

type AxalSessionResponse = {
  credentialSessionId: string;
  validation: AxalValidationResponse;
};

type AxalConnectionStatus = {
  connected: boolean;
  status: string;
  last_synced_at?: string | null;
  workspace: {
    id: string;
    name: string;
    billing_plan: string;
    storage_used: number;
    storage_limit: number;
  };
};

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${value.toFixed(value >= 10 || index === 0 ? 0 : 1)} ${units[index]}`;
}

type Props = {
  busy: boolean;
  setBusy: (busy: boolean) => void;
  // Owned by App() and shared with the DSC and Documents views -- read
  // here, never duplicated locally. This view still *writes* them because
  // validating credentials and checking connection status only ever
  // happens from here.
  axalConnection: AxalConnectionStatus | null;
  axalSession: { id: string; integration: AxalIntegration } | null;
  setAxalSession: (session: { id: string; integration: AxalIntegration } | null) => void;
  setAxalConnection: (connection: AxalConnectionStatus | null) => void;
};

// Owns: the AXAL backend view (view === "axal"), its credential form state
// (base URL, integration, API ID/key), the validation/status result state,
// and the validate/check-status handlers.
//
// Deliberately does NOT own: `axalConnection` or `axalSession`. Those are
// AXAL workspace-session state shared with the DSC and Documents views (both
// already extracted, both receive it as props from App()), so they stay in
// App() and are passed down here rather than duplicated. `busy` is likewise
// a cross-view flag owned by App().
export function AxalScreen({ busy, setBusy, axalConnection, axalSession, setAxalSession, setAxalConnection }: Props) {
  const [axalBaseUrl, setAxalBaseUrl] = React.useState("https://complyeaze.com");
  const [axalIntegration, setAxalIntegration] = React.useState<AxalIntegration>("dsc");
  const [axalApiId, setAxalApiId] = React.useState("");
  const [axalApiKey, setAxalApiKey] = React.useState("");
  const [axalValidation, setAxalValidation] = React.useState<AxalValidationResponse | null>(null);
  const [axalError, setAxalError] = React.useState<string | null>(null);
  const [axalAction, setAxalAction] = React.useState<"validate" | "status" | null>(null);

  function axalCredentials() {
    return {
      api_key: axalApiKey,
      api_id: axalApiId,
      integration: axalIntegration,
      base_url: axalBaseUrl,
    };
  }

  function invalidateAxalSession() {
    const sessionId = axalSession?.id;
    setAxalSession(null);
    setAxalConnection(null);
    if (sessionId) {
      void invoke("revoke_axal_credential_session", {
        credentialSessionId: sessionId,
      }).catch(() => undefined);
    }
  }

  async function validateAxal() {
    setBusy(true);
    setAxalAction("validate");
    setAxalError(null);
    try {
      const result = await invoke<AxalSessionResponse>("validate_axal_credentials", {
        credentials: axalCredentials(),
      });
      setAxalValidation(result.validation);
      setAxalSession({ id: result.credentialSessionId, integration: axalIntegration });
      setAxalConnection(null);
    } catch (error) {
      setAxalError(error instanceof Error ? error.message : String(error));
    } finally {
      setAxalApiKey("");
      setBusy(false);
      setAxalAction(null);
    }
  }

  async function checkAxalStatus() {
    if (!axalSession) {
      setAxalError("Validate AXAL credentials before checking connection status.");
      return;
    }
    setBusy(true);
    setAxalAction("status");
    setAxalError(null);
    try {
      const result = await invoke<AxalConnectionStatus>("check_axal_connection_status", {
        credentialSessionId: axalSession.id,
      });
      setAxalConnection(result);
    } catch (error) {
      setAxalError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setAxalAction(null);
    }
  }

  return (
    <>
      <section className="toolbar">
        <label>
          Base URL
          <input value={axalBaseUrl} onChange={(event) => { setAxalBaseUrl(event.target.value); invalidateAxalSession(); }} />
        </label>
        <label>
          Integration
          <select value={axalIntegration} onChange={(event) => { setAxalIntegration(event.target.value as AxalIntegration); invalidateAxalSession(); }}>
            <option value="tally">Tally Prime</option>
            <option value="documents">Document Sync</option>
            <option value="dsc">DSC Management</option>
          </select>
        </label>
      </section>

      <section className="toolbar secondary-toolbar">
        <label>
          API ID
          <input value={axalApiId} onChange={(event) => { setAxalApiId(event.target.value); invalidateAxalSession(); }} />
        </label>
        <label>
          API Key
          <input type="password" value={axalApiKey} onChange={(event) => { setAxalApiKey(event.target.value); invalidateAxalSession(); }} />
        </label>
        <button onClick={validateAxal} disabled={busy || !axalApiId || !axalApiKey}>
          <RefreshCw size={18} className={axalAction === "validate" ? "spin" : ""} />
          {axalAction === "validate" ? "Validating..." : "Validate"}
        </button>
        <button onClick={checkAxalStatus} disabled={busy || !axalSession}>
          <Cloud size={18} className={axalAction === "status" ? "pulse-icon" : ""} />
          {axalAction === "status" ? "Checking..." : "Check Status"}
        </button>
      </section>

      {axalError && <div className="error-banner">{axalError}</div>}

      <section className="grid">
        <article className="panel">
          <h2>Credential validation</h2>
          {axalAction === "validate" ? (
            <div className="empty-state compact">
              <RefreshCw size={32} className="spin" />
              <strong>Validating credentials</strong>
              <span>Checking the API key against AXAL.</span>
            </div>
          ) : (
            <dl>
              <div><dt>Status</dt><dd>{axalValidation ? (axalValidation.valid ? "Valid" : "Invalid") : "Not checked"}</dd></div>
              <div><dt>Server state</dt><dd>{axalValidation?.status || "-"}</dd></div>
              <div><dt>Last synced</dt><dd>{axalValidation?.last_synced || "-"}</dd></div>
              <div><dt>Error</dt><dd>{axalValidation?.error || "-"}</dd></div>
            </dl>
          )}
        </article>

        <article className="panel">
          <h2>Workspace status</h2>
          {axalAction === "status" ? (
            <div className="empty-state compact">
              <RefreshCw size={32} className="spin" />
              <strong>Checking workspace</strong>
              <span>Fetching integration status and workspace metadata.</span>
            </div>
          ) : (
            <dl>
              <div><dt>Connection</dt><dd>{axalConnection ? (axalConnection.connected ? "Connected" : "Disconnected") : "Not checked"}</dd></div>
              <div><dt>Status</dt><dd>{axalConnection?.status || "-"}</dd></div>
              <div><dt>Workspace</dt><dd>{axalConnection?.workspace.name || "-"}</dd></div>
              <div><dt>Plan</dt><dd>{axalConnection?.workspace.billing_plan || "-"}</dd></div>
              <div><dt>Storage</dt><dd>{axalConnection ? `${formatBytes(axalConnection.workspace.storage_used)} / ${formatBytes(axalConnection.workspace.storage_limit)}` : "-"}</dd></div>
              <div><dt>Last synced</dt><dd>{axalConnection?.last_synced_at || "-"}</dd></div>
            </dl>
          )}
        </article>
      </section>
    </>
  );
}
