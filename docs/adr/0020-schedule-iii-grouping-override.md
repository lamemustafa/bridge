# 0020 — A CA's Schedule III grouping decision is a recorded, reviewable event

- Status: Proposed
- Date: 2026-09-26
- Related: #737; [0002 — Tally company identity](0002-tally-company-identity.md);
  [0019 — Audit read at rest](0019-audit-read-at-rest.md); `docs/tally/privacy-model.md`;
  `docs/tax-audit/config-identity-binding-v1.md`; `docs/tally/TALLY_PROTOCOL_REFERENCE.md` §9.11b

## Context

- **Standards.** Schedule III and ICAI's Guidance Notes set *minimum* disclosure. Presentation, including
  addition or substitution of line items, is "a matter of professional judgement" (Guidance Note on Division I –
  Non Ind AS Schedule III, Revised January 2022, paras 4.1.2 and 6.17). The Act and notified Accounting Standards
  prevail (para 4.1.1).
- **A Tally group is not a heading.** It is the client's bookkeeping chart, not the financial-statement
  classification. A CA will often present a ledger under a head its group does not imply.
- **SA 230's requirement.** SA 230 paras 8(c) and 9(b)–(c) require significant judgements to be documented, with
  who made and reviewed them, and when.
- **What exists in ComplyEaze Bridge today:**
  - **One view.** `src-tauri/src/reports/schedule_iii.rs` derives, from a party/ledger master read, only what that
    read establishes: a **group subtotal** for four built-in group identities (Sundry Debtors, Sundry Creditors,
    Cash-in-Hand, Bank Accounts), with the balance on that group's side.
  - **No statutory head.** A group plus a balance side does not establish a statutory head (for example "trade"
    receivables asserts where the balance came from), so the view names no statutory head.
  - **Everything else is excluded,** with "Group hierarchy does not determine a Schedule III head; client mapping
    decision required."
  - **Nowhere to record the decision.** No store holds any mapping.
- **Other consumers:**
  - The tax-audit engine and its Rust port produce no Schedule III view. They group the P&L by the books' own
    primary groups.
  - The Profit and Loss and Balance Sheet read (#692) is gated line by line on Tally's own Balance Sheet, which
    follows the book's groups.

## Decision

**A grouping decision is an append-only, GUID-bound event made by a named person, with a required reason. The
Schedule III view is built only through `build_schedule_iii_view`, which binds every decision against the read,
applies those that still fit, and reports the rest without applying them. No decision can change an amount.**

1. **Only a decision produces a statutory head.**
   - A line's basis is a type: `LineBasis::GroupSubtotal(kind)` from the read's evidence, or
     `LineBasis::Decided(ScheduleIIIHead)` from a CA's decision. Its section and caption text derive from the
     basis.
   - `ScheduleIIIHead` is a closed type, each head with its section and the side its balances sit on. Its first
     catalogue is a provisional subset until the framework is chosen (open item 1).
2. **The record.**
   - Events are `Set`, `Withdraw`, `Review` and `CarryForward`. The status (active or withdrawn) is a fold of the
     log, and nothing is updated or deleted.
   - Each event names its ledger by GUID, never by name.
   - A `Set` records:
     - the ledger's name at the time;
     - **made against:** what the read said of the ledger when the decision was made (`Derivation`). That is its
       outcome (a group subtotal, or the typed reason none was given) and the names of the groups above it;
     - **head:** the `ScheduleIIIHead` chosen;
     - **why:** required, with an optional reference;
     - **by:** a name or initials the person declares. Bridge has no user accounts, so every output labels it as
       self-declared;
     - **at:** the time.
3. **Scope: the book and the year, not the connection.**
   - Decisions are keyed by the observed company tuple (case-folded GUID, company number and books-from date) and
     one financial year.
   - **This is a deliberate departure from ADR 0002,** whose console correlation does not merge one GUID across
     endpoints. A decision belongs to the book, not to the port it was reached on.
   - Books-from is in the tuple because a year-end split can give the new company its parent's GUID (§9.11b).
   - **The key is the primary guard.** It belongs to the store: the view takes the decisions it is given, so
     the store must hand it only one book's decisions for one year, as a typed per-book set. The per-ledger check
     in item 4 is a partial second guard: a decision
     reached through an unrelated book finds its ledger missing, or its group changed, and is not applied. After
     a year-end split the ledgers and groups match by construction, so there only the year and books-from in the
     key protect.
4. **Bound per read.** A decision applies only while its ledger keeps the **standing** it had when the decision
   was made. The standing is one of three:
   - the nearest predefined group (its `RESERVEDNAME`);
   - a user-created group directly under the account root (by name, so a rename also counts as a change);
   - the account root itself.

   A balance changing side or settling to zero within the same group is not a move. Whether the head still fits
   that balance is checked on its own. Each decision is sorted into exactly one status (`NotApplied` names the
   reasons):
   - **applied,** reporting any drift on the ledger's row and in the decision list: the old name when the
     ledger was renamed in Tally (the GUID is unchanged), and the old and new group chain when its groups changed
     without changing its standing;
   - **for another financial year:** offered, never applied;
   - **ledger missing;**
   - **balance not established;**
   - **group read incomplete:** no parent, a gap in the group read, or a group whose parent Tally did not return.
     None of these gives the ledger a standing, so no decision may rest on it;
   - **group changed:** the ledger's standing differs from when the decision was made;
   - **balance on the other side from the head.**

   A move between user-created subgroups under the same predefined group does not change the standing, so the
   decision still applies. It is never silent: it is reported as drift, as a rename is, because the CA's reason
   may have rested on the subgroup the ledger has left.

   Any status other than applied or another year makes the view `NotFinal`. It is printed as NOT FINAL, not
   withheld, so the CA can see what to resolve. Two decisions for one ledger in one year are an error: the export
   fails rather than render.
5. **Amounts are not an input.**
   - A decision moves a row index between lines, and every line total is summed from the read's own rows. Amounts
     therefore conserve by construction.
   - **What is checked:** every row is placed exactly once, checked against the read's own row count, and the net
     of the lines plus the excluded rows is re-added against the read's net. The placement half is the real
     control; the net half guards the arithmetic.
   - A failure of either half is an error: the export fails rather than render.
   - Splitting one ledger's amount across heads is out of scope until separately decided, because it is the one
     change that would break this.
6. **Output.**
   - Every line and trace row states its basis: Tally group evidence, or a CA grouping decision.
   - Every decision is listed with its status.
   - The register adds who, why, when and review once decisions are stored, keeping withdrawn and superseded
     decisions in a history block.
7. **Storage.**
   - A new append-only table in the encrypted Tally mirror, with update and delete refused by triggers.
   - Book data and a person's name belong only in the encrypted store (privacy model), never in an egress receipt,
     log or diagnostic.
8. **Authoring is a desktop action by a person.** No MCP tool writes a decision, because a professional judgement
   needs a person as its "who". A decision leaves Bridge only in the exported workbook.
9. **Carry-forward.** A decision from the previous year is offered in the new year, and is never applied until a
   person confirms it. Confirming records "made against" afresh from the new year's read.
10. **Review.** A `Review` event is recorded and printed. In this version it does not gate finality.

## What this does not cover

- **#692's statements never take decisions.** That read proves the books' own statements against Tally's Balance
  Sheet, and any regrouping would fail that gate by construction. Schedule III presentation is a different
  product.
- **The tax-audit engine does not consume decisions.** Doing so would change figures it derives from the grouping
  (revenue totals feed the turnover test and clause 40). It would also first need the same ancestry derivation as
  Bridge: the engine walks groups by name, Bridge by `RESERVEDNAME`. That is a separate decision.
- **Group-level decisions.** They need a group GUID in the read that feeds this view. That read's group parser
  keeps no record identity; the core-window group parser does require GUID. They follow live evidence that the
  GUID-bearing read is stable across a group rename (AGENTS.md P1).

## Consequences

- A CA's regrouping becomes reviewable evidence: who decided what, against what, onto which head, and why, with
  history.
- Most early decisions will place ledgers the view refuses to classify today, rather than contradict a group
  subtotal. The record treats both the same way.
- Until the store exists, the export presents no decisions. The view then differs from today's only by:
  - a basis column;
  - a "None recorded" line;
  - reworded notes;
  - one exclusion text. A ledger whose group chain ends in a group with no parent in the read now says so, instead
    of "client mapping decision required".
- **Unverified assumption:** that Tally returns a user-created primary group's parent as the account root, as it
  does for every predefined one captured. No capture holds a user-created primary group yet. If the assumption is
  wrong, those ledgers are excluded and no decision applies (fail closed). Capturing one is on the lab list.

## Open items before Accepted

1. The first framework, Division I (companies) or the non-corporate Guidance Note, and its full head catalogue.
2. Whether a recorded review should gate finality in a later version.
3. Current/non-current splits: amounts or rules, and a separate conservation argument.
