# agents.md

This document defines agent-level expectations and review responsibilities for this repository.

## Agents and responsibilities

- **Core implementation agent**: owns Rust/Tauri and React implementation and module-level code health.
- **Security agent**: owns DSC credential handling, endpoint validation, and data-leak prevention checks.
- **Release agent**: owns CI, packaging, changelog/release prep, branch policy,
  dependency-license inventory, and proof that license/NOTICE resources ship
  in supported installers.
- **Docs and governance agent**: owns onboarding docs, PR templates, issue
  lifecycle, contribution licensing, provenance checks, and NOTICE updates.

## Review flow

- All code changes go through a pull request.
- Every PR must include:
  - Functional summary
  - Test or reproduction command
  - Migration impact notes if changing sync behavior
  - Security impact notes for DSC/Tally/credential changes
- Each PR must link to one line in [review-checklist.md](./review-checklist.md) as completed before merge.

## Rectification expectations

- **When defects are found**: open a follow-up `Bug` issue and include a `Rectify` PR with root-cause and regression check.
- PRs that touch existing workflows must include rollback notes and migration compatibility.
- Keep issue triage actionable:
  - assign one area label (tally / dsc / documents / infra / security)
  - set severity (`P1` urgent / `P2` production / `P3` medium / `P4` cleanup)
  - avoid open "wip" tasks without acceptance evidence.
- If regression was introduced by a specific PR, link it explicitly in the rectify issue and include it in the fix PR summary.
- For non-security production regressions, use a dedicated fix branch and label (`type:rectify`).

## Safety expectations

- Never commit hardcoded secrets, tokens, API keys, or raw certificate output.
- Never commit personal or customer data, local usernames, home directories, or
  developer-specific absolute paths; use synthetic examples and repository-relative paths.
- Any DSC or credential path changes require a security-focused reviewer comment.
- Any platform-sensitive change must be validated on affected Windows and macOS hosts,
  or the missing platform evidence must be called out explicitly in the PR.
- Never merge a PR that introduces destructive DB migrations without rollback notes.
- Never relicense or add third-party code or assets without documented authority
  and preservation of applicable copyright, license, and attribution notices.
- If you discover policy drift from this file, open an explicit PR to rectify before feature work.
