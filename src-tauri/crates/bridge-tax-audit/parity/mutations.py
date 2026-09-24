# SPDX-License-Identifier: Apache-2.0
"""Run the crate's recorded mutations and keep their results.

Each mutation in `parity/mutations.json` edits one source file (`from` -> `to`). It is "killed" when
some test of the crate fails on the edited source, "survives" when none does, and "did not apply"
when its `from` text is no longer in its file exactly once (update or retire it). The list keeps
every mutation an author or a reviewer has written for this crate, with who wrote it.

    python3 parity/mutations.py                         # every mutation, killers first
    python3 parity/mutations.py M08 B12                 # by id
    python3 parity/mutations.py --changed-since origin/master --jobs 3
    python3 parity/mutations.py --changed-since origin/master --verify   # no build: the merge check
    python3 parity/mutations.py --full --shard 2/8 --results shard.json  # nightly, one shard
    python3 parity/mutations.py --merge shard*.json --results all.json --report report.md

Run from anywhere with the pinned toolchain exported (RUSTC, RUSTDOC and PATH; see
docs/release-process.md).

## What a verdict rests on

Mutations are never applied to the working tree. Each worker is a copy of the COMMITTED bytes of
HEAD's tracked `src-tauri` files (plus the root `rust-toolchain.toml`) under
`src-tauri/target/mutants/w<N>/`, with its own `CARGO_TARGET_DIR`; a mutation is applied to the
copy's bytes and undone there. Only files whose bytes differ are re-written, so a worker's build
cache stays warm between runs. A run (not `--list` or `--verify`, which read only
committed trees and the list) refuses to start while tracked `src-tauri` files or
`rust-toolchain.toml` have uncommitted changes, so what it proves is what HEAD holds, and while
another run holds the same worker directory. It writes only the results file, the worker
directory and, with `--report`, the report.

Before any mutation, every worker builds the unmutated copy and the full suite runs on it once.
If a test already fails there, or the copy does not build, the run refuses: a test that fails
without the mutation would otherwise "kill" every mutation.

Each cargo run is two steps: `cargo test --no-run` (bounded by `--build-timeout`), then the tests
(bounded by `--timeout`), so the build's time never counts against the test timeout. The run
refuses unless `--timeout` is at least 3 times the unmutated suite's measured test time. A mutated
test run that still exceeds it is taken as a hang the suite would not let through, and counts as
killed everywhere: in the run, in `--verify` and in the nightly report. That is a bound, not a
proof: a machine slowed more than threefold mid-run could turn a survivor into a "timeout", and
the report lists every timeout as a warning. A build that exceeds `--build-timeout` does not
count. A test binary that dies of a crash signal (SIGABRT, as a stack overflow aborts, SIGSEGV,
SIGBUS, SIGILL, SIGFPE) without naming a failed test counts as killed by that target, recorded as
`<target>::<crashed>`; one killed from outside (SIGKILL, SIGTERM) proves nothing.

Killers first: when the results file records the tests that killed a mutation last time, those
tests run first (`--exact`, only their targets). If one fails or hangs, the mutation is killed
("killers-first" mode, and its margin is then a lower bound). Otherwise the FULL suite runs, so any
verdict other than killed comes from the full suite. `--full` skips the killers-first step (the
nightly run).

SIGINT or SIGTERM stops the cargo runs in flight (their own process groups) and exits; records
already written stay. A run killed outright (SIGKILL) can leave its cargo running and a mutation
in its worker copy; the next run's copy refresh undoes the mutation.

## The results file

`parity/mutation-results.json` is committed. It holds one record per mutation, one line each:
the verdict, the killing tests qualified by their target (`tests/edge_books.rs::name`,
`src/lib.rs::book::tests::name`), the mode, the commit and time, a hash of the mutation's own
definition, and `crate_tree`, a hash of this crate's committed tree without the results file.

`parity/accepted-survivors.json` lists the mutations no test is expected to kill, each with the
hash of its definition, why, and where that is documented. A `survived` verdict counts as passing
only for a listed id whose current definition has that hash; an edited mutation, or one not
listed, must be killed. An entry that accepts nothing (its id retired or its definition changed)
fails `--verify` and the nightly until it is updated or removed, and a listed mutation that is now
killed is reported, so its entry can go.

The results file is self-attested: CI never builds, so it trusts that a record was made by this runner.
Only the nightly run re-proves the records independently.

## The guarantee

At merge, `--verify --changed-since <base>` (in CI, no build) requires:
- every mutation in the list applies exactly once to the merged tree;
- no record for a mutation no longer in the list (retiring a mutation deletes its record); and
- every mutation SELECTED by the change has a killed record made on exactly the merged crate
  tree. A mutation is selected when
  - its target file changed;
  - a file holding one of its recorded killing tests changed, or a file that file pulls in with
    `include_str!`/`include_bytes!`, or `tests/common` for an integration-test killer;
  - its own definition changed, it has no killed record, or a killer's file is unknown;
  - ANY crate file other than Rust source under `src/` and `tests/` changed (fixtures, goldens,
    rules, parity scripts, the Markdown tests read, `Cargo.toml`, `build.rs`): then the whole
    list is selected, because tests read such files at run time in ways no file name reveals.
    The runner's own files and `parity/mutations.json` are the exception: no crate test reads
    them (a unit test fails if any crate source names them), and a change to the list is judged
    entry by entry against the merge base's list: an id that is new there, or whose definition
    changed, is selected whatever record the branch carries (so a record made on another tree, or
    carried over under a renamed id, must be made again on the merged tree), and a removed id
    leaves a record the orphan check refuses; or
  - a nightly tracking issue is open and lists it as failing (`--nightly-issues`), and the
    change touches the crate: so the fix for a nightly failure can merge, and nothing else can
    until the failing mutations are proven killed again (or retired). Every open issue must
    carry that line, and the ids of all of them are required together; if any open issue lacks
    it, every crate change is refused until a maintainer closes that issue.

CI hashes the pull request's MERGE commit, so when master has changed any crate file since the
branch was proven, update the branch from master and re-run the selection before pushing.

What the guarantee does NOT cover. The nightly run re-proves the whole list against the full suite
on master, so each of these carries up to about a day of exposure:
- Inputs outside this crate. Selection and `crate_tree` see only this crate's own files, so a
  change to any of these selects nothing: the path dependencies `bridge-tally-protocol` and
  `bridge-tally-primitives`; that protocol crate's `tests/fixtures/native`, read by
  `tests/tally_text_rule.rs`; the workspace `src-tauri/Cargo.toml`, whose `[workspace.lints]`
  this crate inherits; `src-tauri/Cargo.lock`; and the root `rust-toolchain.toml`.
- A change to crate source that is neither a mutation's target nor a killer's file but still
  weakens a killer: a test helper in another module, or a `mod` line that stops compiling a
  test module.
- A mutation killed by a hang: its record names no killing test, so only a change to its target
  file or a non-source input selects it.
"""
from __future__ import annotations

import argparse
import datetime as _dt
import fcntl
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parent.parent  # the crate
WORKSPACE = ROOT.parent.parent  # src-tauri
REPO = WORKSPACE.parent
CRATE = ROOT.relative_to(REPO).as_posix()  # src-tauri/crates/bridge-tax-audit
PACKAGE = "bridge-tax-audit"
LIST = ROOT / "parity" / "mutations.json"
RESULTS = ROOT / "parity" / "mutation-results.json"
RESULTS_REL = f"{CRATE}/parity/mutation-results.json"
SURVIVORS = ROOT / "parity" / "accepted-survivors.json"
WORKERS = WORKSPACE / "target" / "mutants"
COPIED = ("src-tauri", "rust-toolchain.toml")  # repo-relative: what a worker copy holds
DEFAULT_TIMEOUT = 45 * 60
DEFAULT_BUILD_TIMEOUT = 60 * 60
KILL_GRACE = 30  # seconds a cargo group gets after SIGTERM before SIGKILL
MARGIN = 3  # --timeout must be at least this many times the unmutated suite's test time

KILLED = "killed"
SURVIVED = "survived"
DID_NOT_APPLY = "did_not_apply"
COMPILE_ERROR = "compile_error"
TIMEOUT = "timeout"
BUILD_TIMEOUT = "build_timeout"
FAILED_NO_TEST = "failed_no_test"
PASSING = {KILLED, TIMEOUT}  # a test-run timeout is a hang the suite would not let through
CRASHED = "<crashed>"  # the test name recorded when a test binary dies without naming a failure

# Crate-relative files that never change what a test does: no crate source names them (a unit
# test holds that), and a change to the list is judged entry by entry (see the docstring).
INERT = ("parity/mutations.py", "parity/mutation-results.json", "parity/test_mutations.py",
         "parity/mutations.json")
# The machine-readable line a failed report ends with, which the nightly puts in its issue.
FAILING_MARK = "mutation-nightly-failing:"
# The header ci.yml writes before each open issue's body in the --nightly-issues file.
ISSUE_MARK = "mutation-nightly-issue"
REPORT_ROWS = 100  # rows per report section; GitHub caps an issue body at 65,536 characters


class Stopped(Exception):
    """The run was interrupted; the mutation in flight has no verdict."""


# ------------------------------------------------------------------------------------ pure parts
def mutation_hash(m: dict) -> str:
    """A mutation's own identity: its file, `from` and `to`. An edited mutation is unproven."""
    body = json.dumps({"file": m["file"], "from": m["from"], "to": m["to"]}, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(body.encode("utf-8")).hexdigest()[:16]


def proven(rec: dict | None) -> bool:
    """Whether a record proves its mutation killed (a test-run timeout included)."""
    return rec is not None and rec.get("verdict") in PASSING


def accepted_survivor(m: dict, accepted: dict | None) -> bool:
    """Whether `m` is listed in the accepted-survivors file for exactly this definition. An entry
    for an edited mutation (another hash) accepts nothing."""
    entry = (accepted or {}).get(m["id"])
    return entry is not None and entry.get("mutation") == mutation_hash(m)


def passes(m: dict, rec: dict | None, accepted: dict | None = None) -> bool:
    """Whether a record is a verdict the guarantee accepts: killed, or a survivor listed for this
    exact definition in the accepted-survivors file."""
    return proven(rec) or (rec is not None and rec.get("verdict") == SURVIVED and accepted_survivor(m, accepted))


def survivor_problems(mutations: list[dict], accepted: dict) -> list[str]:
    """Entries of the accepted-survivors file that accept nothing: an id not in the list, or a
    hash that is not the mutation's current definition. Each must be updated or removed."""
    by_id = {m["id"]: m for m in mutations}
    out = [f"{i}: accepted as a survivor but no longer in the list" for i in sorted(accepted) if i not in by_id]
    out += [f"{i}: accepted as a survivor for another definition" for i in sorted(accepted)
            if i in by_id and not accepted_survivor(by_id[i], accepted)]
    return out


_RUNNING = re.compile(r"^\s*Running (?:unittests )?(\S+) \(")
_DOCTESTS = re.compile(r"^\s*Doc-tests (\S+)")
_FAILED = re.compile(r"^test (.+?) \.\.\. FAILED$")
_COMPILE = re.compile(r"^error\[E\d+\]|^error: could not compile ")
# A test binary that died of its own fault; SIGKILL/SIGTERM (an OOM killer, an operator) are not.
_CRASHED = re.compile(r"^\s*process didn't exit successfully: .*\(signal: \d+, (SIGABRT|SIGSEGV|SIGBUS|SIGILL|SIGFPE)\b")


def parse_test_output(out: str) -> tuple[list[str], bool]:
    """(failed tests, each qualified by the target cargo reported it under, compile error?), read
    from `cargo test` output with stderr merged into stdout, so each `Running` line precedes its
    binary's results and cargo's report of how that binary exited. A doc test is qualified as
    `doc`. A binary that died of a crash signal (SIGABRT, as a stack overflow aborts, SIGSEGV,
    SIGBUS, SIGILL, SIGFPE) without naming a failed test is recorded as `<target>::<crashed>`;
    one killed from outside (SIGKILL, SIGTERM) names nothing and proves nothing."""
    failed: list[str] = []
    target = "?"
    named = False  # whether the current target has named a failed test
    compile_error = False
    for line in out.splitlines():
        if m := _RUNNING.match(line):
            target, named = m.group(1), False
        elif m := _DOCTESTS.match(line):
            target, named = "doc", False
        elif m := _FAILED.match(line):
            failed.append(f"{target}::{m.group(1)}")
            named = True
        elif _CRASHED.match(line) and not named:
            failed.append(f"{target}::{CRASHED}")
            named = True
        elif _COMPILE.match(line):
            compile_error = True
    return sorted(set(failed)), compile_error


def killer_file(killer: str, crate: Path = ROOT) -> str | None:
    """The crate-relative source file that holds a qualified killing test, or None when unknown.
    `tests/x.rs::name` lives in tests/x.rs; `src/lib.rs::a::b::name` in the file of the longest
    module path that exists (src/a/b.rs, src/a/b/mod.rs, then src/a.rs ...), so an inline
    `mod tests` maps to its parent's file and a `tests.rs` file to itself; `doc::src/x.rs - item
    (line N)` in src/x.rs (cargo may print it workspace-relative). The file must still exist and
    define `fn <name>`: a renamed file, a `#[path]` remap or a macro-made test is unknown, and an
    unknown killer file selects its mutation on every change. A crashed library binary names no
    test, so its file is unknown too."""
    target, _, name = killer.partition("::")
    if target == "doc":
        path = name.split(" - ", 1)[0].strip()
        prefix = crate.relative_to(crate.parent.parent).as_posix() + "/"
        path = path[len(prefix):] if path.startswith(prefix) else path
        return path if path.endswith(".rs") and (crate / path).is_file() else None
    if name == CRASHED:
        return target if target.startswith("tests/") and (crate / target).is_file() else None
    fn = name.split("::")[-1]
    if target.startswith("tests/"):
        return target if _defines(crate, target, fn) else None
    if target == "src/lib.rs":
        parts = name.split("::")[:-1]  # the module path; the last part is the test fn
        while parts:  # `book::tests` is src/book/tests.rs if that file exists, else src/book.rs
            for candidate in (f"src/{'/'.join(parts)}.rs", f"src/{'/'.join(parts)}/mod.rs"):
                if (crate / candidate).is_file():
                    return candidate if _defines(crate, candidate, fn) else None
            parts = parts[:-1]
        return "src/lib.rs" if _defines(crate, "src/lib.rs", fn) else None
    return None


def _defines(crate: Path, path: str, fn: str) -> bool:
    """Whether the crate file `path` defines a function named `fn`."""
    try:
        text = (crate / path).read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return False
    return re.search(rf"\bfn\s+{re.escape(fn)}\s*[(<]", text) is not None


_INCLUDE = re.compile(r'include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)')


def included(source_file: str, crate: Path = ROOT) -> set[str]:
    """Crate-relative files `source_file` pulls in with `include_str!`/`include_bytes!`, followed
    transitively; each path resolves against the including file's directory, as rustc does."""
    seen: set[str] = set()
    todo = [source_file]
    while todo:
        f = todo.pop()
        try:
            text = (crate / f).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for rel in _INCLUDE.findall(text):
            parts: list[str] = []
            for p in (PurePosixPath(f).parent / rel).parts:
                if p == "..":
                    if parts:
                        parts.pop()
                elif p != ".":
                    parts.append(p)
            path = "/".join(parts)
            if path not in seen:
                seen.add(path)
                todo.append(path)
    return seen


def is_source(path: str) -> bool:
    """Crate-relative Rust source that the file-level selection rules understand."""
    return path.endswith(".rs") and path.startswith(("src/", "tests/")) and not path.startswith("tests/fixtures/")


def select(mutations: list[dict], results: dict, changed: list[str], crate: Path = ROOT,
           accepted: dict | None = None, base: dict[str, str] | None = None) -> dict[str, list[str]]:
    """{id: [reasons]} for every mutation the change selects; see the module docstring. `changed`
    is crate-relative paths."""
    changed = [c for c in changed if c not in INERT]
    other = sorted(c for c in changed if not is_source(c))
    changed_set = {c for c in changed if is_source(c)}
    if any(c.startswith("tests/common/") for c in changed_set):
        # every integration test binary may include tests/common
        changed_set |= {f"tests/{p.name}" for p in (crate / "tests").glob("*.rs")}
    picked: dict[str, list[str]] = {}
    for m in mutations:
        reasons = []
        rec = results.get(m["id"])
        if other:
            more = f" and {len(other) - 3} more" if len(other) > 3 else ""
            reasons.append(f"non-source input changed: {', '.join(other[:3])}{more}")
        if m["file"] in changed_set:
            reasons.append(f"target {m['file']} changed")
        if base is not None and base.get(m["id"]) != mutation_hash(m):
            reasons.append("new or edited since the base")
        if not passes(m, rec, accepted):
            reasons.append("no killed record")
        elif rec.get("mutation") != mutation_hash(m):
            reasons.append("definition changed")
        else:
            files: set[str] = set()
            for k in rec.get("killers", []):
                f = killer_file(k, crate)
                if f is None:
                    reasons.append("a killer's file is unknown")
                else:
                    files |= {f} | included(f, crate)
            hit = sorted(files & changed_set)
            if hit:
                reasons.append(f"killer file changed: {', '.join(hit)}")
        if reasons:
            picked[m["id"]] = list(dict.fromkeys(reasons))
    return picked


def fresh(m: dict, rec: dict | None, tree: str, accepted: dict | None = None) -> bool:
    """Whether `rec` proves `m` killed (or an accepted survivor) on exactly the crate tree `tree`,
    for this definition."""
    return passes(m, rec, accepted) and rec.get("crate_tree") == tree and rec.get("mutation") == mutation_hash(m)


def verify(chosen: list[dict], results: dict, tree: str, order: list[str], accepted: dict | None = None) -> list[str]:
    """What stops a merge: each chosen mutation without a fresh record, and each record for a
    mutation no longer in the list."""
    problems = [f"{m['id']}: not proven on crate tree {tree}" for m in chosen if not fresh(m, results.get(m["id"]), tree, accepted)]
    problems += [f"{i}: a record for a mutation no longer in the list (delete it)"
                 for i in sorted(set(results) - set(order))]
    return problems


def failing_ids(text: str) -> list[str] | None:
    """The mutation ids the open nightly issues list as failing, from the line `report` writes.
    `text` holds each issue's body after a `<!-- mutation-nightly-issue N -->` header, as ci.yml
    writes it. None when ANY issue lacks the line, and when non-empty text holds no header at all
    (the header drifted from ci.yml's): an issue that cannot be read must not be hidden by
    another's line. [] when there is no issue."""
    if not text.strip():
        return []
    blocks = re.split(r"<!--\s*" + re.escape(ISSUE_MARK) + r"\s+\d+\s*-->", text)
    if len(blocks) == 1:
        return None
    blocks = blocks[1:]  # anything before the first header belongs to no issue
    ids: set[str] = set()
    for block in blocks:
        marks = re.findall(r"<!--\s*" + re.escape(FAILING_MARK) + r"([^>]*?)-->", block)
        if not marks:
            return None
        ids |= {i for mark in marks for i in mark.split()}
    return sorted(ids)


def shard(mutations: list[dict], k: int, n: int) -> list[dict]:
    """Shard k of n (1-based), whole files per shard, balanced greedily by mutation count."""
    counts: dict[str, int] = {}
    for m in mutations:
        counts[m["file"]] = counts.get(m["file"], 0) + 1
    loads = [0] * n
    owner: dict[str, int] = {}
    for f in sorted(counts, key=lambda f: (-counts[f], f)):
        i = min(range(n), key=lambda j: (loads[j], j))
        owner[f] = i
        loads[i] += counts[f]
    return [m for m in mutations if owner[m["file"]] == k - 1]


def report(mutations: list[dict], merged: dict, committed: dict, unreadable: list[str] = (),
           accepted: dict | None = None) -> tuple[str, bool]:
    """(Markdown, failed?) for a whole-list run. It fails when any mutation was not run or not
    killed, when a shard's results could not be read, or when the committed results file is stale
    (a record for a mutation not in the list, or none, or one made for a different definition).
    A failed report's first lines name every failing id, for the nightly issue; when a shard could
    not be read, every id in the list, since what it held is unknown. Each section lists at most
    REPORT_ROWS rows, so the report fits an issue body."""
    order = [m["id"] for m in mutations]
    by_id = {m["id"]: m for m in mutations}
    missing = [i for i in order if i not in merged]
    bad = [(i, merged[i]["verdict"]) for i in order if i in merged and not passes(by_id[i], merged[i], accepted)]
    survivors = [i for i in order if i in merged and not proven(merged[i]) and passes(by_id[i], merged[i], accepted)]
    now_killed = [i for i in order if proven(merged.get(i)) and accepted_survivor(by_id[i], accepted)]
    stale_survivors = survivor_problems(mutations, accepted or {})
    stale_ids = [i for i in order if i not in committed or committed[i].get("mutation") != mutation_hash(by_id[i])]
    stale = [f"{i}: no committed record" if i not in committed else f"{i}: committed record is for another definition"
             for i in stale_ids]
    stale += [f"{i}: committed record for a mutation no longer in the list" for i in committed if i not in by_id]
    timeouts = [i for i in order if merged.get(i, {}).get("verdict") == TIMEOUT]
    thin = [(i, merged[i]["killers"]) for i in order
            if i in merged and merged[i]["verdict"] == KILLED and len(merged[i]["killers"]) < 2]
    failed = bool(missing or bad or stale or unreadable or stale_survivors)
    out = [f"# Tax-audit mutations: {'FAILED' if failed else 'all killed'}", ""]
    if failed:
        ids = order if unreadable else sorted(set(missing) | {i for i, _ in bad} | set(stale_ids), key=order.index)
        out += [f"<!-- {FAILING_MARK} {' '.join(ids)} -->", ""]
    out += [f"{len(order)} in the list; {len(merged)} run; {sum(1 for i in order if proven(merged.get(i)))} killed.", ""]
    for title, rows in (("Shard results not read", [f"`{u}`" for u in unreadable]),
                        ("Not killed", [f"`{i}` ({by_id[i]['file']}): {v}" for i, v in bad]),
                        ("Not run", [f"`{i}`" for i in missing]),
                        ("Stale committed records", stale),
                        ("Stale accepted-survivor entries", stale_survivors),
                        ("Accepted survivors", [f"`{i}`: {accepted[i].get('reason', '')}" for i in survivors]),
                        ("Accepted as survivors but now killed: remove the entry (warning)", [f"`{i}`" for i in now_killed]),
                        ("Killed by a hang, not a failing test (warning)", [f"`{i}`" for i in timeouts]),
                        ("Killed by fewer than 2 tests (warning)", [f"`{i}`: {', '.join(k) or 'none'}" for i, k in thin])):
        if rows:
            more = [f"... and {len(rows) - REPORT_ROWS} more"] if len(rows) > REPORT_ROWS else []
            out += [f"## {title} ({len(rows)})", ""] + [f"- {r}" for r in rows[:REPORT_ROWS]] + more + [""]
    return "\n".join(out), failed


def render_results(records: dict, order: list[str]) -> str:
    """One record per line, in list order, so two pull requests touching different mutations
    rarely touch the same line (adjacent lines still conflict: keep both records)."""
    ids = [i for i in order if i in records] + sorted(i for i in records if i not in order)
    lines = [f"  {json.dumps(i, ensure_ascii=False)}: {json.dumps(records[i], sort_keys=True, ensure_ascii=False)}"
             for i in ids]
    return "{\n" + ",\n".join(lines) + ("\n" if lines else "") + "}\n"


def write_atomic(path: Path, text: str) -> None:
    """Replace `path` whole, so no reader and no killed run ever leaves half a file."""
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


def dirty(porcelain: str) -> list[str]:
    """The `git status --porcelain` lines that stop a run: any tracked change but the results file."""
    return [line for line in porcelain.splitlines() if line.strip() and not line.endswith(RESULTS_REL)]


def apply_check(mutations: list[dict], crate: Path = ROOT) -> list[str]:
    """Every mutation's `from` text appears exactly once in its file."""
    bad = []
    for m in mutations:
        path = crate / m["file"]
        n = path.read_bytes().count(m["from"].encode("utf-8")) if path.is_file() else 0
        if n != 1:
            bad.append(f"{m['id']}: `from` found {n} times in {m['file']}")
    return bad


# ------------------------------------------------------------------------------------ git
def git(*args: str, repo: Path | None = None) -> str:
    return subprocess.run(["git", *args], cwd=repo or REPO, capture_output=True, text=True, check=True).stdout


def crate_tree(ref: str = "HEAD", repo: Path = REPO, crate: str = CRATE) -> str:
    """A hash of this crate's committed tree at `ref`, without the results file."""
    results = f"{crate}/parity/mutation-results.json"
    rows = [r for r in git("ls-tree", "-r", ref, "--", crate, repo=repo).splitlines() if not r.endswith("\t" + results)]
    return hashlib.sha256("\n".join(rows).encode("utf-8")).hexdigest()[:16]


def base_definitions(base: str, repo: Path = REPO, crate: str = CRATE) -> dict[str, str]:
    """{id: definition hash} of the mutation list at the merge base of `base` and HEAD ({} when
    the base has no list): what a change to the list is judged against."""
    mb = git("merge-base", base, "HEAD", repo=repo).strip()
    shown = subprocess.run(["git", "show", f"{mb}:{crate}/parity/mutations.json"], cwd=repo,
                           capture_output=True)
    if shown.returncode != 0:
        return {}
    return {m["id"]: mutation_hash(m) for m in json.loads(shown.stdout.decode("utf-8"))}


def changed_since(base: str, repo: Path = REPO, crate: str = CRATE) -> list[str]:
    """Crate-relative paths changed between the merge base of `base` and HEAD's committed tree."""
    mb = git("merge-base", base, "HEAD", repo=repo).strip()
    # --no-renames: a renamed file is listed under its old name too, which a killer may name.
    # -z: git would otherwise quote a path with non-ASCII bytes, and the quoted form would not
    # match the crate prefix below, so the change would silently select nothing.
    out = subprocess.run(["git", "diff", "--name-only", "-z", "--no-renames", mb, "HEAD", "--", crate],
                         cwd=repo, capture_output=True, check=True).stdout
    names = [n.decode("utf-8") for n in out.split(b"\0") if n]
    return [n[len(crate) + 1:] for n in names if n.startswith(crate + "/")]


def committed_files(ref: str = "HEAD", repo: Path = REPO, paths: tuple[str, ...] = COPIED) -> dict[str, bytes]:
    """{repo-relative path: committed bytes} for every tracked file under `paths` at `ref`. A
    tracked symlink or submodule refuses the run: the copy would not reproduce it."""
    listing = subprocess.run(["git", "ls-tree", "-r", "-z", ref, "--", *paths], cwd=repo,
                             capture_output=True, check=True).stdout
    entries = []
    for row in filter(None, listing.split(b"\0")):
        meta, path = row.split(b"\t", 1)
        mode, kind, sha = meta.split()
        if kind != b"blob" or mode == b"120000":
            raise SystemExit(f"refusing: {path.decode()} is a {'symlink' if mode == b'120000' else kind.decode()}")
        entries.append((path.decode("utf-8"), sha.decode()))
    out = subprocess.run(["git", "cat-file", "--batch"], cwd=repo, capture_output=True, check=True,
                         input="".join(f"{sha}\n" for _, sha in entries).encode()).stdout
    files, pos = {}, 0
    for path, sha in entries:
        end = out.index(b"\n", pos)
        header = out[pos:end].split()
        if header[0].decode() != sha:
            raise SystemExit(f"refusing: git cat-file answered {header[0].decode()} for {sha}")
        size = int(header[2])
        files[path] = out[end + 1:end + 1 + size]
        pos = end + 1 + size + 1
    return files


# ------------------------------------------------------------------------------------ workers
_STOP = threading.Event()
_ACTIVE: set[subprocess.Popen] = set()
_ACTIVE_LOCK = threading.Lock()


def _stop(_signum, _frame):
    """SIGINT/SIGTERM: mark the run stopped and unwind the main thread. It takes no lock: the main
    thread may hold one when the signal arrives. The unwinding kills the cargo runs in flight."""
    _STOP.set()
    raise KeyboardInterrupt


def _kill_group(proc: subprocess.Popen) -> None:
    """End the process group `proc` started (its own session), and nothing else."""
    try:
        os.killpg(proc.pid, signal.SIGTERM)
        try:
            proc.wait(timeout=KILL_GRACE)
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGKILL)
            proc.wait()
    except ProcessLookupError:
        pass


def _kill_active() -> None:
    with _ACTIVE_LOCK:
        procs = list(_ACTIVE)
    for p in procs:
        _kill_group(p)


class Worker:
    def __init__(self, index: int, root: Path = WORKERS):
        self.dir = root / f"w{index}"
        self.tree = self.dir / "src-tauri"
        self.target = self.dir / "target"
        self.crate = self.tree / ROOT.relative_to(WORKSPACE)

    def sync(self, files: dict[str, bytes]) -> None:
        """Make the copy hold exactly `files` ({repo-relative path: bytes}), re-writing only what
        differs, and delete any other file under the copied src-tauri."""
        wanted = set()
        for rel, data in files.items():
            dst = self.dir / rel
            wanted.add(dst)
            if not dst.is_file() or dst.read_bytes() != data:
                dst.parent.mkdir(parents=True, exist_ok=True)
                dst.write_bytes(data)
        if self.tree.exists():
            for p in self.tree.rglob("*"):
                if p.is_file() and p not in wanted and self.target not in p.parents:
                    p.unlink()

    def run(self, args: list[str], timeout: int) -> tuple[int | None, str]:
        """(exit code, or None on a timeout; merged output) of one `cargo test` in this copy. A stop,
        or any error, ends the process group this call started before it propagates."""
        if _STOP.is_set():
            raise Stopped
        env = dict(os.environ, CARGO_TARGET_DIR=str(self.target), CARGO_TERM_COLOR="never")
        proc = None
        try:
            proc = subprocess.Popen(["cargo", "test", "--locked", "--no-fail-fast", "-p", PACKAGE, *args],
                                    cwd=self.crate, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                    text=True, errors="replace", start_new_session=True)
            with _ACTIVE_LOCK:
                _ACTIVE.add(proc)
            if _STOP.is_set():  # a stop that came between the check above and registering
                raise Stopped
            try:
                out, _ = proc.communicate(timeout=timeout)
                rc: int | None = proc.returncode
            except subprocess.TimeoutExpired:
                _kill_group(proc)
                out, _ = proc.communicate()
                rc = None
            if _STOP.is_set():  # the run ended because the stop killed it: no verdict
                raise Stopped
            return rc, out
        except BaseException:
            if proc is not None:
                _kill_group(proc)
            raise
        finally:
            if proc is not None:
                with _ACTIVE_LOCK:
                    _ACTIVE.discard(proc)


def phase(worker: Worker, targets: list[str], names: list[str], timeout: int, build_timeout: int) -> tuple[str, list[str]]:
    """(verdict, killers) of one build-then-test step over `targets` (every target when empty),
    restricted to the exact test `names` when some are given."""
    rc, out = worker.run(["--no-run", *targets], build_timeout)
    if rc is None:
        return BUILD_TIMEOUT, []
    if rc != 0 or parse_test_output(out)[1]:
        return COMPILE_ERROR, []
    rc, out = worker.run([*targets, "--", "--exact", *names] if names else list(targets), timeout)
    if rc is None:
        return TIMEOUT, []
    failed, compile_error = parse_test_output(out)
    if compile_error:  # a doc test compiles at test time
        return COMPILE_ERROR, []
    if failed:
        return KILLED, failed
    return (SURVIVED if rc == 0 else FAILED_NO_TEST), []


def killer_args(killers: list[str]) -> tuple[list[str], list[str]] | None:
    """(cargo target flags, exact test names) to run exactly these killers, or None when one of
    them cannot be run on its own (a doc test, a crashed binary, an unknown target)."""
    targets, names = set(), set()
    for k in killers:
        target, _, name = k.partition("::")
        if name == CRASHED:
            return None
        if target == "src/lib.rs":
            targets.add("--lib")
        elif target.startswith("tests/") and target.endswith(".rs"):
            targets.add(f"--test={Path(target).stem}")
        else:
            return None
        names.add(name)
    if not targets:
        return None
    return sorted(targets), sorted(names)


def run_one(worker: Worker, m: dict, record: dict | None, full: bool, timeout: int,
            build_timeout: int = DEFAULT_BUILD_TIMEOUT) -> dict:
    path = worker.crate / m["file"]
    original = path.read_bytes()
    frm, to = m["from"].encode("utf-8"), m["to"].encode("utf-8")
    now = _dt.datetime.now(_dt.timezone.utc).isoformat(timespec="seconds")
    base = {"mutation": mutation_hash(m), "time": now}
    if original.count(frm) != 1:
        return {**base, "verdict": DID_NOT_APPLY, "killers": [], "mode": "none",
                "note": f"{original.count(frm)} matches"}
    path.write_bytes(original.replace(frm, to))
    try:
        if not full and proven(record):
            spec = killer_args(record.get("killers", []))
            if spec:
                verdict, failed = phase(worker, *spec, timeout, build_timeout)
                if verdict in PASSING:
                    return {**base, "verdict": verdict, "killers": failed, "mode": "killers-first"}
        verdict, failed = phase(worker, [], [], timeout, build_timeout)
        return {**base, "verdict": verdict, "killers": failed, "mode": "full"}
    finally:
        path.write_bytes(original)


def baseline(workers: list[Worker], timeout: int, build_timeout: int) -> list[str]:
    """Problems with the unmutated copies: a build that fails or hangs, a test that does not pass,
    or a `timeout` under MARGIN times the unmutated suite's measured test time (a slow run would
    then be counted as a hang). Every worker builds; the full suite runs once, on the first."""
    problems = []
    for w in workers:
        rc, out = w.run(["--no-run"], build_timeout)
        if rc is None or rc != 0 or parse_test_output(out)[1]:
            problems.append(f"{w.dir.name}: {BUILD_TIMEOUT if rc is None else COMPILE_ERROR}")
    if problems:
        return problems
    w = workers[0]
    start = time.monotonic()
    rc, out = w.run([], timeout)
    took = time.monotonic() - start
    failed, compile_error = parse_test_output(out)
    if rc is None:
        problems.append(f"{w.dir.name}: the unmutated suite exceeded --timeout {timeout}s")
    elif compile_error:
        problems.append(f"{w.dir.name}: {COMPILE_ERROR}")
    elif failed or rc != 0:
        problems.append(f"{w.dir.name}: fails unmutated: {', '.join(failed) or f'exit {rc}'}")
    elif timeout < MARGIN * took:
        problems.append(f"--timeout {timeout}s is under {MARGIN}x the unmutated suite's {took:.0f}s")
    return problems


def lock_workdir(workdir: Path):
    """An exclusive lock on the worker directory for this run, or None when another run holds it:
    two runs sharing copies would apply and undo each other's mutations."""
    workdir.mkdir(parents=True, exist_ok=True)
    handle = open(workdir / ".lock", "w")
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        handle.close()
        return None
    return handle


# ------------------------------------------------------------------------------------ main
def load_results(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("ids", nargs="*", help="run only these mutation ids")
    ap.add_argument("--changed-since", metavar="BASE", help="select by the change since BASE's merge base")
    ap.add_argument("--verify", action="store_true",
                    help="build nothing: check the selection has killed records on this crate tree")
    ap.add_argument("--nightly-issues", type=Path, metavar="FILE",
                    help="text of the open nightly issues; a crate change must re-prove the ids they list")
    ap.add_argument("--list", action="store_true", help="print the selection and why, run nothing")
    ap.add_argument("--full", action="store_true", help="always run the full suite (no killers first)")
    ap.add_argument("--jobs", type=int, default=1, help="parallel workers, each a copy with its own target")
    ap.add_argument("--shard", metavar="K/N", help="run shard K of N (whole files per shard)")
    ap.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT, help="seconds per test run")
    ap.add_argument("--build-timeout", type=int, default=DEFAULT_BUILD_TIMEOUT, help="seconds per build")
    ap.add_argument("--results", type=Path, help=f"results file to read and update (default {RESULTS_REL})")
    ap.add_argument("--workdir", type=Path, default=WORKERS, help="where the worker copies live")
    ap.add_argument("--merge", type=Path, nargs="+", metavar="FILE", help="merge shard results; run nothing")
    ap.add_argument("--report", type=Path, metavar="MD", help="with --merge: write a Markdown report")
    args = ap.parse_args(argv)

    mutations = json.loads(LIST.read_text(encoding="utf-8"))
    order = [m["id"] for m in mutations]
    repeated = sorted({i for i in order if order.count(i) > 1})
    if repeated:
        print(f"refusing: mutation ids used more than once: {repeated}", file=sys.stderr)
        return 2
    unknown = sorted(set(args.ids) - set(order))
    if unknown:
        print(f"refusing: no such mutation ids: {unknown}", file=sys.stderr)
        return 2

    # Always on: every mutation in the list applies exactly once.
    stale = apply_check(mutations, ROOT)
    accepted = load_results(SURVIVORS)
    for line in stale:
        print(f"STALE {line}")

    if args.merge:
        if args.results is None:
            print("refusing: --merge writes --results; name that file (the committed one is never the default "
                  "here)", file=sys.stderr)
            return 2
        merged: dict = {}
        unreadable = []
        for path in args.merge:
            try:
                merged.update(json.loads(path.read_text(encoding="utf-8")))
            except (OSError, ValueError) as e:
                unreadable.append(f"{path}: {type(e).__name__}")
        text, failed = report(mutations, merged, load_results(RESULTS), unreadable, accepted)
        write_atomic(args.results, render_results(merged, order))
        if args.report:
            write_atomic(args.report, text + "\n")
        print(text)
        return 1 if failed else 0

    results_path = args.results or RESULTS
    results = load_results(results_path)
    head = git("rev-parse", "HEAD").strip()  # read once: every record names this commit's tree
    tree = crate_tree(head, REPO, CRATE)
    reasons: dict[str, list[str]] = {}
    changed: list[str] = []
    if args.changed_since:
        changed = changed_since(args.changed_since, REPO, CRATE)
        reasons = select(mutations, results, changed, ROOT, accepted,
                         base_definitions(args.changed_since, REPO, CRATE))
    if args.nightly_issues and changed:
        required = failing_ids(args.nightly_issues.read_text(encoding="utf-8"))
        if required is None:
            print("refusing: an open mutation-nightly issue has no failing-ids line, so no change can show it "
                  "re-proves what failed; a maintainer closes that issue by hand to let the fix through",
                  file=sys.stderr)
            return 1
        for i in required:
            if i in order:
                reasons.setdefault(i, []).append("failing on the nightly run")
    chosen = [m for m in mutations
              if (not args.ids or m["id"] in args.ids) and (not args.changed_since or m["id"] in reasons)]
    if args.shard:
        k, n = (int(x) for x in args.shard.split("/"))
        chosen = shard(chosen, k, n)

    if args.list or args.verify:
        for m in chosen:
            why = "; ".join(reasons.get(m["id"], ["listed"]))
            ok = fresh(m, results.get(m["id"]), tree, accepted)
            print(f"{m['id']}: {why}" + ("" if ok else "  [NOT PROVEN ON THIS TREE]"))
        problems = verify(chosen, results, tree, order, accepted) + survivor_problems(mutations, accepted)
        unproven = sum(1 for m in chosen if not fresh(m, results.get(m["id"]), tree, accepted))
        for p in problems:
            if "not proven on crate tree" not in p:
                print(p)
        print(f"\n{len(chosen)} selected, {len(chosen) - unproven} proven on crate tree {tree}")
        if args.verify:
            if problems:
                print("Update the branch from master, re-run the selection with "
                      "`python3 parity/mutations.py --changed-since BASE`, delete any record for a retired "
                      f"mutation, then commit {RESULTS_REL}.", file=sys.stderr)
            return 1 if (problems or stale) else 0
        return 0

    blocking = dirty(git("status", "--porcelain", "--untracked-files=no", "--", *COPIED))
    if blocking:
        print("refusing: uncommitted changes to tracked files the copies hold; a verdict must describe a "
              "committed tree:\n" + "\n".join(blocking), file=sys.stderr)
        return 2
    lock = lock_workdir(args.workdir)
    if lock is None:
        print(f"refusing: another run holds {args.workdir}", file=sys.stderr)
        return 2

    files = committed_files(head, REPO)
    jobs = max(1, min(args.jobs, len(chosen) or 1))
    workers = [Worker(i, args.workdir) for i in range(jobs)]
    signal.signal(signal.SIGINT, _stop)
    signal.signal(signal.SIGTERM, _stop)
    try:
        for w in workers:
            w.sync(files)
        problems = baseline(workers, args.timeout, args.build_timeout)
    except (Stopped, KeyboardInterrupt):
        print("stopped before any mutation", file=sys.stderr)
        return 130
    if problems:
        print("refusing: the unmutated copy does not pass; fix that first:\n  " + "\n  ".join(problems),
              file=sys.stderr)
        return 2
    free = list(workers)
    pool_lock = threading.Lock()
    done: dict[str, dict] = {}

    def task(m: dict) -> None:
        if _STOP.is_set():
            return
        with pool_lock:
            w = free.pop()
        try:
            rec = run_one(w, m, results.get(m["id"]), args.full, args.timeout, args.build_timeout)
        except Stopped:
            return
        finally:
            with pool_lock:
                free.append(w)
        rec.update(commit=head, crate_tree=tree)
        with pool_lock:
            done[m["id"]] = rec
            kill = f" by {len(rec['killers'])}: {', '.join(rec['killers'])}" if rec["killers"] else ""
            print(f"{m['id']} {rec['verdict']} ({rec['mode']}){kill}", flush=True)
            results[m["id"]] = rec
            write_atomic(results_path, render_results({i: results[i] for i in order if i in results}, order))

    pool = ThreadPoolExecutor(max_workers=jobs)
    try:
        list(pool.map(task, chosen))
    except KeyboardInterrupt:
        _kill_active()
        pool.shutdown(wait=True, cancel_futures=True)
        print(f"\nstopped after {len(done)} of {len(chosen)}; the results file keeps those", file=sys.stderr)
        return 130
    pool.shutdown(wait=True)

    ran = [done[m["id"]] for m in chosen]
    bad = [(m, done[m["id"]]) for m in chosen if not passes(m, done[m["id"]], accepted)]
    thin = [(m, done[m["id"]]) for m in chosen
            if done[m["id"]]["verdict"] == KILLED and len(done[m["id"]]["killers"]) < 2]
    print(f"\n{len(ran) - len(bad)} of {len(ran)} killed")
    for m, rec in bad:
        print(f"  {m['id']} ({m['by']}: {m['what']}): {rec['verdict']}")
    if thin:
        print(f"\nwarning: {len(thin)} killed by fewer than 2 tests (a lower bound in killers-first mode):")
        for m, rec in thin:
            print(f"  {m['id']} ({rec['mode']}): {', '.join(rec['killers'])}")
    return 1 if (bad or stale) else 0


if __name__ == "__main__":
    raise SystemExit(main())
