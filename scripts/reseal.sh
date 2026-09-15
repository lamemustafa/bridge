#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Wraps the three-command compatibility-surface reseal sequence documented in
# docs/release-process.md:
#
#   rehash-surface -> seal-surface -> repoint-matrix
#
# That doc is explicit that running step 2 without step 1 first "produces a
# valid-looking digest over stale source content" -- a silent, order-dependent
# footgun. This script exists so the order cannot be gotten wrong by hand: it
# always runs the three commands together, in the right order for the case at
# hand, and refuses to continue past a failed step (`set -e`).
#
# Read docs/release-process.md before using this for anything beyond a
# routine reseal -- it is not a substitute for that document, only an
# enforcement of the one ordering rule it repeatedly warns is easy to violate.
#
# Usage:
#   scripts/reseal.sh                  Ordinary reseal: the CONTENTS of one or
#                                       more already-pinned files changed, but
#                                       the pin list itself did not.
#   scripts/reseal.sh --pins-changed   The pin LIST changed (an entry was
#                                       added to or removed from
#                                       compatibility-surface.json's `files`).
#                                       Runs the documented inverted sequence:
#                                       seal-surface first (to attest the new
#                                       file list), then the ordinary three.
#                                       See "Adding or removing a pin" in
#                                       docs/release-process.md -- and recompute
#                                       MAX_SURFACE_FILES in
#                                       tools/bridge-tally-compatibility/src/lib.rs
#                                       BEFORE running this, per that doc.
#   scripts/reseal.sh --verify         Reseal into a scratch copy and compare
#                                       byte-for-byte against the committed
#                                       files. Exits non-zero if they differ.
#                                       Never mutates the working tree or the
#                                       committed files. For CI.
#
# Optional, mainly for testing against fixtures instead of the real
# compatibility surface:
#   --root DIR       Repository root that pinned file paths are read relative
#                     to (default: this repo's own root).
#   --surface FILE    Path to the surface manifest to reseal (default:
#                     docs/tally/compatibility/compatibility-surface.json).
#   --matrix FILE     Path to the matrix manifest to repoint (default:
#                     docs/tally/compatibility/compatibility-matrix.json).
#
# Never hand-edit compatibility-surface.json or compatibility-matrix.json --
# this script (or a direct call to the tool it wraps) is the only supported
# way to change them.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TOOLS_DIR="$PROJECT_ROOT/tools"

MODE="reseal"
PIN_ROOT="$PROJECT_ROOT"
SURFACE="$PROJECT_ROOT/docs/tally/compatibility/compatibility-surface.json"
MATRIX="$PROJECT_ROOT/docs/tally/compatibility/compatibility-matrix.json"

print_usage() {
  sed -n '2,45p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --pins-changed)
      MODE="pins-changed"
      shift
      ;;
    --verify)
      MODE="verify"
      shift
      ;;
    --root)
      [ "$#" -ge 2 ] || { echo "reseal.sh: --root requires a value" >&2; exit 2; }
      PIN_ROOT="$(cd "$2" && pwd)"
      shift 2
      ;;
    --surface)
      [ "$#" -ge 2 ] || { echo "reseal.sh: --surface requires a value" >&2; exit 2; }
      SURFACE="$2"
      shift 2
      ;;
    --matrix)
      [ "$#" -ge 2 ] || { echo "reseal.sh: --matrix requires a value" >&2; exit 2; }
      MATRIX="$2"
      shift 2
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "reseal.sh: unknown argument: $1" >&2
      print_usage >&2
      exit 2
      ;;
  esac
done

# --- toolchain trap ---
# A Homebrew (or other system) rustc earlier on PATH shadows the pinned
# rustup toolchain. Setting RUSTC alone is not enough (clippy still resolves
# the wrong rustc), so prepend the pinned toolchain's own bin directory to
# PATH, matching docs/release-process.md.
channel="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$PROJECT_ROOT/rust-toolchain.toml")"
if [ -z "$channel" ]; then
  echo "reseal.sh: could not read [toolchain].channel from rust-toolchain.toml" >&2
  exit 2
fi
rustc_path="$(rustup which --toolchain "$channel" rustc)"
export PATH="$(dirname "$rustc_path"):$PATH"
export RUSTC="$rustc_path"
export RUSTDOC="$(rustup which --toolchain "$channel" rustdoc)"
resolved_rustc_version="$(rustc --version)"
echo "reseal.sh: using $resolved_rustc_version" >&2

run_tool() {
  ( cd "$TOOLS_DIR" && cargo run --locked -p bridge-tally-compatibility -- "$@" )
}

# Runs rehash-surface and surfaces the tool's own changed-entry count
# (`rehash_surface_changed:<n>`, printed to stderr by the tool itself) as a
# readable line, instead of leaving it buried in cargo's output.
run_rehash() {
  local surface_path="$1"
  local err_file
  err_file="$(mktemp)"
  if ! run_tool rehash-surface "$surface_path" "$PIN_ROOT" --output "$surface_path" 2>"$err_file"; then
    cat "$err_file" >&2
    rm -f "$err_file"
    return 1
  fi
  cat "$err_file" >&2
  local changed
  changed="$(grep -o 'rehash_surface_changed:[0-9]*' "$err_file" | tail -1 | cut -d: -f2)"
  rm -f "$err_file"
  echo "reseal.sh: rehash-surface changed ${changed:-0} pinned file hash(es)" >&2
}

# Runs the full documented sequence against the given destination files, in
# the order appropriate for whether the pin list itself changed.
reseal_into() {
  local surface_dest="$1"
  local matrix_dest="$2"
  local pins_changed="$3"

  if [ "$pins_changed" = "yes" ]; then
    # Inverted sequence (see docs/release-process.md, "Adding or removing a
    # pin"): seal-surface never reads the repository, it only re-attests the
    # manifest already on disk, so it is safe to run first here purely to
    # produce a checksum that matches the *new* file list. The ordinary three
    # steps that follow re-read every pin's actual bytes from disk, including
    # the newly added one, before the final, real seal.
    echo "reseal.sh: pin list changed -- sealing new file list before rehash" >&2
    run_tool seal-surface "$surface_dest" --output "$surface_dest"
  fi

  run_rehash "$surface_dest"
  run_tool seal-surface "$surface_dest" --output "$surface_dest"
  run_tool repoint-matrix "$matrix_dest" "$surface_dest" --output "$matrix_dest"
}

if [ "$MODE" = "verify" ]; then
  scratch="$(mktemp -d)"
  trap 'rm -rf "$scratch"' EXIT
  cp "$SURFACE" "$scratch/compatibility-surface.json"
  cp "$MATRIX" "$scratch/compatibility-matrix.json"
  # --verify checks that the already-committed pin list is current; it never
  # writes a new pin list, so it always uses the ordinary (non-inverted)
  # order regardless of how the real surface last changed.
  reseal_into "$scratch/compatibility-surface.json" "$scratch/compatibility-matrix.json" "no"

  failed=0
  if ! diff -q "$SURFACE" "$scratch/compatibility-surface.json" >/dev/null; then
    echo "reseal.sh --verify: $SURFACE is stale" >&2
    diff -u "$SURFACE" "$scratch/compatibility-surface.json" >&2 || true
    failed=1
  fi
  if ! diff -q "$MATRIX" "$scratch/compatibility-matrix.json" >/dev/null; then
    echo "reseal.sh --verify: $MATRIX is stale" >&2
    diff -u "$MATRIX" "$scratch/compatibility-matrix.json" >&2 || true
    failed=1
  fi
  if [ "$failed" -ne 0 ]; then
    echo "reseal.sh --verify: FAILED -- run scripts/reseal.sh and commit the result" >&2
    exit 1
  fi
  echo "reseal.sh --verify: compatibility surface and matrix are current"
  exit 0
fi

pins_changed="no"
[ "$MODE" = "pins-changed" ] && pins_changed="yes"
reseal_into "$SURFACE" "$MATRIX" "$pins_changed"
echo "reseal.sh: compatibility surface and matrix resealed"
