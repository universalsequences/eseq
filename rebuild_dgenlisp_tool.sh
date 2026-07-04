#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DGEN_REPO="${DGEN_REPO:-$HOME/code/swift/dgen}"
PRODUCT_NAME="DGenLisp"
SOURCE_BIN="$DGEN_REPO/.build/release/$PRODUCT_NAME"
DEST_BIN="$ROOT_DIR/crates/sequencer/tools/$PRODUCT_NAME"
ALLOW_UNCHANGED=0

usage() {
  cat <<EOF
Usage: $(basename "$0") [--allow-unchanged]

Rebuilds the release DGenLisp binary from:
  $DGEN_REPO

Installs it to:
  $DEST_BIN

Environment:
  DGEN_REPO=/path/to/dgen   Override the Swift dgen repo path.

Options:
  --allow-unchanged         Copy the release artifact even if its timestamp did
                            not advance during this run.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --allow-unchanged)
      ALLOW_UNCHANGED=1
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

mtime() {
  local path="$1"
  if [[ -e "$path" ]]; then
    stat -f '%m' "$path"
  else
    echo 0
  fi
}

describe_file() {
  local path="$1"
  if [[ -e "$path" ]]; then
    stat -f '%Sm %z %N' "$path"
  else
    echo "missing $path"
  fi
}

sha256() {
  shasum -a 256 "$1" | awk '{print $1}'
}

if [[ ! -d "$DGEN_REPO" ]]; then
  echo "error: dgen repo not found: $DGEN_REPO" >&2
  exit 1
fi

if [[ ! -f "$DGEN_REPO/Package.swift" ]]; then
  echo "error: Package.swift not found in dgen repo: $DGEN_REPO" >&2
  exit 1
fi

if [[ ! -d "$(dirname "$DEST_BIN")" ]]; then
  echo "error: destination directory not found: $(dirname "$DEST_BIN")" >&2
  exit 1
fi

SOURCE_MTIME_BEFORE="$(mtime "$SOURCE_BIN")"
SOURCE_DESC_BEFORE="$(describe_file "$SOURCE_BIN")"
DEST_DESC_BEFORE="$(describe_file "$DEST_BIN")"

echo "Release artifact before build:"
echo "  $SOURCE_DESC_BEFORE"
echo
echo "Installed tool before copy:"
echo "  $DEST_DESC_BEFORE"
echo
echo "Building $PRODUCT_NAME release binary in $DGEN_REPO..."
(
  cd "$DGEN_REPO"
  swift build -c release --product "$PRODUCT_NAME"
)

if [[ ! -x "$SOURCE_BIN" ]]; then
  echo "error: release binary not found or not executable after build: $SOURCE_BIN" >&2
  exit 1
fi

SOURCE_MTIME_AFTER="$(mtime "$SOURCE_BIN")"
if [[ "$ALLOW_UNCHANGED" -eq 0 && "$SOURCE_MTIME_AFTER" -le "$SOURCE_MTIME_BEFORE" ]]; then
  echo "error: release artifact timestamp did not advance during build" >&2
  echo "before: $SOURCE_DESC_BEFORE" >&2
  echo "after:  $(describe_file "$SOURCE_BIN")" >&2
  echo "Use --allow-unchanged only when intentionally copying an existing artifact." >&2
  exit 1
fi

echo
echo "Release artifact after build:"
echo "  $(describe_file "$SOURCE_BIN")"

SOURCE_HASH="$(sha256 "$SOURCE_BIN")"
echo "  sha256 $SOURCE_HASH"
echo
echo "Installing to $DEST_BIN..."
install -m 0755 "$SOURCE_BIN" "$DEST_BIN"

if [[ ! -x "$DEST_BIN" ]]; then
  echo "error: installed binary is not executable: $DEST_BIN" >&2
  exit 1
fi

DEST_HASH="$(sha256 "$DEST_BIN")"
if [[ "$SOURCE_HASH" != "$DEST_HASH" ]]; then
  echo "error: installed binary checksum does not match release artifact" >&2
  echo "source: $SOURCE_HASH $SOURCE_BIN" >&2
  echo "dest:   $DEST_HASH $DEST_BIN" >&2
  exit 1
fi

echo "Installed tool after copy:"
echo "  $(describe_file "$DEST_BIN")"
echo "  sha256 $DEST_HASH"
echo
echo "Done."
