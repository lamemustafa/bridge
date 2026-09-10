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

export type SourceDraft = {
  draft_id: string;
  revision: number;
  source_filename: string;
  source_sha256: string;
  source_notices: SourceDraftSourceNotice[];
  rows: SourceDraftRow[];
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
  candidates_truncated: boolean;
};

export type SourceDraftCatalogTargets = {
  capture_id: string;
  source_sha256: string;
  targets: string[];
  bindings: SourceDraftCatalogBinding[];
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
