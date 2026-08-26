import React from "react";
import { FileText, FolderOpen, RefreshCw, UploadCloud } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

const TABLE_PREVIEW_LIMIT = 100;

type DocumentFile = {
  scanId: string;
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
  scanSessionId: string;
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

type SelectedDocumentPath = {
  selectionId: string;
  displayName: string;
};

type AxalIntegration = "tally" | "documents" | "dsc";

type DocumentsWorkspaceState = {
  documentPaths: SelectedDocumentPath[];
  documentScan: ScanDocumentsResponse | null;
  documentSync: SyncDocumentsResponse | null;
  documentError: string | null;
  documentAction: "scan" | "sync" | null;
};

export function createDocumentsWorkspaceState(): DocumentsWorkspaceState {
  return {
    documentPaths: [],
    documentScan: null,
    documentSync: null,
    documentError: null,
    documentAction: null,
  };
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

function formatPreviewCount(total: number, label = "loaded"): string {
  return `Showing ${Math.min(total, TABLE_PREVIEW_LIMIT)} of ${total} returned ${label}; source completeness not established`;
}

type Props = {
  busy: boolean;
  setBusy: (busy: boolean) => void;
  // Owned by App() and shared with the AXAL and DSC views -- read here,
  // never duplicated locally.
  axalConnection: { workspace: { id: string; name: string } } | null;
  axalSession: { id: string; integration: AxalIntegration } | null;
  workspaceState: DocumentsWorkspaceState;
  setWorkspaceState: React.Dispatch<React.SetStateAction<DocumentsWorkspaceState>>;
};

// Owns: the Documents view (view === "documents") and the
// choose/scan/sync/clear handlers.
//
// Deliberately does NOT own cross-view state. The AXAL connection/session,
// busy flag, and prepared Documents workspace all stay in App() so the
// operator can visit AXAL and return without losing selected files or the
// scanSessionId required for sync.
export function DocumentsScreen({
  busy,
  setBusy,
  axalConnection,
  axalSession,
  workspaceState,
  setWorkspaceState,
}: Props) {
  const { documentPaths, documentScan, documentSync, documentError, documentAction } = workspaceState;

  function updateWorkspaceState(patch: Partial<DocumentsWorkspaceState>) {
    setWorkspaceState((current) => ({ ...current, ...patch }));
  }

  async function scanDocuments() {
    setBusy(true);
    updateWorkspaceState({ documentAction: "scan", documentError: null, documentSync: null });
    try {
      const result = await invoke<ScanDocumentsResponse>("scan_document_paths", {
        request: {
          selection_ids: documentPaths.map((path) => path.selectionId),
          use_hash: true,
          exclude_hidden_files: true,
          exclude_zero_byte_files: true,
        },
      });
      updateWorkspaceState({ documentScan: result });
    } catch (error) {
      updateWorkspaceState({
        documentError: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
      updateWorkspaceState({ documentAction: null });
    }
  }

  async function chooseDocumentFiles() {
    updateWorkspaceState({ documentError: null });
    try {
      const paths = await invoke<SelectedDocumentPath[]>("select_document_files");
      if (paths.length > 0) {
        setWorkspaceState((current) => ({
          ...current,
          documentPaths: [...current.documentPaths, ...paths],
          documentScan: null,
          documentSync: null,
        }));
      }
    } catch (error) {
      updateWorkspaceState({
        documentError: error instanceof Error ? error.message : String(error),
      });
    }
  }

  async function chooseDocumentFolder() {
    updateWorkspaceState({ documentError: null });
    try {
      const paths = await invoke<SelectedDocumentPath[]>("select_document_folder");
      if (paths.length > 0) {
        setWorkspaceState((current) => ({
          ...current,
          documentPaths: [...current.documentPaths, ...paths],
          documentScan: null,
          documentSync: null,
        }));
      }
    } catch (error) {
      updateWorkspaceState({
        documentError: error instanceof Error ? error.message : String(error),
      });
    }
  }

  function clearDocuments() {
    void invoke("revoke_document_authorizations", {
      selectionIds: documentPaths.map((path) => path.selectionId),
      scanSessionId: documentScan?.scanSessionId ?? null,
    }).catch(() => undefined);
    setWorkspaceState(createDocumentsWorkspaceState());
  }

  async function syncDocuments() {
    if (!documentScan?.files.length || !axalConnection || axalSession?.integration !== "documents") {
      updateWorkspaceState({
        documentError: "Scan files and check AXAL workspace status before syncing documents.",
      });
      return;
    }

    setBusy(true);
    updateWorkspaceState({ documentAction: "sync", documentError: null });
    try {
      const result = await invoke<SyncDocumentsResponse>("sync_documents_to_axal", {
        request: {
          credentialSessionId: axalSession.id,
          workspaceExternalId: axalConnection.workspace.id,
          scanSessionId: documentScan.scanSessionId,
          files: documentScan.files,
          maxFilesPerBatch: 20,
        },
      });
      updateWorkspaceState({ documentSync: result });
    } catch (error) {
      updateWorkspaceState({
        documentError: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setBusy(false);
      updateWorkspaceState({ documentAction: null });
    }
  }

  return (
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
        <button onClick={clearDocuments} disabled={busy || (documentPaths.length === 0 && !documentScan)}>
          Clear
        </button>
        <button onClick={scanDocuments} disabled={busy || documentPaths.length === 0}>
          <RefreshCw size={18} className={documentAction === "scan" ? "spin" : ""} />
          {documentAction === "scan" ? "Scanning..." : "Scan"}
        </button>
        <button onClick={syncDocuments} disabled={busy || !documentScan?.files.length || !axalConnection || axalSession?.integration !== "documents"}>
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
              <div key={path.selectionId}>{path.displayName}</div>
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
          <span>{formatPreviewCount(documentScan?.files.length ?? 0, "ready")}</span>
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
                {documentScan.files.slice(0, TABLE_PREVIEW_LIMIT).map((file) => (
                  <tr key={file.scanId}>
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
  );
}
