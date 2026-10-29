#!/usr/bin/env bash
# Verifies a macOS app bundle and .dmg produced by scripts/build-macos.sh
# (universal) or scripts/build-macos-arm64.sh (Apple silicon only).
#
# Usage: scripts/verify-macos-bundle.sh [PDFX.app] [PDFX.dmg]
#
# Set PDFX_EXPECTED_ARCH to universal (default), arm64 or x86_64. The arm64
# mode also rejects any bundle that still contains an x86_64 slice, which is
# what macOS 26.4+ may flag as an Intel-based component.
#
# With no arguments the artifacts are resolved from the universal bundle
# directory, or from the aarch64 directory when PDFX_EXPECTED_ARCH=arm64.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
EXPECTED_ARCH="${PDFX_EXPECTED_ARCH:-universal}"
case "${EXPECTED_ARCH}" in
  universal) EXPECTED_ARCHS=(arm64 x86_64) ;;
  arm64) EXPECTED_ARCHS=(arm64) ;;
  x86_64) EXPECTED_ARCHS=(x86_64) ;;
  *)
    printf 'FAIL PDFX_EXPECTED_ARCH must be universal, arm64 or x86_64 (got %s)\n' \
      "${EXPECTED_ARCH}" >&2
    exit 1
    ;;
esac
case "${EXPECTED_ARCH}" in
  arm64)
    BUNDLE_DIR="${REPO_ROOT}/src-tauri/target/aarch64-apple-darwin/release/bundle"
    ;;
  *)
    BUNDLE_DIR="${REPO_ROOT}/src-tauri/target/universal-apple-darwin/release/bundle"
    ;;
esac
APP="${1:-${BUNDLE_DIR}/macos/PDFX.app}"
DMG="${2:-}"

fail() {
  printf 'FAIL %s\n' "$*" >&2
  exit 1
}

pass() {
  printf 'ok   %s\n' "$*"
}

require_archs() { # path label
  local path="$1" label="$2" archs arch
  [ -f "${path}" ] || fail "${label} is missing: ${path}"
  archs="$(lipo -archs "${path}" 2>/dev/null)" \
    || fail "${label} is not a Mach-O file: ${path}"
  for arch in "${EXPECTED_ARCHS[@]}"; do
    case " ${archs} " in
      *" ${arch} "*) ;;
      *) fail "${label} lacks ${arch} (archs: ${archs})" ;;
    esac
  done
  if [ "${EXPECTED_ARCH}" = "arm64" ]; then
    case " ${archs} " in
      *" x86_64 "*)
        fail "${label} still contains an x86_64 slice; macOS 26.4+ can flag it as an Intel component (archs: ${archs})"
        ;;
    esac
  fi
  pass "${label} is ${EXPECTED_ARCH} (${archs})"
}

plist_print() { # key
  /usr/libexec/PlistBuddy -c "Print :$1" "${INFO_PLIST}" 2>/dev/null
}

[ "$(uname -s)" = "Darwin" ] || fail "this verifier only runs on macOS"
command -v lipo >/dev/null 2>&1 || fail "lipo not found (install Xcode Command Line Tools)"
command -v otool >/dev/null 2>&1 || fail "otool not found (install Xcode Command Line Tools)"
command -v codesign >/dev/null 2>&1 || fail "codesign not found (install Xcode Command Line Tools)"

# --- app bundle ------------------------------------------------------------
[ -d "${APP}" ] || fail "app bundle is missing: ${APP}. Run the matching build script first."
INFO_PLIST="${APP}/Contents/Info.plist"
[ -f "${INFO_PLIST}" ] || fail "Info.plist is missing: ${INFO_PLIST}"

EXPECTED_VERSION="$(plutil -extract version raw -o - "${REPO_ROOT}/src-tauri/tauri.conf.json")"
EXPECTED_PRODUCT="$(plutil -extract productName raw -o - "${REPO_ROOT}/src-tauri/tauri.conf.json")"
EXPECTED_IDENTIFIER="$(plutil -extract identifier raw -o - "${REPO_ROOT}/src-tauri/tauri.conf.json")"

[ "$(plist_print CFBundleName)" = "${EXPECTED_PRODUCT}" ] \
  || fail "CFBundleName is not ${EXPECTED_PRODUCT}"
[ "$(plist_print CFBundleIdentifier)" = "${EXPECTED_IDENTIFIER}" ] \
  || fail "CFBundleIdentifier is not ${EXPECTED_IDENTIFIER}"
[ "$(plist_print CFBundleShortVersionString)" = "${EXPECTED_VERSION}" ] \
  || fail "CFBundleShortVersionString is not ${EXPECTED_VERSION}"
[ "$(plist_print CFBundleVersion)" = "${EXPECTED_VERSION}" ] \
  || fail "CFBundleVersion is not ${EXPECTED_VERSION}"
[ "$(plist_print LSMinimumSystemVersion)" = "12.0" ] \
  || fail "LSMinimumSystemVersion is not 12.0"
# Tauri's bundler writes LSRequiresCarbon=true by default; src-tauri/Info.plist
# overrides it because PDFX is a Cocoa app, not a Carbon app.
[ "$(plist_print LSRequiresCarbon)" != "true" ] \
  || fail "Info.plist marks the app as requiring the legacy Carbon environment"
pass "Info.plist name/identifier/version/minimum system version are correct"

EXECUTABLE_NAME="$(plist_print CFBundleExecutable)"
[ -n "${EXECUTABLE_NAME}" ] || fail "CFBundleExecutable is missing from Info.plist"
EXECUTABLE="${APP}/Contents/MacOS/${EXECUTABLE_NAME}"
[ -x "${EXECUTABLE}" ] || fail "app executable is missing or not executable: ${EXECUTABLE}"

ICON_FILE="$(plist_print CFBundleIconFile)"
[ -n "${ICON_FILE}" ] || fail "CFBundleIconFile is missing from Info.plist"
case "${ICON_FILE}" in
  *.icns) ICON_NAME="${ICON_FILE}" ;;
  *) ICON_NAME="${ICON_FILE}.icns" ;;
esac
[ -f "${APP}/Contents/Resources/${ICON_NAME}" ] \
  || fail "app icon is missing: Contents/Resources/${ICON_NAME}"
pass "app executable and icon are present"

# File associations: .pdf, com.adobe.pdf UTI, Editor role.
DOC_TYPES="$(plutil -extract CFBundleDocumentTypes json -o - "${INFO_PLIST}")"
case "${DOC_TYPES}" in
  *'"pdf"'*) ;;
  *) fail "Info.plist CFBundleDocumentTypes does not declare the pdf extension" ;;
esac
case "${DOC_TYPES}" in
  *'"com.adobe.pdf"'*) ;;
  *) fail "Info.plist CFBundleDocumentTypes does not declare the com.adobe.pdf UTI" ;;
esac
case "${DOC_TYPES}" in
  *'"Editor"'*) ;;
  *) fail "Info.plist CFBundleDocumentTypes does not use the Editor role" ;;
esac
pass "Info.plist declares the PDF file association (pdf, com.adobe.pdf, Editor)"

require_archs "${EXECUTABLE}" "app executable"

# Tauri links the @executable_path/../Frameworks rpath when bundle.macOS.frameworks
# is configured; the engine falls back to this directory when resources are absent.
otool -l "${EXECUTABLE}" | grep -Fq '@executable_path/../Frameworks' \
  || fail "app executable has no @executable_path/../Frameworks LC_RPATH"
pass "app executable has the @executable_path/../Frameworks rpath"

FRAMEWORK_DYLIB="${APP}/Contents/Frameworks/libpdfium.dylib"
require_archs "${FRAMEWORK_DYLIB}" "bundled PDFium"

# PDFium's install name is only informative; the engine dlopens the absolute
# path. Still check the binary is a dylib and not a placeholder.
otool -D "${FRAMEWORK_DYLIB}" >/dev/null 2>&1 \
  || fail "bundled PDFium is not a Mach-O dylib"
pass "bundled PDFium is a dylib at Contents/Frameworks/libpdfium.dylib"

# --- licenses and install notes -------------------------------------------
LICENSE_DIR="${APP}/Contents/Resources/pdfium-licenses"
[ -f "${LICENSE_DIR}/LICENSE" ] || fail "PDFium LICENSE is missing from Contents/Resources/pdfium-licenses"
license_count="$(find "${LICENSE_DIR}" -type f | wc -l | tr -d ' ')"
[ "${license_count}" -ge 2 ] \
  || fail "PDFium license tree looks incomplete (${license_count} file(s))"
pass "PDFium LICENSE and license tree are bundled (${license_count} files)"

INSTALL_NOTE="${APP}/Contents/Resources/INSTALL-macos.md"
[ -f "${INSTALL_NOTE}" ] || fail "Gatekeeper install note is missing: ${INSTALL_NOTE}"
grep -Fq 'xattr -dr com.apple.quarantine' "${INSTALL_NOTE}" \
  || fail "install note does not document the xattr Gatekeeper bypass"
grep -Fq '仍要打开' "${INSTALL_NOTE}" \
  || fail "install note does not document the System Settings Gatekeeper bypass"
pass "Gatekeeper install note is bundled with both bypass paths"

# --- unsigned posture ------------------------------------------------------
if codesign -dvv "${APP}" 2>&1 | grep -q 'Authority=Developer ID'; then
  fail "app is signed with a Developer ID certificate; this ticket ships unsigned"
fi
if xcrun stapler validate "${APP}" >/dev/null 2>&1; then
  fail "app is notarized; this ticket ships without notarization"
fi
pass "app has no Developer ID signature and no notarization ticket"

# --- dmg -------------------------------------------------------------------
if [ -z "${DMG}" ]; then
  DMG="$(find "${BUNDLE_DIR}/dmg" -maxdepth 1 -type f -name '*.dmg' 2>/dev/null | sort | tail -n 1)"
fi
[ -n "${DMG}" ] && [ -f "${DMG}" ] || fail "dmg is missing under ${BUNDLE_DIR}/dmg"
case "$(basename "${DMG}")" in
  *"${EXPECTED_ARCH}"*) ;;
  *) fail "dmg name does not identify the ${EXPECTED_ARCH} build: $(basename "${DMG}")" ;;
esac
hdiutil verify "${DMG}" >/dev/null 2>&1 || fail "dmg checksum verification failed"
pass "dmg checksum verifies: ${DMG}"

MOUNT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pdfx-dmg.XXXXXX")"
cleanup() {
  hdiutil detach "${MOUNT_DIR}" >/dev/null 2>&1 || true
  rmdir "${MOUNT_DIR}" >/dev/null 2>&1 || true
}
trap cleanup EXIT
hdiutil attach -nobrowse -readonly -mountpoint "${MOUNT_DIR}" "${DMG}" >/dev/null \
  || fail "could not mount ${DMG}"
[ -d "${MOUNT_DIR}/PDFX.app" ] || fail "dmg does not contain PDFX.app"
[ -f "${MOUNT_DIR}/PDFX.app/Contents/Frameworks/libpdfium.dylib" ] \
  || fail "dmg app does not contain Contents/Frameworks/libpdfium.dylib"
require_archs "${MOUNT_DIR}/PDFX.app/Contents/MacOS/${EXECUTABLE_NAME}" \
  "dmg app executable"
require_archs "${MOUNT_DIR}/PDFX.app/Contents/Frameworks/libpdfium.dylib" \
  "dmg PDFium"
codesign --verify --deep --strict "${MOUNT_DIR}/PDFX.app" \
  || fail "dmg app signature is invalid"
pass "dmg app signature verifies"
[ -d "${MOUNT_DIR}/Applications" ] || [ -L "${MOUNT_DIR}/Applications" ] \
  || fail "dmg does not provide the /Applications shortcut"
pass "dmg contains PDFX.app, the PDFium framework, and the /Applications shortcut"

echo
echo "Verified ${APP}"
echo "Verified ${DMG}"
