#!/usr/bin/env python3
"""Offline controls for scripts/surface_coverage_report.py.

Each test builds a throwaway git repository where `main` holds the base, the
change is committed on a `work` branch (and sometimes left uncommitted on top),
and the report is run against `main`. Committing the change -- rather than
leaving HEAD at the base -- is what lets these tests tell the merge-base from
HEAD or from the tip of `main`.

The module graph itself was checked against rustc rather than asserted: on
every crate in this repository it reached exactly the file sets rustc compiled,
across the build profiles compared in bridge#436's review.
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "surface_coverage_report.py"
spec = importlib.util.spec_from_file_location("surface_coverage_report", SCRIPT)
report_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report_module)

CLEAN = ("surface coverage: no dropped pins and no newly unsealed direct children of"
         " pinned modules since main (not a proof nothing left the seal; see"
         " scripts/surface_coverage_report.py)")
CARGO = '[package]\nname = "fixture"\nversion = "0.0.0"\n'


class Repo:
    def __init__(self, root: str):
        self.root = root
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "test@example.invalid")
        self.git("config", "user.name", "test")

    def git(self, *args: str) -> str:
        return subprocess.run(["git", "-C", self.root, *args], check=True,
                              capture_output=True, text=True).stdout

    def write(self, files: dict[str, str | None]) -> None:
        for path, content in files.items():
            target = Path(self.root, path)
            if content is None:
                target.unlink()
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content, encoding="utf-8")

    def pin(self, *paths: str) -> None:
        entries = [{"path": p, "sha256": "0" * 64} for p in sorted(paths)]
        self.write({"surface.json": json.dumps({"files": entries})})

    def commit(self, message: str) -> None:
        self.git("add", "-A")
        self.git("commit", "-q", "--allow-empty", "-m", message)

    def report(self) -> list[str]:
        return report_module.report(self.root, "surface.json", "main")

    def newly_unsealed(self) -> set[str]:
        return {line.split()[3] for line in self.report() if "newly unsealed module:" in line}


class SurfaceCoverageReport(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.repo = Repo(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def base(self, files: dict[str, str], pinned: tuple[str, ...]) -> None:
        self.repo.write({"Cargo.toml": CARGO, **files})
        self.repo.pin(*pinned)
        self.repo.commit("base")
        self.repo.git("checkout", "-q", "-b", "work")

    def change(self, files: dict[str, str | None], pinned: tuple[str, ...] | None = None) -> None:
        self.repo.write(files)
        if pinned is not None:
            self.repo.pin(*pinned)
        self.repo.commit("change")

    # --- what it reports -------------------------------------------------

    def test_a_committed_extraction_from_a_pinned_file_is_reported(self):
        self.base({"src/lib.rs": "pub fn admit() {}\n"}, ("src/lib.rs",))
        self.change({"src/lib.rs": "mod admit;\n", "src/admit.rs": "pub fn admit() {}\n"})
        self.assertIn(
            "  newly unsealed module:  src/admit.rs  (declared by pinned src/lib.rs)",
            self.repo.report())

    def test_an_uncommitted_extraction_is_reported_too(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.repo.write({"src/lib.rs": "mod admit;\n", "src/admit.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/admit.rs"})

    def test_a_dropped_pin_is_reported(self):
        self.base({"src/lib.rs": "", "src/gate.rs": ""}, ("src/gate.rs", "src/lib.rs"))
        self.change({}, pinned=("src/lib.rs",))
        self.assertIn("  pin removed:            src/gate.rs", self.repo.report())

    def test_a_module_unpinned_on_this_branch_is_reported_as_both(self):
        self.base({"src/lib.rs": "mod gate;\n", "src/gate.rs": ""}, ("src/gate.rs", "src/lib.rs"))
        self.change({}, pinned=("src/lib.rs",))
        lines = self.repo.report()
        self.assertIn("  pin removed:            src/gate.rs", lines)
        self.assertEqual(self.repo.newly_unsealed(), {"src/gate.rs"})

    def test_an_existing_file_newly_attached_to_a_pinned_module_is_reported(self):
        # The file existed at the base but no pinned module declared it.
        self.base({"src/lib.rs": "", "src/orphan.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": "mod orphan;\n"})
        self.assertEqual(self.repo.newly_unsealed(), {"src/orphan.rs"})

    def test_test_only_modules_are_counted_not_listed(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({
            "src/lib.rs": '#[cfg(test)]\n#[path = "lib_tests.rs"]\nmod tests;\n'
                          '#[cfg(all(feature = "x", test))]\nmod gated_tests;\n',
            "src/lib_tests.rs": "", "src/gated_tests.rs": "",
        })
        lines = self.repo.report()
        self.assertIn("  newly unsealed test-only modules: 2 (not listed; unpinned tests are the norm)", lines)
        self.assertEqual(self.repo.newly_unsealed(), set())

    def test_a_module_inside_a_test_only_module_is_test_only(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({
            "src/lib.rs": '#[cfg(test)]\nmod tests {\n    #[path = "../fixtures/helper.rs"]\n    mod helper;\n}\n',
            # rustc: relative to src/tests/, the inline module's directory.
            "src/fixtures/helper.rs": "",
        })
        self.assertEqual(self.repo.newly_unsealed(), set())
        self.assertIn("  newly unsealed test-only modules: 1 (not listed; unpinned tests are the norm)",
                      self.repo.report())

    def test_not_test_and_any_test_are_production(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": '#[cfg(not(test))]\nmod real;\n'
                                   '#[cfg(any(test, feature = "x"))]\nmod either;\n',
                     "src/real.rs": "", "src/either.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/real.rs", "src/either.rs"})

    def test_a_module_inside_a_test_only_file_is_test_only(self):
        # Inheritance through a file, not just an inline block.
        self.base({"src/lib.rs": "#[cfg(test)]\nmod tests;\n", "src/tests.rs": ""},
                  ("src/lib.rs", "src/tests.rs"))
        self.change({"src/tests.rs": "mod helper;\n", "src/tests/helper.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), set())
        self.assertIn("  newly unsealed test-only modules: 1 (not listed; unpinned tests are the norm)",
                      self.repo.report())

    def test_restricted_visibility_keeps_its_attributes(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": "#[cfg(test)]\npub(crate) mod tests;\n"
                                   "pub(in crate) mod scoped;\npub(super) mod parent_scoped;\n",
                     "src/tests.rs": "", "src/scoped.rs": "", "src/parent_scoped.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/scoped.rs", "src/parent_scoped.rs"})
        self.assertIn("  newly unsealed test-only modules: 1 (not listed; unpinned tests are the norm)",
                      self.repo.report())

    def test_a_feature_gated_module_is_labelled(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": '#[cfg(feature = "lab")]\nmod lab;\n'
                                   '#[cfg(target_os = "macos")]\nmod mac;\n',
                     "src/lab.rs": "", "src/mac.rs": ""})
        lines = self.repo.report()
        self.assertIn(
            "  newly unsealed module:  src/lab.rs  (declared by pinned src/lib.rs, behind a cargo feature)",
            lines)
        # A non-feature key = value is not a feature gate.
        self.assertIn("  newly unsealed module:  src/mac.rs  (declared by pinned src/lib.rs)", lines)

    def test_a_feature_gate_is_inherited_through_a_file(self):
        self.base({"src/lib.rs": '#[cfg(feature = "lab")]\nmod lab;\n', "src/lab.rs": ""},
                  ("src/lab.rs", "src/lib.rs"))
        self.change({"src/lab.rs": "mod inner;\n", "src/lab/inner.rs": ""})
        self.assertIn(
            "  newly unsealed module:  src/lab/inner.rs  (declared by pinned src/lab.rs, behind a cargo feature)",
            self.repo.report())

    def test_a_new_file_under_a_parent_pinned_on_this_branch_is_reported(self):
        # The parent was not pinned at the base; the child is a new file.
        self.base({"src/lib.rs": "mod post;\n", "src/post.rs": ""}, ("src/lib.rs",))
        self.change({"src/post.rs": "mod guard;\n", "src/post/guard.rs": ""},
                    pinned=("src/lib.rs", "src/post.rs"))
        self.assertEqual(self.repo.newly_unsealed(), {"src/post/guard.rs"})

    def test_a_file_pinned_at_the_base_and_unpinned_under_a_newly_pinned_parent(self):
        self.base({"src/lib.rs": "mod post;\n", "src/post.rs": "mod guard;\n", "src/post/guard.rs": ""},
                  ("src/lib.rs", "src/post/guard.rs"))
        self.change({}, pinned=("src/lib.rs", "src/post.rs"))
        lines = self.repo.report()
        self.assertIn("  pin removed:            src/post/guard.rs", lines)
        self.assertEqual(self.repo.newly_unsealed(), {"src/post/guard.rs"})

    # --- what it deliberately does not report ----------------------------

    def test_pinning_a_parent_does_not_report_its_existing_children(self):
        # The false positive the first real run produced: pinning thirteen files
        # made their existing test modules look like new departures.
        self.base({"src/lib.rs": "mod post;\n", "src/post.rs": "#[cfg(test)]\nmod tests;\nmod rules;\n",
                   "src/post/tests.rs": "", "src/post/rules.rs": ""}, ("src/lib.rs",))
        self.change({}, pinned=("src/lib.rs", "src/post.rs"))
        self.assertEqual(self.repo.report(), [CLEAN])

    def test_a_module_left_unpinned_before_the_branch_is_not_reprinted(self):
        self.base({"src/lib.rs": "mod labels;\n", "src/labels.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": "mod labels;\npub fn changed() {}\n"})
        self.assertEqual(self.repo.report(), [CLEAN])

    def test_a_declared_module_with_no_file_is_not_reported(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": "mod ghost;\n"})
        self.assertEqual(self.repo.report(), [CLEAN])

    def test_commented_out_modules_are_ignored(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": "// mod ghost;\n/* mod phantom; /* nested */ mod still_comment; */\n",
                     "src/ghost.rs": "", "src/phantom.rs": "", "src/still_comment.rs": ""})
        self.assertEqual(self.repo.report(), [CLEAN])

    def test_it_compares_with_the_merge_base_not_the_tip_of_main(self):
        # main moves on after the branch point and pins a new file. Compared with
        # main's tip, this branch would appear to drop that pin.
        self.base({"src/lib.rs": "", "src/later.rs": ""}, ("src/lib.rs",))
        self.change({"src/unrelated.txt": "x"})
        self.repo.git("checkout", "-q", "main")
        self.repo.pin("src/later.rs", "src/lib.rs")
        self.repo.commit("main pins later.rs")
        self.repo.git("checkout", "-q", "work")
        self.assertEqual(self.repo.report(), [CLEAN])

    # --- module resolution, per rustc -------------------------------------

    def test_a_bare_mod_in_a_path_loaded_file_resolves_beside_it(self):
        # The case review found missing: rustc treats a #[path]-loaded file as
        # owning its directory, so `mod guard;` there is `src/guard.rs`.
        self.base({"src/lib.rs": '#[path = "post_impl.rs"]\nmod post;\n', "src/post_impl.rs": ""},
                  ("src/lib.rs", "src/post_impl.rs"))
        self.change({"src/post_impl.rs": "mod guard;\n", "src/guard.rs": "", "src/post_impl/guard.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/guard.rs"})

    def test_a_path_attribute_in_an_ordinary_file_resolves_beside_it(self):
        # The shape agent.rs uses for nearly every agent_*.rs file. rustc
        # resolves `#[path]` from the declaring file's own directory, never from
        # `<stem>/`, even though a bare `mod` in the same file would.
        self.base({"src/lib.rs": "mod agent;\n", "src/agent.rs": ""}, ("src/agent.rs", "src/lib.rs"))
        self.change({"src/agent.rs": '#[path = "agent_post.rs"]\nmod post;\n',
                     "src/agent_post.rs": "", "src/agent/agent_post.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/agent_post.rs"})

    def test_a_bare_mod_in_an_ordinary_file_resolves_under_its_stem(self):
        self.base({"src/lib.rs": "mod foo;\n", "src/foo.rs": ""}, ("src/foo.rs", "src/lib.rs"))
        self.change({"src/foo.rs": "mod bar;\n", "src/foo/bar.rs": "", "src/bar.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/foo/bar.rs"})

    def test_a_mod_rs_directory_module_is_found(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": "mod db;\n", "src/db/mod.rs": "mod store;\n", "src/db/store.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/db/mod.rs"})

    def test_a_path_attribute_inside_an_inline_module(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": 'mod outer {\n    #[path = "inner_impl.rs"]\n    mod inner;\n}\n',
                     "src/outer/inner_impl.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/outer/inner_impl.rs"})

    def test_crate_roots_come_from_cargo_conventions_and_manifest_paths(self):
        self.base({"src/bin/tool.rs": "", "tests/it.rs": "", "src/daemon.rs": "", "build.rs": "",
                   "src/bin/multi/main.rs": "", "sub/Cargo.toml": CARGO, "sub/src/main.rs": ""},
                  ("build.rs", "src/bin/multi/main.rs", "src/bin/tool.rs", "src/daemon.rs",
                   "sub/src/main.rs", "tests/it.rs"))
        self.repo.write({"Cargo.toml": CARGO + '[[bin]]\nname = "d"\npath = "./src/daemon.rs"\n'})
        self.change({"src/bin/tool.rs": "mod a;\n", "src/bin/a.rs": "",
                     "tests/it.rs": "mod b;\n", "tests/b.rs": "",
                     "src/daemon.rs": "mod c;\n", "src/c.rs": "",
                     "build.rs": "mod d;\n", "d.rs": "",
                     "src/bin/multi/main.rs": "mod e;\n", "src/bin/multi/e.rs": "",
                     "sub/src/main.rs": "mod f;\n", "sub/src/f.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(),
                         {"src/bin/a.rs", "tests/b.rs", "src/c.rs", "d.rs",
                          "src/bin/multi/e.rs", "sub/src/f.rs"})

    def test_a_file_under_a_nested_tests_directory_is_an_ordinary_module(self):
        # Only `<crate>/tests/*.rs` is a crate root. `src/tests/case.rs`, reached
        # through `mod tests;`, is an ordinary file, so rustc resolves its
        # `mod stray;` under its stem -- and a report that took it for a root
        # would neither look there nor name it as a child.
        # `src/tests.rs` rather than `src/tests/mod.rs`: it sorts ahead of the
        # nested files, so a resolver that wrongly took them for roots would
        # visit them first. With `mod.rs` the correct parent happens to win the
        # visit order and the mistake goes unseen -- measured.
        self.base({"src/lib.rs": "mod tests;\n", "src/tests.rs": "mod case;\n",
                   "src/tests/case.rs": ""},
                  ("src/lib.rs", "src/tests.rs", "src/tests/case.rs"))
        self.change({"src/tests/case.rs": "mod stray;\n",
                     "src/tests/case/stray.rs": "", "src/tests/stray.rs": ""})
        self.assertEqual(self.repo.newly_unsealed(), {"src/tests/case/stray.rs"})

    # --- lexing ------------------------------------------------------------

    def test_comment_markers_inside_strings_do_not_hide_a_module(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({
            "src/lib.rs": 'const A: &str = "src/*"; mod first;\n'
                          'const B: &str = "http://x"; mod second;\n'
                          'const C: &str = r#"*/ //"#; mod third;\n'
                          "const D: char = '\"'; mod fourth;\n"
                          'const E: &str = r#"a"b /* "#; mod fifth;\n'
                          "const F: char = '\\''; mod sixth; // */\n"
                          # An escaped quote in a char literal must not open a string
                          # that runs to the end of the file.
                          "const G: char = '\\\"'; mod seventh;\n",
            "src/first.rs": "", "src/second.rs": "", "src/third.rs": "", "src/fourth.rs": "",
            "src/fifth.rs": "", "src/sixth.rs": "", "src/seventh.rs": "",
        })
        self.assertEqual(self.repo.newly_unsealed(),
                         {"src/first.rs", "src/second.rs", "src/third.rs", "src/fourth.rs",
                          "src/fifth.rs", "src/sixth.rs", "src/seventh.rs"})

    def test_an_attribute_containing_a_bracket_still_classifies_the_module(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.change({"src/lib.rs": '#[doc = "a]"]\n#[cfg(test)]\nmod tests;\n', "src/tests.rs": ""})
        # Found, and classified: a parser that loses the module entirely would
        # also report no production module, so assert the test-only count.
        self.assertEqual(self.repo.newly_unsealed(), set())
        self.assertIn("  newly unsealed test-only modules: 1 (not listed; unpinned tests are the norm)",
                      self.repo.report())

    # --- it never blocks ---------------------------------------------------

    def test_no_merge_base_is_reported_not_raised(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.assertEqual(report_module.report(self.repo.root, "surface.json", "no-such-ref"),
                         ["surface coverage: no merge-base with no-such-ref; nothing compared"])

    def test_a_surface_absent_at_the_merge_base_is_reported_not_raised(self):
        self.repo.write({"Cargo.toml": CARGO, "src/lib.rs": ""})
        self.repo.commit("base without a surface")
        self.repo.git("checkout", "-q", "-b", "work")
        self.repo.pin("src/lib.rs")
        self.repo.commit("surface")
        self.assertEqual(self.repo.report(),
                         ["surface coverage: surface.json absent at merge-base; nothing compared"])

    def test_the_command_accepts_the_surface_path_the_hook_passes(self):
        # reseal.sh passes an absolute path; a person may pass one relative to
        # where they stand. Both must reach the same comparison.
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        # Run from a subdirectory, so a relative path only works if it is taken
        # relative to the caller rather than to --root.
        for surface in (str(Path(self.repo.root, "surface.json")), "../surface.json"):
            out = io.StringIO()
            with contextlib.redirect_stdout(out), contextlib.chdir(Path(self.repo.root, "src")):
                code = report_module.main(["--root", self.repo.root, "--surface", surface,
                                           "--base", "main"])
            self.assertEqual((code, out.getvalue().strip()), (0, CLEAN), surface)

    def test_the_command_exits_zero_even_when_the_report_breaks(self):
        self.base({"src/lib.rs": ""}, ("src/lib.rs",))
        self.repo.write({"surface.json": "not json"})
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = report_module.main(["--root", self.repo.root, "--surface", "surface.json",
                                       "--base", "main"])
        self.assertEqual(code, 0)
        self.assertIn("surface coverage: report unavailable", out.getvalue())


if __name__ == "__main__":
    unittest.main(verbosity=1)
