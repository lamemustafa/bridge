# Tally roadmap backlog (parked scope)

Scope that any prompt, reviewer, or contributor proposes beyond the
current phase lands here — never in the current PR (see
[PROMPT_PLAYBOOK.md](./PROMPT_PLAYBOOK.md) §7.1 standing rules). Items
graduate only through a plan amendment (§7.4 deviation prompt) or when
their gating condition in [IMPROVEMENT_PLAN_2026H2.md](./IMPROVEMENT_PLAN_2026H2.md)
is met.

## Parked by ruling (do not build this horizon)

| Item | Why parked | Revisit condition |
| --- | --- | --- |
| GSTR-2B bulk-resolution layer | Tally native owns single-company recon; solo dev can't fight the platform now | Write substrate has run one clean quarter AND a design partner asks |
| Remote agent on client machines | Fleet product a solo dev cannot operate; reputational risk lands on the firm | Cloud relay + support capacity exist |
| Client-maintained-books drift via backup/TCP ingestion | Lighter alternative to the remote agent; still post-wedge | Drift Sentinel v1 adopted at 2+ firms |
| Tally-on-cloud (hosted RDP) topology — headless agent in VM | v1 is local single-machine only; declared `Unsupported` in Passport | A design partner runs hosted Tally; local topology at GA |
| Bank-statement PDF/OCR parsing | Format-zoo maintenance tail; CSV/Excel covers most rows | Thin product loop in daily use |
| GSTR-1 prep/validation engine | GST-rules maintenance tail worse than no tool when stale | After month 12, with a rules-update commitment |
| TDS compliance engine | Same maintenance-tail class | After month 12 |
| Multi-company control tower | Green wall over unloaded companies = silent-failure theater; sells renewals not trials | 20+ live companies at one firm; redesign around expected staleness |
| E-invoice / e-way bill | ClearTax owns it; needs GSP infra + per-machine TDL | Not planned |
| Connected banking / payment initiation | TallyPrime native; bank-API arms race | Not planned |
| Receivables dunning (SMS/WhatsApp/call) | CredFlow's company; comms infra + support headcount | Not planned (Pulse carries approvals/alerts only) |
| Mobile analytics dashboards | Biz Analyst's entrenched turf | Evidence-viewer only, post cloud relay |
| AI OCR document extraction at scale | Model arms race vs funded teams | Post cloud consent architecture |
| TDL plugin with in-Tally UI | Breaks zero-install purity; per-machine support burden | Not planned |
| Inventory depth / store-keeper flows | Not the CA/CS buyer's job | Not planned |
| Education-mode posting scheduler | Test constraint leaking into product | Never (Passport-detected restriction only) |

## Parked engineering ideas (unscheduled)

- ODBC read-only cross-check channel for reconciliation totals.
- JSONEX transport promotion (existing shadow-comparator machinery; needs
  measured operational benefit per the legacy plan's PR 10 rules).
- Mirror data-map & key-management screen ("which machines hold which
  clients' books") — adoption-critic suggestion, revisit with multi-seat.
- Proof-of-Post PDF localization (Hindi/Gujarati).

_Add new items with date + one-line reason + revisit condition._
