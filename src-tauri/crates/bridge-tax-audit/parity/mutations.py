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
    python3 parity/mutations.py --merge shard*.json --report report.md   # nightly, all shards

Run from anywhere with the pinned toolchain exported (RUSTC, RUSTDOC and PATH; see AGENTS.md).

## What a verdict rests on

The runner never edits the working tree. Each worker is a copy of the tracked `src-tauri` files
under `src-tauri/target/mutants/w<N>/`, with its own `CARGO_TARGET_DIR`; a mutation is applied to
the copy and undone there. Only files whose bytes differ are re-copied, so a worker's build cache
stays warm between runs. Stopping a run cannot leave a mutation in the tree.

Killers first: when the results file records the tests that killed a mutation last time, those
tests run first (`--exact`, only their targets). If one fails, the mutation is killed ("killers-first"
mode, and its margin is then a lower bound). If none fails, the FULL suite runs, so a "survived"
verdict always comes from the full suite. `--full` skips the killers-first step (the nightly run).

## The results file

`parity/mutation-results.json` is committed. It holds one record per mutation, one line each:
the verdict, the killing tests qualified by their target (`tests/edge_books.rs::name`,
`src/lib.rs::book::tests::name`), the mode, the commit and time, a hash of the mutation's own
definition, and `crate_tree`, a hash of this crate's committed tree without the results file.

## The guarantee

At merge, `--verify --changed-since <base>` (in CI, no build) requires:
- every mutation in the list applies exactly once to the merged tree; and
- every mutation SELECTED by the change has a "killed" record made on exactly the merged crate
  tree. A mutation is selected when its target file changed, when a file holding one of its
  recorded killing tests changed, when a fixture, golden or rules input its killing tests read
  changed, when its own definition changed, or when it has no killed record at all.

Every other mutation was last proven on an earlier tree; the nightly workflow re-proves the whole
list against the full suite, so such a mutation carries up to about a day of exposure to a change
elsewhere in the crate that weakens its killing tests.
"""
from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent  # the crate
WORKSPACE = ROOT.parent.parent  # src-tauri
REPO = WORKSPACE.parent
CRATE = ROOT.relative_to(REPO).as_posix()  # src-tauri/crates/bridge-tax-audit
PACKAGE = "bridge-tax-audit"
LIST = ROOT / "parity" / "mutations.json"
RESULTS = ROOT / "parity" / "mutation-results.json"
RESULTS_REL = f"{CRATE}/parity/mutation-results.json"
WORKERS = WORKSPACE / "target" / "mutants"
DEFAULT_TIMEOUT = 45 * 60

KILLED = "killed"
SURVIVED = "survived"
DID_NOT_APPLY = "did_not_apply"
COMPILE_ERROR = "compile_error"
TIMEOUT = "timeout"
FAILED_NO_TEST = "failed_no_test"
PASSING = {KILLED, TIMEOUT}  # a timeout is a hang the suite would not let through

# A change to one of these (crate-relative) selects every mutation: the build itself changed.
EVERYTHING = ("Cargo.toml", "build.rs", "rules/")
# Crate-relative prefixes whose change is a data input to tests that read fixtures.
DATA = ("tests/fixtures/",)
# Crate-relative files that never change what a test does.
INERT = ("parity/mutations.py", "parity/mutation-results.json", "parity/test_mutations.py")


# ------------------------------------------------------------------------------------ pure parts
def mutation_hash(m: dict) -> str:
    """A mutation's own identity: its file, `from` and `to`. An edited mutation is unproven."""
    body = json.dumps({"file": m["file"], "from": m["from"], "to": m["to"]}, sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(body.encode("utf-8")).hexdigest()[:16]


_RUNNING = re.compile(r"^\s*Running (?:unittests )?(\S+) \(")
_DOCTESTS = re.compile(r"^\s*Doc-tests (\S+)")
_FAILED = re.compile(r"^test (.+?) \.\.\. FAILED$")
_COMPILE = re.compile(r"^error\[E\d+\]|^error: could not compile ")


def parse_test_output(out: str) -> tuple[list[str], bool]:
    """(failed tests, each qualified by the target cargo reported it under, compile error?), read
    from `cargo test` output with stderr merged into stdout, so each `Running` line precedes its
    binary's results. A doc test is qualified as `doc`."""
    failed: list[str] = []
    target = "?"
    compile_error = False
    for line in out.splitlines():
        line = line.rstrip("\r")
        if m := _RUNNING.match(line):
            target = m.group(1)
        elif m := _DOCTESTS.match(line):
            target = "doc"
        elif m := _FAILED.match(line):
            failed.append(f"{target}::{m.group(1)}")
        elif _COMPILE.match(line):
            compile_error = True
    return sorted(set(failed)), compile_error


def killer_file(killer: str, crate: Path = ROOT) -> str | None:
    """The crate-relative source file that holds a qualified killing test, or None when unknown.
    `tests/x.rs::name` lives in tests/x.rs; `src/lib.rs::a::b::name` in the file of the longest
    module path that exists (src/a/b.rs, src/a/b/mod.rs, then src/a.rs ...), so an inline
    `mod tests` maps to its parent's file and a `tests.rs` file to itself; `doc::src/x.rs - item
    (line N)` in src/x.rs."""
    target, _, name = killer.partition("::")
    if target == "doc":
        path = name.split(" - ", 1)[0].strip()
        return path if path.endswith(".rs") else None
    if target.startswith("tests/"):
        return target
    if target == "src/lib.rs":
        parts = name.split("::")[:-1]  # the module path; the last part is the test fn
        while parts:  # `book::tests` is src/book/tests.rs if that file exists, else src/book.rs
            for candidate in (f"src/{'/'.join(parts)}.rs", f"src/{'/'.join(parts)}/mod.rs"):
                if (crate / candidate).is_file():
                    return candidate
            parts = parts[:-1]
        return "src/lib.rs"
    return None


def reads_fixtures(source_file: str, crate: Path = ROOT) -> bool:
    """Whether a test file reads fixture data: it names `fixtures`, or it is an integration test
    and `tests/common` (which every such binary may include) does."""
    path = crate / source_file
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return True  # unknown: assume it does
    if "fixtures" in text or "golden" in text:
        return True
    if source_file.startswith("tests/") and "mod common" in text:
        common = crate / "tests" / "common"
        return any("fixtures" in p.read_text(encoding="utf-8") for p in common.rglob("*.rs"))
    return False


def select(mutations: list[dict], results: dict, changed: list[str], crate: Path = ROOT) -> dict[str, list[str]]:
    """{id: [reasons]} for every mutation the change selects; see the module docstring. `changed`
    is crate-relative paths."""
    changed = [c for c in changed if not c.startswith(INERT)]
    everything = any(c == e or c.startswith(e) for c in changed for e in EVERYTHING)
    data = any(c.startswith(DATA) for c in changed)
    changed_set = set(changed)
    if any(c.startswith("tests/common/") for c in changed):
        # every integration test binary may include tests/common
        changed_set |= {f"tests/{p.name}" for p in (crate / "tests").glob("*.rs")}
    picked: dict[str, list[str]] = {}
    for m in mutations:
        reasons = []
        rec = results.get(m["id"])
        if everything:
            reasons.append("build input changed")
        if m["file"] in changed_set:
            reasons.append(f"target {m['file']} changed")
        if rec is None or rec.get("verdict") != KILLED:
            reasons.append("no killed record")
        elif rec.get("mutation") != mutation_hash(m):
            reasons.append("definition changed")
        else:
            files = {killer_file(k, crate) for k in rec.get("killers", [])}
            if None in files:
                reasons.append("a killer's file is unknown")
            hit = sorted(f for f in files if f in changed_set)
            if hit:
                reasons.append(f"killer file changed: {', '.join(hit)}")
            if data and any(f is None or reads_fixtures(f, crate) for f in files):
                reasons.append("fixture input changed")
        if reasons:
            picked[m["id"]] = reasons
    return picked


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


def report(mutations: list[dict], merged: dict, committed: dict) -> tuple[str, bool]:
    """(Markdown, failed?) for a whole-list run. It fails when any mutation was not run, did not
    survive the full suite's judgement as killed, or when the committed results file is stale (a
    record for a mutation not in the list, or none, or one made for a different definition)."""
    order = [m["id"] for m in mutations]
    by_id = {m["id"]: m for m in mutations}
    missing = [i for i in order if i not in merged]
    bad = [(i, merged[i]["verdict"]) for i in order if i in merged and merged[i]["verdict"] not in PASSING]
    stale = [f"{i}: no committed record" for i in order if i not in committed]
    stale += [f"{i}: committed record is for another definition" for i in order
              if i in committed and committed[i].get("mutation") != mutation_hash(by_id[i])]
    stale += [f"{i}: committed record for a mutation no longer in the list" for i in committed if i not in by_id]
    thin = [(i, merged[i]["killers"]) for i in order
            if i in merged and merged[i]["verdict"] == KILLED and len(merged[i]["killers"]) < 2]
    failed = bool(missing or bad or stale)
    out = [f"# Tax-audit mutations: {'FAILED' if failed else 'all killed'}", "",
           f"{len(order)} in the list; {len(merged)} run; "
           f"{sum(1 for i in order if merged.get(i, {}).get('verdict') in PASSING)} killed.", ""]
    for title, rows in (("Not killed", [f"`{i}` ({by_id[i]['file']}): {v}" for i, v in bad]),
                        ("Not run", [f"`{i}`" for i in missing]),
                        ("Stale committed records", stale),
                        ("Killed by fewer than 2 tests (warning)", [f"`{i}`: {', '.join(k) or 'none'}" for i, k in thin])):
        if rows:
            out += [f"## {title} ({len(rows)})", ""] + [f"- {r}" for r in rows] + [""]
    return "\n".join(out), failed


def render_results(records: dict, order: list[str]) -> str:
    """One record per line, in list order, so two pull requests touching different mutations
    rarely touch the same line."""
    ids = [i for i in order if i in records] + sorted(i for i in records if i not in order)
    lines = [f"  {json.dumps(i, ensure_ascii=False)}: {json.dumps(records[i], sort_keys=True, ensure_ascii=False)}"
             for i in ids]
    return "{\n" + ",\n".join(lines) + ("\n" if lines else "") + "}\n"


# ------------------------------------------------------------------------------------ git
def git(*args: str) -> str:
    return subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True, check=True).stdout


def crate_tree(ref: str = "HEAD") -> str:
    """A hash of this crate's committed tree at `ref`, without the results file."""
    rows = [r for r in git("ls-tree", "-r", ref, "--", CRATE).splitlines() if not r.endswith("\t" + RESULTS_REL)]
    return hashlib.sha256("\n".join(rows).encode("utf-8")).hexdigest()[:16]


def changed_since(base: str) -> list[str]:
    """Crate-relative paths changed between the merge base of `base` and HEAD's committed tree."""
    mb = git("merge-base", base, "HEAD").strip()
    names = git("diff", "--name-only", mb, "HEAD", "--", CRATE).splitlines()
    return [n[len(CRATE) + 1:] for n in names if n.startswith(CRATE + "/")]


# ------------------------------------------------------------------------------------ workers
class Worker:
    def __init__(self, index: int, root: Path = WORKERS):
        self.dir = root / f"w{index}"
        self.tree = self.dir / "src-tauri"
        self.target = self.dir / "target"
        self.crate = self.tree / ROOT.relative_to(WORKSPACE)

    def sync(self, tracked: list[str]) -> None:
        """Make the copy hold exactly the tracked files given (src-tauri and the root toolchain pin),
        re-copying only what differs, and delete any other file under the copied src-tauri."""
        wanted = set()
        for rel in tracked:  # repo-relative
            src = REPO / rel
            if not src.is_file():
                continue
            dst = self.dir / rel
            wanted.add(dst)
            data = src.read_bytes()
            if not dst.is_file() or dst.read_bytes() != data:
                dst.parent.mkdir(parents=True, exist_ok=True)
                dst.write_bytes(data)
        if self.tree.exists():
            for p in self.tree.rglob("*"):
                if p.is_file() and p not in wanted and self.target not in p.parents:
                    p.unlink()

    def run(self, args: list[str], timeout: int) -> tuple[int | None, str]:
        env = dict(os.environ, CARGO_TARGET_DIR=str(self.target))
        proc = subprocess.Popen(["cargo", "test", "--locked", "--no-fail-fast", "-p", PACKAGE, *args],
                                cwd=self.crate, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, errors="replace", start_new_session=True)
        try:
            out, _ = proc.communicate(timeout=timeout)
            return proc.returncode, out
        except subprocess.TimeoutExpired:
            os.killpg(proc.pid, signal.SIGTERM)  # the group this Popen started, and nothing else
            try:
                out, _ = proc.communicate(timeout=30)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGKILL)
                out, _ = proc.communicate()
            return None, out


def killer_args(killers: list[str]) -> tuple[list[str], list[str]] | None:
    """(cargo target flags, exact test names) to run exactly these killers, or None when one of
    them cannot be run on its own (a doc test, an unknown target)."""
    targets, names = set(), set()
    for k in killers:
        target, _, name = k.partition("::")
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


def run_one(worker: Worker, m: dict, record: dict | None, full: bool, timeout: int) -> dict:
    path = worker.crate / m["file"]
    original = path.read_text(encoding="utf-8")
    now = _dt.datetime.now(_dt.timezone.utc).isoformat(timespec="seconds")
    base = {"mutation": mutation_hash(m), "time": now}
    if original.count(m["from"]) != 1:
        return {**base, "verdict": DID_NOT_APPLY, "killers": [], "mode": "none",
                "note": f"{original.count(m['from'])} matches"}
    path.write_text(original.replace(m["from"], m["to"]), encoding="utf-8")
    try:
        if not full and record and record.get("verdict") == KILLED:
            spec = killer_args(record.get("killers", []))
            if spec:
                targets, names = spec
                rc, out = worker.run([*targets, "--", "--exact", *names], timeout)
                failed, compile_error = parse_test_output(out)
                if rc is None:
                    return {**base, "verdict": TIMEOUT, "killers": [], "mode": "killers-first"}
                if compile_error:
                    return {**base, "verdict": COMPILE_ERROR, "killers": [], "mode": "killers-first"}
                if failed:
                    return {**base, "verdict": KILLED, "killers": failed, "mode": "killers-first"}
        rc, out = worker.run([], timeout)
        failed, compile_error = parse_test_output(out)
        if rc is None:
            verdict = TIMEOUT
        elif compile_error:
            verdict = COMPILE_ERROR
        elif failed:
            verdict = KILLED
        else:
            verdict = SURVIVED if rc == 0 else FAILED_NO_TEST
        return {**base, "verdict": verdict, "killers": failed, "mode": "full"}
    finally:
        path.write_text(original, encoding="utf-8")


# ------------------------------------------------------------------------------------ main
def load_results(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else {}


def apply_check(mutations: list[dict]) -> list[str]:
    """Every mutation's `from` text appears exactly once in the working tree's file."""
    bad = []
    for m in mutations:
        path = ROOT / m["file"]
        n = path.read_text(encoding="utf-8").count(m["from"]) if path.is_file() else 0
        if n != 1:
            bad.append(f"{m['id']}: `from` found {n} times in {m['file']}")
    return bad


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("ids", nargs="*", help="run only these mutation ids")
    ap.add_argument("--changed-since", metavar="BASE", help="select by the change since BASE's merge base")
    ap.add_argument("--verify", action="store_true",
                    help="build nothing: check the selection has killed records on this crate tree")
    ap.add_argument("--list", action="store_true", help="print the selection and why, run nothing")
    ap.add_argument("--full", action="store_true", help="always run the full suite (no killers first)")
    ap.add_argument("--jobs", type=int, default=1, help="parallel workers, each a copy with its own target")
    ap.add_argument("--shard", metavar="K/N", help="run shard K of N (whole files per shard)")
    ap.add_argument("--timeout", type=int, default=DEFAULT_TIMEOUT, help="seconds per cargo test run")
    ap.add_argument("--results", type=Path, default=RESULTS, help="results file to read and update")
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
    stale = apply_check(mutations)
    for line in stale:
        print(f"STALE {line}")

    if args.merge:
        merged: dict = {}
        for path in args.merge:
            merged.update(load_results(path))
        text, failed = report(mutations, merged, load_results(RESULTS))
        args.results.write_text(render_results(merged, order), encoding="utf-8")
        if args.report:
            args.report.write_text(text + "\n", encoding="utf-8")
        print(text)
        return 1 if failed else 0

    results = load_results(args.results)
    tree = crate_tree("HEAD")
    reasons: dict[str, list[str]] = {}
    if args.changed_since:
        reasons = select(mutations, results, changed_since(args.changed_since))
    chosen = [m for m in mutations
              if (not args.ids or m["id"] in args.ids) and (not args.changed_since or m["id"] in reasons)]
    if args.shard:
        k, n = (int(x) for x in args.shard.split("/"))
        chosen = shard(chosen, k, n)

    if args.list or args.verify:
        failures = []
        for m in chosen:
            why = "; ".join(reasons.get(m["id"], ["listed"]))
            rec = results.get(m["id"])
            fresh = (rec is not None and rec.get("verdict") == KILLED and rec.get("crate_tree") == tree
                     and rec.get("mutation") == mutation_hash(m))
            print(f"{m['id']}: {why}" + ("" if fresh else "  [NOT PROVEN ON THIS TREE]"))
            if not fresh:
                failures.append(m["id"])
        print(f"\n{len(chosen)} selected, {len(chosen) - len(failures)} proven on crate tree {tree}")
        if args.verify:
            if failures:
                print("Re-run them: python3 parity/mutations.py --changed-since BASE, then commit "
                      f"{RESULTS_REL}.", file=sys.stderr)
            return 1 if (failures or stale) else 0
        return 0

    dirty = [line for line in git("status", "--porcelain", "--untracked-files=no", "--", "src-tauri").splitlines()
             if not line.endswith(RESULTS_REL)]
    if dirty:
        print("refusing: uncommitted changes to tracked src-tauri files; a verdict must describe a "
              "committed tree:\n" + "\n".join(dirty), file=sys.stderr)
        return 2

    # The root toolchain pin too, so cargo in a copy picks the same toolchain it would here.
    tracked = git("ls-files", "--", "src-tauri", "rust-toolchain.toml").splitlines()
    head = git("rev-parse", "HEAD").strip()
    jobs = max(1, min(args.jobs, len(chosen) or 1))
    workers = [Worker(i, args.workdir) for i in range(jobs)]
    for w in workers:
        w.sync(tracked)
    free = list(workers)
    lock = threading.Lock()
    done: dict[str, dict] = {}

    def task(m: dict) -> None:
        with lock:
            w = free.pop()
        try:
            rec = run_one(w, m, results.get(m["id"]), args.full, args.timeout)
        finally:
            with lock:
                free.append(w)
        rec.update(commit=head, crate_tree=tree)
        with lock:
            done[m["id"]] = rec
            kill = f" by {len(rec['killers'])}: {', '.join(rec['killers'])}" if rec["killers"] else ""
            print(f"{m['id']} {rec['verdict']} ({rec['mode']}){kill}", flush=True)
            results[m["id"]] = rec
            args.results.write_text(render_results({i: results[i] for i in order if i in results}, order),
                                    encoding="utf-8")

    with ThreadPoolExecutor(max_workers=jobs) as pool:
        list(pool.map(task, chosen))

    ran = [done[m["id"]] for m in chosen]
    bad = [(m, done[m["id"]]) for m in chosen if done[m["id"]]["verdict"] not in PASSING]
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
