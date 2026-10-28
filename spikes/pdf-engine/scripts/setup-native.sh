#!/usr/bin/env bash
# Downloads and verifies the native assets used by the PDF engine spike.
#
# Everything lands under spikes/pdf-engine/vendor/, which is git-ignored:
#   vendor/pdfium/   PDFium shared library + headers + licenses
#   vendor/fonts/    Noto Sans SC (SIL OFL 1.1) for the CJK subset probe
#
# Usage: scripts/setup-native.sh [--force]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPIKE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
VENDOR="${SPIKE_DIR}/vendor"
FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

# --- pinned versions -------------------------------------------------------
# 7881 is the PDFium release pdfium-render 0.9.4's `pdfium_latest` feature
# declares (151.0.7881.0), and it contains the incremental-save fix.
PDFIUM_TAG="chromium/7881" # PDFium 151.0.7881.0
PDFIUM_VERSION="151.0.7881.0"
PDFIUM_RELEASE_URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881"

# Noto Sans SC variable TTF, pinned to a google/fonts commit.
FONT_COMMIT="ddf386ba1b9fbc6b2c818f1ddcd9148f54ced94b"
FONT_URL="https://raw.githubusercontent.com/google/fonts/${FONT_COMMIT}/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf"
FONT_OFL_URL="https://raw.githubusercontent.com/google/fonts/${FONT_COMMIT}/ofl/notosanssc/OFL.txt"
FONT_SHA256="a3041811a78c361b1de50f953c805e0244951c21c5bd412f7232ef0d899af0da"

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
  Darwin/arm64)  PDFIUM_ASSET="pdfium-mac-univ.tgz"; PDFIUM_SHA256="df451a413c3609585e84a4a91110a9bc889cff05fe3b2db0ed817c9e90c3f7d3"; PDFIUM_LIB="lib/libpdfium.dylib" ;;
  Darwin/x86_64) PDFIUM_ASSET="pdfium-mac-univ.tgz"; PDFIUM_SHA256="df451a413c3609585e84a4a91110a9bc889cff05fe3b2db0ed817c9e90c3f7d3"; PDFIUM_LIB="lib/libpdfium.dylib" ;;
  Linux/x86_64)  PDFIUM_ASSET="pdfium-linux-x64.tgz"; PDFIUM_SHA256="1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d"; PDFIUM_LIB="lib/libpdfium.so" ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT*)
    if [ "${ARCH}" = "x86_64" ] || [ "${ARCH}" = "AMD64" ]; then
      PDFIUM_ASSET="pdfium-win-x64.tgz"; PDFIUM_SHA256="73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08"; PDFIUM_LIB="bin/pdfium.dll"
    else
      echo "ERROR: unsupported Windows architecture: ${ARCH}" >&2; exit 1
    fi ;;
  *)
    echo "ERROR: unsupported platform ${OS}/${ARCH}." >&2
    echo "Add the asset name and SHA-256 from ${PDFIUM_RELEASE_URL} to this script." >&2
    exit 1 ;;
esac

PDFIUM_ARCHIVE="${VENDOR}/downloads/${PDFIUM_ASSET}"
download "${PDFIUM_RELEASE_URL}/${PDFIUM_ASSET}" "${PDFIUM_ARCHIVE}" "${PDFIUM_SHA256}"

PDFIUM_BUILD="${PDFIUM_TAG#chromium/}"

if [ ! -f "${VENDOR}/pdfium/${PDFIUM_LIB}" ] \
  || [ "${FORCE}" -eq 1 ] \
  || ! grep -qx "BUILD=${PDFIUM_BUILD}" "${VENDOR}/pdfium/VERSION" 2>/dev/null; then
  rm -rf "${VENDOR}/pdfium"
  mkdir -p "${VENDOR}/pdfium"
  tar -xzf "${PDFIUM_ARCHIVE}" -C "${VENDOR}/pdfium"
fi
cat > "${VENDOR}/pdfium/SOURCE.txt" <<EOF
PDFIUM binaries ${PDFIUM_TAG} (PDFium ${PDFIUM_VERSION})
URL:     ${PDFIUM_RELEASE_URL}/${PDFIUM_ASSET}
SHA-256: ${PDFIUM_SHA256}
Builds:  https://github.com/bblanchon/pdfium-binaries (MIT)
PDFium:  BSD-3-Clause, see vendor/pdfium/LICENSE
EOF
echo "ok        vendor/pdfium/${PDFIUM_LIB}"

# --- CJK font --------------------------------------------------------------
FONT_DEST="${VENDOR}/fonts/NotoSansSC-VF.ttf"
download "${FONT_URL}" "${FONT_DEST}" "${FONT_SHA256}"
if [ ! -f "${VENDOR}/fonts/OFL.txt" ] || [ "${FORCE}" -eq 1 ]; then
  curl -fL --max-time 60 -o "${VENDOR}/fonts/OFL.txt" "${FONT_OFL_URL}"
fi
cat > "${VENDOR}/fonts/SOURCE.txt" <<EOF
Noto Sans SC (variable TTF), google/fonts commit ${FONT_COMMIT}
URL:     ${FONT_URL}
SHA-256: ${FONT_SHA256}
License: SIL Open Font License 1.1, see vendor/fonts/OFL.txt
EOF
echo "ok        vendor/fonts/NotoSansSC-VF.ttf"

cat <<EOF

Native assets ready. Export these before running the spike:

  export PDFIUM_DYNAMIC_LIB_PATH="${VENDOR}/pdfium/${PDFIUM_LIB}"
  export PDFX_CJK_FONT="${FONT_DEST}"

Then:

  cd "${SPIKE_DIR}" && cargo test -- --nocapture
  cd "${SPIKE_DIR}" && cargo run --release
EOF
