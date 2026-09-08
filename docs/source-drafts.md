# Prepare a local XML draft

Open **Prepare file** in the desktop sidebar, then **Choose source XML**. This
screen accepts a bounded Tally voucher-export XML file and shows its original
voucher and ledger-entry observations beside proposals you enter yourself.
It does not require a connected company.

Select a row to enter a proposed date, voucher type, narration, preparation
notes and ledger-entry choices. All choices start unanswered. Source amounts
and polarity remain separate: Bridge does not infer debit/credit sides or
confirm ledger names from these files. **Proposal started** means that some
text or choice was entered; it does not mean the proposal is complete or valid
for accounting.

**Save draft** opens a native save dialog for a `.bridge-draft.json` file.
Unanswered proposals can be saved. **Open saved draft** verifies the embedded
source hash, reparses its original XML, and checks proposal row/entry counts.
Navigating to another Bridge view preserves the current edits. Choosing a new
source or opening another draft asks before discarding unsaved edits; cancelling
the native picker keeps the current draft. Save before closing the application.

The saved JSON contains the original XML and your proposals, including any
sensitive information they contain. Store and share it accordingly. This
feature performs local file preparation; it does not upload the source or
send it to Tally or an AI provider. It cannot approve or post vouchers, create
masters, verify company-specific ledger mapping, or reconcile a statement PDF.
A saved draft is not an import payload or an accounting approval.

Source XML is limited to 2 MiB, 2,000 vouchers and 20 direct ledger entries per
voucher. Text fields are bounded to 4,096 bytes, proposed text to 2 MiB in
aggregate, and saved JSON to 8 MiB. Files must be regular local files; aliases,
symlinks and hard links are refused. DTDs, custom entities and unsupported
nested voucher fields are refused. Header/request metadata and non-voucher
records remain in the original XML; source notices identify records that are
not edited in the row view. Other unrepresented scalar fields are listed per
row.

The source checksum detects changes within a saved draft; it does not establish
who produced the original file or that its accounting contents are correct.
