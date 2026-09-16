#!/usr/bin/env python3
"""Report two ways code leaves the sealed compatibility surface. Never fails.

Checked (bridge#416), comparing the working tree with the merge-base against a
base ref (default `origin/master`):

1. A dropped pin. `rehash-surface` never adds paths, so a conflict resolved by
   taking the base side of `compatibility-surface.json` loses entries a branch
   added, and the gate still passes.
2. A module declared *directly* by a pinned module and left unpinned, where it
   is a new file, a file newly attached to that pinned module, or a file that
   was pinned at the base.

Not checked -- a clean report is not evidence that nothing left the seal:

- code moved between files that already existed, in any direction;
- a module declared by an unpinned module, even when its code came out of a
  pinned file (e.g. a new file under an unpinned `db/mod.rs`), and any deeper
  descendant of a pinned module;
- a pinned file that stops being compiled because its declaration was removed;
- a module that was test-only or feature-gated at the base becoming production;
- a new crate root.

Only what is new on the branch is printed, so modules left unpinned before it
are not reprinted on every reseal. It decides nothing: a person reads it and
either pins the file or leaves it.

The module graph follows rustc's rules for out-of-line modules, walked from
each crate root: a bare `mod name;` resolves beside a file that owns its
directory -- a crate root, a `mod.rs`, or a file loaded through `#[path]` -- and
under `<stem>/` otherwise; `#[path]` resolves beside the declaring file. When
checked for bridge#436 it reached exactly the file sets rustc compiled for each
crate in this repository, across the build profiles compared. It does
not handle `#[cfg_attr(..., path = ...)]`, `mod r#name;`, `include!`,
macro-generated modules, `#[path]` on an inline module, raw-string `#[path]`
values or inner `#![cfg]`; none occurs here today. A `cfg` it cannot evaluate
leaves the module classified as production, which can add noise but does not
hide a module.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import PurePosixPath
from typing import NamedTuple

IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
MOD_ITEM = re.compile(
    r"(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?P<end>[;{])"
)
CRATE_TARGET_DIRS = {"tests", "benches", "examples"}
RAW_STRING = re.compile(r'b?r(#*)"')
CHAR_LITERAL = re.compile(r"b?'(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]+\}|.)|[^\\'\n])'")


def git(root: str, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", root, *args], check=True, capture_output=True, text=True
    ).stdout


class Tree:
    """File access over the working tree, or over one git revision."""

    def __init__(self, root: str, rev: str | None = None):
        self.root, self.rev = root, rev
        if rev is None:
            listed = git(root, "ls-files", "--cached", "--others", "--exclude-standard")
            self.paths = {p for p in listed.splitlines() if os.path.isfile(os.path.join(root, p))}
        else:
            self.paths = set(git(root, "ls-tree", "-r", "--name-only", rev).splitlines())

    def exists(self, path: str) -> bool:
        return path in self.paths

    def read(self, path: str) -> str:
        if self.rev is None:
            with open(os.path.join(self.root, path), encoding="utf-8", errors="replace") as handle:
                return handle.read()
        return git(self.root, "show", f"{self.rev}:{path}")


# --- lexing ---------------------------------------------------------------


def skip_string(text: str, i: int) -> int:
    """Index just past a string or char literal starting at `i`, else `i`."""
    raw = RAW_STRING.match(text, i)
    if raw:
        closing = '"' + raw.group(1)
        end = text.find(closing, raw.end())
        return len(text) if end < 0 else end + len(closing)
    if text.startswith('"', i) or text.startswith('b"', i):
        j = i + (2 if text[i] == "b" else 1)
        while j < len(text):
            if text[j] == "\\":
                j += 2
            elif text[j] == '"':
                return j + 1
            else:
                j += 1
        return len(text)
    char = CHAR_LITERAL.match(text, i)
    if char:
        return char.end()
    return i


def strip_comments(source: str) -> str:
    """Blank out comments, leaving string literals intact and positions stable."""
    out, i = [], 0
    while i < len(source):
        end = skip_string(source, i)
        if end != i:
            out.append(source[i:end])
            i = end
        elif source.startswith("//", i):
            end = source.find("\n", i)
            end = len(source) if end < 0 else end
            out.append(" " * (end - i))
            i = end
        elif source.startswith("/*", i):
            depth, j = 1, i + 2
            while j < len(source) and depth:
                if source.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif source.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out.append(re.sub(r"[^\n]", " ", source[i:j]))
            i = j
        else:
            out.append(source[i])
            i += 1
    return "".join(out)


def read_balanced(text: str, i: int, open_: str, close: str) -> int:
    """Index just past the bracket group opened at `text[i] == open_`."""
    depth, j = 0, i
    while j < len(text):
        end = skip_string(text, j)
        if end != j:
            j = end
            continue
        if text[j] == open_:
            depth += 1
        elif text[j] == close:
            depth -= 1
            if depth == 0:
                return j + 1
        j += 1
    return len(text)


class Declaration(NamedTuple):
    name: str
    attrs: tuple[str, ...]
    inline: tuple[str, ...]
    inline_attrs: tuple[str, ...]


def declarations(source: str) -> list[Declaration]:
    """Out-of-line `mod name;` items, with their outer attributes and the inline
    modules they sit inside."""
    text = strip_comments(source)
    found, attrs, inline, depth = [], [], [], 0
    i = 0
    while i < len(text):
        ch = text[i]
        if ch.isspace():
            i += 1
            continue
        end = skip_string(text, i)
        if end != i:
            attrs, i = [], end
            continue
        if text.startswith("#![", i):
            i = read_balanced(text, i + 2, "[", "]")
            continue
        if text.startswith("#[", i):
            close = read_balanced(text, i + 1, "[", "]")
            attrs.append(text[i + 2 : close - 1])
            i = close
            continue
        item = MOD_ITEM.match(text, i)
        if item and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            if item.group("end") == ";":
                found.append(Declaration(
                    item.group("name"), tuple(attrs),
                    tuple(n for n, _, _ in inline), tuple(a for _, _, xs in inline for a in xs)))
            else:
                depth += 1
                inline.append((item.group("name"), depth, tuple(attrs)))
            attrs, i = [], item.end()
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            while inline and inline[-1][1] == depth:
                inline.pop()
            depth -= 1
        word = IDENT.match(text, i)
        attrs = []
        i = word.end() if word else i + 1
    return found


# --- cfg ------------------------------------------------------------------


def parse_cfg(text: str):
    """`cfg(...)` body -> nested tuples: ('name', x) | ('kv', k) | (op, [..])."""
    pos = 0

    def ws():
        nonlocal pos
        while pos < len(text) and text[pos].isspace():
            pos += 1

    def predicate():
        nonlocal pos
        ws()
        word = IDENT.match(text, pos)
        if not word:
            raise ValueError(text)
        name, pos = word.group(0), word.end()
        ws()
        if pos < len(text) and text[pos] == "(":
            pos += 1
            items = []
            ws()
            while pos < len(text) and text[pos] != ")":
                items.append(predicate())
                ws()
                if pos < len(text) and text[pos] == ",":
                    pos += 1
                    ws()
            pos += 1
            return (name, items)
        if pos < len(text) and text[pos] == "=":
            pos += 1
            ws()
            pos = skip_string(text, pos)
            return ("kv", name)
        return ("name", name)

    return predicate()


def implies(pred, test) -> bool:
    kind, value = pred
    if kind == "all":
        return any(implies(p, test) for p in value)
    if kind == "any":
        return bool(value) and all(implies(p, test) for p in value)
    if kind in ("not",) or not isinstance(value, str):
        return False
    return test(kind, value)


def cfg_predicates(attrs) -> list:
    preds = []
    for attr in attrs:
        match = re.match(r"\s*cfg\s*\(", attr)
        if match:
            try:
                preds.append(parse_cfg(attr[match.end() : attr.rindex(")")]))
            except ValueError:
                pass
    return preds


def path_attr(attrs) -> str | None:
    for attr in attrs:
        match = re.match(r'\s*path\s*=\s*"([^"]*)"\s*$', attr)
        if match:
            return match.group(1)
    return None


# --- module graph ---------------------------------------------------------


class Module(NamedTuple):
    path: str
    parent: str | None
    test_only: bool
    feature_gated: bool


def crate_roots(tree: Tree) -> list[str]:
    """Files cargo compiles as crate roots, which own their directory."""
    roots = set()
    for manifest in sorted(p for p in tree.paths if PurePosixPath(p).name == "Cargo.toml"):
        crate = PurePosixPath(manifest).parent
        prefix = "" if str(crate) == "." else f"{crate}/"
        for default in ("src/lib.rs", "src/main.rs", "build.rs"):
            if tree.exists(prefix + default):
                roots.add(prefix + default)
        for path in tree.paths:
            if not (path.startswith(prefix) and path.endswith(".rs")):
                continue
            rel = PurePosixPath(path[len(prefix):]).parts
            if rel[:2] == ("src", "bin") and (len(rel) == 3 or (len(rel) == 4 and rel[3] == "main.rs")):
                roots.add(path)
            elif rel[0] in CRATE_TARGET_DIRS and (len(rel) == 2 or (len(rel) == 3 and rel[2] == "main.rs")):
                roots.add(path)
        for value in re.findall(r'^\s*path\s*=\s*"([^"]+\.rs)"', tree.read(manifest), re.M):
            candidate = os.path.normpath(prefix + value)
            if tree.exists(candidate):
                roots.add(candidate)
    return sorted(roots)


def module_graph(tree: Tree) -> dict[str, Module]:
    """Every module file reachable from a crate root, keyed by path."""
    modules: dict[str, Module] = {}
    stack = [(root, None, True, False, False) for root in crate_roots(tree)]
    while stack:
        path, parent, owns_dir, test_only, gated = stack.pop()
        seen = modules.get(path)
        if not tree.exists(path) or (seen and (seen.test_only <= test_only)):
            continue
        modules[path] = Module(path, parent, test_only, gated)
        here = PurePosixPath(path).parent
        base = here if owns_dir else here / PurePosixPath(path).stem
        for decl in declarations(tree.read(path)):
            preds = cfg_predicates(decl.inline_attrs + decl.attrs)
            child_test = test_only or any(implies(p, lambda k, v: k == "name" and v == "test") for p in preds)
            child_gated = gated or any(implies(p, lambda k, v: k == "kv" and v == "feature") for p in preds)
            explicit = path_attr(decl.attrs)
            if explicit is not None:
                start = here if not decl.inline else base.joinpath(*decl.inline)
                candidates, child_owns = [start / explicit], True
            else:
                nested = base.joinpath(*decl.inline)
                candidates = [nested / f"{decl.name}.rs", nested / decl.name / "mod.rs"]
                child_owns = None
            for candidate in candidates:
                child = os.path.normpath(str(candidate))
                if tree.exists(child):
                    owns = child_owns if child_owns is not None else PurePosixPath(child).name == "mod.rs"
                    stack.append((child, path, owns, child_test, child_gated))
                    break
    return modules


def surface_paths(tree: Tree, surface: str) -> set[str]:
    return {entry["path"] for entry in json.loads(tree.read(surface))["files"]}


def unpinned_under_pinned(tree: Tree, surface: str) -> dict[str, Module]:
    pinned = surface_paths(tree, surface)
    return {
        path: module
        for path, module in module_graph(tree).items()
        if module.parent in pinned and path not in pinned
    }


# --- report ---------------------------------------------------------------


def report(root: str, surface: str, base_ref: str) -> list[str]:
    try:
        base = git(root, "merge-base", "HEAD", base_ref).strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return [f"surface coverage: no merge-base with {base_ref}; nothing compared"]
    now, then = Tree(root), Tree(root, base)
    if not then.exists(surface):
        return [f"surface coverage: {surface} absent at merge-base; nothing compared"]

    pinned_then = surface_paths(then, surface)
    before = unpinned_under_pinned(then, surface)
    lines = [f"  pin removed:            {p}" for p in sorted(pinned_then - surface_paths(now, surface))]
    # Newly unsealed under a pinned module: not already there at the base, and
    # either its parent was pinned at the base (so attaching it moved something
    # out), or it is a new file, or it was itself pinned at the base. An
    # existing unpinned file whose parent this branch pinned did not move -- the
    # seal grew around it.
    added = {
        path: module
        for path, module in unpinned_under_pinned(now, surface).items()
        if path not in before
        and (module.parent in pinned_then or not then.exists(path) or path in pinned_then)
    }
    for path, module in sorted(added.items()):
        if not module.test_only:
            gated = ", behind a cargo feature" if module.feature_gated else ""
            lines.append(f"  newly unsealed module:  {path}  (declared by pinned {module.parent}{gated})")
    tests = sum(1 for module in added.values() if module.test_only)
    if tests:
        lines.append(f"  newly unsealed test-only modules: {tests} (not listed; unpinned tests are the norm)")
    if not lines:
        return [
            f"surface coverage: no dropped pins and no newly unsealed direct children of"
            f" pinned modules since {base_ref} (not a proof nothing left the seal; see"
            " scripts/surface_coverage_report.py)"
        ]
    return [
        f"surface coverage: since {base_ref}, this branch dropped pins or unsealed modules.",
        "  Nothing is refused. Pin each file that decides what Bridge posts or lets leave",
        "  the machine; leave the rest. See bridge#416 and the comment on MAX_SURFACE_FILES.",
        *lines,
    ]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    parser.add_argument("--surface", default="docs/tally/compatibility/compatibility-surface.json")
    parser.add_argument("--base", default=os.environ.get("SURFACE_REPORT_BASE", "origin/master"))
    args = parser.parse_args(argv)
    try:
        surface = args.surface
        if not os.path.isabs(surface) and os.path.exists(surface):
            surface = os.path.abspath(surface)  # relative to the caller's directory
        if os.path.isabs(surface):
            # Compare real paths: on macOS /tmp and /var are symlinks, so an
            # absolute path built from the working directory and a root given
            # through the symlink would otherwise look unrelated.
            surface = os.path.relpath(os.path.realpath(surface), os.path.realpath(args.root))
        if surface.startswith(".."):
            print("surface coverage: surface is outside the repository; nothing compared")
        else:
            print("\n".join(report(args.root, surface, args.base)))
    except Exception as error:  # a report must never block a reseal
        print(f"surface coverage: report unavailable ({type(error).__name__}: {error})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
