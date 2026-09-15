#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// git merge driver for docs/tally/compatibility/compatibility-surface.json
// and compatibility-matrix.json. Wired via .gitattributes plus a one-time
// `git config` line documented in docs/release-process.md -- see that doc's
// "Merge driver (local only)" section, and READ THE WARNING THERE: this only
// affects merges performed locally with this repo's git config set. GitHub's
// server-side merge (the "Merge pull request" button, and the merge it
// computes to evaluate a PR's mergeability) does NOT run a repository's
// custom merge drivers at all -- GitHub has no mechanism to run arbitrary
// code as part of that merge. So this reduces LOCAL merge/rebase pain for
// maintainers who juggle multiple compatibility-surface branches; it does
// not touch what a PR shows as conflicting on GitHub, and does not replace
// the manual reconciliation procedure in docs/release-process.md for the
// cases it declines to resolve (see below).
//
// git invokes this as (configured exactly this way by the documented
// `git config` line):
//   node scripts/reseal-merge-driver.mjs %O %A %B %P
// %O = common ancestor's blob (temp file), %A = "ours" blob (temp file --
// git expects the resolved content written back into THIS path), %B =
// "theirs" blob (temp file), %P = the path being merged (one of the two
// files above; this script is invoked once per conflicting path, in an
// order git does not document or guarantee).
//
// WHY THIS IS SAFE FOR THE COMMON CASE, AND WHY IT REFUSES THE OTHERS
// --------------------------------------------------------------------
// docs/release-process.md ("When the surface itself conflicts in a merge or
// rebase") is explicit that these two files have a DERIVED half (every
// sha256, manifest_sha256, compatibility_surface_sha256 -- safe to
// regenerate) and an AUTHORED half (the surface's pin list, the matrix's
// claims) that "nothing regenerates" and that a blind "take one side" can
// silently drop. Taking either side wholesale and resealing -- literally
// what was asked for -- is correct for the derived half (the reseal
// recomputes every digest from actual file bytes on disk, so whichever
// side's stale digest value you started from is irrelevant) but would
// reproduce exactly the silent-drop hazard that doc warns about for the
// authored half if implemented as a blind pick.
//
// So this driver does a real three-way reconciliation of the two AUTHORED
// lists first (surface.files, keyed by path; matrix.claims, keyed by
// claim_id) -- see reconcileKeyed() below. That correctly handles
// independent additions (both kept), independent removals (both honored),
// and a change to only one side (that side's value wins) entirely
// automatically. It deliberately refuses to guess only in the one case a
// default cannot be right: the SAME entry changed on both sides to
// DIFFERENT values (e.g. two branches each promote the same claim_id to a
// different level).
//
// Reconciling the pin LIST is not the whole job: each pinned file's own
// CONTENT hash also has to be correct post-merge, and the first version of
// this driver got that part wrong. It ran the tool's rehash-surface
// subcommand against the working tree, and a real two-branch merge
// experiment (see docs/release-process.md and the PR this shipped in) caught
// it sealing a WRONG hash -- git does not guarantee every other path has
// already been checked out to its final post-merge content by the time this
// driver runs, so rehashing from disk mid-merge is a genuine race, not
// merely inelegant. computeCorrectSurfaceFiles() below reads git refs
// (base/ours/theirs) instead, which depends only on the commit objects
// involved and cannot exhibit that race; a pinned file both sides changed to
// DIFFERENT content is, like a claim conflict, refused rather than guessed.
//
// On any refusal, this falls back to plain `git merge-file` (git's own
// default three-way text merge) for the one file this invocation is
// responsible for, so the developer sees the ordinary conflict markers and
// resolves it exactly as docs/release-process.md's manual procedure
// describes -- this driver's fast path is additive, not a replacement.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

const SURFACE_REL = "docs/tally/compatibility/compatibility-surface.json";
const MATRIX_REL = "docs/tally/compatibility/compatibility-matrix.json";

function fail(message) {
  process.stderr.write(`reseal-merge-driver: ${message}\n`);
  process.exit(2);
}

function warn(message) {
  process.stderr.write(`reseal-merge-driver: ${message}\n`);
}

function repoRoot() {
  const result = spawnSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" });
  if (result.status !== 0) fail("not inside a git repository (git rev-parse failed)");
  return result.stdout.trim();
}

// Index stages 1/2/3 are NOT a reliable source here: git only populates them
// for a path AFTER a configured merge driver has run and reported failure
// (or after the whole merge otherwise leaves it conflicted) -- they are not
// yet present while THIS process is running, which is exactly the case
// docs/release-process.md's own stage-reading advice is written for a human
// resolving a conflict *after* git has stopped, not for a driver running
// *during* the merge. So this reads "ours" / "theirs" / "base" from refs
// instead: HEAD, .git/MERGE_HEAD (present for the duration of an ordinary
// `git merge`), and their merge-base. That works for a plain `git merge`
// (the case this driver, and the merge-experiment it was proven against,
// targets); a rebase's REBASE_HEAD or a cherry-pick's CHERRY_PICK_HEAD are
// different plumbing this driver does not attempt to read, so
// detectMergeRefs() returning null degrades to per-file-only handling (see
// the two "*InputsUsable" checks below), never to a silent wrong answer.
function detectMergeRefs(root) {
  let theirs;
  try {
    theirs = readFileSync(join(root, ".git", "MERGE_HEAD"), "utf8").trim();
  } catch {
    return null; // not an ordinary `git merge` in progress
  }
  const headResult = spawnSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" });
  if (headResult.status !== 0) return null;
  const ours = headResult.stdout.trim();
  const baseResult = spawnSync("git", ["merge-base", ours, theirs], { cwd: root, encoding: "utf8" });
  if (baseResult.status !== 0) return null;
  return { ours, theirs, base: baseResult.stdout.trim() };
}

function readAtRef(root, ref, path) {
  const result = spawnSync("git", ["show", `${ref}:${path}`], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  return result.status === 0 ? result.stdout : null; // null: absent at that ref (e.g. file added after base)
}

// Same as readAtRef but returns raw bytes (no encoding option -> Buffer),
// because this one feeds sha256Hex(): decoding through UTF-8 first and
// re-encoding would corrupt a pinned file that isn't valid UTF-8, and would
// silently produce a WRONG hash (not an error) for one that is, since
// Node's UTF-8 decode/encode round-trip is not always byte-identical (e.g.
// for lone surrogates or overlong sequences). sha256_file() in
// tools/bridge-tally-compatibility/src/lib.rs hashes raw bytes; this must
// match it exactly or a "correctly" resealed surface would still fail
// validate_files's byte-for-byte check.
function readAtRefBuffer(root, ref, path) {
  const result = spawnSync("git", ["show", `${ref}:${path}`], {
    cwd: root,
    maxBuffer: 64 * 1024 * 1024,
  });
  return result.status === 0 ? result.stdout : null;
}

function changedPathSet(root, fromRef, toRef) {
  const result = spawnSync("git", ["diff", "--name-only", fromRef, toRef], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.status !== 0) return null;
  return new Set(result.stdout.split("\n").filter(Boolean));
}

function sha256Hex(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

// Computes each pinned file's CORRECT post-merge sha256 directly from git
// refs -- never from the working tree. This is not an optimization: an
// earlier version of this driver ran the tool's own rehash-surface
// subcommand against the working tree mid-merge, and a real two-branch
// merge experiment caught it producing a WRONG hash for a pinned file that
// only one side had touched. git does not guarantee that every other path's
// final content has already been checked out to disk by the time this
// driver runs for compatibility-surface.json -- alphabetical tree-walk order
// is not a documented guarantee, and in that experiment the file being
// hashed had not yet been updated to its post-merge content when
// rehash-surface read it, so a stale hash got sealed into a manifest that
// otherwise looked completely valid. Reading `git diff --name-only` and
// `git show <ref>:<path>` instead depends only on the commit objects
// involved, never on working-tree checkout timing, so it cannot exhibit that
// race. For a path neither side touched, the previously-sealed hash is kept
// as-is (nothing changed, nothing to recompute); for a path exactly one side
// touched, that side's content is hashed; for a path BOTH sides touched,
// their content is compared byte-for-byte -- identical changes are fine,
// different ones are a genuine conflict on that pinned file's own content
// (not on the surface list) and are refused rather than guessed at, exactly
// like reconcileKeyed()'s refusal above.
function computeCorrectSurfaceFiles(root, refs, files) {
  const oursChanged = changedPathSet(root, refs.base, refs.ours);
  const theirsChanged = changedPathSet(root, refs.base, refs.theirs);
  if (!oursChanged || !theirsChanged) return null;

  const finalFiles = [];
  const conflicts = [];
  for (const file of files) {
    const inOurs = oursChanged.has(file.path);
    const inTheirs = theirsChanged.has(file.path);

    if (!inOurs && !inTheirs) {
      finalFiles.push(file); // untouched by either side -- its recorded hash is still correct
      continue;
    }
    if (inOurs && inTheirs) {
      const oursBuf = readAtRefBuffer(root, refs.ours, file.path);
      const theirsBuf = readAtRefBuffer(root, refs.theirs, file.path);
      if (oursBuf === null || theirsBuf === null) {
        conflicts.push({ key: file.path, reason: "pinned file missing at ours or theirs after both sides changed it" });
        continue;
      }
      if (!oursBuf.equals(theirsBuf)) {
        conflicts.push({ key: file.path, reason: "both sides changed this pinned file's actual content differently" });
        continue;
      }
      finalFiles.push({ path: file.path, sha256: sha256Hex(oursBuf) });
      continue;
    }
    const ref = inOurs ? refs.ours : refs.theirs;
    const buf = readAtRefBuffer(root, ref, file.path);
    if (buf === null) {
      conflicts.push({
        key: file.path,
        reason: `pinned file missing at ${inOurs ? "ours" : "theirs"} after it changed there`,
      });
      continue;
    }
    finalFiles.push({ path: file.path, sha256: sha256Hex(buf) });
  }
  return { finalFiles, conflicts };
}

// Resolves the pinned toolchain the same way scripts/reseal.sh does (see its
// own header): a Homebrew/system rustc earlier on PATH shadows rustup, so
// the toolchain's own bin directory has to go in front of PATH, not just
// RUSTC/RUSTDOC.
function pinnedToolchainEnv(root) {
  const tomlText = readFileSync(join(root, "rust-toolchain.toml"), "utf8");
  const match = /^channel *= *"(.*)"/m.exec(tomlText);
  if (!match) fail("could not read [toolchain].channel from rust-toolchain.toml");
  const channel = match[1];
  const which = (bin) => {
    const result = spawnSync("rustup", ["which", "--toolchain", channel, bin], { encoding: "utf8" });
    if (result.status !== 0) fail(`rustup could not resolve ${bin} for toolchain ${channel} -- is it installed?`);
    return result.stdout.trim();
  };
  const rustc = which("rustc");
  return {
    ...process.env,
    PATH: `${dirname(rustc)}:${process.env.PATH ?? ""}`,
    RUSTC: rustc,
    RUSTDOC: which("rustdoc"),
  };
}

// Runs one bridge-tally-compatibility subcommand directly (seal-surface,
// repoint-matrix) -- NOT via scripts/reseal.sh, and deliberately never
// rehash-surface: rehash-surface reads pinned files from the *working tree*,
// which is exactly what computeCorrectSurfaceFiles() above exists to avoid
// depending on mid-merge.
function runCompatTool(root, env, args) {
  const result = spawnSync("cargo", ["run", "--locked", "-p", "bridge-tally-compatibility", "--", ...args], {
    cwd: join(root, "tools"),
    env,
    stdio: "inherit",
  });
  return result.status === 0;
}

function parseJsonOrNull(text) {
  if (text === null) return null;
  try {
    return JSON.parse(text);
  } catch {
    return undefined; // present but unparseable -- caller must treat as unsafe
  }
}

function deepEqual(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

// Three-way reconciliation of a JSON array of objects, keyed by keyOf(item).
// See the file header for the exact cases this resolves automatically versus
// defers as a genuine conflict.
function reconcileKeyed(baseList, oursList, theirsList, keyOf) {
  const toMap = (list) => new Map((list ?? []).map((item) => [keyOf(item), item]));
  const b = toMap(baseList);
  const o = toMap(oursList);
  const t = toMap(theirsList);
  const keys = new Set([...b.keys(), ...o.keys(), ...t.keys()]);
  const result = [];
  const conflicts = [];

  for (const key of keys) {
    const inB = b.has(key);
    const inO = o.has(key);
    const inT = t.has(key);
    const bv = b.get(key);
    const ov = o.get(key);
    const tv = t.get(key);

    if (!inB) {
      // A pure addition (not present at the merge base at all).
      if (inO && inT) {
        if (deepEqual(ov, tv)) result.push(ov);
        else conflicts.push({ key, reason: "both sides added this entry with different content" });
      } else if (inO) {
        result.push(ov);
      } else if (inT) {
        result.push(tv);
      }
      continue;
    }

    const oUnchanged = inO && deepEqual(ov, bv);
    const tUnchanged = inT && deepEqual(tv, bv);

    if (!inO && !inT) continue; // removed by both
    if (!inO) {
      if (tUnchanged) continue; // ours removed it, theirs left it alone -- honor the removal
      conflicts.push({ key, reason: "removed by ours, modified by theirs" });
      continue;
    }
    if (!inT) {
      if (oUnchanged) continue; // theirs removed it, ours left it alone -- honor the removal
      conflicts.push({ key, reason: "removed by theirs, modified by ours" });
      continue;
    }
    // Present (possibly modified) on both sides.
    if (deepEqual(ov, tv)) {
      result.push(ov);
    } else if (oUnchanged) {
      result.push(tv); // only theirs changed it
    } else if (tUnchanged) {
      result.push(ov); // only ours changed it
    } else {
      conflicts.push({ key, reason: "both sides modified this entry differently" });
    }
  }

  return { result, conflicts };
}

// Falls back to git's own default three-way text merge for the single file
// this invocation is responsible for (%O/%A/%B, already on disk as the temp
// files git handed us). Used whenever reconciliation found a genuine
// conflict this driver should not paper over.
function fallbackToPlainMerge(baseFile, oursFile, theirsFile) {
  const result = spawnSync("git", ["merge-file", oursFile, baseFile, theirsFile], {
    encoding: "utf8",
  });
  process.exit(result.status === null ? 1 : result.status);
}

function main() {
  const [baseFile, oursFile, theirsFile, path] = process.argv.slice(2);
  if (!baseFile || !oursFile || !theirsFile || !path) {
    fail("expected 4 arguments: %O %A %B %P (see the git config line in docs/release-process.md)");
  }
  if (path !== SURFACE_REL && path !== MATRIX_REL) {
    fail(
      `configured for ${SURFACE_REL} and ${MATRIX_REL} only, but was invoked for ${path} -- check the .gitattributes entry`,
    );
  }

  const root = repoRoot();
  const refs = detectMergeRefs(root);

  // Everything below -- the cross-file reconciliation AND the ref-based
  // hashing in computeCorrectSurfaceFiles() -- depends on knowing base/ours/
  // theirs as commits, not just as this invocation's own three temp files.
  // Without that (a rebase, cherry-pick, or anything else that doesn't leave
  // .git/MERGE_HEAD), there is no safe fast path: fall straight back to a
  // plain three-way text merge of the one file this invocation is
  // responsible for.
  if (!refs) {
    warn(
      "no .git/MERGE_HEAD (not an ordinary `git merge`) -- this driver only auto-resolves that case; falling back",
    );
    fallbackToPlainMerge(baseFile, oursFile, theirsFile);
    return;
  }

  const readTriple = (filePath) => ({
    base: parseJsonOrNull(readAtRef(root, refs.base, filePath)),
    ours: parseJsonOrNull(readAtRef(root, refs.ours, filePath)),
    theirs: parseJsonOrNull(readAtRef(root, refs.theirs, filePath)),
  });

  const surfaceTriple = readTriple(SURFACE_REL);
  const matrixTriple = readTriple(MATRIX_REL);

  const usable = (triple) =>
    triple.ours !== null && triple.ours !== undefined && triple.theirs !== null && triple.theirs !== undefined;
  const surfaceInputsUsable = usable(surfaceTriple);
  const matrixInputsUsable = usable(matrixTriple);

  // The invoked path's own ours/theirs must be readable -- git guarantees
  // the path exists at both HEAD and MERGE_HEAD when it invokes this driver
  // for a content conflict on it, so this failing means something odd (a
  // detached/unusual history); refuse rather than guess.
  if (path === SURFACE_REL && !surfaceInputsUsable) {
    warn(`invoked for ${SURFACE_REL} but could not read it at both refs -- falling back`);
    fallbackToPlainMerge(baseFile, oursFile, theirsFile);
    return;
  }
  if (path === MATRIX_REL && !matrixInputsUsable) {
    warn(`invoked for ${MATRIX_REL} but could not read it at both refs -- falling back`);
    fallbackToPlainMerge(baseFile, oursFile, theirsFile);
    return;
  }

  const conflicts = [];
  let reconciledSurfaceFiles = null;
  let reconciledMatrixClaims = null;

  if (surfaceInputsUsable) {
    const { result, conflicts: surfaceConflicts } = reconcileKeyed(
      surfaceTriple.base?.files,
      surfaceTriple.ours.files,
      surfaceTriple.theirs.files,
      (file) => file.path,
    );
    reconciledSurfaceFiles = result;
    conflicts.push(...surfaceConflicts.map((c) => `compatibility-surface.json pin list: ${c.key} -- ${c.reason}`));

    // The pin LIST reconciled cleanly above; separately, each pinned file's
    // own CONTENT must also be reconciled -- see computeCorrectSurfaceFiles's
    // header for why this reads git refs and never the working tree.
    if (!conflicts.length) {
      const hashed = computeCorrectSurfaceFiles(root, refs, reconciledSurfaceFiles);
      if (!hashed) {
        conflicts.push("compatibility-surface.json: could not diff base..ours/theirs to hash pinned files");
      } else {
        reconciledSurfaceFiles = hashed.finalFiles;
        conflicts.push(...hashed.conflicts.map((c) => `compatibility-surface.json content: ${c.key} -- ${c.reason}`));
      }
    }
  }
  if (matrixInputsUsable) {
    const { result, conflicts: matrixConflicts } = reconcileKeyed(
      matrixTriple.base?.claims,
      matrixTriple.ours.claims,
      matrixTriple.theirs.claims,
      (claim) => claim.claim_id,
    );
    reconciledMatrixClaims = result;
    conflicts.push(...matrixConflicts.map((c) => `compatibility-matrix.json: ${c.key} -- ${c.reason}`));
  }

  if (conflicts.length) {
    process.stderr.write(
      "reseal-merge-driver: refusing to auto-resolve -- a genuine conflict exists that a default " +
        "could silently get wrong (this is exactly the case docs/release-process.md's manual " +
        "reconciliation procedure exists for):\n" +
        conflicts.map((c) => `  - ${c}`).join("\n") +
        `\nFalling back to a plain three-way text merge of ${path} -- resolve the conflict ` +
        "markers by hand, following the procedure in docs/release-process.md, then re-run " +
        "scripts/reseal.sh yourself.\n",
    );
    fallbackToPlainMerge(baseFile, oursFile, theirsFile);
    return;
  }

  // Every pinned file's hash is now already CORRECT (computed from refs
  // above, not from disk), so sealing is the only remaining step -- no
  // rehash-surface, and so no dependency on working-tree checkout timing.
  const env = pinnedToolchainEnv(root);
  const surfacePath = join(root, SURFACE_REL);
  const matrixPath = join(root, MATRIX_REL);

  if (reconciledSurfaceFiles) {
    reconciledSurfaceFiles.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    const draft = {
      schema_version: surfaceTriple.ours.schema_version,
      files: reconciledSurfaceFiles,
      manifest_sha256: "",
    };
    writeFileSync(surfacePath, `${JSON.stringify(draft, null, 2)}\n`);
    if (!runCompatTool(root, env, ["seal-surface", surfacePath, "--output", surfacePath])) {
      fail("seal-surface failed on the reconciled surface -- see output above");
    }
  }
  if (reconciledMatrixClaims) {
    reconciledMatrixClaims.sort((a, b) => (a.claim_id < b.claim_id ? -1 : a.claim_id > b.claim_id ? 1 : 0));
    // bridge_commit_sha is a plain scalar neither seal-surface, repoint-matrix
    // nor reconcileKeyed touch or cross-validate (only format-checked) --
    // "ours" is taken arbitrarily and harmlessly.
    const draft = {
      schema_version: matrixTriple.ours.schema_version,
      bridge_commit_sha: matrixTriple.ours.bridge_commit_sha,
      compatibility_surface_sha256: "0".repeat(64), // repoint-matrix overwrites this next
      claims: reconciledMatrixClaims,
    };
    writeFileSync(matrixPath, `${JSON.stringify(draft, null, 2)}\n`);
  }
  if (reconciledSurfaceFiles || reconciledMatrixClaims) {
    // Repoint regardless of which of the two changed in THIS invocation: the
    // real surface.json on disk is, by this point, always either what this
    // invocation just wrote above or already the final correct content from
    // this file's own separate invocation (or untouched, if genuinely
    // unaffected) -- never a stale intermediate, because every invocation
    // that reaches this point recomputes both files from refs rather than
    // trusting whatever the OTHER invocation may or may not have written yet.
    if (!runCompatTool(root, env, ["repoint-matrix", matrixPath, surfacePath, "--output", matrixPath])) {
      fail("repoint-matrix failed -- see output above");
    }
  }

  // Stage both real files explicitly: whichever of the two this invocation's
  // %P was for is also written into %A below so git's own driver-result
  // handling stages it too, but the OTHER file may not have had a pending
  // driver invocation of its own (e.g. only one of the two actually
  // conflicted) and still needs to be added.
  spawnSync("git", ["add", SURFACE_REL, MATRIX_REL], { cwd: root });

  const finalContent = readFileSync(join(root, path), "utf8");
  writeFileSync(oursFile, finalContent);
  process.exit(0);
}

main();
