#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DGEN_REPO="${DGEN_REPO:-$HOME/code/dgen-audio}"
TOOLCHAIN_DEST="$ROOT_DIR/crates/sequencer/tools/dgen-toolchain"
LOCK_FILE="$ROOT_DIR/content/dgen-toolchain.lock"
UPDATE_LOCK=0

usage() {
  cat <<EOF
Usage: $(basename "$0") [--update-lock]

Stages the host target's hermetic dgen toolchain archive, selected from
content/dgen-toolchain.lock, into:
  $TOOLCHAIN_DEST        (gitignored, ~147 MB)
and records the vendored archive's identity in the committed lock file:
  $LOCK_FILE

This script no longer builds or installs the DGenLisp compiler itself.
DGenLisp binaries are not tracked in git: published distributions are pinned
in content/dgenlisp.lock and installed by scripts/fetch_dgenlisp.sh. New
compiler releases are built and published from the dgen-audio repository;
consuming one here means updating content/dgenlisp.lock and re-running the
fetch script. To try a locally built compiler, set
ESEQ_DGENLISP_TOOL=\$DGEN_REPO/.build/release/DGenLisp instead of copying
anything into the tree.

Environment:
  DGEN_REPO=/path/to/dgen-audio
                            Override the Swift dgen repo path.

Options:
  --update-lock             Accept a toolchain archive whose sha256 differs
                            from the committed lock file and rewrite the lock.
                            Without this flag a divergent archive is an error.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --update-lock)
      UPDATE_LOCK=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

describe_file() {
  local path="$1"
  if [[ ! -e "$path" ]]; then
    echo "missing $path"
  elif [[ "$(uname -s)" == "Darwin" ]]; then
    stat -f '%Sm %z %N' "$path"
  else
    stat -c '%y %s %n' "$path"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

host_target() {
  case "$(uname -s):$(uname -m)" in
    Darwin:arm64) echo "arm64-apple-macos" ;;
    Linux:x86_64) echo "x86_64-unknown-linux-gnu" ;;
    *)
      echo "error: no DGen toolchain target is defined for $(uname -s)/$(uname -m)" >&2
      return 1
      ;;
  esac
}

lock_value() {
  local key="target.$HOST_TARGET.$1"
  awk -v key="$key" '$1 == key { print $2 }' "$LOCK_FILE"
}

# ── Stage the hermetic toolchain ──
# The archive layout contract is $DGEN_REPO/toolchain/LAYOUT.md: untarring
# yields a single top-level dgen-toolchain/ directory, which is the root
# passed to `DGenLisp compile --toolchain-root`.

HOST_TARGET="$(host_target)"
if ! command -v python3 >/dev/null 2>&1; then
  echo "error: required tool not found: python3" >&2
  exit 1
fi
if [[ ! -f "$LOCK_FILE" ]]; then
  echo "error: DGen toolchain lock file not found: $LOCK_FILE" >&2
  exit 1
fi
ARCHIVE_NAME="$(lock_value archive)"
LOCKED_HASH="$(lock_value archive_sha256)"
if [[ -z "$ARCHIVE_NAME" || -z "$LOCKED_HASH" ]]; then
  echo "error: no hermetic DGen toolchain archive is pinned for host target $HOST_TARGET" >&2
  echo "  lock: $LOCK_FILE" >&2
  echo "DGen compiles cannot fall back to the system compiler." >&2
  echo "Publish this target from dgen-audio's scripts/build-toolchain.sh, then add" >&2
  echo "target.$HOST_TARGET archive and sha256 entries to the lock file." >&2
  exit 1
fi
if [[ ! -d "$DGEN_REPO" ]]; then
  echo "error: dgen repo not found: $DGEN_REPO" >&2
  exit 1
fi

ARCHIVE="$DGEN_REPO/.toolchain/$ARCHIVE_NAME"
echo "Locating $HOST_TARGET toolchain archive..."
if [[ ! -f "$ARCHIVE" ]]; then
  echo "error: pinned $HOST_TARGET toolchain archive not found: $ARCHIVE" >&2
  echo "Build or fetch the exact archive recorded in $LOCK_FILE." >&2
  exit 1
fi
echo "  $(describe_file "$ARCHIVE")"

ARCHIVE_HASH="$(sha256 "$ARCHIVE")"
echo "  sha256 $ARCHIVE_HASH"

# The lock file is the committed, reviewable record of which toolchain is
# vendored. A divergent archive sha requires an explicit --update-lock.
if [[ "$LOCKED_HASH" != "$ARCHIVE_HASH" ]]; then
  if [[ "$UPDATE_LOCK" -eq 0 ]]; then
    echo "error: $HOST_TARGET toolchain archive sha256 differs from the committed lock file" >&2
    echo "  locked:  $LOCKED_HASH  ($LOCK_FILE)" >&2
    echo "  archive: $ARCHIVE_HASH  ($ARCHIVE)" >&2
    echo "Re-run with --update-lock to vendor the new toolchain (and commit the lock change)." >&2
    exit 1
  fi
  echo "Lock update requested (--update-lock):"
  echo "  old sha256: $LOCKED_HASH"
  echo "  new sha256: $ARCHIVE_HASH"
fi

echo
echo "Staging toolchain into $TOOLCHAIN_DEST..."
STAGING_TMP="$TOOLCHAIN_DEST.staging.$$"
rm -rf "$STAGING_TMP"
mkdir -p "$STAGING_TMP"
tar -xzf "$ARCHIVE" -C "$STAGING_TMP"

STAGED_ROOT="$STAGING_TMP/dgen-toolchain"
if [[ ! -d "$STAGED_ROOT" ]]; then
  echo "error: archive did not contain a top-level dgen-toolchain/ directory (see toolchain/LAYOUT.md)" >&2
  rm -rf "$STAGING_TMP"
  exit 1
fi

# Required staged files per LAYOUT.md; an incomplete stage is useless (the
# compiler preflight-checks it and hard-errors), so fail here instead.
COMMON_REQUIRED_FILES=(
  "VERSION.json"
  "include/dgen_runtime.h"
  "bin/dgen-clang"
)
case "$HOST_TARGET" in
  arm64-apple-macos)
    REQUIRED_FILES=(
      "${COMMON_REQUIRED_FILES[@]}"
      "abi/exports-v1.txt"
      "abi/libsystem-symbols-v1.txt"
      "bin/ld64.lld"
      "lib/clang/20/lib/darwin/libclang_rt.builtins.a"
      "lib/libSystem.tbd"
    )
    ;;
  x86_64-unknown-linux-gnu)
    REQUIRED_FILES=(
      "${COMMON_REQUIRED_FILES[@]}"
      "abi/exports-v1-elf.txt"
      "abi/libsystem-symbols-v1-elf.txt"
    )
    ;;
esac
for rel in "${REQUIRED_FILES[@]}"; do
  if [[ ! -f "$STAGED_ROOT/$rel" ]]; then
    echo "error: staged toolchain is missing required file: $rel" >&2
    rm -rf "$STAGING_TMP"
    exit 1
  fi
done

LLVM_VERSION="$(sed -n 's/.*"llvm_version": *"\([^"]*\)".*/\1/p' "$STAGED_ROOT/VERSION.json")"
DGEN_ABI_VERSION="$(sed -n 's/.*"dgen_abi_version": *\([0-9][0-9]*\).*/\1/p' "$STAGED_ROOT/VERSION.json")"
STAGED_TARGET="$(sed -n 's/.*"target": *"\([^"]*\)".*/\1/p' "$STAGED_ROOT/VERSION.json")"
if [[ -z "$LLVM_VERSION" || -z "$DGEN_ABI_VERSION" || -z "$STAGED_TARGET" ]]; then
  echo "error: could not read target / llvm_version / dgen_abi_version from staged VERSION.json" >&2
  rm -rf "$STAGING_TMP"
  exit 1
fi
if [[ "$STAGED_TARGET" != "$HOST_TARGET" ]]; then
  echo "error: archive targets $STAGED_TARGET but this host requires $HOST_TARGET" >&2
  echo "Refusing to stage a wrong-architecture compiler." >&2
  rm -rf "$STAGING_TMP"
  exit 1
fi

# Replace any prior stage atomically: untarred to a temp sibling above, now
# swap the verified root into place.
rm -rf "$TOOLCHAIN_DEST"
mv "$STAGED_ROOT" "$TOOLCHAIN_DEST"
rm -rf "$STAGING_TMP"

python3 - "$LOCK_FILE" "$HOST_TARGET" "$ARCHIVE_NAME" "$ARCHIVE_HASH" "$LLVM_VERSION" "$DGEN_ABI_VERSION" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
target = sys.argv[2]
updates = {
    f"target.{target}.archive": sys.argv[3],
    f"target.{target}.archive_sha256": sys.argv[4],
    f"target.{target}.llvm_version": sys.argv[5],
    f"target.{target}.dgen_abi_version": sys.argv[6],
}
lines = path.read_text().splitlines()
seen = set()
for index, line in enumerate(lines):
    fields = line.split(maxsplit=1)
    if fields and fields[0] in updates:
        key = fields[0]
        lines[index] = f"{key} {updates[key]}"
        seen.add(key)
if seen != updates.keys():
    missing = sorted(updates.keys() - seen)
    raise SystemExit(f"error: lock file is missing the {target} record: {', '.join(missing)}")
temporary = path.with_name(path.name + ".tmp")
temporary.write_text("\n".join(lines) + "\n")
temporary.replace(path)
PY

echo "Staged toolchain:"
echo "  root: $TOOLCHAIN_DEST"
echo "  llvm_version: $LLVM_VERSION"
echo "  dgen_abi_version: $DGEN_ABI_VERSION"
echo
echo "Lock file ($LOCK_FILE):"
sed 's/^/  /' "$LOCK_FILE"
echo
echo "Done."
