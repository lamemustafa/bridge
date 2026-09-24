# SPDX-License-Identifier: Apache-2.0
"""Tests of mutations.py: output parsing, killer-to-file mapping, selection, verdicts (with a fake
worker in place of cargo), the merge check, the nightly report and issue loop, the worker copy and
the results file. No cargo run; a throwaway git repository where git is needed.

    python3 -m unittest discover -s src-tauri/crates/bridge-tax-audit/parity -p 'test_mutations.py'
"""
from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import mutations as mu  # noqa: E402

# Output shaped as `cargo test` prints it with stderr merged: a status line per target, then that
# binary's own results. "error: test failed" and "error: 2 targets failed" are not compile errors.
OUTPUT = """\
   Compiling bridge-tax-audit v0.1.0 (/x/src-tauri/crates/bridge-tax-audit)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 9.12s
     Running unittests src/lib.rs (/x/target/debug/deps/bridge_tax_audit-0a1b)

running 3 tests
test book::tests::a_voucher_reference_is_read ... FAILED
test support::tests::ok ... ok
test tests::root_level ... FAILED

failures:

---- book::tests::a_voucher_reference_is_read stdout ----
thread panicked
     Running tests/edge_books.rs (/x/target/debug/deps/edge_books-9f)

running 2 tests
test every_edge_book_matches_the_reference ... FAILED
test other ... ok
error: test failed, to rerun pass `-p bridge-tax-audit --test edge_books`

Caused by:
  process didn't exit successfully: `/x/target/debug/deps/edge_books-9f` (exit status: 101)
     Running tests/registry.rs (/x/target/debug/deps/registry-11)

running 1 test
test registry_ok ... ok
   Doc-tests bridge_tax_audit

running 1 test
test src/book.rs - book::Voucher (line 12) ... FAILED
error: 2 targets failed:
"""

# A binary that overflowed its stack: libtest names no failed test; cargo reports the signal.
CRASH = """\
     Running tests/deep.rs (/x/target/debug/deps/deep-1)

running 1 test

thread 'recurse' has overflowed its stack
fatal runtime error: stack overflow
error: test failed, to rerun pass `-p bridge-tax-audit --test deep`

Caused by:
  process didn't exit successfully: `/x/target/debug/deps/deep-1` (signal: 6, SIGABRT: process abort signal)
     Running tests/fine.rs (/x/target/debug/deps/fine-2)

running 1 test
test fine ... ok
"""


def crate_layout(root: Path) -> Path:
    for rel, text in {
        "src/lib.rs": "pub mod book;\n#[cfg(test)]\nmod tests { #[test] fn root_level() {} }\n",
        "src/book.rs": "// book\n#[test] fn a() {}\n#[test] fn b() {}\n",
        "src/support.rs": "#[test] fn tables() { let _ = include_str!(\"text_tables.rs\"); }\n",
        "src/text_tables.rs": "// generated\n",
        "src/read.rs": "#[cfg(test)]\nmod tests;\n",
        "src/read/tests.rs": "#[test] fn a() {}\n",
        "tests/deep.rs": "#[test] fn recurse() {}\n",
        "src/rules.rs": "include_str!(\"../rules/x.toml\")\n",
        "tests/edge_books.rs": "mod common;\n#[test] fn every_edge_book_matches_the_reference() {}\n",
        "tests/registry.rs": "#[test] fn registry_ok() {}\n",
        "tests/common/mod.rs": "pub fn fixtures() -> &'static str { \"tests/fixtures\" }\n",
        "tests/fixtures/golden/x.json": "{}\n",
    }.items():
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")
    return root


def mutation(i: str, file: str) -> dict:
    return {"id": i, "by": "author", "what": i, "file": file, "from": f"from-{i}", "to": f"to-{i}"}


def issue(body: str, number: int = 612) -> str:
    """One open issue as ci.yml's step writes it into the --nightly-issues file."""
    return f"<!-- {mu.ISSUE_MARK} {number} -->\n{body}\n"


def record(m: dict, killers: list[str], verdict: str = mu.KILLED, tree: str = "t") -> dict:
    return {"verdict": verdict, "killers": killers, "mutation": mu.mutation_hash(m), "mode": "full",
            "crate_tree": tree, "commit": "c", "time": "now"}


class ParseOutput(unittest.TestCase):
    def test_failures_are_qualified_by_their_target(self):
        failed, compile_error = mu.parse_test_output(OUTPUT)
        self.assertEqual(failed, sorted([
            "src/lib.rs::book::tests::a_voucher_reference_is_read",
            "src/lib.rs::tests::root_level",
            "tests/edge_books.rs::every_edge_book_matches_the_reference",
            "doc::src/book.rs - book::Voucher (line 12)",
        ]), "a binary that named its failures is not also recorded as crashed")
        self.assertFalse(compile_error, "cargo's 'error: test failed' is not a compile error")

    def test_a_binary_that_crashes_without_naming_a_failure_is_killed_by_its_target(self):
        self.assertEqual(mu.parse_test_output(CRASH), (["tests/deep.rs::<crashed>"], False))
        for signal in ("signal: 9, SIGKILL: kill", "signal: 15, SIGTERM: termination signal", "exit status: 101"):
            killed = CRASH.replace("signal: 6, SIGABRT: process abort signal", signal)
            self.assertEqual(mu.parse_test_output(killed), ([], False), f"killed from outside: {signal}")

    def test_a_long_list_is_not_cut(self):
        out = "     Running tests/x.rs (/t)\n" + "".join(f"test t{i:03} ... FAILED\n" for i in range(300))
        failed, _ = mu.parse_test_output(out)
        self.assertEqual(len(failed), 300)

    def test_compile_errors_are_seen(self):
        for line in ("error[E0308]: mismatched types", "error: could not compile `bridge-tax-audit` (lib)"):
            self.assertTrue(mu.parse_test_output(f"   Compiling x\n{line}\n")[1], line)


class KillerFile(unittest.TestCase):
    def test_each_kind_maps_to_the_file_holding_the_test(self):
        with tempfile.TemporaryDirectory() as t:
            crate = crate_layout(Path(t))
            cases = {
                "tests/edge_books.rs::every_edge_book_matches_the_reference": "tests/edge_books.rs",
                "src/lib.rs::book::tests::a": "src/book.rs",
                "src/lib.rs::book::inner::tests::a": "src/book.rs",  # nearest existing module file
                "src/lib.rs::read::tests::a": "src/read/tests.rs",  # tests in their own file
                "src/lib.rs::tests::root_level": "src/lib.rs",
                "doc::src/book.rs - book::Voucher (line 12)": "src/book.rs",
                "tests/deep.rs::<crashed>": "tests/deep.rs",
                "src/lib.rs::<crashed>": None,  # any test in the library binary
                "weird::x": None,
                # the file must still define the test: a rename, a #[path] remap or a macro is unknown
                "src/lib.rs::book::tests::gone": None,
                "src/lib.rs::tests::gone": None,
                "tests/renamed.rs::every_edge_book_matches_the_reference": None,
                "tests/registry.rs::gone": None,
                "tests/renamed.rs::<crashed>": None,
                "doc::src/renamed.rs - x (line 1)": None,
                # cargo prints a doc test's path relative to the workspace
                f"doc::{crate.relative_to(crate.parent.parent).as_posix()}/src/book.rs - book::V (line 3)": "src/book.rs",
            }
            for killer, want in cases.items():
                self.assertEqual(mu.killer_file(killer, crate), want, killer)
            self.assertEqual(mu.included("src/support.rs", crate), {"src/text_tables.rs"})
            self.assertEqual(mu.included("src/rules.rs", crate), {"rules/x.toml"})

    def test_killers_first_arguments(self):
        self.assertEqual(
            mu.killer_args(["src/lib.rs::book::tests::a", "tests/edge_books.rs::every"]),
            (["--lib", "--test=edge_books"], ["book::tests::a", "every"]))
        self.assertIsNone(mu.killer_args(["doc::src/book.rs - x (line 1)"]), "a doc test is not run alone")
        self.assertIsNone(mu.killer_args(["tests/deep.rs::<crashed>"]), "a crash names no test to run")
        self.assertIsNone(mu.killer_args([]))


class Select(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.TemporaryDirectory()
        self.crate = crate_layout(Path(self.t.name))
        self.book = mutation("B1", "src/book.rs")
        self.support = mutation("S1", "src/support.rs")
        self.edge = mutation("E1", "src/support.rs")
        self.muts = [self.book, self.support, self.edge]
        self.results = {
            "B1": record(self.book, ["src/lib.rs::book::tests::a"]),
            # killed by a test that lives in src/book.rs, not in its own target file
            "S1": record(self.support, ["src/lib.rs::book::tests::b"]),
            "E1": record(self.edge, ["tests/edge_books.rs::every_edge_book_matches_the_reference"]),
        }

    def tearDown(self):
        self.t.cleanup()

    def picked(self, changed, results=None):
        return set(mu.select(self.muts, self.results if results is None else results, changed, self.crate))

    def test_nothing_changed_selects_nothing_proven(self):
        self.assertEqual(self.picked([]), set())

    def test_a_target_file_change_selects_its_mutations(self):
        self.assertEqual(self.picked(["src/support.rs"]), {"S1", "E1"})

    def test_a_killers_file_change_selects_the_mutation(self):
        self.assertEqual(self.picked(["tests/edge_books.rs"]), {"E1"})
        self.assertEqual(self.picked(["src/book.rs"]), {"B1", "S1"})

    def test_a_file_a_killers_file_includes_selects_the_mutation(self):
        self.results["B1"] = record(self.book, ["src/lib.rs::support::tests::tables"])
        self.assertEqual(self.picked(["src/text_tables.rs"]), {"B1"})

    def test_tests_common_selects_every_integration_killer(self):
        self.assertEqual(self.picked(["tests/common/mod.rs"]), {"E1"})

    def test_any_non_source_change_selects_everything(self):
        for changed in ("tests/fixtures/golden/x.json", "tests/fixtures/PROVENANCE.md", "parity/python_golden.py",
                        "rules/x.toml", "Cargo.toml", "build.rs", "tests/fixtures/helper.rs"):
            self.assertEqual(self.picked([changed]), {"B1", "S1", "E1"}, changed)

    def test_unproven_edited_or_unmappable_mutations_are_always_selected(self):
        results = dict(self.results)
        del results["B1"]
        results["S1"] = record(self.support, ["src/lib.rs::book::tests::b"], verdict=mu.SURVIVED)
        edited = dict(self.edge, to="something else")
        self.muts[2] = edited
        self.assertEqual(self.picked([], results), {"B1", "S1", "E1"})
        self.muts[2] = self.edge
        crashed = {"B1": record(self.book, ["src/lib.rs::<crashed>"]), "S1": self.results["S1"],
                   "E1": record(self.edge, [], verdict=mu.TIMEOUT)}
        self.assertEqual(self.picked([], crashed), {"B1"}, "a timeout record proves; an unknown killer file does not")

    def test_the_runner_and_its_results_are_inert(self):
        self.assertEqual(self.picked(list(mu.INERT)), set())

    def test_a_change_to_the_list_selects_only_the_entries_it_changes(self):
        changed = ["parity/mutations.json"]
        added = mutation("N1", "src/book.rs")
        self.muts.append(added)
        self.assertEqual(self.picked(changed), {"N1"}, "an added entry has no record")
        self.muts[0] = dict(self.book, to="edited")
        self.assertEqual(self.picked(changed), {"N1", "B1"}, "an edited entry fails its hash")
        order = [m["id"] for m in self.muts if m["id"] != "S1"]  # S1 removed from the list
        self.assertEqual(mu.verify([], self.results, "t", order),
                         ["S1: a record for a mutation no longer in the list (delete it)"])

    def test_no_crate_source_reads_the_inert_files(self):
        # INERT is sound only while no test can read these files; name one in a crate source and
        # this fails, so the file must leave INERT (or the test must not read it).
        crate = Path(__file__).resolve().parents[1]
        names = [Path(f).name for f in mu.INERT] + ["accepted-survivors.json"]
        sources = [p for d in ("src", "tests") for p in (crate / d).rglob("*.rs")]
        sources += [crate / "build.rs"] if (crate / "build.rs").is_file() else []
        self.assertGreater(len(sources), 20, "the crate's sources were found")
        hits = [f"{p.relative_to(crate)}: {n}" for p in sources for n in names
                if n in p.read_text(encoding="utf-8")]
        self.assertEqual(hits, [])


class Verify(unittest.TestCase):
    def test_a_record_is_fresh_only_on_its_tree_for_its_definition(self):
        m = mutation("A", "src/a.rs")
        self.assertTrue(mu.fresh(m, record(m, ["tests/x.rs::t"], tree="T"), "T"))
        self.assertTrue(mu.fresh(m, record(m, [], verdict=mu.TIMEOUT, tree="T"), "T"), "a hang is killed here too")
        self.assertFalse(mu.fresh(m, record(m, ["tests/x.rs::t"], tree="T"), "U"), "another tree")
        self.assertFalse(mu.fresh(dict(m, to="edited"), record(m, ["tests/x.rs::t"], tree="T"), "T"))
        for verdict in (mu.SURVIVED, mu.COMPILE_ERROR, mu.BUILD_TIMEOUT, mu.FAILED_NO_TEST, mu.DID_NOT_APPLY):
            self.assertFalse(mu.fresh(m, record(m, [], verdict=verdict, tree="T"), "T"), verdict)
        self.assertFalse(mu.fresh(m, None, "T"))

    def test_a_record_for_a_retired_mutation_stops_the_merge(self):
        a, gone = mutation("A", "src/a.rs"), mutation("GONE", "src/a.rs")
        results = {"A": record(a, ["tests/x.rs::t"], tree="T"), "GONE": record(gone, [], tree="T")}
        self.assertEqual(mu.verify([a], results, "T", ["A", "GONE"]), [])
        self.assertEqual(mu.verify([a], results, "T", ["A"]),
                         ["GONE: a record for a mutation no longer in the list (delete it)"])

    def test_an_accepted_survivor_passes_only_for_its_listed_definition(self):
        m, other = mutation("A", "src/a.rs"), mutation("B", "src/a.rs")
        accepted = {"A": {"mutation": mu.mutation_hash(m), "reason": "equivalent", "documented": "#1"}}
        survived = record(m, [], verdict=mu.SURVIVED, tree="T")
        self.assertTrue(mu.passes(m, survived, accepted))
        self.assertTrue(mu.fresh(m, survived, "T", accepted))
        self.assertFalse(mu.fresh(m, survived, "U", accepted), "still bound to its tree")
        self.assertFalse(mu.passes(other, record(other, [], verdict=mu.SURVIVED), accepted), "not listed")
        edited = dict(m, to="edited")  # the entry's hash is the old definition's
        self.assertFalse(mu.passes(edited, record(edited, [], verdict=mu.SURVIVED), accepted), "edited since listed")
        for verdict in (mu.COMPILE_ERROR, mu.FAILED_NO_TEST, mu.BUILD_TIMEOUT):
            self.assertFalse(mu.passes(m, record(m, [], verdict=verdict), accepted), verdict)
        self.assertEqual(mu.select([m], {"A": survived}, [], Path("."), accepted), {}, "not re-selected by itself")
        self.assertFalse(mu.passes(m, survived, {"A": {"reason": "no hash"}}), "an entry must name a hash")
        self.assertEqual(mu.survivor_problems([m, other], accepted), [])
        self.assertEqual(mu.survivor_problems([edited], accepted), ["A: accepted as a survivor for another definition"])
        self.assertEqual(mu.survivor_problems([other], accepted), ["A: accepted as a survivor but no longer in the list"])
        committed = {"A": survived, "B": record(other, ["tests/x.rs::t"])}
        text, failed = mu.report([m, other], committed, committed, accepted=accepted)
        self.assertFalse(failed, text)
        self.assertIn("## Accepted survivors (1)", text)
        text, failed = mu.report([m, other], dict(committed, A=record(m, ["tests/x.rs::t"])), committed, accepted=accepted)
        self.assertFalse(failed, text)
        self.assertIn("now killed: remove the entry (warning) (1)", text)
        text, failed = mu.report([edited, other], {"A": record(edited, [], verdict=mu.SURVIVED), "B": committed["B"]},
                                 {"A": record(edited, []), "B": committed["B"]}, accepted=accepted)
        self.assertTrue(failed)
        self.assertEqual(mu.failing_ids(issue(text)), ["A"])
        self.assertIn("Stale accepted-survivor entries (1)", text)
        killed = {"A": record(m, ["tests/x.rs::t"]), "B": committed["B"]}
        text, failed = mu.report([m, other], killed, killed, accepted={"GONE": {"mutation": "x"}})
        self.assertTrue(failed, "a stale entry alone fails the nightly")

    def test_apply_check_wants_exactly_one_match(self):
        with tempfile.TemporaryDirectory() as t:
            crate = Path(t)
            (crate / "a.rs").write_bytes("x = 1;\ny = ₹2;\ny = ₹2;\n".encode())
            muts = [dict(mutation("ONE", "a.rs"), **{"from": "x = 1;"}),
                    dict(mutation("TWO", "a.rs"), **{"from": "y = ₹2;"}),
                    dict(mutation("NONE", "a.rs"), **{"from": "z"}),
                    dict(mutation("NOFILE", "b.rs"), **{"from": "x"})]
            self.assertEqual(mu.apply_check(muts, crate), [
                "TWO: `from` found 2 times in a.rs", "NONE: `from` found 0 times in a.rs",
                "NOFILE: `from` found 0 times in b.rs"])


class FakeWorker:
    """Stands in for cargo: answers each run with the next scripted (exit code, output), and keeps
    the arguments and the target file's bytes as each run saw them."""

    def __init__(self, crate: Path, answers: list, file: str = "src/a.rs"):
        self.crate, self.answers, self.file = crate, list(answers), file
        self.calls: list[list[str]] = []
        self.seen: list[bytes] = []
        self.dir = crate

    def run(self, args, timeout):
        self.calls.append(args)
        self.seen.append((self.crate / self.file).read_bytes())
        answer = self.answers.pop(0)
        if isinstance(answer, Exception):
            raise answer
        if len(answer) == 3:  # (exit code, output, seconds the run takes)
            time.sleep(answer[2])
        return answer[:2]


BUILT = (0, "    Finished `test` profile\n")
PASSED = (0, "     Running tests/x.rs (/t)\ntest t ... ok\n")
FAILED = (101, "     Running tests/x.rs (/t)\ntest t ... FAILED\ntest u ... FAILED\n")
BROKEN = (101, "error[E0308]: mismatched types\n")
HUNG = (None, "")


class RunOne(unittest.TestCase):
    def setUp(self):
        self.t = tempfile.TemporaryDirectory()
        self.crate = Path(self.t.name)
        (self.crate / "src").mkdir()
        self.original = b"let a = 1;\r\nlet b = 2;\n"  # a CR byte must survive the round trip
        (self.crate / "src/a.rs").write_bytes(self.original)
        self.m = dict(mutation("A", "src/a.rs"), **{"from": "let a = 1;", "to": "let a = 0;"})

    def tearDown(self):
        self.t.cleanup()

    def run_one(self, answers, record=None, full=False):
        w = FakeWorker(self.crate, answers)
        rec = mu.run_one(w, self.m, record, full, timeout=5, build_timeout=5)
        self.assertEqual((self.crate / "src/a.rs").read_bytes(), self.original, "the copy is restored")
        return w, rec

    def test_each_verdict_of_the_full_suite(self):
        cases = [([BUILT, PASSED], mu.SURVIVED, []),
                 ([BUILT, FAILED], mu.KILLED, ["tests/x.rs::t", "tests/x.rs::u"]),
                 ([BUILT, HUNG], mu.TIMEOUT, []),
                 ([BUILT, (101, "")], mu.FAILED_NO_TEST, []),
                 ([BUILT, (101, "error: could not compile `x` (doctest)\n")], mu.COMPILE_ERROR, []),
                 ([BROKEN], mu.COMPILE_ERROR, []),
                 ([(101, "")], mu.COMPILE_ERROR, []),
                 ([HUNG], mu.BUILD_TIMEOUT, [])]
        for answers, verdict, killers in cases:
            w, rec = self.run_one(answers)
            self.assertEqual((rec["verdict"], rec["killers"], rec["mode"]), (verdict, killers, "full"), answers)
            self.assertEqual(w.calls[0], ["--no-run"], "the build is its own step")
            self.assertTrue(all(b"let a = 0;\r\n" in seen for seen in w.seen), "every run saw the mutation")
        self.assertIn(mu.TIMEOUT, mu.PASSING)
        self.assertNotIn(mu.BUILD_TIMEOUT, mu.PASSING)

    def test_killers_run_first_and_the_full_suite_decides_otherwise(self):
        prior = record(self.m, ["tests/x.rs::t", "src/lib.rs::a::tests::b"])
        w, rec = self.run_one([BUILT, FAILED], prior)
        self.assertEqual((rec["verdict"], rec["mode"]), (mu.KILLED, "killers-first"))
        self.assertEqual(w.calls, [["--no-run", "--lib", "--test=x"],
                                   ["--lib", "--test=x", "--", "--exact", "a::tests::b", "t"]])
        w, rec = self.run_one([BUILT, HUNG], prior)
        self.assertEqual((rec["verdict"], rec["mode"]), (mu.TIMEOUT, "killers-first"))
        for killers_answer in ([BUILT, PASSED], [BROKEN], [BUILT, (101, "")]):
            w, rec = self.run_one([*killers_answer, BUILT, PASSED], prior)
            self.assertEqual((rec["verdict"], rec["mode"]), (mu.SURVIVED, "full"), killers_answer)
            self.assertEqual(w.calls[-1], [], "a survivor comes from the full suite")
        w, rec = self.run_one([BUILT, FAILED], prior, full=True)
        self.assertEqual((rec["mode"], len(w.calls)), ("full", 2), "--full skips the killers")
        w, rec = self.run_one([BUILT, FAILED], record(self.m, ["tests/x.rs::t"], verdict=mu.SURVIVED))
        self.assertEqual(rec["mode"], "full", "only a killed record has killers to try")

    def test_a_stopped_run_restores_the_file_and_a_stale_mutation_does_not_apply(self):
        with self.assertRaises(mu.Stopped):
            mu.run_one(FakeWorker(self.crate, [BUILT, mu.Stopped()]), self.m, None, False, 5, 5)
        self.assertEqual((self.crate / "src/a.rs").read_bytes(), self.original, "restored on the way out")
        for body, n in ((self.original + b"let a = 1;\n", 2), (b"nothing\n", 0)):
            (self.crate / "src/a.rs").write_bytes(body)
            self.original = body
            w, rec = self.run_one([])
            self.assertEqual((rec["verdict"], rec["note"], w.calls), (mu.DID_NOT_APPLY, f"{n} matches", []))

    def test_the_baseline_refuses_an_unmutated_suite_that_fails_or_leaves_no_margin(self):
        name = self.crate.name
        ok = [FakeWorker(self.crate, [BUILT, PASSED]), FakeWorker(self.crate, [BUILT])]
        self.assertEqual(mu.baseline(ok, 5, 5), [])
        self.assertEqual(ok[1].calls, [["--no-run"]], "other workers only build")
        broken = [FakeWorker(self.crate, [BUILT, PASSED]), FakeWorker(self.crate, [BROKEN])]
        self.assertEqual(mu.baseline(broken, 5, 5), [f"{name}: compile_error"])
        self.assertEqual(broken[0].calls, [["--no-run"]], "no suite runs until every copy builds")
        self.assertEqual(mu.baseline([FakeWorker(self.crate, [BUILT, FAILED])], 5, 5),
                         [f"{name}: fails unmutated: tests/x.rs::t, tests/x.rs::u"])
        self.assertEqual(mu.baseline([FakeWorker(self.crate, [BUILT, (101, "")])], 5, 5), [f"{name}: fails unmutated: exit 101"])
        self.assertEqual(mu.baseline([FakeWorker(self.crate, [BUILT, HUNG])], 5, 5),
                         [f"{name}: the unmutated suite exceeded --timeout 5s"])
        slow = [FakeWorker(self.crate, [BUILT, (*PASSED, 0.3)])]
        self.assertEqual(mu.baseline(slow, 0.5, 5), ["--timeout 0.5s is under 3x the unmutated suite's 0s"])
        self.assertEqual(mu.baseline([FakeWorker(self.crate, [BUILT, (*PASSED, 0.1)])], 5, 5), [])


class Shard(unittest.TestCase):
    def test_shards_partition_the_list_by_whole_files(self):
        muts = [mutation(f"M{i}", f"src/f{i % 5}.rs") for i in range(23)]
        seen = []
        for k in range(1, 4):
            part = mu.shard(muts, k, 3)
            files = {m["file"] for m in part}
            for other in range(1, 4):
                if other != k:
                    self.assertFalse(files & {m["file"] for m in mu.shard(muts, other, 3)})
            seen += [m["id"] for m in part]
        self.assertEqual(sorted(seen), sorted(m["id"] for m in muts))


class Report(unittest.TestCase):
    def setUp(self):
        self.muts = [mutation("A", "src/a.rs"), mutation("B", "src/b.rs"), mutation("C", "src/c.rs")]
        self.committed = {m["id"]: record(m, ["tests/x.rs::t", "tests/x.rs::u"]) for m in self.muts}
        self.killed = {m["id"]: record(m, ["tests/x.rs::t", "tests/x.rs::u"]) for m in self.muts}

    def test_all_killed_with_fresh_committed_records_passes(self):
        text, failed = mu.report(self.muts, self.killed, self.committed)
        self.assertFalse(failed, text)
        self.assertEqual(mu.failing_ids(issue(text)), None, "a passing report names no failing ids")

    def test_each_failure_kind_fails_the_run_and_names_its_ids(self):
        merged = {"A": record(self.muts[0], [], verdict=mu.SURVIVED), "C": self.killed["C"]}  # B not run
        text, failed = mu.report(self.muts, merged, self.committed)
        self.assertTrue(failed)
        self.assertIn("Not killed (1)", text)
        self.assertIn("Not run (1)", text)
        self.assertEqual(mu.failing_ids(issue(text)), ["A", "B"])
        stale = dict(self.committed, A=record(dict(self.muts[0], to="edited"), ["tests/x.rs::t"]))
        stale["gone"] = record(mutation("gone", "src/z.rs"), [])
        del stale["B"]
        text, failed = mu.report(self.muts, self.killed, stale)
        self.assertTrue(failed)
        self.assertIn("Stale committed records (3)", text)
        self.assertEqual(mu.failing_ids(issue(text)), ["A", "B"])
        # An unreadable shard leaves what it held unknown: every id must be re-proved. (An earlier
        # version pinned an EMPTY line here, which let every crate change through.)
        text, failed = mu.report(self.muts, self.killed, self.committed, ["shard-3.json: JSONDecodeError"])
        self.assertTrue(failed)
        self.assertIn("Shard results not read (1)", text)
        self.assertEqual(mu.failing_ids(issue(text)), ["A", "B", "C"])
        self.assertIn(mu.FAILING_MARK, text.splitlines()[2], "the line comes first, before any capped section")
        many = [mutation(f"M{i:03}", "src/a.rs") for i in range(mu.REPORT_ROWS + 50)]
        text, failed = mu.report(many, {}, {})
        self.assertIn("- `M099`", text)
        self.assertNotIn("- `M100`", text)
        self.assertIn("... and 50 more", text)
        self.assertEqual(len(mu.failing_ids(issue(text))), mu.REPORT_ROWS + 50, "the line is never capped")

    def test_warnings_do_not_fail_the_run(self):
        merged = dict(self.killed, A=record(self.muts[0], ["tests/x.rs::t"]),
                      B=record(self.muts[1], [], verdict=mu.TIMEOUT))
        text, failed = mu.report(self.muts, merged, self.committed)
        self.assertFalse(failed, text)
        self.assertIn("fewer than 2 tests (warning) (1)", text)
        self.assertIn("a hang, not a failing test (warning) (1)", text)


def sh(repo: Path, *args: str) -> str:
    return subprocess.run(["git", *args], cwd=repo, capture_output=True, text=True, check=True).stdout


class GitRepo(unittest.TestCase):
    """The runner's git-facing paths, run through `main` against a throwaway repository laid out as
    this one is."""

    CRATE = "src-tauri/crates/bridge-tax-audit"

    def setUp(self):
        self.t = tempfile.TemporaryDirectory()
        self.repo = Path(self.t.name) / "repo"
        self.root = self.repo / self.CRATE
        crate_layout(self.root)
        self.muts = [dict(mutation("B1", "src/book.rs"), **{"from": "// book", "to": "// koob"}),
                     dict(mutation("R1", "src/read.rs"), **{"from": "mod tests;", "to": "mod tset;"})]
        self.write("parity/mutations.json", json.dumps(self.muts))
        (self.repo / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.96.0"\n')
        (self.repo / "src-tauri/Cargo.toml").write_text("[workspace]\n")
        sh(self.repo, "init", "-q", "-b", "master")
        sh(self.repo, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "add", "-A")
        self.commit("base")
        self.saved = {k: getattr(mu, k) for k in ("ROOT", "WORKSPACE", "REPO", "CRATE", "LIST", "RESULTS", "RESULTS_REL", "SURVIVORS")}
        mu.ROOT, mu.WORKSPACE, mu.REPO, mu.CRATE = self.root, self.repo / "src-tauri", self.repo, self.CRATE
        mu.LIST, mu.RESULTS = self.root / "parity/mutations.json", self.root / "parity/mutation-results.json"
        mu.RESULTS_REL = f"{self.CRATE}/parity/mutation-results.json"
        mu.SURVIVORS = self.root / "parity/accepted-survivors.json"

    def tearDown(self):
        for k, v in self.saved.items():
            setattr(mu, k, v)
        self.t.cleanup()

    def write(self, rel: str, text: str) -> None:
        p = self.root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")

    def commit(self, message: str) -> None:
        sh(self.repo, "add", "-A")
        sh(self.repo, "-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", message)

    def main(self, *argv: str) -> tuple[int, str]:
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = mu.main(list(argv))
        return rc, out.getvalue() + err.getvalue()

    def prove(self, *ids: str) -> None:
        """Commit killed records for `ids` made on the current tree, as a run would."""
        tree = mu.crate_tree("HEAD", self.repo, self.CRATE)
        results = mu.load_results(mu.RESULTS)
        for m in self.muts:
            if m["id"] in ids:
                results[m["id"]] = record(m, ["tests/registry.rs::registry_ok"], tree=tree)
        mu.RESULTS.write_text(mu.render_results(results, [m["id"] for m in self.muts]))
        self.commit("prove " + " ".join(ids))
        self.assertEqual(mu.crate_tree("HEAD", self.repo, self.CRATE), tree, "the results file is not in the tree")

    def test_changed_since_diffs_from_the_merge_base(self):
        sh(self.repo, "checkout", "-q", "-b", "pr")
        self.write("src/book.rs", "// book, edited\n")
        self.commit("pr edit")
        sh(self.repo, "checkout", "-q", "-b", "fixture-only")
        self.write("tests/fixtures/श्री ₹.json", "{}\n")  # git quotes such a path unless -z
        self.commit("a non-ASCII fixture")
        self.assertIn("tests/fixtures/श्री ₹.json", mu.changed_since("pr", self.repo, self.CRATE))
        sh(self.repo, "checkout", "-q", "pr")
        sh(self.repo, "checkout", "-q", "master")
        self.write("src/read.rs", "#[cfg(test)]\nmod tests; // master moved\n")
        self.commit("master edit")
        sh(self.repo, "checkout", "-q", "pr")
        self.assertEqual(mu.changed_since("master", self.repo, self.CRATE), ["src/book.rs"])
        sh(self.repo, "mv", "tests/../" + self.CRATE + "/tests/registry.rs", self.CRATE + "/tests/reg.rs")
        self.commit("rename a killer's file")
        self.assertEqual(mu.changed_since("master", self.repo, self.CRATE),
                         ["src/book.rs", "tests/reg.rs", "tests/registry.rs"], "a rename lists its old name too")

    def test_the_nightly_issue_loop_closes(self):
        self.prove("B1", "R1")
        issue = self.root.parent / "issues.txt"
        text, failed = mu.report(self.muts, {"B1": record(self.muts[0], [], verdict=mu.SURVIVED),
                                             "R1": mu.load_results(mu.RESULTS)["R1"]}, mu.load_results(mu.RESULTS))
        self.assertTrue(failed)
        head7, head8 = f"<!-- {mu.ISSUE_MARK} 7 -->", f"<!-- {mu.ISSUE_MARK} 8 -->"  # as ci.yml writes them
        issue.write_text(f"{head7}\n{text}\nRun: https://example.invalid/run\n"
                         f"{head8}\n<!-- {mu.FAILING_MARK} R1 -->\n")  # a second open issue
        sh(self.repo, "branch", "base")
        # A crate change that selects nothing by itself ...
        self.write("src/support.rs", "// unrelated\n")
        self.commit("unrelated crate change")
        self.assertEqual(self.main("--verify", "--changed-since", "base")[0], 0)
        # ... is refused while the issue is open, because it does not re-prove B1 ...
        rc, out = self.main("--verify", "--changed-since", "base", "--nightly-issues", str(issue))
        self.assertEqual(rc, 1, out)
        self.assertIn("B1: failing on the nightly run  [NOT PROVEN ON THIS TREE]", out)
        self.assertIn("R1: failing on the nightly run  [NOT PROVEN ON THIS TREE]", out)
        # ... while a change outside the crate is not held by it, B1 still unproven ...
        (self.repo / "README.md").write_text("outside the crate\n")
        self.commit("outside the crate")
        rc, out = self.main("--verify", "--changed-since", "HEAD~1", "--nightly-issues", str(issue))
        self.assertEqual((rc, "B1:" in out), (0, False), out)
        # ... re-proving only one of the failing ids is not enough ...
        self.prove("B1")
        rc, out = self.main("--verify", "--changed-since", "base", "--nightly-issues", str(issue))
        self.assertEqual(rc, 1, out)
        # ... and the fix that re-proves them all on its own tree passes; an issue without the line refuses.
        self.prove("R1")
        rc, out = self.main("--verify", "--changed-since", "base", "--nightly-issues", str(issue))
        self.assertEqual(rc, 0, out)
        for body in (f"{head7}\nsomeone rewrote the body\n",
                     # one issue lost its line while another kept one: the other must not hide it
                     f"{head7}\nsomeone rewrote the body\n{head8}\n<!-- {mu.FAILING_MARK} R1 -->\n",
                     f"{head8}\n<!-- {mu.FAILING_MARK} R1 -->\n{head7}\n"):
            issue.write_text(body)
            rc, out = self.main("--verify", "--changed-since", "base", "--nightly-issues", str(issue))
            self.assertEqual(rc, 1, body)
            self.assertIn("no failing-ids line", out)
        self.assertEqual(mu.failing_ids(""), [], "no open issue holds nothing")

    def test_failing_ids_reads_the_header_ci_yml_writes(self):
        ci = (Path(__file__).resolve().parents[4] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn(f'"<!-- {mu.ISSUE_MARK} \\(.number) -->', ci, "ci.yml writes the header failing_ids reads")
        line = f"<!-- {mu.FAILING_MARK} A1 -->"
        self.assertEqual(mu.failing_ids(issue(line, 612) + issue(line.replace("A1", "B2"), 1613)), ["A1", "B2"])
        self.assertIsNone(mu.failing_ids(issue(line, 612) + issue("rewritten", 1613)), "multi-digit numbers")
        self.assertIsNone(mu.failing_ids(f"{line}\nno header at all\n"), "a drifted header refuses")

    def test_verify_fails_on_an_unproven_selection_a_stale_mutation_or_a_retired_record(self):
        self.prove("B1", "R1")
        sh(self.repo, "branch", "base")
        self.write("src/book.rs", "// book, edited\n")
        self.commit("edit")
        rc, out = self.main("--verify", "--changed-since", "base")
        self.assertEqual(rc, 1, out)
        self.prove("B1")
        self.assertEqual(self.main("--verify", "--changed-since", "base")[0], 0)
        self.write("src/book.rs", "// nothing to mutate\n")
        self.commit("the mutation's text is gone")
        self.prove("B1")
        rc, out = self.main("--verify", "--changed-since", "base")
        self.assertEqual(rc, 1, out)
        self.assertIn("STALE B1: `from` found 0 times", out)
        self.write("src/book.rs", "// book\n")
        self.muts = self.muts[1:]
        self.write("parity/mutations.json", json.dumps(self.muts))
        self.commit("retire B1")
        rc, out = self.main("--verify", "--changed-since", "HEAD")
        self.assertEqual(rc, 1, out)
        self.assertIn("B1: a record for a mutation no longer in the list", out)
        # An accepted-survivor entry for a retired mutation fails the merge check too.
        del_results = mu.load_results(mu.RESULTS)
        del del_results["B1"]
        mu.RESULTS.write_text(mu.render_results(del_results, ["R1"]))
        (self.root / "parity/accepted-survivors.json").write_text(json.dumps({"B1": {"mutation": "x"}}))
        self.commit("drop the record; list B1 as a survivor")
        rc, out = self.main("--verify", "--changed-since", "HEAD")
        self.assertEqual(rc, 1, out)
        self.assertIn("B1: accepted as a survivor but no longer in the list", out)

    def test_a_run_refuses_a_dirty_tree_and_a_held_workdir_before_any_build(self):
        work = self.root.parent / "mutants"
        self.prove("B1", "R1")
        (self.repo / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "stable"\n')
        rc, out = self.main("B1", "--workdir", str(work))
        self.assertEqual(rc, 2, out)
        self.assertIn("M rust-toolchain.toml", out)
        sh(self.repo, "checkout", "-q", "--", "rust-toolchain.toml")
        mu.RESULTS.write_text("{\n}\n")  # an uncommitted results file does not stop a run
        porcelain = sh(self.repo, "status", "--porcelain", "--untracked-files=no")
        self.assertIn("mutation-results.json", porcelain)
        self.assertEqual(mu.dirty(porcelain), [])
        held = mu.lock_workdir(work)
        try:
            rc, out = self.main("B1", "--workdir", str(work))
            self.assertEqual(rc, 2, out)
            self.assertIn("another run holds", out)
        finally:
            held.close()

    def test_the_copy_holds_the_committed_bytes_and_only_them(self):
        (self.root / "src/book.rs").write_bytes(b"// book\r\n")
        self.commit("a CR byte")
        (self.root / "src/book.rs").write_bytes(b"// uncommitted\n")
        files = mu.committed_files("HEAD", self.repo)
        self.assertEqual(files[f"{self.CRATE}/src/book.rs"], b"// book\r\n")
        self.assertIn("rust-toolchain.toml", files)
        w = mu.Worker(0, self.root.parent / "mutants")
        w.sync(files)
        kept = w.tree / "Cargo.toml"
        os.utime(kept, (1, 1))
        (w.tree / "stale.rs").write_text("old")  # e.g. a test file since deleted
        w.sync(files)
        self.assertEqual((w.dir / self.CRATE / "src/book.rs").read_bytes(), b"// book\r\n")
        self.assertFalse((w.tree / "stale.rs").exists())
        self.assertEqual(kept.stat().st_mtime, 1, "an unchanged file is not re-written")
        (self.root / "src/book.rs").write_bytes(b"// book, weakened\n")
        self.commit("a changed file")
        w.sync(mu.committed_files("HEAD", self.repo))
        self.assertEqual((w.dir / self.CRATE / "src/book.rs").read_bytes(), b"// book, weakened\n",
                         "a copy file whose bytes differ from HEAD is re-written")
        (self.root / "src/link.rs").symlink_to("book.rs")
        self.commit("a tracked symlink")
        with self.assertRaises(SystemExit):
            mu.committed_files("HEAD", self.repo)

    def test_a_run_through_main_proves_on_the_committed_tree_and_refuses_a_failing_baseline(self):
        outside = Path(self.t.name)  # beside the repository, not in it
        work, results = outside / "mutants", outside / "r.json"
        head = sh(self.repo, "rev-parse", "HEAD").strip()
        tree = mu.crate_tree("HEAD", self.repo, self.CRATE)
        refs = []
        saved = mu.crate_tree, mu.committed_files
        mu.crate_tree = lambda ref, *a: refs.append(("tree", ref)) or saved[0](ref, *a)
        mu.committed_files = lambda ref, *a: refs.append(("files", ref)) or saved[1](ref, *a)
        try:
            with fake_cargo(outside, FAKE_CARGO):
                rc, out = self.main("B1", "R1", "--workdir", str(work), "--results", str(results))
        finally:
            mu.crate_tree, mu.committed_files = saved
        self.assertEqual(refs, [("tree", head), ("files", head)], "HEAD is read once and both use that commit")
        with fake_cargo(outside, FAKE_CARGO):
            self.assertEqual(rc, 1, out)  # R1 survives: the fake suite ignores it
            recs = json.loads(results.read_text())
            self.assertEqual({k: (v["verdict"], v["killers"], v["mode"], v["crate_tree"], v["commit"])
                              for k, v in recs.items()},
                             {"B1": (mu.KILLED, ["tests/registry.rs::registry_ok"], "full", tree, head),
                              "R1": (mu.SURVIVED, [], "full", tree, head)})
            rc, out = self.main("B1", "--workdir", str(work), "--results", str(results))
            self.assertEqual((rc, json.loads(results.read_text())["B1"]["mode"]), (0, "killers-first"), out)
            os.environ["FAKE_FAIL"] = "1"
            rc, out = self.main("B1", "--workdir", str(work), "--results", str(outside / "r2.json"))
            self.assertEqual(rc, 2, out)
            self.assertIn("the unmutated copy does not pass", out)
            self.assertFalse((outside / "r2.json").exists())
            del os.environ["FAKE_FAIL"]
            self.muts.append(dict(mutation("X1", "src/book.rs"), **{"from": "not in the file"}))
            self.write("parity/mutations.json", json.dumps(self.muts))
            self.commit("a stale mutation")
            rc, out = self.main("B1", "--workdir", str(work), "--results", str(results))
            self.assertEqual(rc, 1, "B1 is killed, but a stale mutation in the list fails the run")
            self.assertIn("STALE X1", out)
        self.assertEqual(sh(self.repo, "status", "--porcelain"), "", "the working tree is never touched")

    def test_an_accepted_survivor_passes_every_path_through_main(self):
        outside = Path(self.t.name)
        work, results = outside / "mutants", outside / "r.json"
        r1 = next(m for m in self.muts if m["id"] == "R1")  # the fake suite never kills R1
        mu.SURVIVORS.write_text(json.dumps({"R1": {"mutation": mu.mutation_hash(r1), "reason": "test"}}))
        self.commit("accept R1 as a survivor")
        with fake_cargo(outside, FAKE_CARGO):
            rc, out = self.main("B1", "R1", "--workdir", str(work), "--results", str(results))
        self.assertEqual(rc, 0, out)  # the run's own judgement
        self.assertEqual(json.loads(results.read_text())["R1"]["verdict"], mu.SURVIVED)
        rc, out = self.main("R1", "--verify", "--results", str(results))
        self.assertEqual(rc, 0, out)  # the merge check
        self.assertIn("1 selected, 1 proven", out)
        self.assertNotIn("NOT PROVEN", out)
        (self.root / "src/support.rs").write_text("// unrelated\n")
        self.commit("an unrelated source change")
        rc, out = self.main("--list", "--changed-since", "HEAD~1", "--results", str(results))
        self.assertNotIn("R1:", out, "selection does not re-pick an accepted survivor by itself")
        mu.RESULTS.write_text(results.read_text())
        self.commit("commit the records")
        rc, out = self.main("--merge", str(results), "--results", str(outside / "all.json"))
        self.assertEqual(rc, 0, out)  # the nightly's judgement
        self.assertIn("## Accepted survivors (1)", out)

    def test_sigterm_stops_a_run_ends_its_cargo_and_restores_the_copy(self):
        self.sigterm_run(HANG_ON_B1)

    def test_sigterm_during_the_baseline_ends_its_cargo(self):
        # The baseline runs on the main thread, where the signal unwinds cargo's own call.
        self.sigterm_run(HANG_ALWAYS)

    def sigterm_run(self, script: str) -> None:
        outside = Path(self.t.name)
        runner = self.root / "parity/mutations.py"  # the runner committed into the throwaway repo
        runner.write_bytes((Path(__file__).resolve().parent / "mutations.py").read_bytes())
        self.commit("the runner")
        pidfile, work, results = outside / "pid", outside / "mutants", outside / "r.json"
        with fake_cargo(outside, script, PIDFILE=str(pidfile)):
            proc = subprocess.Popen([sys.executable, "-B", str(runner), "B1", "--workdir", str(work),
                                     "--results", str(results)], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                    text=True, env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"))
            try:
                for _ in range(200):
                    if pidfile.exists() and pidfile.read_text().strip():
                        break
                    time.sleep(0.05)
                self.assertTrue(pidfile.exists(), "the mutated suite started")
                start = time.monotonic()
                proc.send_signal(15)  # this test's own child, by its PID
                out, _ = proc.communicate(timeout=20)
            finally:
                if proc.poll() is None:
                    proc.kill()
                    proc.wait()
        self.assertEqual(proc.returncode, 130, out)
        self.assertLess(time.monotonic() - start, 15, "the stop did not wait for the hung suite")
        self.assertTrue(gone(int(pidfile.read_text())), "the hung suite's child is gone")
        self.assertNotIn(b"koob", (work / "w0" / self.CRATE / "src/book.rs").read_bytes(), "the copy is restored")
        self.assertFalse(results.exists(), "no verdict for the stopped mutation")

    def test_merge_reports_an_unreadable_shard_and_never_writes_the_committed_file(self):
        shard = self.root.parent / "shard-1.json"
        shard.write_text('{\n  "B1": {"verdict": "kil')  # truncated
        rc, out = self.main("--merge", str(shard), str(self.root.parent / "missing.json"))
        self.assertEqual(rc, 2, out)
        self.assertIn("refusing: --merge writes --results", out)
        rc, out = self.main("--merge", str(shard), str(self.root.parent / "missing.json"),
                            "--results", str(self.root.parent / "all.json"), "--report", str(self.root.parent / "r.md"))
        self.assertEqual(rc, 1, out)
        report = (self.root.parent / "r.md").read_text()
        self.assertIn("Shard results not read (2)", report)
        self.assertEqual(mu.failing_ids(issue(report)), ["B1", "R1"])
        self.assertFalse(mu.RESULTS.exists())
        # Every shard killed everything, but the committed file lacks those records: still a failure.
        good = self.root.parent / "shard-2.json"
        good.write_text(json.dumps({m["id"]: record(m, ["tests/registry.rs::registry_ok"]) for m in self.muts}))
        rc, out = self.main("--merge", str(good), "--results", str(self.root.parent / "all.json"))
        self.assertEqual(rc, 1, out)
        self.assertIn("B1: no committed record", out)
        self.prove("B1", "R1")
        self.assertEqual(self.main("--merge", str(good), "--results", str(self.root.parent / "all.json"))[0], 0)


FAKE_CARGO = """#!/bin/sh
# A stand-in for cargo: builds instantly; the suite fails when the copy holds B1's mutation.
case " $* " in *" --no-run "*) echo "    Finished"; exit 0;; esac
echo "     Running tests/registry.rs (/x)"
if [ -n "$FAKE_FAIL" ] || grep -q koob src/book.rs; then echo "test registry_ok ... FAILED"; exit 101; fi
echo "test registry_ok ... ok"
"""
HANG_ALWAYS = """#!/bin/sh
# A stand-in for cargo: builds instantly, then every test run hangs (a child in its group).
case " $* " in *" --no-run "*) echo "    Finished"; exit 0;; esac
sleep 30 & echo $! > "$PIDFILE"; wait
"""
HANG_ON_B1 = """#!/bin/sh
# A stand-in for cargo: builds and passes instantly, but hangs (a child in its group) on B1's mutation.
case " $* " in *" --no-run "*) echo "    Finished"; exit 0;; esac
if grep -q koob src/book.rs; then sleep 30 & echo $! > "$PIDFILE"; wait; fi
echo "     Running tests/registry.rs (/x)"
echo "test registry_ok ... ok"
"""
SLOW_CARGO = """#!/bin/sh
# A stand-in for cargo that runs a long child in its own process group and records its pid.
sleep 30 &
echo $! > "$PIDFILE"
wait
"""


@contextlib.contextmanager
def fake_cargo(tmp: Path, script: str, **env: str):
    """PATH (and `env`) set so `cargo` runs `script`, restored afterwards."""
    bin_dir = tmp / "bin"
    bin_dir.mkdir(exist_ok=True)
    (bin_dir / "cargo").write_text(script)
    (bin_dir / "cargo").chmod(0o755)
    saved = dict(os.environ)
    os.environ.update(env, PATH=f"{bin_dir}{os.pathsep}{saved['PATH']}")
    try:
        yield
    finally:
        os.environ.clear()
        os.environ.update(saved)


def gone(pid: int) -> bool:
    for _ in range(50):
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        time.sleep(0.1)
    return False


class StopPath(unittest.TestCase):
    def test_a_stop_or_a_timeout_ends_the_cargo_group_and_a_stop_gives_no_verdict(self):
        with tempfile.TemporaryDirectory() as t:
            tmp = Path(t)
            pidfile = tmp / "pid"
            w = mu.Worker(0, tmp / "mutants")
            w.crate.mkdir(parents=True)
            with fake_cargo(tmp, SLOW_CARGO, PIDFILE=str(pidfile)):
                try:
                    mu._STOP.set()
                    with self.assertRaises(mu.Stopped):
                        w.run([], 5)
                    self.assertFalse(pidfile.exists(), "a stopped run starts nothing")
                    mu._STOP.clear()
                    def stop_once_started():  # a loaded machine can start the fake cargo late
                        for _ in range(200):
                            if pidfile.exists() and pidfile.read_text().strip():
                                break
                            time.sleep(0.05)
                        mu._STOP.set()
                        mu._kill_active()
                    stopper = threading.Thread(target=stop_once_started)
                    start = time.monotonic()
                    stopper.start()
                    with self.assertRaises(mu.Stopped):
                        w.run([], 60)
                    stopper.join()
                    self.assertLess(time.monotonic() - start, 15, "the stop ended the run")
                    self.assertTrue(gone(int(pidfile.read_text())), "the child in cargo's group is gone")
                    mu._STOP.clear()
                    pidfile.unlink()
                    rc, _ = w.run([], 3)
                    self.assertIsNone(rc, "a timeout")
                    self.assertTrue(gone(int(pidfile.read_text())))
                    self.assertEqual(mu._ACTIVE, set())
                finally:
                    mu._STOP.clear()
            pidfile.unlink()
            saved_grace = mu.KILL_GRACE
            mu.KILL_GRACE = 0.5
            try:
                with fake_cargo(tmp, "#!/bin/sh\ntrap '' TERM\n" + SLOW_CARGO.split("\n", 2)[2], PIDFILE=str(pidfile)):
                    start = time.monotonic()
                    rc, _ = w.run([], 3)
                self.assertIsNone(rc)
                self.assertLess(time.monotonic() - start, 15, "SIGKILL follows an ignored SIGTERM")
                self.assertTrue(gone(int(pidfile.read_text())), "the group that ignored SIGTERM is gone")
            finally:
                mu.KILL_GRACE = saved_grace


class ResultsFile(unittest.TestCase):
    def test_one_record_per_line_in_list_order_and_valid_json(self):
        records = {"B": {"verdict": "killed", "killers": ["x"]}, "A": {"verdict": "survived", "killers": []}}
        text = mu.render_results(records, ["A", "B"])
        self.assertEqual(json.loads(text), records)
        lines = text.splitlines()
        self.assertEqual((lines[0], lines[-1]), ("{", "}"))
        self.assertTrue(lines[1].lstrip().startswith('"A"') and lines[2].lstrip().startswith('"B"'))
        self.assertEqual(mu.render_results({}, []), "{\n}\n")
        with tempfile.TemporaryDirectory() as t:
            path = Path(t) / "r.json"
            mu.write_atomic(path, text)
            self.assertEqual((path.read_text(), sorted(p.name for p in Path(t).iterdir())), (text, ["r.json"]))

    def test_a_mutations_hash_covers_file_from_and_to_only(self):
        m = mutation("X", "src/a.rs")
        self.assertEqual(mu.mutation_hash(m), mu.mutation_hash(dict(m, what="reworded", by="reviewer")))
        for key in ("file", "from", "to"):
            self.assertNotEqual(mu.mutation_hash(m), mu.mutation_hash(dict(m, **{key: "changed"})), key)


if __name__ == "__main__":
    unittest.main()
