import React from "react";
import { FilePlus2, FolderOpen, Save, ShieldAlert, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import "./source-draft.css";
import {
  SourceDraft,
  SourceDraftAction,
  SourceDraftCatalogBinding,
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

function displayCatalogTarget(target: string) {
  return target.replace(/(^ +| +$| {2,})/g, (spaces) => "␠".repeat(spaces.length));
}

function catalogBindingFor(catalog: SourceDraftCatalogTargets | null, rowPosition: number, entryPosition: number) {
  return catalog?.bindings?.find((binding) => binding.row_position === rowPosition && binding.entry_position === entryPosition) ?? null;
}

/// The ledgers Bridge could defend for this source name, most defensible first.
/// A bound target leads only because binding decided it; a candidate list is
/// ordered by the rule that surfaced it and carries no ranking of its own.
function narrowedTargets(binding: SourceDraftCatalogBinding | null) {
  if (!binding) return [];
  return binding.bound_target ? [binding.bound_target] : binding.candidates;
}

/// States what binding did, in the operator's terms. It never says "best",
/// "recommended" or "suggested match": nothing here is chosen for anyone, and a
/// listed ledger is a shortcut through the list, not an answer.
function catalogBindingSummary(binding: SourceDraftCatalogBinding | null, total: number) {
  if (!binding) return null;
  if (binding.bound_target) {
    const how = binding.bound_basis === "identifier"
      ? "a number inside the ledger name"
      : binding.bound_basis === "exact_name"
        ? "the exact ledger name"
        : "the same ledger name, differently written";
    return `Listed first because ${how} matches this source line. Nothing is selected for you, and choosing it stays an unapproved proposal.`;
  }
  if (binding.candidate_count === 0) {
    return `No existing ledger matched this source line. All ${total} are listed.`;
  }
  const shown = binding.candidates.length;
  const listed = binding.candidates_truncated ? `${shown} of ${binding.candidate_count}` : `${shown}`;
  return `No single ledger matched this source line, so nothing is chosen. ${listed} possible ${shown === 1 ? "ledger is" : "ledgers are"} listed first; the full list of ${total} follows.`;
}

function hasStartedProposal(row: SourceDraftRow) {
  const proposal = row.proposal;
  return Boolean(proposal.date || proposal.voucher_type || proposal.narration !== null || proposal.notes.trim() || proposal.entries.some((entry) => entry.ledger !== null || entry.side !== null || entry.amount !== null));
}

function isUnenteredOperatorChoice(value: string | null) {
  return value === null || value === "";
}

function unenteredOperatorChoices(row: SourceDraftRow) {
  const choices: string[] = [];
  if (isUnenteredOperatorChoice(row.proposal.date)) choices.push("date");
  if (row.proposal.voucher_type === null) choices.push("voucher type");
  row.proposal.entries.forEach((entry, index) => {
    const entryNumber = index + 1;
    if (isUnenteredOperatorChoice(entry.ledger)) choices.push(`entry ${entryNumber} target ledger`);
    if (entry.side === null) choices.push(`entry ${entryNumber} side`);
    if (isUnenteredOperatorChoice(entry.amount)) choices.push(`entry ${entryNumber} amount`);
  });
  return choices;
}

function emptyToNull(value: string) {
  return value === "" ? null : value;
}

function sourceEntryLabel(entry: SourceDraftSourceEntry) {
  return `${displayObserved(entry.source_ledger, "Empty source ledger")} · ${displayObserved(entry.source_polarity, "Empty source polarity")} · ${displayObserved(entry.source_amount, "Empty source amount")}`;
}

export function SourceDraftScreen({
  onBusyChange,
  onTallyReadActivityChange,
  catalogScope,
  catalogScopeKey = "unavailable",
  onDirtyChange,
  editingEnabled = true,
  lifecycleInteractionBlocked = false,
  isLifecycleInteractionBlocked = () => false,
  protectionError = null,
}: SourceDraftScreenProps) {
  const [draft, setDraft] = React.useState<SourceDraft | null>(null);
  const [dirty, setDirty] = React.useState(false);
  const [selectedPosition, setSelectedPosition] = React.useState<number | null>(null);
  const [page, setPage] = React.useState(0);
  const [action, setAction] = React.useState<SourceDraftAction>(null);
  const [pendingAction, setPendingAction] = React.useState<"choose" | "open" | null>(null);
  const [error, setError] = React.useState<string | null>(null);
  const [savedPath, setSavedPath] = React.useState<string | null>(null);
  const [catalog, setCatalog] = React.useState<SourceDraftCatalogTargets | null>(null);
  const [catalogSelections, setCatalogSelections] = React.useState<Record<string, string>>({});
  const [catalogInvalidating, setCatalogInvalidating] = React.useState(false);
  const mounted = React.useRef(true);
  const actionRef = React.useRef<SourceDraftAction>(null);
  const dirtyRef = React.useRef(false);
  const onBusyChangeRef = React.useRef(onBusyChange);
  const onTallyReadActivityChangeRef = React.useRef(onTallyReadActivityChange);
  const busyLeaseCountRef = React.useRef(0);
  const catalogReadActiveRef = React.useRef(false);
  dirtyRef.current = dirty;
  onBusyChangeRef.current = onBusyChange;
  onTallyReadActivityChangeRef.current = onTallyReadActivityChange;

  const setDraftDirty = React.useCallback((next: boolean) => {
    dirtyRef.current = next;
    if (mounted.current) setDirty(next);
    onDirtyChange?.(next);
  }, [onDirtyChange]);
  const operationGeneration = React.useRef(0);
  const previousCatalogScope = React.useRef(catalogScopeKey);
  const catalogInvalidationTail = React.useRef<Promise<unknown>>(Promise.resolve());

  function beginBusy() {
    if (busyLeaseCountRef.current++ === 0) onBusyChangeRef.current?.(true);
  }

  function endBusy() {
    if (busyLeaseCountRef.current === 0) return;
    if (--busyLeaseCountRef.current === 0) onBusyChangeRef.current?.(false);
  }

  function invalidateNativeCatalog() {
    beginBusy();
    const next = catalogInvalidationTail.current
      .catch(() => undefined)
      .then(() => invoke("desktop_invalidate_source_draft_existing_ledger_targets"));
    catalogInvalidationTail.current = next;
    void next.finally(endBusy).catch(() => undefined);
    return next;
  }

  function releaseCatalogReadActivity() {
    if (!catalogReadActiveRef.current) return;
    catalogReadActiveRef.current = false;
    onTallyReadActivityChangeRef.current?.(false);
  }

  React.useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
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
      .catch((cause) => {
        if (mounted.current && operationGeneration.current === invalidationGeneration) setError(errorMessage(cause));
      })
      .finally(() => {
        if (mounted.current && operationGeneration.current === invalidationGeneration) setCatalogInvalidating(false);
      });
  }, [catalogScopeKey]);

  const selectedRow = draft?.rows.find((row) => row.position === selectedPosition) ?? null;
  const pageCount = Math.max(1, Math.ceil((draft?.rows.length ?? 0) / PAGE_SIZE));
  const pageRows = draft?.rows.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE) ?? [];
  const rowsWithoutProposal = draft?.rows.filter((row) => !hasStartedProposal(row)).length ?? 0;

  async function load(kind: "choose" | "open") {
    if (!editingEnabled || lifecycleInteractionBlocked || isLifecycleInteractionBlocked() || actionRef.current !== null) return;
    actionRef.current = kind;
    setAction(kind);
    setError(null);
    beginBusy();
    try {
      const next = await invoke<SourceDraft | null>(kind === "choose" ? "desktop_pick_source_draft" : "desktop_open_source_draft");
      if (!mounted.current || !next) {
        if (mounted.current) setPendingAction(null);
        return;
      }
      const copy = cloneDraft(next);
      setDraft(copy);
      setDraftDirty(false);
      setSelectedPosition(copy.rows[0]?.position ?? null);
      setPage(0);
      setSavedPath(null);
      setCatalog(null);
      setCatalogSelections({});
      operationGeneration.current += 1;
      setCatalogInvalidating(false);
      setPendingAction(null);
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      endBusy();
      if (mounted.current) {
        setAction(null);
      }
    }
  }

  function requestLoad(kind: "choose" | "open") {
    if (!editingEnabled || lifecycleInteractionBlocked || isLifecycleInteractionBlocked() || actionRef.current !== null) return;
    if (dirty) setPendingAction(kind);
    else void load(kind);
  }

  async function save() {
    if (!draft || !editingEnabled || lifecycleInteractionBlocked || isLifecycleInteractionBlocked() || actionRef.current !== null) return;
    actionRef.current = "save";
    setAction("save");
    setError(null);
    setSavedPath(null);
    beginBusy();
    try {
      const next = await invoke<SourceDraft | null>("desktop_save_source_draft", {
        request: { draft_id: draft.draft_id, revision: draft.revision, proposals: draft.rows.map((row) => row.proposal) },
      });
      if (!mounted.current || !next) return;
      const copy = cloneDraft(next);
      setDraft(copy);
      setDraftDirty(false);
      setCatalogSelections((current) => retainCurrentSessionSelections(copy, current));
      setSavedPath("Draft saved locally as JSON.");
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      endBusy();
      if (mounted.current) {
        setAction(null);
      }
    }
  }

  async function loadExistingLedgerTargets() {
    if (!draft || !catalogScope || catalogInvalidating || previousCatalogScope.current !== catalogScopeKey || !editingEnabled || lifecycleInteractionBlocked || isLifecycleInteractionBlocked() || actionRef.current !== null) return;
    const generation = operationGeneration.current;
    actionRef.current = "catalog_load";
    setAction("catalog_load");
    setError(null);
    beginBusy();
    catalogReadActiveRef.current = true;
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
      endBusy();
      releaseCatalogReadActivity();
      if (mounted.current) {
        setAction(null);
      }
    }
  }

  async function applyExistingLedgerTarget(rowPosition: number, entryPosition: number, targetName: string) {
    if (!draft || !catalog || !catalogScope || catalogInvalidating || previousCatalogScope.current !== catalogScopeKey || !editingEnabled || lifecycleInteractionBlocked || isLifecycleInteractionBlocked() || actionRef.current !== null || !targetName) return;
    const generation = operationGeneration.current;
    actionRef.current = "catalog_apply";
    setAction("catalog_apply");
    setError(null);
    beginBusy();
    catalogReadActiveRef.current = true;
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
      // The native apply may have committed even if this screen was removed by
      // its error boundary while it was pending. Record that fact in the
      // stable parent before deciding whether child state may still be updated.
      setDraftDirty(true);
      if (!mounted.current) return;
      const copy = cloneDraft(next);
      if (generation !== operationGeneration.current) {
        // The native apply may have committed before a concurrent company-scope
        // invalidation reached it. Keep its new revision visible, but never
        // present the old capture or session binding as current for this scope.
        setDraft(copy);
        setCatalog(null);
        setCatalogSelections({});
        setSavedPath(null);
        return;
      }
      setDraft(copy);
      setCatalogSelections((current) => ({
        ...retainCurrentSessionSelections(copy, current),
        [catalogSelectionKey(rowPosition, entryPosition)]: targetName,
      }));
      setSavedPath(null);
    } catch (cause) {
      if (mounted.current && generation === operationGeneration.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      endBusy();
      releaseCatalogReadActivity();
      if (mounted.current) {
        setAction(null);
      }
    }
  }

  function updateProposal(change: (proposal: SourceDraftProposal) => SourceDraftProposal) {
    if (!editingEnabled || lifecycleInteractionBlocked || isLifecycleInteractionBlocked() || selectedPosition === null) return;
    setDraftDirty(true);
    setSavedPath(null);
    setDraft((current) => current && ({ ...current, rows: current.rows.map((row) => row.position === selectedPosition ? { ...row, proposal: change(cloneProposal(row.proposal)) } : row) }));
  }

  function changePage(change: (value: number) => number) {
    if (!editingEnabled || lifecycleInteractionBlocked || isLifecycleInteractionBlocked()) return;
    setSelectedPosition(null);
    setPage(change);
  }

  function updateEntry(position: number, change: (entry: SourceDraftProposedEntry) => SourceDraftProposedEntry) {
    updateProposal((proposal) => ({ ...proposal, entries: proposal.entries.map((entry, index) => index === position ? change({ ...entry }) : entry) }));
  }

  async function clearExistingLedgerTarget(rowPosition: number, entryPosition: number) {
    if (!draft || !catalog || !editingEnabled || lifecycleInteractionBlocked || isLifecycleInteractionBlocked() || actionRef.current !== null) return;
    const generation = ++operationGeneration.current;
    actionRef.current = "catalog_clear";
    setAction("catalog_clear");
    setError(null);
    setCatalogInvalidating(true);
    beginBusy();
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
      setDraftDirty(true);
      setSavedPath(null);
      setCatalogInvalidating(false);
    } catch (cause) {
      if (mounted.current && generation === operationGeneration.current) setError(errorMessage(cause));
    } finally {
      actionRef.current = null;
      endBusy();
      if (mounted.current && generation === operationGeneration.current) {
        setCatalogInvalidating(false);
      }
      if (mounted.current) {
        setAction(null);
      }
    }
  }

  const busy = action !== null;
  const interactionDisabled = busy || !editingEnabled || lifecycleInteractionBlocked;
  return (
    <section className="panel wide source-draft" aria-labelledby="source-draft-heading" aria-busy={interactionDisabled}>
      <div className="source-draft-heading">
        <div>
          <h2 id="source-draft-heading">Prepare a source draft</h2>
          <p className="panel-description">Open a source XML file, keep its observations intact, and prepare explicit voucher proposals for review.</p>
        </div>
        <FilePlus2 size={24} aria-hidden="true" />
      </div>

      {(protectionError || error) && <div className="error-banner" role="alert"><strong>{protectionError ? "Native close protection unavailable" : "Source draft action failed"}</strong><span>{protectionError ?? error}</span></div>}
      {pendingAction && <div className="source-draft-confirm" role="alertdialog" aria-labelledby="source-draft-confirm-heading"><div><h3 id="source-draft-confirm-heading">Discard unsaved proposals?</h3><p>Opening another source will replace the current draft and its unsaved edits.</p></div><div className="source-draft-actions"><button className="primary" type="button" onClick={() => void load(pendingAction)} disabled={interactionDisabled}>Discard and open</button><button className="secondary-action" type="button" onClick={() => setPendingAction(null)} disabled={interactionDisabled}>Keep editing</button></div></div>}

      {!draft ? (
        <div className="source-draft-empty">
          <ShieldAlert size={28} aria-hidden="true" />
          <div><h3>No source draft is open</h3><p>Choose an XML source to create a local preparation draft, or open a saved <code>.bridge-draft.json</code>. Nothing is uploaded or posted from this screen.</p></div>
          <div className="source-draft-actions"><button className="primary" type="button" onClick={() => requestLoad("choose")} disabled={interactionDisabled}><FilePlus2 size={18} aria-hidden="true" />{action === "choose" ? "Opening source…" : "Choose source XML"}</button><button className="secondary-action" type="button" onClick={() => requestLoad("open")} disabled={interactionDisabled}><FolderOpen size={18} aria-hidden="true" />{action === "open" ? "Opening draft…" : "Open saved draft"}</button></div>
        </div>
      ) : (
        <>
          <div className="source-draft-toolbar"><div><strong>{draft.source_filename}</strong><span>{draft.rows.length} source rows · {rowsWithoutProposal} rows without a proposal · revision {draft.revision}</span></div><div className="source-draft-actions"><button className="secondary-action" type="button" onClick={() => requestLoad("choose")} disabled={interactionDisabled}>{action === "choose" ? "Opening source…" : "Choose new source"}</button><button className="secondary-action" type="button" onClick={() => requestLoad("open")} disabled={interactionDisabled}>{action === "open" ? "Opening draft…" : "Open saved draft"}</button><button className="primary" type="button" onClick={() => void save()} disabled={interactionDisabled}><Save size={17} aria-hidden="true" />{action === "save" ? "Saving draft…" : "Save draft"}</button></div></div>
          <p className="source-draft-boundary">Source values are immutable observations. A target chosen from the current ledger list remains an unverified proposal; this preparation screen cannot approve or post anything to Tally.</p>
          <div className="source-draft-actions"><button className="secondary-action" type="button" disabled={interactionDisabled || catalogInvalidating || !catalogScope} onClick={() => void loadExistingLedgerTargets()}>{action === "catalog_load" ? "Loading existing ledgers…" : catalog ? "Refresh existing ledgers" : "Load existing ledgers"}</button>{!catalogScope && <span className="source-draft-catalogue-state">Check Tally and select a current company before loading existing ledgers.</span>}{catalogInvalidating && <span className="source-draft-catalogue-state">Existing-ledger context is changing.</span>}{catalog && <span className="source-draft-catalogue-state">{catalog.targets.length} existing ledgers captured for this source. Choosing one remains unverified.</span>}</div>
          <details className="source-draft-file-evidence"><summary>Source file evidence</summary><dl><div><dt>Source file</dt><dd>{draft.source_filename}</dd></div><div><dt>Source SHA-256</dt><dd><code>{draft.source_sha256}</code></dd></div></dl></details>
          {((draft.source_notices ?? []).length > 0) && <details className="source-draft-notices"><summary>Source-level notices ({(draft.source_notices ?? []).length})</summary><ul>{(draft.source_notices ?? []).map((notice, index) => <li key={`${notice.kind}-${index}`}><strong>{notice.kind}</strong><span>{notice.count} retained records</span></li>)}</ul></details>}
          {savedPath && <p className="source-draft-saved" role="status">{savedPath}</p>}
          <div className="source-draft-layout">
            <div className="source-draft-list-wrap">
              <table className="source-draft-list"><caption className="visually-hidden">Source rows</caption><thead><tr><th scope="col">Row</th><th scope="col">Source observation</th><th scope="col">Proposal</th></tr></thead><tbody>{pageRows.map((row) => <tr key={row.position} className={row.position === selectedPosition ? "is-selected" : ""}><th scope="row"><button type="button" className="source-draft-row-button" onClick={() => setSelectedPosition(row.position)} disabled={interactionDisabled} aria-pressed={row.position === selectedPosition}>#{row.position}</button></th><td><strong>{displayDate(row.source_date)}</strong><span>{displayObserved(row.source_voucher_type, "Empty voucher type")}</span><span>{displayObserved(row.source_narration, "Empty narration")}</span></td><td><span className="source-draft-state">{hasStartedProposal(row) ? "Proposal started" : "No proposal"}</span><span>{row.proposal.entries.length} entry lines</span></td></tr>)}</tbody></table>
              <div className="source-draft-pagination"><span>Rows {draft.rows.length === 0 ? 0 : page * PAGE_SIZE + 1}–{Math.min((page + 1) * PAGE_SIZE, draft.rows.length)} of {draft.rows.length}</span><div><button className="secondary-action" type="button" onClick={() => changePage((value) => Math.max(0, value - 1))} disabled={page === 0 || interactionDisabled}>Previous</button><button className="secondary-action" type="button" onClick={() => changePage((value) => Math.min(pageCount - 1, value + 1))} disabled={page >= pageCount - 1 || interactionDisabled}>Next</button></div></div>
            </div>
            {selectedRow && <SourceDraftEditor row={selectedRow} disabled={interactionDisabled || catalogInvalidating} catalog={catalog} catalogSelections={catalogSelections} onSelectExistingLedger={applyExistingLedgerTarget} onClearExistingLedger={clearExistingLedgerTarget} onUpdateProposal={updateProposal} onUpdateEntry={updateEntry} onClose={() => setSelectedPosition(null)} />}
          </div>
        </>
      )}
    </section>
  );
}

function SourceDraftEditor({ row, disabled, catalog, catalogSelections, onSelectExistingLedger, onClearExistingLedger, onUpdateProposal, onUpdateEntry, onClose }: { row: SourceDraftRow; disabled: boolean; catalog: SourceDraftCatalogTargets | null; catalogSelections: Record<string, string>; onSelectExistingLedger: (rowPosition: number, entryPosition: number, targetName: string) => void; onClearExistingLedger: (rowPosition: number, entryPosition: number) => void; onUpdateProposal: (change: (proposal: SourceDraftProposal) => SourceDraftProposal) => void; onUpdateEntry: (position: number, change: (entry: SourceDraftProposedEntry) => SourceDraftProposedEntry) => void; onClose: () => void }) {
  const proposal = row.proposal;
  const choices = unenteredOperatorChoices(row);
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
        {choices.length > 0 && <p className="source-draft-catalogue-state">Still to choose: {choices.join(", ")}.</p>}
        <div className="source-draft-field"><label htmlFor={fieldId("date")}>Proposed date</label><input id={fieldId("date")} type="date" value={proposal.date ? displayDate(proposal.date) : ""} onChange={(event) => onUpdateProposal((current) => ({ ...current, date: dateForBackend(event.target.value) }))} disabled={disabled} /></div>
        <div className="source-draft-field"><label htmlFor={fieldId("type")}>Proposed voucher type</label><select id={fieldId("type")} value={proposal.voucher_type ?? ""} onChange={(event) => onUpdateProposal((current) => ({ ...current, voucher_type: event.target.value ? event.target.value as SourceDraftVoucherType : null }))} disabled={disabled}><option value="">Choose type</option>{VOUCHER_TYPES.map((type) => <option key={type} value={type}>{type}</option>)}</select></div>
        <div className="source-draft-field"><label htmlFor={fieldId("narration")}>Proposed narration</label><textarea id={fieldId("narration")} value={proposal.narration ?? ""} onChange={(event) => onUpdateProposal((current) => ({ ...current, narration: emptyToNull(event.target.value) }))} rows={2} disabled={disabled} /></div>
        <div className="source-draft-field"><label htmlFor={fieldId("notes")}>Preparation notes</label><textarea id={fieldId("notes")} value={proposal.notes} onChange={(event) => onUpdateProposal((current) => ({ ...current, notes: event.target.value }))} rows={2} disabled={disabled} /></div>
        <h4>Proposed entries</h4>
        <div className="source-draft-entry-list">
          {proposal.entries.map((entry, index) => {
            const entryId = (name: string) => fieldId(`entry-${index}-${name}`);
            const binding = catalogBindingFor(catalog, row.position, index + 1);
            const narrowed = narrowedTargets(binding);
            const bindingSummary = catalog ? catalogBindingSummary(binding, catalog.targets.length) : null;
            return <div className="source-draft-entry" key={`${row.position}-${index}`}>
              <p><span>Source line {index + 1}</span>{sourceEntryLabel(row.entries[index] ?? { position: index, source_ledger: "", source_amount: "", source_polarity: "" })}</p>
              <div className="source-draft-field">
                <label htmlFor={entryId("ledger")}>Existing target ledger</label>
                {catalog ? <>
                  <select id={entryId("ledger")} value={catalogSelections[catalogSelectionKey(row.position, index + 1)] === entry.ledger ? entry.ledger ?? "" : ""} onChange={(event) => event.target.value && onSelectExistingLedger(row.position, index + 1, event.target.value)} disabled={disabled}>
                    <option value="">Choose existing ledger</option>
                    {narrowed.length > 0 && <optgroup label={binding?.bound_target ? "Matched to this source line" : "Possible for this source line"}>
                      {narrowed.map((target) => <option key={`narrowed-${target}`} value={target}>{displayCatalogTarget(target)}</option>)}
                    </optgroup>}
                    {/* The whole catalogue always remains reachable. Narrowing is a shortcut through the list, never a restriction on it. */}
                    <optgroup label={narrowed.length > 0 ? `All ${catalog.targets.length} existing ledgers` : "Existing ledgers"}>
                      {catalog.targets.map((target) => <option key={target} value={target}>{displayCatalogTarget(target)}</option>)}
                    </optgroup>
                  </select>
                  {entry.ledger && <button className="secondary-action source-draft-clear-target" type="button" onClick={() => onClearExistingLedger(row.position, index + 1)} disabled={disabled}>Clear target</button>}
                  <p className="source-draft-catalogue-state">{catalogSelections[catalogSelectionKey(row.position, index + 1)] === entry.ledger ? "This current-session target was re-read and bound. It remains an unapproved proposal." : entry.ledger ? `Saved unverified target: ${entry.ledger}. Select it to check it against this current capture.` : "Choose a current existing ledger to make an unapproved proposal."}</p>
                  {bindingSummary && <p className="source-draft-catalogue-state">{bindingSummary}</p>}
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
