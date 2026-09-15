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
// claim_id) -- see reconcileKeyed() below -- using the same stage-1/2/3 read
// the doc already recommends for manual resolution (`git show :1:/:2:/:3:`).
// That correctly handles independent additions (both kept), independent
// removals (both honored), and a change to only one side (that side's value
// wins) entirely automatically. It deliberately refuses to guess only in the
// one case a default cannot be right: the SAME entry changed on both sides
// to DIFFERENT values (e.g. two branches each promote the same claim_id to a
// different level, or edit the same pinned file's surface entry -- for a
// surface entry this is actually always safe since only sha256 differs and
// rehash-surface recomputes it from disk regardless, but the check is kept
// uniform across both files for one auditable code path rather than two).
// On that refusal, it falls back to plain `git merge-file` (git's own
// default three-way text merge) for the one file this invocation is
// responsible for, so the developer sees the ordinary conflict markers and
// resolves it exactly as docs/release-process.md's manual procedure
// describes -- this driver's fast path is additive, not a replacement.

import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const RESEAL = join(here, "reseal.sh");

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

  // For the path THIS invocation is actually responsible for, read straight
  // from the temp files git handed us -- %O/%A/%B are always correct,
  // independent of which git operation (merge, rebase, ...) triggered this,
  // and independent of whether detectMergeRefs() found an ordinary
  // `git merge` in progress. For the OTHER file, there is no equivalent
  // direct source, so it depends on refs being available.
  function readTriple(filePath) {
    if (filePath === path) {
      return {
        base: parseJsonOrNull(readFileSync(baseFile, "utf8")),
        ours: parseJsonOrNull(readFileSync(oursFile, "utf8")),
        theirs: parseJsonOrNull(readFileSync(theirsFile, "utf8")),
      };
    }
    if (!refs) return null;
    return {
      base: parseJsonOrNull(readAtRef(root, refs.base, filePath)),
      ours: parseJsonOrNull(readAtRef(root, refs.ours, filePath)),
      theirs: parseJsonOrNull(readAtRef(root, refs.theirs, filePath)),
    };
  }

  const surfaceTriple = readTriple(SURFACE_REL);
  const matrixTriple = readTriple(MATRIX_REL);

  const usable = (triple) =>
    !!triple &&
    triple.ours !== null &&
    triple.ours !== undefined &&
    triple.theirs !== null &&
    triple.theirs !== undefined;
  const surfaceInputsUsable = usable(surfaceTriple);
  const matrixInputsUsable = usable(matrixTriple);

  // This can only fail for the invoked path itself if its own temp files
  // were not valid JSON (git guarantees they exist); for the other path it
  // means refs weren't available (not an ordinary `git merge`), which is
  // fine -- that file is simply left for its own separate invocation, if
  // any, to handle.
  if (path === SURFACE_REL && !surfaceInputsUsable) {
    warn(`invoked for ${SURFACE_REL} but its own ours/theirs content did not parse as JSON -- falling back`);
    fallbackToPlainMerge(baseFile, oursFile, theirsFile);
    return;
  }
  if (path === MATRIX_REL && !matrixInputsUsable) {
    warn(`invoked for ${MATRIX_REL} but its own ours/theirs content did not parse as JSON -- falling back`);
    fallbackToPlainMerge(baseFile, oursFile, theirsFile);
    return;
  }

  const surfaceBase = surfaceTriple?.base;
  const surfaceOurs = surfaceTriple?.ours;
  const surfaceTheirs = surfaceTriple?.theirs;
  const matrixBase = matrixTriple?.base;
  const matrixOurs = matrixTriple?.ours;
  const matrixTheirs = matrixTriple?.theirs;

  const conflicts = [];
  let reconciledSurfaceFiles = null;
  let reconciledMatrixClaims = null;

  if (surfaceInputsUsable) {
    const { result, conflicts: surfaceConflicts } = reconcileKeyed(
      surfaceBase?.files,
      surfaceOurs.files,
      surfaceTheirs.files,
      (file) => file.path,
    );
    reconciledSurfaceFiles = result;
    conflicts.push(...surfaceConflicts.map((c) => `compatibility-surface.json: ${c.key} -- ${c.reason}`));
  }
  if (matrixInputsUsable) {
    const { result, conflicts: matrixConflicts } = reconcileKeyed(
      matrixBase?.claims,
      matrixOurs.claims,
      matrixTheirs.claims,
      (claim) => claim.claim_id,
    );
    reconciledMatrixClaims = result;
    conflicts.push(...matrixConflicts.map((c) => `compatibility-matrix.json: ${c.key} -- ${c.reason}`));
  }

  if (conflicts.length) {
    process.stderr.write(
      "reseal-merge-driver: refusing to auto-resolve -- the AUTHORED half of these files " +
        "genuinely conflicts (this is exactly the case docs/release-process.md's manual " +
        "reconciliation procedure exists for; a default here could silently drop an entry):\n" +
        conflicts.map((c) => `  - ${c}`).join("\n") +
        `\nFalling back to a plain three-way text merge of ${path} -- resolve the conflict ` +
        "markers by hand, following the procedure in docs/release-process.md, then re-run " +
        "scripts/reseal.sh yourself.\n",
    );
    fallbackToPlainMerge(baseFile, oursFile, theirsFile);
    return;
  }

  // Both AUTHORED halves are now the same regardless of which side originally
  // "won" -- write draft files (correct pin list / claim list, placeholder
  // digests) to the real destinations and let scripts/reseal.sh compute the
  // actual, correct digests from bytes on disk. bridge_commit_sha is a plain
  // scalar reseal.sh does not touch and is not cross-validated against
  // anything (only format-checked) -- "ours" is taken arbitrarily and
  // harmlessly.
  if (reconciledSurfaceFiles) {
    reconciledSurfaceFiles.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    const draft = {
      schema_version: surfaceOurs.schema_version,
      files: reconciledSurfaceFiles,
      manifest_sha256: "",
    };
    writeFileSync(join(root, SURFACE_REL), `${JSON.stringify(draft, null, 2)}\n`);
  }
  if (reconciledMatrixClaims) {
    reconciledMatrixClaims.sort((a, b) => (a.claim_id < b.claim_id ? -1 : a.claim_id > b.claim_id ? 1 : 0));
    const draft = {
      schema_version: matrixOurs.schema_version,
      bridge_commit_sha: matrixOurs.bridge_commit_sha,
      compatibility_surface_sha256: "0".repeat(64), // repoint-matrix overwrites this
      claims: reconciledMatrixClaims,
    };
    writeFileSync(join(root, MATRIX_REL), `${JSON.stringify(draft, null, 2)}\n`);
  }

  // The draft written above always starts with manifest_sha256 (and the
  // matrix's placeholder compatibility_surface_sha256) empty -- reseal.sh's
  // ordinary order requires an already-valid checksum to rehash from (see
  // its own header), so an unsealed draft needs --pins-changed's inverted
  // order (seal first to attest the reconciled list, then the ordinary
  // three) every time this driver runs it, regardless of whether the pin
  // *list* actually changed on either side.
  const reseal = spawnSync("bash", [RESEAL, "--pins-changed"], {
    cwd: root,
    encoding: "utf8",
    stdio: "inherit",
  });
  if (reseal.status !== 0) {
    fail(
      "the reconciled pin/claim list failed to reseal -- see scripts/reseal.sh's output above. " +
        "The working tree now holds the reconciled-but-unsealed draft; fix the underlying problem " +
        "(e.g. a pinned file genuinely missing) and re-run scripts/reseal.sh, or resolve manually.",
    );
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
