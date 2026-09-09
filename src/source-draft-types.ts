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

export type SourceDraftAction = "choose" | "open" | "save" | null;

export type SourceDraftScreenProps = {
  onBusyChange?: (busy: boolean) => void;
  isNativeLifecycleCompletionBlocked?: () => boolean;
  onNativeLifecycleModalChange?: (open: boolean) => void;
  onNativeLifecycleModalClosed?: (restoreFocus: () => void) => void;
};
