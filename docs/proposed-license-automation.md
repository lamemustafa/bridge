# Proposed CI wiring for `scripts/generate-frontend-licenses.mjs`

This repo's `.github/` workflows are intentionally untouched by the change that
added `scripts/generate-frontend-licenses.mjs`. This document is the proposal
for wiring it in, for a maintainer to review and apply.

## What already works without any CI change

`ci.yml`'s `frontend` job already runs `corepack pnpm run license:check`
(`check-license-metadata.mjs` + `check-dependency-inventory.mjs --frontend`),
and `license:all` runs in the release workflow. Both keep failing a Dependabot
PR exactly as before until `THIRD_PARTY_LICENSES.txt` is regenerated — the
generator does not change what CI enforces, only how a human (or a bot)
satisfies it: `corepack pnpm run license:generate:frontend` now does in one
command what used to be a hand-edit.

## Gap 1 (pre-existing, not introduced by this change): the `bundle` path filter

`ci.yml` line 66 decides whether the bundle/license jobs run by grepping
`changed_files` against an explicit allowlist. That allowlist already omits
`scripts/generate-rust-licenses.mjs` — a change to the Rust generator alone
does not mark `bundle=true` — and the same gap now applies to
`scripts/generate-frontend-licenses.mjs`. In practice this is low-risk (the
generators are touched rarely and almost always alongside `package.json` /
`pnpm-lock.yaml`, which are already in the allowlist), but it is a real gap.
Proposed fix, folded into the existing regex on line 66:

```diff
- scripts/(capture-package-log(\.test)?\.py|check-mcpb-bundle(\.test)?\.py|package-mcpb\.mjs|check-license-metadata\.mjs|check-dependency-inventory\.mjs|check-windows-bundle-resources\.ps1|check-macos-bundle-resources(\.mutation)?\.mjs)$
+ scripts/(capture-package-log(\.test)?\.py|check-mcpb-bundle(\.test)?\.py|package-mcpb\.mjs|check-license-metadata\.mjs|check-dependency-inventory\.mjs|generate-rust-licenses\.mjs|generate-frontend-licenses(\.test)?\.mjs|check-windows-bundle-resources\.ps1|check-macos-bundle-resources(\.mutation)?\.mjs)$
```

## Gap 2 (the actual manual-regeneration tax): nothing regenerates the file for you

Today a Dependabot PR that bumps a frontend dependency still needs a human to
run the generator and push a commit before `license:check` goes green. The
generator makes that a one-line, no-judgment-calls command, so it is a good
candidate for a bot step. Proposed addition to `ci.yml`'s `frontend` job (or a
separate `dependabot`-triggered job — either works; shown here as a step
appended to the existing job, gated so it only ever runs for Dependabot's own
branches and never mutates a human-authored PR silently):

```yaml
      - run: corepack pnpm install --frozen-lockfile
      - run: corepack pnpm run license:generate:frontend
      - name: Fail if the frontend license inventory needed regeneration
        if: github.actor != 'dependabot[bot]'
        run: git diff --exit-code -- THIRD_PARTY_LICENSES.txt
      - name: Commit regenerated frontend license inventory
        if: github.actor == 'dependabot[bot]' && !cancelled()
        run: |
          if ! git diff --quiet -- THIRD_PARTY_LICENSES.txt; then
            git config user.name "github-actions[bot]"
            git config user.email "github-actions[bot]@users.noreply.github.com"
            git add THIRD_PARTY_LICENSES.txt
            git commit -m "chore(licenses): regenerate frontend inventory"
            git push "https://x-access-token:${{ secrets.GITHUB_TOKEN }}@github.com/${{ github.repository }}.git" "HEAD:${{ github.head_ref }}"
          fi
      - run: corepack pnpm run license:check
```

Notes for whoever applies this:

- Dependabot PRs from forks (n/a here, since this repo's Dependabot runs
  against the base repo directly) would need `pull_request_target` plus the
  usual secret-exposure caution; verify this repo's Dependabot config before
  copying the push step as-is.
- The default `GITHUB_TOKEN` needs `contents: write` on that job, and pushing
  to a PR branch from an Actions run must not be blocked by branch protection
  on Dependabot's branch naming pattern (`dependabot/npm_and_yarn/*`).
- For a non-Dependabot PR, the added `git diff --exit-code` step turns a stale
  inventory into a clear, fast CI failure ("run `pnpm run
  license:generate:frontend` and commit the result") instead of the opaque
  `check-dependency-inventory.mjs` drift error a contributor currently has to
  interpret and fix by hand.
- The same shape (regenerate, diff-or-commit, then re-check) applies to
  `license:generate:rust` / `THIRD_PARTY_LICENSES_RUST.txt`, which already
  exists but has never been wired into CI this way either.
