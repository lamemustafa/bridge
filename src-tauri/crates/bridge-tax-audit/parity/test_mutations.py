# SPDX-License-Identifier: Apache-2.0
"""Tests of mutations.py's pure parts: output parsing, killer-to-file mapping, selection, sharding
and the results file's format. No cargo run.

    python3 -m unittest discover -s src-tauri/crates/bridge-tax-audit/parity -p 'test_mutations.py'
"""
from __future__ import annotations

import json
import os
import sys
import tempfile
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
     Running tests/registry.rs (/x/target/debug/deps/registry-11)

running 1 test
test registry_ok ... ok
   Doc-tests bridge_tax_audit

running 1 test
test src/book.rs - book::Voucher (line 12) ... FAILED
error: 2 targets failed:
"""


def crate_layout(root: Path) -> Path:
    for rel, text in {
        "src/lib.rs": "pub mod book;\n",
        "src/book.rs": "// book\n",
        "src/support.rs": "// support\n",
        "src/read.rs": "#[cfg(test)]\nmod tests;\n",
        "src/read/tests.rs": "// read's tests\n",
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


def record(m: dict, killers: list[str], verdict: str = mu.KILLED) -> dict:
    return {"verdict": verdict, "killers": killers, "mutation": mu.mutation_hash(m), "mode": "full",
            "crate_tree": "t", "commit": "c", "time": "now"}


class ParseOutput(unittest.TestCase):
    def test_failures_are_qualified_by_their_target(self):
        failed, compile_error = mu.parse_test_output(OUTPUT)
        self.assertEqual(failed, sorted([
            "src/lib.rs::book::tests::a_voucher_reference_is_read",
            "src/lib.rs::tests::root_level",
            "tests/edge_books.rs::every_edge_book_matches_the_reference",
            "doc::src/book.rs - book::Voucher (line 12)",
        ]))
        self.assertFalse(compile_error, "cargo's 'error: test failed' is not a compile error")

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
                "weird::x": None,
            }
            for killer, want in cases.items():
                self.assertEqual(mu.killer_file(killer, crate), want, killer)

    def test_killers_first_arguments(self):
        self.assertEqual(
            mu.killer_args(["src/lib.rs::book::tests::a", "tests/edge_books.rs::every"]),
            (["--lib", "--test=edge_books"], ["book::tests::a", "every"]))
        self.assertIsNone(mu.killer_args(["doc::src/book.rs - x (line 1)"]), "a doc test is not run alone")
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

    def test_tests_common_selects_every_integration_killer(self):
        self.assertEqual(self.picked(["tests/common/mod.rs"]), {"E1"})

    def test_a_fixture_change_selects_what_fixture_reading_tests_kill(self):
        self.assertEqual(self.picked(["tests/fixtures/golden/x.json"]), {"E1"})

    def test_a_build_input_selects_everything(self):
        self.assertEqual(self.picked(["rules/x.toml"]), {"B1", "S1", "E1"})
        self.assertEqual(self.picked(["Cargo.toml"]), {"B1", "S1", "E1"})

    def test_unproven_or_edited_mutations_are_always_selected(self):
        results = dict(self.results)
        del results["B1"]
        results["S1"] = record(self.support, ["src/lib.rs::book::tests::b"], verdict=mu.SURVIVED)
        edited = dict(self.edge, to="something else")
        self.muts[2] = edited
        self.assertEqual(self.picked([], results), {"B1", "S1", "E1"})

    def test_the_runner_and_its_results_are_inert(self):
        self.assertEqual(self.picked(list(mu.INERT)), set())


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
        self.muts = [mutation("A", "src/a.rs"), mutation("B", "src/b.rs")]
        self.committed = {m["id"]: record(m, ["tests/x.rs::t", "tests/x.rs::u"]) for m in self.muts}

    def test_all_killed_with_fresh_committed_records_passes(self):
        merged = {m["id"]: record(m, ["tests/x.rs::t", "tests/x.rs::u"]) for m in self.muts}
        text, failed = mu.report(self.muts, merged, self.committed)
        self.assertFalse(failed, text)

    def test_each_failure_kind_fails_the_run(self):
        merged = {"A": record(self.muts[0], [], verdict=mu.SURVIVED)}  # B not run at all
        text, failed = mu.report(self.muts, merged, self.committed)
        self.assertTrue(failed)
        self.assertIn("Not killed (1)", text)
        self.assertIn("Not run (1)", text)
        merged = {m["id"]: record(m, ["tests/x.rs::t", "tests/x.rs::u"]) for m in self.muts}
        stale = dict(self.committed, A=record(dict(self.muts[0], to="edited"), ["tests/x.rs::t"]))
        stale["gone"] = record(mutation("gone", "src/z.rs"), [])
        del stale["B"]
        text, failed = mu.report(self.muts, merged, stale)
        self.assertTrue(failed)
        self.assertIn("Stale committed records (3)", text)

    def test_a_thin_margin_warns_without_failing(self):
        merged = {m["id"]: record(m, ["tests/x.rs::t"]) for m in self.muts}
        text, failed = mu.report(self.muts, merged, self.committed)
        self.assertFalse(failed)
        self.assertIn("fewer than 2 tests (warning) (2)", text)

    def test_a_timeout_counts_as_killed(self):
        merged = {m["id"]: record(m, [], verdict=mu.TIMEOUT) for m in self.muts}
        self.assertFalse(mu.report(self.muts, merged, self.committed)[1])


class WorkerSync(unittest.TestCase):
    def test_the_copy_holds_exactly_the_tracked_files_and_keeps_unchanged_ones(self):
        with tempfile.TemporaryDirectory() as t:
            repo = Path(t) / "repo"
            for rel, text in {"src-tauri/a.rs": "a", "src-tauri/crates/c/lib.rs": "c"}.items():
                (repo / rel).parent.mkdir(parents=True, exist_ok=True)
                (repo / rel).write_text(text)
            saved = mu.REPO
            mu.REPO = repo
            try:
                w = mu.Worker(0, Path(t) / "mutants")
                w.sync(["src-tauri/a.rs", "src-tauri/crates/c/lib.rs"])
                kept = w.dir / "src-tauri/crates/c/lib.rs"
                os.utime(kept, (1, 1))
                (w.dir / "src-tauri/stale.rs").write_text("old")  # e.g. a test file since deleted
                (repo / "src-tauri/a.rs").write_text("a2")
                w.sync(["src-tauri/a.rs", "src-tauri/crates/c/lib.rs"])
            finally:
                mu.REPO = saved
            self.assertEqual((w.dir / "src-tauri/a.rs").read_text(), "a2")
            self.assertFalse((w.dir / "src-tauri/stale.rs").exists())
            self.assertEqual(kept.stat().st_mtime, 1, "an unchanged file is not re-copied")


class ResultsFile(unittest.TestCase):
    def test_one_record_per_line_in_list_order_and_valid_json(self):
        records = {"B": {"verdict": "killed", "killers": ["x"]}, "A": {"verdict": "survived", "killers": []}}
        text = mu.render_results(records, ["A", "B"])
        self.assertEqual(json.loads(text), records)
        lines = text.splitlines()
        self.assertEqual((lines[0], lines[-1]), ("{", "}"))
        self.assertTrue(lines[1].lstrip().startswith('"A"') and lines[2].lstrip().startswith('"B"'))
        self.assertEqual(mu.render_results({}, []), "{\n}\n")

    def test_a_mutations_hash_covers_file_from_and_to_only(self):
        m = mutation("X", "src/a.rs")
        self.assertEqual(mu.mutation_hash(m), mu.mutation_hash(dict(m, what="reworded", by="reviewer")))
        for key in ("file", "from", "to"):
            self.assertNotEqual(mu.mutation_hash(m), mu.mutation_hash(dict(m, **{key: "changed"})), key)


if __name__ == "__main__":
    unittest.main()
