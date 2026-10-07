#!/usr/bin/env bash
# Build the sqlsift WebAssembly module for the browser playground in site/.
#
# Requirements:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.100 --locked
#   (optional) wasm-opt from binaryen, for a smaller module
#
# Output: site/pkg/ (gitignored). Serve site/ with any static file server, e.g.
#   python3 -m http.server -d site 8000
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

WASM_BINDGEN_VERSION="0.2.100"
TARGET="wasm32-unknown-unknown"
OUT_DIR="site/pkg"
WASM="target/$TARGET/release/sqlsift_wasm.wasm"

if ! command -v wasm-bindgen >/dev/null 2>&1; then
  echo "error: wasm-bindgen not found. Install with:" >&2
  echo "  cargo install wasm-bindgen-cli --version $WASM_BINDGEN_VERSION --locked" >&2
  exit 1
fi

installed="$(wasm-bindgen --version | awk '{print $2}')"
if [[ "$installed" != "$WASM_BINDGEN_VERSION" ]]; then
  echo "error: wasm-bindgen CLI $installed does not match the crate's pinned $WASM_BINDGEN_VERSION" >&2
  exit 1
fi

echo "==> cargo build (sqlsift-wasm, $TARGET, release)"
# Size-oriented settings for the wasm build only; the workspace release
# profile used by the CLI/LSP is left untouched.
CARGO_PROFILE_RELEASE_LTO=true \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
CARGO_PROFILE_RELEASE_OPT_LEVEL=s \
CARGO_PROFILE_RELEASE_PANIC=abort \
  cargo build -p sqlsift-wasm --release --target "$TARGET"

echo "==> wasm-bindgen --target web"
rm -rf "$OUT_DIR"
wasm-bindgen --target web --no-typescript --out-dir "$OUT_DIR" "$WASM"

if command -v wasm-opt >/dev/null 2>&1; then
  echo "==> wasm-opt -Os"
  wasm-opt -Os --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    "$OUT_DIR/sqlsift_wasm_bg.wasm" -o "$OUT_DIR/sqlsift_wasm_bg.wasm"
else
  echo "==> wasm-opt not found; skipping (install binaryen for a smaller module)"
fi

size=$(wc -c <"$OUT_DIR/sqlsift_wasm_bg.wasm")
echo "==> done: $OUT_DIR/sqlsift_wasm_bg.wasm ($((size / 1024)) KiB)"
