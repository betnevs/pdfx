#!/usr/bin/env bash
# Builds the unsigned, universal (Apple Silicon + Intel) macOS app and .dmg.
#
# Prerequisites: Node.js, Rust with the aarch64- and x86_64-apple-darwin
# targets, and Xcode Command Line Tools. Run from anywhere; the script changes
# to the repository root.
#
# Usage: scripts/build-macos.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

fail() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

note() {
  printf '\n==> %s\n' "$*"
}

[ "$(uname -s)" = "Darwin" ] \
  || fail "the macOS universal build must run on macOS (found $(uname -s))."

for tool in node npm cargo rustup lipo otool hdiutil; do
  command -v "${tool}" >/dev/null 2>&1 || fail "required tool not found: ${tool}"
done

# --- Rust targets ----------------------------------------------------------
RUST_TARGETS="aarch64-apple-darwin x86_64-apple-darwin"
installed_targets="$(rustup target list --installed)"
for target in ${RUST_TARGETS}; do
  if ! grep -qx "${target}" <<<"${installed_targets}"; then
    fail "Rust target ${target} is not installed.
       Install both universal targets, then re-run:
         rustup target add aarch64-apple-darwin x86_64-apple-darwin"
  fi
done
note "Rust targets ready: ${RUST_TARGETS}"

# --- JavaScript dependencies ----------------------------------------------
if [ ! -d node_modules ]; then
  note "node_modules is missing; running npm ci"
  npm ci
fi
if ! npx --no-install tauri --version >/dev/null 2>&1; then
  fail "the Tauri CLI is not installed. Run: npm ci"
fi

# --- native PDFium ---------------------------------------------------------
note "Fetching and verifying PDFium 151.0.7881.0"
npm run setup:native

PDFIUM_DYLIB="src-tauri/vendor/pdfium/lib/libpdfium.dylib"
[ -f "${PDFIUM_DYLIB}" ] \
  || fail "${PDFIUM_DYLIB} is missing after npm run setup:native."
PDFIUM_ARCHS="$(lipo -archs "${PDFIUM_DYLIB}" 2>/dev/null)" \
  || fail "${PDFIUM_DYLIB} is not a universal Mach-O dylib."
for arch in arm64 x86_64; do
  case " ${PDFIUM_ARCHS} " in
    *" ${arch} "*) ;;
    *) fail "${PDFIUM_DYLIB} lacks ${arch} (archs: ${PDFIUM_ARCHS}). Re-run: npm run setup:native -- --force" ;;
  esac
done
[ -f src-tauri/vendor/pdfium/LICENSE ] && [ -d src-tauri/vendor/pdfium/licenses ] \
  || fail "PDFium license files are missing from src-tauri/vendor/pdfium/."
note "PDFium is universal (${PDFIUM_ARCHS}) with license files"

# --- build -----------------------------------------------------------------
note "Building PDFX for universal-apple-darwin (unsigned, no notarization)"
npx tauri build --target universal-apple-darwin --bundles app,dmg

# --- verify ----------------------------------------------------------------
note "Verifying the app bundle and dmg"
bash "${SCRIPT_DIR}/verify-macos-bundle.sh"

cat <<EOF

Build complete (unsigned; Gatekeeper bypass required on first launch).

  app: src-tauri/target/universal-apple-darwin/release/bundle/macos/PDFX.app
  dmg: src-tauri/target/universal-apple-darwin/release/bundle/dmg/PDFX_$(plutil -extract version raw -o - src-tauri/tauri.conf.json)_universal.dmg

Install instructions: docs/install-macos.md
Verify again any time: npm run verify:macos
EOF
