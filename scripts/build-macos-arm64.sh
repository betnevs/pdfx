#!/usr/bin/env bash
# Builds an Apple-silicon-only, unsigned macOS app and .dmg.
#
# Apple's macOS 26.4+ compatibility notice about Intel apps can fire for a
# bundle that still carries an x86_64 slice, even when the process runs natively
# on Apple silicon. The universal build intentionally keeps that slice for Intel
# Macs; this build removes it so an Apple silicon machine has nothing to flag.
#
# PDFium is downloaded as a universal dylib, so the copy inside the bundle is
# thinned to arm64 before the app is re-signed ad-hoc. The universal build
# remains available through scripts/build-macos.sh.
#
# Prerequisites: Node.js, Rust with the aarch64-apple-darwin target, and Xcode
# Command Line Tools. Run from anywhere; the script changes to the repository
# root.
#
# Usage: scripts/build-macos-arm64.sh
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
  || fail "the macOS arm64 build must run on macOS (found $(uname -s))."

for tool in node npm cargo rustup lipo otool codesign hdiutil ditto; do
  command -v "${tool}" >/dev/null 2>&1 || fail "required tool not found: ${tool}"
done

# --- Rust target -----------------------------------------------------------
RUST_TARGET="aarch64-apple-darwin"
if ! rustup target list --installed | grep -qx "${RUST_TARGET}"; then
  fail "Rust target ${RUST_TARGET} is not installed.
       Install it, then re-run:
         rustup target add aarch64-apple-darwin"
fi
note "Rust target ready: ${RUST_TARGET}"

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

SOURCE_PDFIUM="src-tauri/vendor/pdfium/lib/libpdfium.dylib"
[ -f "${SOURCE_PDFIUM}" ] \
  || fail "${SOURCE_PDFIUM} is missing after npm run setup:native."
SOURCE_ARCHS="$(lipo -archs "${SOURCE_PDFIUM}" 2>/dev/null)" \
  || fail "${SOURCE_PDFIUM} is not a Mach-O dylib."
case " ${SOURCE_ARCHS} " in
  *" arm64 "*) ;;
  *) fail "${SOURCE_PDFIUM} lacks arm64 (archs: ${SOURCE_ARCHS}). Re-run: npm run setup:native -- --force" ;;
esac
note "PDFium has an arm64 slice (${SOURCE_ARCHS})"

# --- build -----------------------------------------------------------------
note "Building PDFX for ${RUST_TARGET} (unsigned, no notarization)"
npx tauri build --target "${RUST_TARGET}" --bundles app

BUNDLE_DIR="src-tauri/target/${RUST_TARGET}/release/bundle"
APP="${BUNDLE_DIR}/macos/PDFX.app"
[ -d "${APP}" ] || fail "expected app bundle was not produced: ${APP}"

# --- thin PDFium -----------------------------------------------------------
# The vendored dylib is universal. A universal slice inside the bundle is
# exactly what macOS may report as an Intel component, so keep only arm64.
BUNDLED_PDFIUM="${APP}/Contents/Frameworks/libpdfium.dylib"
[ -f "${BUNDLED_PDFIUM}" ] || fail "bundled PDFium is missing: ${BUNDLED_PDFIUM}"
BUNDLED_ARCHS="$(lipo -archs "${BUNDLED_PDFIUM}")"
if [ "${BUNDLED_ARCHS}" != "arm64" ]; then
  note "Thinning bundled PDFium from '${BUNDLED_ARCHS}' to 'arm64'"
  THIN_TMP="$(mktemp "${TMPDIR:-/tmp}/libpdfium-arm64.XXXXXX")"
  trap 'rm -f "${THIN_TMP}"' EXIT
  lipo -thin arm64 -output "${THIN_TMP}" "${BUNDLED_PDFIUM}"
  chmod "$(stat -f '%Lp' "${BUNDLED_PDFIUM}")" "${THIN_TMP}"
  mv "${THIN_TMP}" "${BUNDLED_PDFIUM}"
  trap - EXIT
fi

# Never ship a bundle with a stray Intel slice: check every Mach-O file, not
# just the two known ones, so a future helper binary cannot reintroduce one.
note "Checking every Mach-O in the bundle is arm64"
MACHO_COUNT=0
while IFS= read -r -d '' path; do
  if file "${path}" | grep -q 'Mach-O'; then
    archs="$(lipo -archs "${path}" 2>/dev/null)" \
      || fail "could not read architectures of ${path}"
    [ "${archs}" = "arm64" ] \
      || fail "${path#"${APP}/"} is not arm64-only (archs: ${archs})"
    MACHO_COUNT=$((MACHO_COUNT + 1))
  fi
done < <(find "${APP}" -type f -print0)
[ "${MACHO_COUNT}" -ge 2 ] \
  || fail "expected at least the executable and PDFium to be Mach-O files (found ${MACHO_COUNT})"
note "All ${MACHO_COUNT} Mach-O files are arm64-only"

# --- re-sign ---------------------------------------------------------------
note "Re-signing the thinned app ad-hoc (still unsigned, no notarization)"
codesign --force --deep --sign - "${APP}"
codesign --verify --deep --strict --verbose=2 "${APP}"

# --- dmg -------------------------------------------------------------------
# Tauri is asked for the app bundle only, because its dmg would be created
# before PDFium is thinned. Build the dmg from the corrected, re-signed app.
VERSION="$(plutil -extract version raw -o - src-tauri/tauri.conf.json)"
DMG_DIR="${BUNDLE_DIR}/dmg"
DMG="${DMG_DIR}/PDFX_${VERSION}_arm64.dmg"
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/pdfx-dmg-stage.XXXXXX")"
cleanup() {
  rm -rf "${STAGE}"
}
trap cleanup EXIT
mkdir -p "${DMG_DIR}"
ditto "${APP}" "${STAGE}/PDFX.app"
ln -s /Applications "${STAGE}/Applications"
rm -f "${DMG}"
note "Creating ${DMG}"
hdiutil create -volname "PDFX" -srcfolder "${STAGE}" -ov -format UDZO "${DMG}" >/dev/null

# --- verify ----------------------------------------------------------------
note "Verifying the arm64 app bundle and dmg"
PDFX_EXPECTED_ARCH=arm64 bash "${SCRIPT_DIR}/verify-macos-bundle.sh" "${APP}" "${DMG}"

cat <<EOF

Build complete (unsigned; Gatekeeper bypass required on first launch).

  app: ${APP}
  dmg: ${DMG}

This build contains no Intel code, so macOS 26.4+ will not report PDFX as an
Intel-based app on Apple silicon. Intel Macs need the universal build from
scripts/build-macos.sh instead.

Install instructions: docs/install-macos.md
Verify again any time:
  PDFX_EXPECTED_ARCH=arm64 npm run verify:macos
EOF
