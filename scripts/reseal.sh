#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Reseals the compatibility surface: rewrites every pinned file's sha256 in
# docs/tally/compatibility/compatibility-surface.json from the file's bytes
# (`rehash-surface`).
#
# The surface stores only per-file hashes. Its digest is computed by the gate,
# never stored, so this one step is the whole reseal, for a changed pin list
# as well, and the matrix never changes on a reseal (bridge#760). Two changes
# that pin different files therefore merge without conflict.
#
# Read docs/release-process.md before changing the pin list itself.
#
# Usage:
#   scripts/reseal.sh                  Rehash every pinned file.
#   scripts/reseal.sh --pins-changed   The same; kept so existing instructions
#                                       still work. After adding a pin (with any
#                                       64-hex placeholder hash, in sorted
#                                       order), this writes its real hash.
#   scripts/reseal.sh --verify         Rehash into a scratch copy and compare
#                                       byte-for-byte against the committed
#                                       surface. Exits non-zero if they differ.
#                                       Never mutates the working tree or the
#                                       committed files. For CI.
#
# Optional, mainly for testing against fixtures instead of the real
# compatibility surface:
#   --root DIR       Repository root that pinned file paths are read relative
#                     to (default: this repo's own root).
#   --surface FILE    Path to the surface manifest to reseal (default:
#                     docs/tally/compatibility/compatibility-surface.json).
#
# Never hand-edit a pin's hash: this script (or a direct call to the tool it
# wraps) is the only supported way to write them.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TOOLS_DIR="$PROJECT_ROOT/tools"

MODE="reseal"
PIN_ROOT="$PROJECT_ROOT"
SURFACE="$PROJECT_ROOT/docs/tally/compatibility/compatibility-surface.json"

print_usage() {
  sed -n '2,35p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
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

if [ "$MODE" = "verify" ]; then
  scratch="$(mktemp -d)"
  trap 'rm -rf "$scratch"' EXIT
  cp "$SURFACE" "$scratch/compatibility-surface.json"
  run_rehash "$scratch/compatibility-surface.json"
  if ! diff -q "$SURFACE" "$scratch/compatibility-surface.json" >/dev/null; then
    echo "reseal.sh --verify: $SURFACE is stale" >&2
    diff -u "$SURFACE" "$scratch/compatibility-surface.json" >&2 || true
    echo "reseal.sh --verify: FAILED -- run scripts/reseal.sh and commit the result" >&2
    exit 1
  fi
  echo "reseal.sh --verify: compatibility surface is current"
  exit 0
fi

run_rehash "$SURFACE"
echo "reseal.sh: compatibility surface resealed"

# Report-only, never fails the reseal: pins this branch dropped, and modules
# newly left unpinned directly under a pinned one, since origin/master -- two
# things the gate cannot see. Much else is not checked; see the script (#416).
python3 "$SCRIPT_DIR/surface_coverage_report.py" --root "$PIN_ROOT" --surface "$SURFACE" || true
