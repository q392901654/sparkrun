#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXE="sparkrun-sidecar"
DEST="$ROOT/extensions/vscode/bin"
mkdir -p "$DEST"
for c in "$ROOT/target/release/$EXE" "$ROOT/target/debug/$EXE"; do
  if [[ -f "$c" ]]; then
    cp -f "$c" "$DEST/$EXE"
    echo "Copied $c -> $DEST/$EXE"
    exit 0
  fi
done
echo "sidecar binary not found. Run: cargo build -p sparkrun-sidecar --release" >&2
exit 1
