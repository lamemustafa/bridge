import React from "react";
import { FilePlus2, FolderOpen, Save, ShieldAlert, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./source-draft.css";
import {
  SourceDraft,
  SourceDraftAction,
  SourceDraftCatalogTargets,
  SourceDraftProposedEntry,
  SourceDraftProposal,
  SourceDraftRow,
  SourceDraftSourceEntry,
  SourceDraftScreenProps,
  SourceDraftSide,
  SourceDraftVoucherType,
} from "./source-draft-types";

const PAGE_SIZE = 25;
const VOUCHER_TYPES: SourceDraftVoucherType[] = ["Payment", "Receipt", "Journal", "Contra"];

function catalogSelectionKey(rowPosition: number, entryPosition: number) {
  return `${rowPosition}:${entryPosition}`;
}

function retainCurrentSessionSelections(draft: SourceDraft, selections: Record<string, string>) {
  return Object.fromEntries(Object.entries(selections).filter(([key, target]) => {
    const [rowPosition, entryPosition] = key.split(":").map(Number);
    return draft.rows.find((row) => row.position === rowPosition)?.proposal.entries[entryPosition - 1]?.ledger === target;
  }));
}

function cloneProposal(proposal: SourceDraftProposal): SourceDraftProposal {
  return { ...proposal, entries: proposal.entries.map((entry) => ({ ...entry })) };
}

function cloneDraft(draft: SourceDraft): SourceDraft {
  return { ...draft, rows: draft.rows.map((row) => ({ ...row, entries: row.entries.map((entry) => ({ ...entry })), proposal: cloneProposal(row.proposal) })) };
}

function errorMessage(cause: unknown) {
  if (cause instanceof Error) return cause.message;
  if (typeof cause === "string") return cause;
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") return cause.message;
  return "Bridge could not complete that source-draft action.";
}

function displayDate(value: string | null) {
  if (value === null) return "Not observed";
  if (value === "") return "Empty field returned";
  return /^\d{8}$/.test(value) ? `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6)}` : value;
}

function dateForBackend(value: string) {
  return value === "" ? null : value.replace(/-/g, "");
}

function displayObserved(value: string | null, emptyLabel = "Empty field returned") {
  if (value === null) return "Not observed";
  return value === "" ? emptyLabel : value;
}

function hasStartedProposal(row: SourceDraftRow) {
  const proposal = row.proposal;
  return Boolean(proposal.date || proposal.voucher_type || proposal.narration !== null || proposal.notes.trim() || proposal.entries.some((entry) => entry.ledger !== null || entry.side !== null || entry.amount !== null));
}

function emptyToNull(value: string) {
  return value === "" ? null : value;
}

function sourceEntryLabel(entry: SourceDraftSourceEntry) {
  return `${displayObserved(entry.source_ledger, "Empty source ledger")} · ${displayObserved(entry.source_polarity, "Empty source polarity")} · ${displayObserved(entry.source_amount, "Empty source amount")}`;
}

type NativeLifecycleKind = "close" | "exit";

type NativeLifecycleRequest = {
  request_id: string;
  kind: NativeLifecycleKind;
};

function sameNativeLifecycleRequest(left: NativeLifecycleRequest, right: NativeLifecycleRequest) {
  return left.request_id === right.request_id && left.kind === right.kind;
}

function hasNativeWindowRuntime() {
  const internals = (window as unknown as {
    __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: unknown } } };
  }).__TAURI_INTERNALS__;
  return typeof internals?.metadata?.currentWindow?.label === "string";
}

export function SourceDraftScreen({ onBusyChange, onNativeLifecycleRequested, onTallyReadActivityChange, catalogScope, catalogScopeKey = "unavailable" }: SourceDraftScreenProps) {
  const [draft, setDraft] = React.useState<SourceDraft | null>(null);
  const [dirty, setDirty] = React.useState(false);
  const [selectedPosition, setSelectedPosition] = React.useState<number | null>(null);
  const [page, setPage] = React.useState(0);
  const [action, setAction] = React.useState<SourceDraftAction>(null);
  const [pendingAction, setPendingAction] = React.useState<"choose" | "open" | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [savedPath, setSavedPath] = React.useState<string | null>(null);
  const [nativeLifecycleRequest, setNativeLifecycleRequest] = React.useState<NativeLifecycleRequest | null>(null);
  const [nativeLifecycleBusy, setNativeLifecycleBusy] = React.useState(false);
  const [catalog, setCatalog] = React.useState<SourceDraftCatalogTargets | null>(null);
  const [catalogSelections, setCatalogSelections] = React.useState<Record<string, string>>({});
  const [catalogInvalidating, setCatalogInvalidating] = React.useState(false);
  const mounted = React.useRef(true);
  const actionRef = React.useRef<SourceDraftAction>(null);
  const dirtyRef = React.useRef(false);
  const lifecycleEpoch = React.useRef(0);
  const onBusyChangeRef = React.useRef(onBusyChange);
  const onNativeLifecycleRequestedRef = React.useRef(onNativeLifecycleRequested);
  const onTallyReadActivityChangeRef = React.useRef(onTallyReadActivityChange);
  dirtyRef.current = dirty;
  onBusyChangeRef.current = onBusyChange;
  onNativeLifecycleRequestedRef.current = onNativeLifecycleRequested;
  onTallyReadActivityChangeRef.current = onTallyReadActivityChange;
  const operationGeneration = React.useRef(0);
  const previousCatalogScope = React.useRef(catalogScopeKey);
  const catalogInvalidationTail = React.useRef<Promise<unknown>>(Promise.resolve());

  function invalidateNativeCatalog() {
    const next = catalogInvalidationTail.current
      .catch(() => undefined)
      .then(() => invoke("desktop_invalidate_source_draft_existing_ledger_targets"));
    catalogInvalidationTail.current = next;
    return next;
  }

  React.useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (actionRef.current !== null) onBusyChangeRef.current?.(false);
      if (actionRef.current === "catalog_load" || actionRef.current === "catalog_apply") onTallyReadActivityChangeRef.current?.(false);
    };
  }, []);

  React.useEffect(() => {
    if (previousCatalogScope.current === catalogScopeKey) return;
    previousCatalogScope.current = catalogScopeKey;
    const invalidationGeneration = ++operationGeneration.current;
    setCatalog(null);
    setCatalogSelections({});
    setCatalogInvalidating(true);
    void invalidateNativeCatalog()
      .then(() => {
        if (mounted.current && operationGeneration.current === invalidationGeneration) setCatalogInvalidating(false);
      })
      .catch((cause) => {
        if (mounted.current && operationGeneration.current === invalidationGeneration) setError(errorMessage(cause));
      });
  }, [catalogScopeKey]);

  const selectedRow = draft?.rows.find((row) => row.position === selectedPosition) ?? null;
  const pageCount = Math.max(1, Math.ceil((draft?.rows.length ?? 0) / PAGE_SIZE));
  const pageRows = draft?.rows.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE) ?? [];
  const rowsWithoutProposal = draft?.rows.filter((row) => !hasStartedProposal(row)).length ?? 0;

  React.useEffect(() => {
    if (!dirty) return;
    const beforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", beforeUnload);
    return () => window.removeEventListener("beforeunload", beforeUnload);
  }, [dirty]);

  React.useEffect(() => {
    if (!hasNativeWindowRuntime()) return;
    let active = true;
    let unlisten: (() => void) | undefined;

    const handleRequest = async (request: NativeLifecycleRequest) => {
      if (!active) return;
      const epoch = ++lifecycleEpoch.current;
      let pending: NativeLifecycleRequest | null;
      try {
        pending = await invoke<NativeLifecycleRequest | null>("desktop_pending_source_draft_lifecycle_request");
      } catch (cause) {
        if (active) setError(errorMessage(cause));
        return;
      }
      if (!active || epoch !== lifecycleEpoch.current || !pending || !sameNativeLifecycleRequest(pending, request)) return;
      if (dirtyRef.current || actionRef.current !== null) {
        onNativeLifecycleRequestedRef.current?.();
        setPendingAction(null);
        setNativeLifecycleRequest(request);
        return;
      }
      try {
        await invoke("desktop_complete_source_draft_lifecycle_request", { request });
      } catch (cause) {
        if (active) setError(errorMessage(cause));
      }
    };

    void (async () => {
      try {
        const registeredUnlisten = await listen<NativeLifecycleRequest>("source-draft-lifecycle-requested", (event) => {
          void handleRequest(event.payload);
        });
        if (!active) {
          registeredUnlisten();
          return;
        }
        unlisten = registeredUnlisten;
        const pending = await invoke<NativeLifecycleRequest | null>("desktop_pending_source_draft_lifecycle_request");
        if (active && pending) await handleRequest(pending);
      } catch (cause) {
        if (active) setError(`Bridge could not install native close protection: ${errorMessage(cause)}`);
      }
    })();

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  async function load(kind: "choose" | "open") {
    if (actionRef.current !== null) return;
    actionRef.current = kind;
    setAction(kind);
    setError(null);
    onBusyChange?.(true);
    try {
      const next = await invoke<SourceDraft | null>(kind === "choose" ? "desktop_pick_source_draft" : "desktop_open_source_draft");
      if (!mounted.current || !next) {
        if (mounted.current) setPendingAction(null);
        return;
      }
      const copy = cloneDraft(next);
      setDraft(copy);
      dirtyRef.current = false;
      setDirty(false);
      setSelectedPosition(copy.rows[0]?.position ?? null);
      setPage(0);
      setSavedPath(null);
      setCatalog(null);
      setCatalogSelections({});
      operationGeneration.current += 1;
      setPendingAction(null);
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      if (mounted.current) {
        setAction(null);
        onBusyChange?.(false);
      }
    }
  }

  function requestLoad(kind: "choose" | "open") {
    if (actionRef.current !== null) return;
    if (dirty) setPendingAction(kind);
    else void load(kind);
  }

  async function save() {
    if (!draft || actionRef.current !== null) return;
    actionRef.current = "save";
    setAction("save");
    setError(null);
    setSavedPath(null);
    onBusyChange?.(true);
    try {
      const next = await invoke<SourceDraft | null>("desktop_save_source_draft", {
        request: { draft_id: draft.draft_id, revision: draft.revision, proposals: draft.rows.map((row) => row.proposal) },
      });
      if (!mounted.current || !next) return;
      const copy = cloneDraft(next);
      setDraft(copy);
      dirtyRef.current = false;
      setDirty(false);
      setCatalogSelections((current) => retainCurrentSessionSelections(copy, current));
      setSavedPath("Draft saved locally as JSON.");
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      if (mounted.current) {
        setAction(null);
        onBusyChange?.(false);
      }
    }
  }

  async function loadExistingLedgerTargets() {
    if (!draft || !catalogScope || catalogInvalidating || previousCatalogScope.current !== catalogScopeKey || actionRef.current !== null) return;
    const generation = operationGeneration.current;
    actionRef.current = "catalog_load";
    setAction("catalog_load");
    setError(null);
    onBusyChange?.(true);
    onTallyReadActivityChange?.(true);
    try {
      const next = await invoke<SourceDraftCatalogTargets>("desktop_load_source_draft_existing_ledger_targets", {
        request: { draft_id: draft.draft_id, ...catalogScope },
      });
      if (!mounted.current || generation !== operationGeneration.current) return;
      setCatalog(next);
      setCatalogSelections({});
    } catch (cause) {
      if (mounted.current && generation === operationGeneration.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      if (mounted.current) {
        setAction(null);
        onBusyChange?.(false);
        onTallyReadActivityChange?.(false);
      }
    }
  }

  async function applyExistingLedgerTarget(rowPosition: number, entryPosition: number, targetName: string) {
    if (!draft || !catalog || !catalogScope || catalogInvalidating || previousCatalogScope.current !== catalogScopeKey || actionRef.current !== null || !targetName) return;
    const generation = operationGeneration.current;
    actionRef.current = "catalog_apply";
    setAction("catalog_apply");
    setError(null);
    onBusyChange?.(true);
    onTallyReadActivityChange?.(true);
    try {
      const next = await invoke<SourceDraft>("desktop_apply_source_draft_existing_ledger_target", {
        request: {
          draft_id: draft.draft_id,
          revision: draft.revision,
          capture_id: catalog.capture_id,
          ...catalogScope,
          row_position: rowPosition,
          entry_position: entryPosition,
          target_name: targetName,
          proposals: draft.rows.map((row) => cloneProposal(row.proposal)),
        },
      });
      if (!mounted.current) return;
      const copy = cloneDraft(next);
      if (generation !== operationGeneration.current) {
        // The native apply may have committed before a concurrent company-scope
        // invalidation reached it. Keep its new revision visible, but never
        // present the old capture or session binding as current for this scope.
        dirtyRef.current = true;
        setDraft(copy);
        setCatalog(null);
        setCatalogSelections({});
        setDirty(true);
        setSavedPath(null);
        return;
      }
      setDraft(copy);
      setCatalogSelections((current) => ({
        ...retainCurrentSessionSelections(copy, current),
        [catalogSelectionKey(rowPosition, entryPosition)]: targetName,
      }));
      dirtyRef.current = true;
      setDirty(true);
      setSavedPath(null);
    } catch (cause) {
      if (mounted.current && generation === operationGeneration.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      if (mounted.current) {
        setAction(null);
        onBusyChange?.(false);
        onTallyReadActivityChange?.(false);
      }
    }
  }

  function updateProposal(change: (proposal: SourceDraftProposal) => SourceDraftProposal) {
    if (selectedPosition === null) return;
    dirtyRef.current = true;
    setDirty(true);
    setSavedPath(null);
    setDraft((current) => current && ({ ...current, rows: current.rows.map((row) => row.position === selectedPosition ? { ...row, proposal: change(cloneProposal(row.proposal)) } : row) }));
  }

  function changePage(change: (value: number) => number) {
    setSelectedPosition(null);
    setPage(change);
  }

  function updateEntry(position: number, change: (entry: SourceDraftProposedEntry) => SourceDraftProposedEntry) {
    updateProposal((proposal) => ({ ...proposal, entries: proposal.entries.map((entry, index) => index === position ? change({ ...entry }) : entry) }));
  }

  async function cancelNativeLifecycleRequest() {
    if (!nativeLifecycleRequest || nativeLifecycleBusy) return;
    lifecycleEpoch.current += 1;
    setNativeLifecycleBusy(true);
    setError(null);
    try {
      await invoke("desktop_cancel_source_draft_lifecycle_request", { request: nativeLifecycleRequest });
      if (mounted.current) setNativeLifecycleRequest(null);
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    } finally {
      if (mounted.current) setNativeLifecycleBusy(false);
    }
  }

  async function completeNativeLifecycleRequest() {
    if (!nativeLifecycleRequest || nativeLifecycleBusy) return;
    lifecycleEpoch.current += 1;
    setNativeLifecycleBusy(true);
    setError(null);
    try {
      await invoke("desktop_complete_source_draft_lifecycle_request", { request: nativeLifecycleRequest });
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    } finally {
      if (mounted.current) setNativeLifecycleBusy(false);
    }
  }

  async function clearExistingLedgerTarget(rowPosition: number, entryPosition: number) {
    if (!draft || !catalog || actionRef.current !== null) return;
    const generation = ++operationGeneration.current;
    actionRef.current = "catalog_clear";
    setAction("catalog_clear");
    setError(null);
    setCatalogInvalidating(true);
    onBusyChange?.(true);
    try {
      await invalidateNativeCatalog();
      if (!mounted.current || generation !== operationGeneration.current) return;
      setCatalog(null);
      setCatalogSelections({});
      setDraft((current) => current && ({
        ...current,
        rows: current.rows.map((row) => row.position === rowPosition ? {
          ...row,
          proposal: {
            ...row.proposal,
            entries: row.proposal.entries.map((entry, index) => index === entryPosition - 1 ? { ...entry, ledger: null } : entry),
          },
        } : row),
      }));
      setDirty(true);
      setSavedPath(null);
      setCatalogInvalidating(false);
    } catch (cause) {
      if (mounted.current && generation === operationGeneration.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      if (mounted.current) {
        setAction(null);
        onBusyChange?.(false);
      }
    }
  }

  const busy = action !== null || nativeLifecycleRequest !== null;
  const confirmationBusy = action !== null || nativeLifecycleBusy;
  const lifecycleIsExit = nativeLifecycleRequest?.kind === "exit";
  const lifecycleHeading = dirty
    ? lifecycleIsExit ? "Discard unsaved proposals and quit Bridge?" : "Discard unsaved proposals and close this window?"
    : lifecycleIsExit ? "Quit Bridge?" : "Close this window?";
  const lifecycleDescription = dirty
    ? "Unsaved proposal edits are local only and will be lost."
    : "No unsaved proposals remain.";
  return (
    <section className="panel wide source-draft" aria-labelledby="source-draft-heading" aria-busy={busy}>
      <div className="source-draft-heading">
        <div>
          <h2 id="source-draft-heading">Prepare a source draft</h2>
          <p className="panel-description">Open a source XML file, keep its observations intact, and prepare explicit voucher proposals for review.</p>
        </div>
        <FilePlus2 size={24} aria-hidden="true" />
      </div>

      {error && <div className="error-banner" role="alert"><strong>Source draft action failed</strong><span>{error}</span></div>}
      {pendingAction && <div className="source-draft-confirm" role="alertdialog" aria-labelledby="source-draft-confirm-heading"><div><h3 id="source-draft-confirm-heading">Discard unsaved proposals?</h3><p>Opening another source will replace the current draft and its unsaved edits.</p></div><div className="source-draft-actions"><button className="primary" type="button" onClick={() => void load(pendingAction)} disabled={confirmationBusy}>Discard and open</button><button className="secondary-action" type="button" onClick={() => setPendingAction(null)} disabled={confirmationBusy}>Keep editing</button></div></div>}
      {nativeLifecycleRequest && <div className="source-draft-confirm" role="alertdialog" aria-labelledby="source-draft-lifecycle-heading"><div><h3 id="source-draft-lifecycle-heading">{lifecycleHeading}</h3><p>{lifecycleDescription}</p></div><div className="source-draft-actions"><button className="primary" type="button" onClick={() => void completeNativeLifecycleRequest()} disabled={confirmationBusy}>{lifecycleIsExit ? "Discard and quit" : "Discard and close"}</button><button className="secondary-action" type="button" onClick={() => void cancelNativeLifecycleRequest()} disabled={confirmationBusy}>Keep editing</button></div></div>}

      {!draft ? (
        <div className="source-draft-empty">
          <ShieldAlert size={28} aria-hidden="true" />
          <div><h3>No source draft is open</h3><p>Choose an XML source to create a local preparation draft, or open a saved <code>.bridge-draft.json</code>. Nothing is uploaded or posted from this screen.</p></div>
          <div className="source-draft-actions"><button className="primary" type="button" onClick={() => requestLoad("choose")} disabled={busy}><FilePlus2 size={18} aria-hidden="true" />{action === "choose" ? "Opening source…" : "Choose source XML"}</button><button className="secondary-action" type="button" onClick={() => requestLoad("open")} disabled={busy}><FolderOpen size={18} aria-hidden="true" />{action === "open" ? "Opening draft…" : "Open saved draft"}</button></div>
        </div>
      ) : (
        <>
          <div className="source-draft-toolbar"><div><strong>{draft.source_filename}</strong><span>{draft.rows.length} source rows · {rowsWithoutProposal} rows without a proposal · revision {draft.revision}</span></div><div className="source-draft-actions"><button className="secondary-action" type="button" onClick={() => requestLoad("choose")} disabled={busy}>{action === "choose" ? "Opening source…" : "Choose new source"}</button><button className="secondary-action" type="button" onClick={() => requestLoad("open")} disabled={busy}>{action === "open" ? "Opening draft…" : "Open saved draft"}</button><button className="primary" type="button" onClick={() => void save()} disabled={busy}><Save size={17} aria-hidden="true" />{action === "save" ? "Saving draft…" : "Save draft"}</button></div></div>
          <p className="source-draft-boundary">Source values are immutable observations. A target chosen from the current ledger list remains an unverified proposal; this preparation screen cannot approve or post anything to Tally.</p>
          <div className="source-draft-actions"><button className="secondary-action" type="button" disabled={busy || catalogInvalidating || !catalogScope} onClick={() => void loadExistingLedgerTargets()}>{action === "catalog_load" ? "Loading existing ledgers…" : catalog ? "Refresh existing ledgers" : "Load existing ledgers"}</button>{!catalogScope && <span className="source-draft-catalogue-state">Check Tally and select a current company before loading existing ledgers.</span>}{catalogInvalidating && <span className="source-draft-catalogue-state">Existing-ledger context is changing.</span>}{catalog && <span className="source-draft-catalogue-state">{catalog.targets.length} existing ledgers captured for this source. Choosing one remains unverified.</span>}</div>
          <details className="source-draft-file-evidence"><summary>Source file evidence</summary><dl><div><dt>Source file</dt><dd>{draft.source_filename}</dd></div><div><dt>Source SHA-256</dt><dd><code>{draft.source_sha256}</code></dd></div></dl></details>
          {((draft.source_notices ?? []).length > 0) && <details className="source-draft-notices"><summary>Source-level notices ({(draft.source_notices ?? []).length})</summary><ul>{(draft.source_notices ?? []).map((notice, index) => <li key={`${notice.kind}-${index}`}><strong>{notice.kind}</strong><span>{notice.count} retained records</span></li>)}</ul></details>}
          {savedPath && <p className="source-draft-saved" role="status">{savedPath}</p>}
          <div className="source-draft-layout">
            <div className="source-draft-list-wrap">
              <table className="source-draft-list"><caption className="visually-hidden">Source rows</caption><thead><tr><th scope="col">Row</th><th scope="col">Source observation</th><th scope="col">Proposal</th></tr></thead><tbody>{pageRows.map((row) => <tr key={row.position} className={row.position === selectedPosition ? "is-selected" : ""}><th scope="row"><button type="button" className="source-draft-row-button" onClick={() => setSelectedPosition(row.position)} aria-pressed={row.position === selectedPosition}>#{row.position}</button></th><td><strong>{displayDate(row.source_date)}</strong><span>{displayObserved(row.source_voucher_type, "Empty voucher type")}</span><span>{displayObserved(row.source_narration, "Empty narration")}</span></td><td><span className="source-draft-state">{hasStartedProposal(row) ? "Proposal started" : "No proposal"}</span><span>{row.proposal.entries.length} entry lines</span></td></tr>)}</tbody></table>
              <div className="source-draft-pagination"><span>Rows {draft.rows.length === 0 ? 0 : page * PAGE_SIZE + 1}–{Math.min((page + 1) * PAGE_SIZE, draft.rows.length)} of {draft.rows.length}</span><div><button className="secondary-action" type="button" onClick={() => changePage((value) => Math.max(0, value - 1))} disabled={page === 0 || busy}>Previous</button><button className="secondary-action" type="button" onClick={() => changePage((value) => Math.min(pageCount - 1, value + 1))} disabled={page >= pageCount - 1 || busy}>Next</button></div></div>
            </div>
            {selectedRow && <SourceDraftEditor row={selectedRow} disabled={busy || catalogInvalidating} catalog={catalog} catalogSelections={catalogSelections} onSelectExistingLedger={applyExistingLedgerTarget} onClearExistingLedger={clearExistingLedgerTarget} onUpdateProposal={updateProposal} onUpdateEntry={updateEntry} onClose={() => setSelectedPosition(null)} />}
          </div>
        </>
      )}
    </section>
  );
}

function SourceDraftEditor({ row, disabled, catalog, catalogSelections, onSelectExistingLedger, onClearExistingLedger, onUpdateProposal, onUpdateEntry, onClose }: { row: SourceDraftRow; disabled: boolean; catalog: SourceDraftCatalogTargets | null; catalogSelections: Record<string, string>; onSelectExistingLedger: (rowPosition: number, entryPosition: number, targetName: string) => void; onClearExistingLedger: (rowPosition: number, entryPosition: number) => void; onUpdateProposal: (change: (proposal: SourceDraftProposal) => SourceDraftProposal) => void; onUpdateEntry: (position: number, change: (entry: SourceDraftProposedEntry) => SourceDraftProposedEntry) => void; onClose: () => void }) {
  const proposal = row.proposal;
  const fieldId = (name: string) => `source-draft-${row.position}-${name}`;
  return (
    <section className="source-draft-editor" aria-labelledby="source-draft-editor-heading">
      <div className="source-draft-editor-heading">
        <div><h3 id="source-draft-editor-heading">Row #{row.position}</h3><p>Source observation beside an explicit proposal.</p></div>
        <button className="icon-button" type="button" onClick={onClose} aria-label="Close row editor" disabled={disabled}><X size={18} aria-hidden="true" /></button>
      </div>
      <dl className="source-draft-source">
        <div><dt>Source date</dt><dd>{displayDate(row.source_date)}</dd></div>
        <div><dt>Source voucher type</dt><dd>{displayObserved(row.source_voucher_type)}</dd></div>
        <div><dt>Source narration</dt><dd>{displayObserved(row.source_narration, "Empty narration")}</dd></div>
        <div><dt>XML REMOTEID</dt><dd>{displayObserved(row.source_remote_id)}</dd></div>
      </dl>
      {row.source_omitted_fields.length > 0 && <details className="source-draft-omitted"><summary>Other source fields retained in original XML ({row.source_omitted_fields.length})</summary><p>{row.source_omitted_fields.join(", ")}</p></details>}
      <div className="source-draft-proposal">
        <h4>Unverified proposal</h4>
        <div className="source-draft-field"><label htmlFor={fieldId("date")}>Proposed date</label><input id={fieldId("date")} type="date" value={proposal.date ? displayDate(proposal.date) : ""} onChange={(event) => onUpdateProposal((current) => ({ ...current, date: dateForBackend(event.target.value) }))} disabled={disabled} /></div>
        <div className="source-draft-field"><label htmlFor={fieldId("type")}>Proposed voucher type</label><select id={fieldId("type")} value={proposal.voucher_type ?? ""} onChange={(event) => onUpdateProposal((current) => ({ ...current, voucher_type: event.target.value ? event.target.value as SourceDraftVoucherType : null }))} disabled={disabled}><option value="">Choose type</option>{VOUCHER_TYPES.map((type) => <option key={type} value={type}>{type}</option>)}</select></div>
        <div className="source-draft-field"><label htmlFor={fieldId("narration")}>Proposed narration</label><textarea id={fieldId("narration")} value={proposal.narration ?? ""} onChange={(event) => onUpdateProposal((current) => ({ ...current, narration: emptyToNull(event.target.value) }))} rows={2} disabled={disabled} /></div>
        <div className="source-draft-field"><label htmlFor={fieldId("notes")}>Preparation notes</label><textarea id={fieldId("notes")} value={proposal.notes} onChange={(event) => onUpdateProposal((current) => ({ ...current, notes: event.target.value }))} rows={2} disabled={disabled} /></div>
        <h4>Proposed entries</h4>
        <div className="source-draft-entry-list">
          {proposal.entries.map((entry, index) => {
            const entryId = (name: string) => fieldId(`entry-${index}-${name}`);
            return <div className="source-draft-entry" key={`${row.position}-${index}`}>
              <p><span>Source line {index + 1}</span>{sourceEntryLabel(row.entries[index] ?? { position: index, source_ledger: "", source_amount: "", source_polarity: "" })}</p>
              <div className="source-draft-field">
                <label htmlFor={entryId("ledger")}>Existing target ledger</label>
                {catalog ? <>
                  <select id={entryId("ledger")} value={catalogSelections[catalogSelectionKey(row.position, index + 1)] === entry.ledger ? entry.ledger ?? "" : ""} onChange={(event) => event.target.value && onSelectExistingLedger(row.position, index + 1, event.target.value)} disabled={disabled}>
                    <option value="">Choose existing ledger</option>
                    {catalog.targets.map((target) => <option key={target} value={target}>{target}</option>)}
                  </select>
                  {entry.ledger && <button className="secondary-action source-draft-clear-target" type="button" onClick={() => onClearExistingLedger(row.position, index + 1)} disabled={disabled}>Clear target</button>}
                  <p className="source-draft-catalogue-state">{catalogSelections[catalogSelectionKey(row.position, index + 1)] === entry.ledger ? "This current-session target was re-read and bound. It remains an unapproved proposal." : entry.ledger ? `Saved unverified target: ${entry.ledger}. Select it to check it against this current capture.` : "Choose a current existing ledger to make an unapproved proposal."}</p>
                </> : <>
                  <input id={entryId("ledger")} placeholder="Unverified ledger name" value={entry.ledger ?? ""} onChange={(event) => onUpdateEntry(index, (current) => ({ ...current, ledger: emptyToNull(event.target.value) }))} disabled={disabled} />
                  <p className="source-draft-catalogue-state">{entry.ledger ? `Saved unverified target: ${entry.ledger}` : "Load existing ledgers to choose a target."}</p>
                </>}
              </div>
              <div className="source-draft-field"><label htmlFor={entryId("side")}>Side</label><select id={entryId("side")} value={entry.side ?? ""} onChange={(event) => onUpdateEntry(index, (current) => ({ ...current, side: event.target.value ? event.target.value as SourceDraftSide : null }))} disabled={disabled}><option value="">Choose side</option><option value="Dr">Dr</option><option value="Cr">Cr</option></select></div>
              <div className="source-draft-field"><label htmlFor={entryId("amount")}>Amount</label><input id={entryId("amount")} inputMode="decimal" value={entry.amount ?? ""} onChange={(event) => onUpdateEntry(index, (current) => ({ ...current, amount: emptyToNull(event.target.value) }))} disabled={disabled} /></div>
            </div>;
          })}
        </div>
      </div>
    </section>
  );
}
