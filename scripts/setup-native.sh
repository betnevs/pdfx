#!/usr/bin/env bash
# Downloads and verifies the native PDFium library used by the PDFX desktop app.
#
# The archive lands under src-tauri/vendor/pdfium/, which is git-ignored. The
# app and the Rust integration tests load the shared library from there unless
# PDFIUM_DYNAMIC_LIB_PATH points somewhere else.
#
# Usage: scripts/setup-native.sh [--force]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VENDOR_ROOT="${REPO_ROOT}/src-tauri/vendor"
VENDOR="${VENDOR_ROOT}/pdfium"
DOWNLOADS="${VENDOR_ROOT}/downloads"
FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

# --- pinned version --------------------------------------------------------
# PDFium 151.0.7881.0 is the release pdfium-render 0.9.4 declares support for
# (`pdfium_7881`) and the one the M0→M1 spike verified.
PDFIUM_TAG="chromium/7881"
PDFIUM_VERSION="151.0.7881.0"
PDFIUM_RELEASE_URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881"

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

download() { # url dest expected_sha256
  local url="$1" dest="$2" expected="$3"
  if [ -f "${dest}" ] && [ "${FORCE}" -eq 0 ]; then
    if [ "$(sha256_of "${dest}")" = "${expected}" ]; then
      echo "ok        $(basename "${dest}") (cached)"
      return 0
    fi
    echo "checksum mismatch for cached $(basename "${dest}"); re-downloading" >&2
  fi
  mkdir -p "$(dirname "${dest}")"
  local tmp="${dest}.part"
  echo "download  ${url}"
  curl -fL --retry 3 --retry-delay 2 --max-time 300 -o "${tmp}" "${url}"
  local actual
  actual="$(sha256_of "${tmp}")"
  if [ "${actual}" != "${expected}" ]; then
    rm -f "${tmp}"
    echo "ERROR: checksum mismatch for ${url}" >&2
    echo "  expected ${expected}" >&2
    echo "  actual   ${actual}" >&2
    exit 1
  fi
  mv "${tmp}" "${dest}"
  echo "ok        $(basename "${dest}") sha256=${actual}"
}

# --- platform selection ----------------------------------------------------
OS="$(uname -s)"
ARCH="$(uname -m)"
case "${OS}/${ARCH}" in
  Darwin/arm64|Darwin/x86_64)
    PDFIUM_ASSET="pdfium-mac-univ.tgz"
    PDFIUM_SHA256="df451a413c3609585e84a4a91110a9bc889cff05fe3b2db0ed817c9e90c3f7d3"
    PDFIUM_LIB="lib/libpdfium.dylib"
    ;;
  Linux/x86_64)
    PDFIUM_ASSET="pdfium-linux-x64.tgz"
    PDFIUM_SHA256="1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d"
    PDFIUM_LIB="lib/libpdfium.so"
    ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT*)
    if [ "${ARCH}" = "x86_64" ] || [ "${ARCH}" = "AMD64" ]; then
      PDFIUM_ASSET="pdfium-win-x64.tgz"
      PDFIUM_SHA256="73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08"
      PDFIUM_LIB="bin/pdfium.dll"
    else
      echo "ERROR: unsupported Windows architecture: ${ARCH}" >&2
      exit 1
    fi
    ;;
  *)
    echo "ERROR: unsupported platform ${OS}/${ARCH}." >&2
    echo "Add the asset name and SHA-256 from ${PDFIUM_RELEASE_URL} to this script." >&2
    exit 1
    ;;
esac

PDFIUM_ARCHIVE="${DOWNLOADS}/${PDFIUM_ASSET}"
download "${PDFIUM_RELEASE_URL}/${PDFIUM_ASSET}" "${PDFIUM_ARCHIVE}" "${PDFIUM_SHA256}"

PDFIUM_BUILD="${PDFIUM_TAG#chromium/}"

if [ ! -f "${VENDOR}/${PDFIUM_LIB}" ] \
  || [ "${FORCE}" -eq 1 ] \
  || ! grep -qx "BUILD=${PDFIUM_BUILD}" "${VENDOR}/VERSION" 2>/dev/null; then
  rm -rf "${VENDOR}"
  mkdir -p "${VENDOR}"
  tar -xzf "${PDFIUM_ARCHIVE}" -C "${VENDOR}"
fi

if [ ! -f "${VENDOR}/${PDFIUM_LIB}" ]; then
  echo "ERROR: archive did not contain ${PDFIUM_LIB}" >&2
  exit 1
fi

cat > "${VENDOR}/SOURCE.txt" <<EOF
PDFium binaries ${PDFIUM_TAG} (PDFium ${PDFIUM_VERSION})
URL:     ${PDFIUM_RELEASE_URL}/${PDFIUM_ASSET}
SHA-256: ${PDFIUM_SHA256}
Builds:  https://github.com/bblanchon/pdfium-binaries (MIT)
PDFium:  BSD-3-Clause, see vendor/pdfium/LICENSE
EOF

echo "ok        src-tauri/vendor/pdfium/${PDFIUM_LIB}"
cat <<EOF

PDFium ${PDFIUM_VERSION} ready.

The app and \`cargo test\` load it from:

  ${VENDOR}/${PDFIUM_LIB}

Override with PDFIUM_DYNAMIC_LIB_PATH if you keep the library elsewhere.
EOF
