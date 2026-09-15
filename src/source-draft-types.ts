export type SourceDraftVoucherType = "Payment" | "Receipt" | "Journal" | "Contra";
export type SourceDraftSide = "Dr" | "Cr";

export type SourceDraftSourceEntry = {
  position: number;
  source_ledger: string | null;
  source_amount: string | null;
  source_polarity: string | null;
};

export type SourceDraftProposedEntry = {
  ledger: string | null;
  side: SourceDraftSide | null;
  amount: string | null;
};

export type SourceDraftProposal = {
  date: string | null;
  voucher_type: SourceDraftVoucherType | null;
  narration: string | null;
  notes: string;
  entries: SourceDraftProposedEntry[];
};

export type SourceDraftRow = {
  position: number;
  source_remote_id: string | null;
  source_date: string | null;
  source_voucher_type: string | null;
  source_narration: string | null;
  entries: SourceDraftSourceEntry[];
  source_omitted_fields: string[];
  proposal: SourceDraftProposal;
};

export type SourceDraftSourceNotice = {
  kind: string;
  count: number;
};

export type SourceDraftCurrentCatalogBinding = {
  row_position: number;
  entry_position: number;
};

export type SourceDraft = {
  draft_id: string;
  revision: number;
  source_filename: string;
  source_sha256: string;
  source_notices: SourceDraftSourceNotice[];
  rows: SourceDraftRow[];
  current_catalog_bindings: SourceDraftCurrentCatalogBinding[];
  // The draft's current catalog generation. An invalidation request names the
  // draft and generation it means to clear, so a stale one -- queued before a
  // draft replacement or an earlier invalidation -- can be told apart from a
  // current one instead of landing on whatever draft happens to be active.
  catalog_generation: number;
};

export type SourceDraftAction = "choose" | "open" | "save" | "catalog_load" | "catalog_apply" | "catalog_clear" | null;

export type SourceDraftCompanyScope = {
  config: { host: string; port: number };
  selected_company: {
    display_name: string;
    company_guid: string;
    company_number: string;
    books_from_yyyymmdd: string;
  };
};

/// One source entry's deterministic binding against the captured catalog.
/// Advisory: it narrows the target list and confers no authority. Applying a
/// name still goes through the unchanged assign path, which rereads the
/// catalog and proves the selection is current.
export type SourceDraftCatalogBinding = {
  row_position: number;
  entry_position: number;
  bound_target: string | null;
  bound_basis: "identifier" | "exact_name" | "normalized_name" | null;
  unbound_reason: string | null;
  candidates: string[];
  candidate_count: number;
  /** Whether candidate_count is a conservative lower bound. */
  candidate_count_is_lower_bound: boolean;
  /** "none" | "listed" | "truncated" | "withheld" — the core's own word for
   *  this state, carried rather than inferred: an empty list beside a nonzero
   *  count is a withheld family or an exhausted budget, and they differ. */
  candidate_listing: "none" | "listed" | "truncated" | "withheld";
};

export type SourceDraftCatalogTargets = {
  capture_id: string;
  source_sha256: string;
  targets: string[];
  bindings: SourceDraftCatalogBinding[];
  /// Whether the narrowing pass **ran**, not whether it resolved anything.
  /// "complete" means every source entry was put through binding and carries a
  /// result — which for many of them will be a near miss or nothing at all;
  /// "unavailable" means the pass could not run, so an empty list says nothing.
  /// An empty list alone cannot distinguish the two, which is why this exists.
  ///
  /// It is **not** an all-bound signal and must not gate resolution: a draft
  /// whose every entry is unmatched still reports "complete". Read the bindings
  /// for that.
  bindings_state: "complete" | "unavailable";
  evidence: { request_sha256: string; response_sha256: string; bytes: number; state: "complete" };
};

export type SourceDraftScreenProps = {
  onBusyChange?: (busy: boolean) => void;
  onTallyReadActivityChange?: (active: boolean) => void;
  catalogScope?: SourceDraftCompanyScope;
  catalogScopeKey?: string;
  onDirtyChange?: (dirty: boolean) => void;
  editingEnabled?: boolean;
  lifecycleInteractionBlocked?: boolean;
  isLifecycleInteractionBlocked?: () => boolean;
  protectionError?: string | null;
};
