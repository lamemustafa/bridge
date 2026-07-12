import React from "react";
import ReactDOM from "react-dom/client";
import { Activity, Building2, Cable, Cloud, Database, FileText, FolderOpen, KeyRound, Play, RefreshCw, ShieldCheck, UploadCloud } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import "./styles.css";

type TallyConfig = {
  host: string;
  port: number;
};

type ConnectionStatus = {
  reachable: boolean;
  server_text: string;
  product: "TallyPrime" | "Tally ERP 9" | "Unknown";
  error?: string;
};

type TallyCompany = {
  name: string;
  address?: string;
  state?: string;
  phone?: string;
  email?: string;
  income_tax_number?: string;
  mobile?: string;
  tan_reg_no?: string;
  tan_number?: string;
  website?: string;
  pincode?: string;
  gst_number?: string;
};

type TallyLedger = {
  name: string;
  parent?: string;
  email?: string;
  phone?: string;
  mobile?: string;
  state?: string;
  party_gstin?: string;
  opening_balance?: string;
};

type TallyVoucherLedgerEntry = {
  ledger_name?: string;
  amount?: string;
  is_deemed_positive?: string;
};

type TallyVoucher = {
  id?: string;
  date?: string;
  voucher_type?: string;
  voucher_number?: string;
  party_ledger_name?: string;
  narration?: string;
  amount?: string;
  ledger_entries: TallyVoucherLedgerEntry[];
};

type GstReturnDraft = {
  company: string;
  financial_year: string;
  gstr1: {
    b2b_invoice_count: number;
    b2c_invoice_count: number;
    credit_debit_note_count: number;
    hsn_summary_count: number;
  };
  gstr3b: {
    outward_taxable_value: string;
    integrated_tax: string;
    central_tax: string;
    state_tax: string;
    cess: string;
  };
  missing_fields: string[];
};

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
  token_info?: string | null;
  login_success: boolean;
  certificate_count?: number | null;
  certificates: DscCertificate[];
  error?: string | null;
};

type DscProbeReport = {
  platform: string;
  arch: string;
  workspace_root: string;
  bundled_library_root: string;
  physical_token_hint?: string | null;
  force_load: boolean;
  detect_only: boolean;
  attempts: DscAttempt[];
};

type AxalIntegration = "tally" | "documents" | "dsc";

type AxalValidationResponse = {
  valid: boolean;
  status?: string | null;
  last_synced?: string | null;
  error?: string | null;
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

type DocumentFile = {
  fullPath: string;
  relativePath: string;
  size: number;
  mtime: number;
  extension?: string | null;
  mimeType: string;
  hash?: string | null;
  contentHash?: string | null;
  serverFileKey?: string | null;
  multipartInfo?: {
    uploadId: string;
    parts: {
      partNumber: number;
      etag: string;
      size: number;
      bytesRead: number;
    }[];
  } | null;
};

type ScanDocumentsResponse = {
  files: DocumentFile[];
  totalSize: number;
  skipped: { path: string; reason: string }[];
};

type SyncDocumentsResponse = {
  success: boolean;
  uploadedFiles: DocumentFile[];
  failedFiles: { relativePath: string; error: string }[];
  duplicateCount: number;
  batchIds: string[];
};

type View = "dashboard" | "companies" | "gst" | "mirror" | "dsc" | "documents" | "axal";

function App() {
  const currentFinancialYear = React.useMemo(() => getCurrentFinancialYear(), []);
  const [config, setConfig] = React.useState<TallyConfig>({ host: "localhost", port: 9000 });
  const [status, setStatus] = React.useState<ConnectionStatus | null>(null);
  const [companies, setCompanies] = React.useState<TallyCompany[]>([]);
  const [selectedCompany, setSelectedCompany] = React.useState("");
  const [ledgers, setLedgers] = React.useState<TallyLedger[]>([]);
  const [vouchers, setVouchers] = React.useState<TallyVoucher[]>([]);
  const [voucherFrom, setVoucherFrom] = React.useState(currentFinancialYear.from);
  const [voucherTo, setVoucherTo] = React.useState(currentFinancialYear.to);
  const [companyError, setCompanyError] = React.useState<string | null>(null);
  const [dashboardError, setDashboardError] = React.useState<string | null>(null);
  const [gstCompany, setGstCompany] = React.useState("");
  const [gstFinancialYear, setGstFinancialYear] = React.useState(currentFinancialYear.label);
  const [draft, setDraft] = React.useState<GstReturnDraft | null>(null);
  const [dscReport, setDscReport] = React.useState<DscProbeReport | null>(null);
  const [dscDetectReport, setDscDetectReport] = React.useState<DscProbeReport | null>(null);
  const [dscPin, setDscPin] = React.useState("");
  const [dscError, setDscError] = React.useState<string | null>(null);
  const [dscAction, setDscAction] = React.useState<"detect" | "extract" | null>(null);
  const [dscSync, setDscSync] = React.useState<DscSyncResponse | null>(null);
  const [dscSyncing, setDscSyncing] = React.useState(false);
  const [axalBaseUrl, setAxalBaseUrl] = React.useState("https://complyeaze.com");
  const [axalIntegration, setAxalIntegration] = React.useState<AxalIntegration>("dsc");
  const [axalApiId, setAxalApiId] = React.useState("");
  const [axalApiKey, setAxalApiKey] = React.useState("");
  const [axalValidation, setAxalValidation] = React.useState<AxalValidationResponse | null>(null);
  const [axalConnection, setAxalConnection] = React.useState<AxalConnectionStatus | null>(null);
  const [axalError, setAxalError] = React.useState<string | null>(null);
  const [axalAction, setAxalAction] = React.useState<"validate" | "status" | null>(null);
  const [documentPaths, setDocumentPaths] = React.useState<string[]>([]);
  const [documentScan, setDocumentScan] = React.useState<ScanDocumentsResponse | null>(null);
  const [documentSync, setDocumentSync] = React.useState<SyncDocumentsResponse | null>(null);
  const [documentError, setDocumentError] = React.useState<string | null>(null);
  const [documentAction, setDocumentAction] = React.useState<"scan" | "sync" | null>(null);
  const [view, setView] = React.useState<View>("dashboard");
  const [busy, setBusy] = React.useState(false);

  async function checkTally() {
    setBusy(true);
    setDashboardError(null);
    try {
      const result = await invoke<ConnectionStatus>("check_tally_connection", { config });
      setStatus(result);
    } catch (error) {
      setStatus(null);
      setDashboardError(toErrorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function prepareDraft() {
    const company = gstCompany.trim();
    const financialYear = gstFinancialYear.trim();
    if (!company || !/^\d{4}-\d{4}$/.test(financialYear)) {
      setDashboardError("Enter a company and a financial year in YYYY-YYYY format.");
      return;
    }

    setBusy(true);
    setDashboardError(null);
    try {
      const result = await invoke<GstReturnDraft>("prepare_gst_return_draft", {
        request: {
          company,
          financial_year: financialYear,
        },
      });
      setDraft(result);
    } catch (error) {
      setDraft(null);
      setDashboardError(toErrorMessage(error));
    } finally {
      setBusy(false);
    }
  }

  async function fetchCompanies() {
    setBusy(true);
    setCompanyError(null);
    try {
      const result = await invoke<TallyCompany[]>("fetch_tally_companies", { config });
      setCompanies(result);
      setSelectedCompany((current) =>
        result.some((company) => company.name === current) ? current : result[0]?.name || "",
      );
    } catch (error) {
      setCompanyError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  async function fetchLedgers() {
    if (!selectedCompany) {
      setCompanyError("Select a company before fetching ledgers.");
      return;
    }

    setBusy(true);
    setCompanyError(null);
    try {
      const result = await invoke<TallyLedger[]>("fetch_tally_ledgers", {
        request: { config, company: selectedCompany },
      });
      setLedgers(result);
    } catch (error) {
      setCompanyError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  async function fetchVouchers() {
    if (!selectedCompany) {
      setCompanyError("Select a company before fetching vouchers.");
      return;
    }
    if (!voucherFrom || !voucherTo || voucherFrom > voucherTo) {
      setCompanyError("Choose a valid voucher date range with the from date on or before the to date.");
      return;
    }

    setBusy(true);
    setCompanyError(null);
    try {
      const result = await invoke<TallyVoucher[]>("fetch_tally_vouchers", {
        request: {
          config,
          company: selectedCompany,
          from: toTallyDate(voucherFrom),
          to: toTallyDate(voucherTo),
        },
      });
      setVouchers(result);
    } catch (error) {
      setCompanyError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }

  async function runDsc(detectOnly: boolean) {
    const pin = dscPin.trim();
    if (!detectOnly && !pin) {
      setDscError("Enter the DSC token PIN before extracting certificates.");
      return;
    }

    setBusy(true);
    setDscAction(detectOnly ? "detect" : "extract");
    setDscError(null);
    try {
      const result = detectOnly
        ? await invoke<DscProbeReport>("detect_dsc_token")
        : await invoke<DscProbeReport>("extract_dsc_certificates", { pins: [pin] });
      if (detectOnly) {
        setDscDetectReport(result);
      } else {
        setDscReport(result);
      }
    } catch (error) {
      setDscError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setDscAction(null);
    }
  }

  function axalCredentials() {
    return {
      api_key: axalApiKey,
      api_id: axalApiId,
      integration: axalIntegration,
      base_url: axalBaseUrl,
    };
  }

  async function validateAxal() {
    setBusy(true);
    setAxalAction("validate");
    setAxalError(null);
    try {
      const result = await invoke<AxalValidationResponse>("validate_axal_credentials", {
        credentials: axalCredentials(),
      });
      setAxalValidation(result);
    } catch (error) {
      setAxalError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setAxalAction(null);
    }
  }

  async function checkAxalStatus() {
    setBusy(true);
    setAxalAction("status");
    setAxalError(null);
    try {
      const result = await invoke<AxalConnectionStatus>("check_axal_connection_status", {
        credentials: axalCredentials(),
      });
      setAxalConnection(result);
    } catch (error) {
      setAxalError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setAxalAction(null);
    }
  }

  async function syncDscCertificate() {
    if (!primaryCertificate || !successfulDscAttempt || !axalConnection) {
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
          credentials: {
            ...axalCredentials(),
            integration: "dsc",
          },
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

  async function scanDocuments() {
    setBusy(true);
    setDocumentAction("scan");
    setDocumentError(null);
    setDocumentSync(null);
    try {
      const result = await invoke<ScanDocumentsResponse>("scan_document_paths", {
        request: {
          paths: documentPaths,
          use_hash: true,
          exclude_hidden_files: true,
          exclude_zero_byte_files: true,
        },
      });
      setDocumentScan(result);
    } catch (error) {
      setDocumentError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setDocumentAction(null);
    }
  }

  async function chooseDocumentFiles() {
    setDocumentError(null);
    try {
      const paths = await invoke<string[]>("select_document_files");
      if (paths.length > 0) {
        setDocumentPaths((current) => Array.from(new Set([...current, ...paths])));
        setDocumentScan(null);
        setDocumentSync(null);
      }
    } catch (error) {
      setDocumentError(error instanceof Error ? error.message : String(error));
    }
  }

  async function chooseDocumentFolder() {
    setDocumentError(null);
    try {
      const paths = await invoke<string[]>("select_document_folder");
      if (paths.length > 0) {
        setDocumentPaths((current) => Array.from(new Set([...current, ...paths])));
        setDocumentScan(null);
        setDocumentSync(null);
      }
    } catch (error) {
      setDocumentError(error instanceof Error ? error.message : String(error));
    }
  }

  async function syncDocuments() {
    if (!documentScan?.files.length || !axalConnection) {
      setDocumentError("Scan files and check AXAL workspace status before syncing documents.");
      return;
    }

    setBusy(true);
    setDocumentAction("sync");
    setDocumentError(null);
    try {
      const result = await invoke<SyncDocumentsResponse>("sync_documents_to_axal", {
        request: {
          credentials: {
            ...axalCredentials(),
            integration: "documents",
          },
          workspaceExternalId: axalConnection.workspace.id,
          files: documentScan.files,
          maxFilesPerBatch: 20,
        },
      });
      setDocumentSync(result);
    } catch (error) {
      setDocumentError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
      setDocumentAction(null);
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

  return (
    <main className="shell">
      <aside className="sidebar">
        <div className="brand">
          <ShieldCheck size={24} />
          <div>
            <strong>Bridge</strong>
            <span>Tauri Agent</span>
          </div>
        </div>
        <nav>
          <button className={view === "dashboard" ? "active" : ""} onClick={() => setView("dashboard")}>
            <Activity size={18} /> Dashboard
          </button>
          <button className={view === "companies" ? "active" : ""} onClick={() => setView("companies")}>
            <Building2 size={18} /> Companies
          </button>
          <button className={view === "gst" ? "active" : ""} onClick={() => setView("gst")}>
            <FileText size={18} /> GST Returns
          </button>
          <button className={view === "mirror" ? "active" : ""} onClick={() => setView("mirror")}>
            <Database size={18} /> Local Mirror
          </button>
          <button className={view === "dsc" ? "active" : ""} onClick={() => setView("dsc")}>
            <KeyRound size={18} /> DSC Token
          </button>
          <button className={view === "documents" ? "active" : ""} onClick={() => setView("documents")}>
            <FolderOpen size={18} /> Documents
          </button>
          <button className={view === "axal" ? "active" : ""} onClick={() => setView("axal")}>
            <Cloud size={18} /> AXAL Backend
          </button>
        </nav>
      </aside>

      <section className="content">
        <header>
          <div>
            <p className="eyebrow">Foundation build</p>
            <h1>Tally and GST sync core</h1>
          </div>
          <button className="primary" onClick={checkTally} disabled={busy}>
            <Cable size={18} />
            Check Tally
          </button>
        </header>

        {view === "dashboard" && (
          <>
            <section className="toolbar">
              <label>
                Host
                <input value={config.host} onChange={(event) => setConfig({ ...config, host: event.target.value })} />
              </label>
              <label>
                Port
                <input
                  type="number"
                  min="1"
                  max="65535"
                  value={config.port}
                  onChange={(event) => setConfig({ ...config, port: Number(event.target.value) })}
                />
              </label>
              <label>
                GST company
                <input value={gstCompany} onChange={(event) => setGstCompany(event.target.value)} />
              </label>
              <label>
                Financial year
                <input
                  value={gstFinancialYear}
                  placeholder="YYYY-YYYY"
                  onChange={(event) => setGstFinancialYear(event.target.value)}
                />
              </label>
              <button onClick={prepareDraft} disabled={busy}>
                <Play size={18} />
                Prepare GST Draft
              </button>
            </section>

            {dashboardError && <div className="error-banner">{dashboardError}</div>}

            <section className="grid">
              <article className="panel">
                <h2>Tally connection</h2>
                <dl>
                  <div><dt>Status</dt><dd>{status ? (status.reachable ? "Reachable" : "Not reachable") : "Not checked"}</dd></div>
                  <div><dt>Product</dt><dd>{status?.product ?? "Unknown"}</dd></div>
                  <div><dt>Server</dt><dd>{status?.server_text || status?.error || "Waiting for check"}</dd></div>
                </dl>
              </article>

              <article className="panel">
                <h2>GST preparation</h2>
                <dl>
                  <div><dt>Company</dt><dd>{draft?.company ?? "No draft yet"}</dd></div>
                  <div><dt>GSTR-1 B2B</dt><dd>{draft?.gstr1.b2b_invoice_count ?? 0}</dd></div>
                  <div><dt>GSTR-3B taxable</dt><dd>{draft?.gstr3b.outward_taxable_value ?? "0.00"}</dd></div>
                </dl>
              </article>
            </section>

            <section className="status-strip">
              <span>Serial Tally queue: ready</span>
              <span>SQLite mirror: schema only</span>
              <span>DSC: token detection and certificate extraction</span>
            </section>
          </>
        )}

        {view === "companies" && (
          <>
            <section className="toolbar">
              <label>
                Host
                <input value={config.host} onChange={(event) => setConfig({ ...config, host: event.target.value })} />
              </label>
              <label>
                Port
                <input
                  type="number"
                  min="1"
                  max="65535"
                  value={config.port}
                  onChange={(event) => setConfig({ ...config, port: Number(event.target.value) })}
                />
              </label>
              <button onClick={fetchCompanies} disabled={busy}>
                <RefreshCw size={18} />
                {busy ? "Fetching..." : "Fetch Companies"}
              </button>
            </section>

            <section className="toolbar secondary-toolbar">
              <label>
                Company
                <select
                  value={selectedCompany}
                  onChange={(event) => {
                    setSelectedCompany(event.target.value);
                    setLedgers([]);
                    setVouchers([]);
                  }}
                >
                  <option value="">Select company</option>
                  {companies.map((company) => (
                    <option value={company.name} key={company.name}>
                      {company.name}
                    </option>
                  ))}
                </select>
              </label>
              <button onClick={fetchLedgers} disabled={busy || !selectedCompany}>
                <RefreshCw size={18} />
                Fetch Ledgers
              </button>
              <label>
                From
                <input type="date" value={voucherFrom} onChange={(event) => setVoucherFrom(event.target.value)} />
              </label>
              <label>
                To
                <input type="date" value={voucherTo} onChange={(event) => setVoucherTo(event.target.value)} />
              </label>
              <button onClick={fetchVouchers} disabled={busy || !selectedCompany}>
                <RefreshCw size={18} />
                Fetch Vouchers
              </button>
            </section>

            <article className="panel wide">
              <div className="panel-heading">
                <h2>Companies</h2>
                <span>{companies.length} loaded</span>
              </div>

              {companyError && <div className="error-banner">{companyError}</div>}

              {companies.length === 0 && !companyError ? (
                <div className="empty-state">
                  <Building2 size={32} />
                  <strong>No companies fetched yet</strong>
                  <span>Start Tally, confirm the XML server is enabled, then fetch from localhost:9000.</span>
                </div>
              ) : (
                <div className="table-wrap">
                  <table>
                    <thead>
                      <tr>
                        <th>Name</th>
                        <th>State</th>
                        <th>GSTIN</th>
                        <th>Email</th>
                        <th>Phone</th>
                        <th>Pincode</th>
                      </tr>
                    </thead>
                    <tbody>
                      {companies.map((company) => (
                        <tr key={company.name}>
                          <td>{company.name}</td>
                          <td>{company.state || "-"}</td>
                          <td>{company.gst_number || "-"}</td>
                          <td>{company.email || "-"}</td>
                          <td>{company.phone || company.mobile || "-"}</td>
                          <td>{company.pincode || "-"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </article>

            <section className="grid data-grid">
              <article className="panel">
                <div className="panel-heading">
                  <h2>Ledgers</h2>
                  <span>{ledgers.length} loaded</span>
                </div>
                {ledgers.length === 0 ? (
                  <div className="empty-state compact">
                    <strong>No ledgers fetched yet</strong>
                    <span>Select a company and fetch ledgers.</span>
                  </div>
                ) : (
                  <div className="table-wrap">
                    <table>
                      <thead>
                        <tr>
                          <th>Name</th>
                          <th>Parent</th>
                          <th>GSTIN</th>
                          <th>Balance</th>
                        </tr>
                      </thead>
                      <tbody>
                        {ledgers.slice(0, 100).map((ledger) => (
                          <tr key={`${ledger.parent || ""}-${ledger.name}`}>
                            <td>{ledger.name}</td>
                            <td>{ledger.parent || "-"}</td>
                            <td>{ledger.party_gstin || "-"}</td>
                            <td>{ledger.opening_balance || "-"}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </article>

              <article className="panel">
                <div className="panel-heading">
                  <h2>Vouchers</h2>
                  <span>{vouchers.length} loaded</span>
                </div>
                {vouchers.length === 0 ? (
                  <div className="empty-state compact">
                    <strong>No vouchers fetched yet</strong>
                    <span>Select a company and date range, then fetch vouchers.</span>
                  </div>
                ) : (
                  <div className="table-wrap">
                    <table>
                      <thead>
                        <tr>
                          <th>Date</th>
                          <th>Type</th>
                          <th>No.</th>
                          <th>Party</th>
                          <th>Entries</th>
                        </tr>
                      </thead>
                      <tbody>
                        {vouchers.slice(0, 100).map((voucher, index) => (
                          <tr key={voucher.id || `${voucher.voucher_number || "voucher"}-${index}`}>
                            <td>{formatTallyDate(voucher.date)}</td>
                            <td>{voucher.voucher_type || "-"}</td>
                            <td>{voucher.voucher_number || "-"}</td>
                            <td>{voucher.party_ledger_name || "-"}</td>
                            <td>{voucher.ledger_entries.length}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </article>
            </section>
          </>
        )}

        {view === "gst" && (
          <section className="grid">
            <article className="panel">
              <h2>GSTR-1 draft</h2>
              <dl>
                <div><dt>B2B invoices</dt><dd>{draft?.gstr1.b2b_invoice_count ?? 0}</dd></div>
                <div><dt>B2C invoices</dt><dd>{draft?.gstr1.b2c_invoice_count ?? 0}</dd></div>
                <div><dt>Credit/debit notes</dt><dd>{draft?.gstr1.credit_debit_note_count ?? 0}</dd></div>
                <div><dt>HSN summaries</dt><dd>{draft?.gstr1.hsn_summary_count ?? 0}</dd></div>
              </dl>
            </article>
            <article className="panel">
              <h2>GSTR-3B draft</h2>
              <dl>
                <div><dt>Taxable value</dt><dd>{draft?.gstr3b.outward_taxable_value ?? "0.00"}</dd></div>
                <div><dt>IGST</dt><dd>{draft?.gstr3b.integrated_tax ?? "0.00"}</dd></div>
                <div><dt>CGST</dt><dd>{draft?.gstr3b.central_tax ?? "0.00"}</dd></div>
                <div><dt>SGST</dt><dd>{draft?.gstr3b.state_tax ?? "0.00"}</dd></div>
              </dl>
            </article>
          </section>
        )}

        {view === "mirror" && (
          <section className="grid">
            <article className="panel wide">
              <h2>Local mirror</h2>
              <div className="empty-state">
                <Database size={32} />
                <strong>SQLite mirror is schema-only</strong>
                <span>Persistence wiring is not enabled in this Tauri agent build yet.</span>
              </div>
            </article>
          </section>
        )}

        {view === "dsc" && (
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
                    <button onClick={syncDscCertificate} disabled={busy || dscSyncing || !axalConnection || !axalApiId || !axalApiKey}>
                      <Cloud size={18} className={dscSyncing ? "pulse-icon" : ""} />
                      {dscSyncing ? "Syncing..." : "Sync Certificate"}
                    </button>
                  </div>
                )}
              </article>
            </section>
          </>
        )}

        {view === "documents" && (
          <>
            <section className="toolbar document-toolbar">
              <button onClick={chooseDocumentFiles} disabled={busy}>
                <FileText size={18} />
                Choose Files
              </button>
              <button onClick={chooseDocumentFolder} disabled={busy}>
                <FolderOpen size={18} />
                Choose Folder
              </button>
              <button onClick={() => { setDocumentPaths([]); setDocumentScan(null); setDocumentSync(null); }} disabled={busy || documentPaths.length === 0}>
                Clear
              </button>
              <button onClick={scanDocuments} disabled={busy || documentPaths.length === 0}>
                <RefreshCw size={18} className={documentAction === "scan" ? "spin" : ""} />
                {documentAction === "scan" ? "Scanning..." : "Scan"}
              </button>
              <button onClick={syncDocuments} disabled={busy || !documentScan?.files.length || !axalConnection || !axalApiId || !axalApiKey}>
                <UploadCloud size={18} className={documentAction === "sync" ? "pulse-icon" : ""} />
                {documentAction === "sync" ? "Syncing..." : "Sync Documents"}
              </button>
            </section>

            {documentError && <div className="error-banner">{documentError}</div>}

            <article className="panel wide selected-paths">
              <div className="panel-heading">
                <h2>Selected paths</h2>
                <span>{documentPaths.length} selected</span>
              </div>
              {documentPaths.length === 0 ? (
                <div className="empty-state compact">
                  <FolderOpen size={32} />
                  <strong>No paths selected</strong>
                  <span>Choose files or a folder before scanning.</span>
                </div>
              ) : (
                <div className="path-list">
                  {documentPaths.map((path) => (
                    <div key={path}>{path}</div>
                  ))}
                </div>
              )}
            </article>

            <section className="grid">
              <article className="panel">
                <h2>Scan summary</h2>
                {documentAction === "scan" ? (
                  <div className="empty-state compact">
                    <RefreshCw size={32} className="spin" />
                    <strong>Scanning documents</strong>
                    <span>Hashing files and preparing document metadata.</span>
                  </div>
                ) : (
                  <dl>
                    <div><dt>Files</dt><dd>{documentScan?.files.length ?? 0}</dd></div>
                    <div><dt>Total size</dt><dd>{formatBytes(documentScan?.totalSize ?? 0)}</dd></div>
                    <div><dt>Skipped</dt><dd>{documentScan?.skipped.length ?? 0}</dd></div>
                    <div><dt>Workspace</dt><dd>{axalConnection?.workspace.name || "Check AXAL status first"}</dd></div>
                  </dl>
                )}
              </article>

              <article className="panel">
                <h2>Sync summary</h2>
                {documentAction === "sync" ? (
                  <div className="empty-state compact">
                    <UploadCloud size={32} className="pulse-icon" />
                    <strong>Uploading documents</strong>
                    <span>Requesting upload URLs, sending files, and confirming the batch.</span>
                  </div>
                ) : (
                  <dl>
                    <div><dt>Status</dt><dd>{documentSync ? (documentSync.success ? "Complete" : "Partial") : "Not synced"}</dd></div>
                    <div><dt>Uploaded</dt><dd>{documentSync?.uploadedFiles.length ?? 0}</dd></div>
                    <div><dt>Failed</dt><dd>{documentSync?.failedFiles.length ?? 0}</dd></div>
                    <div><dt>Duplicates</dt><dd>{documentSync?.duplicateCount ?? 0}</dd></div>
                  </dl>
                )}
              </article>
            </section>

            <article className="panel wide data-grid">
              <div className="panel-heading">
                <h2>Files</h2>
                <span>{documentScan?.files.length ?? 0} ready</span>
              </div>
              {!documentScan?.files.length ? (
                <div className="empty-state compact">
                  <FolderOpen size={32} />
                  <strong>No files scanned</strong>
                  <span>Enter one or more file/folder paths, then scan.</span>
                </div>
              ) : (
                <div className="table-wrap">
                  <table>
                    <thead>
                      <tr>
                        <th>Path</th>
                        <th>Type</th>
                        <th>Size</th>
                        <th>Hash</th>
                      </tr>
                    </thead>
                    <tbody>
                      {documentScan.files.slice(0, 100).map((file) => (
                        <tr key={file.fullPath}>
                          <td>{file.relativePath}</td>
                          <td>{file.mimeType}</td>
                          <td>{formatBytes(file.size)}</td>
                          <td>{file.contentHash ? `${file.contentHash.slice(0, 12)}...` : "-"}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </article>
          </>
        )}

        {view === "axal" && (
          <>
            <section className="toolbar">
              <label>
                Base URL
                <input value={axalBaseUrl} onChange={(event) => setAxalBaseUrl(event.target.value)} />
              </label>
              <label>
                Integration
                <select value={axalIntegration} onChange={(event) => setAxalIntegration(event.target.value as AxalIntegration)}>
                  <option value="tally">Tally Prime</option>
                  <option value="documents">Document Sync</option>
                  <option value="dsc">DSC Management</option>
                </select>
              </label>
            </section>

            <section className="toolbar secondary-toolbar">
              <label>
                API ID
                <input value={axalApiId} onChange={(event) => setAxalApiId(event.target.value)} />
              </label>
              <label>
                API Key
                <input type="password" value={axalApiKey} onChange={(event) => setAxalApiKey(event.target.value)} />
              </label>
              <button onClick={validateAxal} disabled={busy || !axalApiId || !axalApiKey}>
                <RefreshCw size={18} className={axalAction === "validate" ? "spin" : ""} />
                {axalAction === "validate" ? "Validating..." : "Validate"}
              </button>
              <button onClick={checkAxalStatus} disabled={busy || !axalApiId || !axalApiKey}>
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
        )}
      </section>
    </main>
  );
}

ReactDOM.createRoot(document.getElementById("root")!).render(<App />);

function toTallyDate(value: string): string {
  return value.replace(/-/g, "");
}

function formatTallyDate(value?: string): string {
  if (!value || value.length !== 8) {
    return value || "-";
  }

  return `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6, 8)}`;
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / 1024 ** index;
  return `${value.toFixed(value >= 10 || index === 0 ? 0 : 1)} ${units[index]}`;
}

function getCurrentFinancialYear(now = new Date()): { label: string; from: string; to: string } {
  const year = now.getFullYear();
  const startYear = now.getMonth() >= 3 ? year : year - 1;
  const endYear = startYear + 1;
  return {
    label: `${startYear}-${endYear}`,
    from: `${startYear}-04-01`,
    to: `${endYear}-03-31`,
  };
}

function toErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
