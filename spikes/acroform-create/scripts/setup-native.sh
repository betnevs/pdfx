#!/usr/bin/env bash
# Fetches the pinned PDFium build used by the AcroForm creation spike.
#
# The probe reopens every generated PDF with PDFium. It reuses the PDFium
# version and checksum already pinned by the M0→M1 engine spike, so there is a
# single native dependency in the repository.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
spike="$(cd "$here/.." && pwd)"
engine="$spike/../pdf-engine"

if [[ -f "$engine/vendor/pdfium/lib/libpdfium.dylib" || -f "$engine/vendor/pdfium/bin/pdfium.dll" || -f "$engine/vendor/pdfium/lib/libpdfium.so" ]]; then
  echo "PDFium already present in $engine/vendor/pdfium"
  exit 0
fi

if [[ ! -x "$engine/scripts/setup-native.sh" ]]; then
  echo "error: $engine/scripts/setup-native.sh is missing" >&2
  exit 1
fi

# setup-native.sh downloads both PDFium and the CJK font. The font is unused by
# this spike but harmless; the script is the repository's single pin/verify path.
"$engine/scripts/setup-native.sh" "$@"
