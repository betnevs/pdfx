#!/usr/bin/env python3
"""Generate the PDFX fixture corpus using only the Python standard library.

Run this script to (re)create every PDF under fixtures/pdfs/ and the manifest:

    python3 fixtures/generate.py

The output is deterministic (fixed document ids, no timestamps), so rerunning
it on the same Python produces byte-identical files. `--check` regenerates in
memory and fails if the committed files or manifest differ.

Encrypted fixtures use the PDF 1.4 Standard Security Handler (RC4, 40-bit,
revision 2). Passwords are documented in fixtures/README.md and manifest.json.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
import zlib
from dataclasses import dataclass
from pathlib import Path
from typing import Dict, List, Optional, Tuple

FIXTURES_DIR = Path(__file__).resolve().parent
PDFS_DIR = FIXTURES_DIR / "pdfs"
MANIFEST_PATH = FIXTURES_DIR / "manifest.json"

PDF_HEADER = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n"
DOC_ID = hashlib.md5(b"pdfx-fixtures-v1").digest()

PASSWORD_PADDING = bytes.fromhex(
    "28BF4E5E4E758A4164004E56FFFA01082E2E00B6D0683E802F0CA9FE6453697A"
)


# ---------------------------------------------------------------------------
# Small PDF writer
# ---------------------------------------------------------------------------


def build_pdf(
    objects: Dict[int, bytes],
    root_ref: bytes,
    info_ref: Optional[bytes] = None,
    doc_id: Optional[bytes] = None,
    extra_trailer: bytes = b"",
) -> bytes:
    """Serialize numbered objects into a PDF with a classic cross-reference table."""
    max_num = max(objects)
    out = bytearray(PDF_HEADER)
    offsets = [0] * (max_num + 1)

    for number in range(1, max_num + 1):
        offsets[number] = len(out)
        body = objects.get(number, b"null")
        out += f"{number} 0 obj\n".encode("ascii")
        out += body
        out += b"\nendobj\n"

    xref_offset = len(out)
    out += b"xref\n0 " + str(max_num + 1).encode("ascii") + b"\n"
    out += b"0000000000 65535 f \n"
    for number in range(1, max_num + 1):
        out += f"{offsets[number]:010d} 00000 n \n".encode("ascii")

    trailer = b"<< /Size " + str(max_num + 1).encode("ascii") + b" /Root " + root_ref
    if info_ref is not None:
        trailer += b" /Info " + info_ref
    if doc_id is not None:
        trailer += b" /ID [<" + doc_id.hex().encode("ascii") + b"><" + doc_id.hex().encode("ascii") + b">]"
    trailer += extra_trailer + b" >>"

    out += b"trailer\n" + trailer + b"\nstartxref\n" + str(xref_offset).encode("ascii") + b"\n%%EOF\n"
    return bytes(out)


def literal(value: str) -> bytes:
    """ASCII literal string with PDF escapes for the characters that matter."""
    escaped = value.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
    return b"(" + escaped.encode("ascii") + b")"


def hex_string(value: bytes) -> bytes:
    return b"<" + value.hex().encode("ascii") + b">"


def utf16be_hex(text: str) -> bytes:
    """PDF text string with a UTF-16BE byte-order mark (for non-ASCII names)."""
    return hex_string(b"\xfe\xff" + text.encode("utf-16-be"))


def stream_object(entries: bytes, data: bytes) -> bytes:
    return (
        b"<< " + entries + b" /Length " + str(len(data)).encode("ascii") + b" >>\nstream\n"
        + data
        + b"\nendstream"
    )


def content_lines(lines: List[str], size: int = 18) -> bytes:
    parts = [f"BT /F1 {size} Tf 72 720 Td".encode("ascii")]
    for index, line in enumerate(lines):
        if index:
            parts.append(f"0 -{size + 10} Td".encode("ascii"))
        parts.append(literal(line) + b" Tj")
    parts.append(b"ET")
    return b" ".join(parts) + b"\n"


HELVETICA = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
PAGE_FONT_RESOURCES = b"<< /Font << /F1 4 0 R >> >>"


# ---------------------------------------------------------------------------
# Standard Security Handler (R2 / V1 / RC4-40)
# ---------------------------------------------------------------------------


def rc4(key: bytes, data: bytes) -> bytes:
    state = list(range(256))
    j = 0
    for i in range(256):
        j = (j + state[i] + key[i % len(key)]) % 256
        state[i], state[j] = state[j], state[i]

    output = bytearray(len(data))
    i = j = 0
    for index, byte in enumerate(data):
        i = (i + 1) % 256
        j = (j + state[i]) % 256
        state[i], state[j] = state[j], state[i]
        output[index] = byte ^ state[(state[i] + state[j]) % 256]
    return bytes(output)


def pad_password(password: bytes) -> bytes:
    return (password + PASSWORD_PADDING)[:32]


@dataclass
class SecurityHandler:
    """PDF Standard Security Handler revision 2 (40-bit RC4)."""

    user_password: bytes
    owner_password: bytes
    permissions: int
    doc_id: bytes

    def __post_init__(self) -> None:
        owner_seed = pad_password(self.owner_password or self.user_password)
        owner_key = hashlib.md5(owner_seed).digest()[:5]
        self.o_value = rc4(owner_key, pad_password(self.user_password))

        key_material = (
            pad_password(self.user_password)
            + self.o_value
            + struct.pack("<i", self.permissions)
            + self.doc_id
        )
        self.file_key = hashlib.md5(key_material).digest()[:5]
        self.u_value = rc4(self.file_key, PASSWORD_PADDING)

    def object_key(self, number: int, generation: int = 0) -> bytes:
        material = (
            self.file_key
            + number.to_bytes(3, "little")
            + generation.to_bytes(2, "little")
        )
        return hashlib.md5(material).digest()[:10]

    def encrypt(self, number: int, data: bytes, generation: int = 0) -> bytes:
        return rc4(self.object_key(number, generation), data)

    def dictionary(self) -> bytes:
        return (
            b"<< /Filter /Standard /V 1 /R 2 /Length 40 /P "
            + str(self.permissions).encode("ascii")
            + b" /O "
            + hex_string(self.o_value)
            + b" /U "
            + hex_string(self.u_value)
            + b" >>"
        )


def encrypted_text_pdf(
    user_password: str,
    owner_password: str,
    permissions: int,
    title: str,
) -> bytes:
    handler = SecurityHandler(
        user_password.encode("ascii"),
        owner_password.encode("ascii"),
        permissions,
        DOC_ID,
    )
    content = content_lines(
        [
            "PDFX encrypted fixture.",
            "Standard Security Handler, RC4 40-bit, revision 2.",
            "See fixtures/README.md for the passwords.",
        ],
        size=16,
    )
    info_number = 6
    encrypt_number = 7

    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", handler.encrypt(5, content)),
        info_number: (
            b"<< /Title " + hex_string(handler.encrypt(info_number, title.encode("ascii")))
            + b" /Author " + hex_string(handler.encrypt(info_number, b"PDFX QA"))
            + b" >>"
        ),
        encrypt_number: handler.dictionary(),
    }
    return build_pdf(
        objects,
        root_ref=b"1 0 R",
        info_ref=b"6 0 R",
        doc_id=DOC_ID,
        extra_trailer=b" /Encrypt 7 0 R",
    )


# ---------------------------------------------------------------------------
# Individual fixtures
# ---------------------------------------------------------------------------


def text_latin_pdf() -> bytes:
    content = content_lines(
        [
            "PDFX fixture: text layer.",
            "Latin text is extractable from this page.",
            "The quick brown fox jumps over the lazy dog. (1234567890)",
        ]
    )
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", content),
        6: b"<< /Title (PDFX text fixture) /Author (PDFX QA) >>",
    }
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=b"6 0 R")


def _scan_pixels(width: int, height: int) -> bytes:
    pixels = bytearray()
    for y in range(height):
        for x in range(width):
            # Deterministic diagonal bands with a little texture, no text.
            pixels.append(40 + ((x * 3 + y * 5) % 180))
    return bytes(pixels)


def scanned_pdf() -> bytes:
    width, height = 80, 60
    pixels = _scan_pixels(width, height)
    content = b"q 612 0 0 792 0 0 cm /Im1 Do Q\n"
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /XObject << /Im1 6 0 R >> >> /Contents 5 0 R >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", content),
        6: stream_object(
            (
                b"/Type /XObject /Subtype /Image /Width "
                + str(width).encode("ascii")
                + b" /Height "
                + str(height).encode("ascii")
                + b" /ColorSpace /DeviceGray /BitsPerComponent 8"
            ),
            pixels,
        ),
        7: b"<< /Title (PDFX scanned fixture) /Author (PDFX QA) >>",
    }
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=b"7 0 R")


def form_pdf() -> bytes:
    """Two-page AcroForm covering every fillable widget type.

    Page 0 holds text, checkbox, a three-widget radio group, a list box and a
    combo box. Page 1 holds a multiline text field, a required checkbox, a text
    field with a JavaScript keystroke action (which PDFX must never run) and a
    read-only field. Required fields are empty so required-field validation has
    something to find.
    """
    page_one_content = content_lines(
        [
            "PDFX AcroForm fixture",
            "Full name:  Ada Lovelace",
            "Email (required):",
            "Subscribe to updates",
            "Gender (required):",
            "Country:",
            "Department:",
        ],
        size=16,
    )
    page_two_content = content_lines(
        [
            "PDFX AcroForm fixture - page 2",
            "Notes:",
            "I agree to the terms (required)",
            "JavaScript guard:",
            "Read-only value:",
        ],
        size=16,
    )
    text_field_appearance = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 248 24] /Resources << /Font << /Helv 4 0 R >> >>",
        b"BT /Helv 12 Tf 4 7 Td (Ada Lovelace) Tj ET\n",
    )
    checkbox_off = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        b"q 0 0 0 rg 1 1 14 14 re S Q\n",
    )
    checkbox_yes = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        b"q 0 0 0 rg 1 1 14 14 re S 3 8 m 6 4 l 12 12 l S Q\n",
    )
    radio_off = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        b"q 0 0 0 RG 1 1 14 14 re S Q\n",
    )
    radio_on = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        b"q 0 0 0 RG 1 1 14 14 re S 4 4 8 8 re f Q\n",
    )
    # A keystroke action that would rewrite the value if a viewer executed it.
    javascript_guard = b"<< /S /JavaScript /JS (event.value = 'JS_RAN';) >>"

    objects = {
        1: (
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields ["
            b"6 0 R 7 0 R 13 0 R 14 0 R 20 0 R 21 0 R 22 0 R 23 0 R 24 0 R 25 0 R"
            b"] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> "
            b"/NeedAppearances true >> >>"
        ),
        2: b"<< /Type /Pages /Kids [3 0 R 12 0 R] /Count 2 >>",
        # Page 0.
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R "
            b"/Annots [6 0 R 13 0 R 7 0 R 15 0 R 16 0 R 17 0 R 20 0 R 21 0 R] >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", page_one_content),
        6: (
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (full_name) "
            b"/V (Ada Lovelace) /Rect [72 620 320 644] /P 3 0 R /F 4 "
            b"/DA (/Helv 12 Tf 0 g) /AP << /N 8 0 R >> >>"
        ),
        7: (
            b"<< /Type /Annot /Subtype /Widget /FT /Btn /T (subscribe) "
            b"/V /Off /AS /Off /Rect [72 520 88 536] /P 3 0 R /F 4 "
            b"/AP << /N << /Off 9 0 R /Yes 10 0 R >> >> >>"
        ),
        8: text_field_appearance,
        9: checkbox_off,
        10: checkbox_yes,
        # Page 1.
        11: stream_object(b"", page_two_content),
        12: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 11 0 R "
            b"/Annots [22 0 R 23 0 R 24 0 R 25 0 R] >>"
        ),
        # Required, empty text field. First missing required field.
        13: (
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (email) /Ff 2 "
            b"/Rect [72 560 320 584] /P 3 0 R /F 4 /DA (/Helv 12 Tf 0 g) >>"
        ),
        # Required radio group; the widgets are separate kids.
        14: (
            b"<< /FT /Btn /T (gender) /Ff 32770 /Kids [15 0 R 16 0 R 17 0 R] "
            b"/V /Off >>"
        ),
        15: (
            b"<< /Type /Annot /Subtype /Widget /Parent 14 0 R /Rect [72 480 88 496] "
            b"/P 3 0 R /F 4 /AP << /N << /male 18 0 R /Off 19 0 R >> >> /AS /Off >>"
        ),
        16: (
            b"<< /Type /Annot /Subtype /Widget /Parent 14 0 R /Rect [96 480 112 496] "
            b"/P 3 0 R /F 4 /AP << /N << /female 18 0 R /Off 19 0 R >> >> /AS /Off >>"
        ),
        17: (
            b"<< /Type /Annot /Subtype /Widget /Parent 14 0 R /Rect [120 480 136 496] "
            b"/P 3 0 R /F 4 /AP << /N << /other 18 0 R /Off 19 0 R >> >> /AS /Off >>"
        ),
        18: radio_on,
        19: radio_off,
        # Single-select list box with one value already selected.
        20: (
            b"<< /Type /Annot /Subtype /Widget /FT /Ch /T (country) "
            b"/Rect [300 440 460 520] /P 3 0 R /F 4 /DA (/Helv 12 Tf 0 g) "
            b"/Opt [(China) (Japan) (Germany)] /V (Japan) /I [1] >>"
        ),
        # Drop-down (combo box), initially empty.
        21: (
            b"<< /Type /Annot /Subtype /Widget /FT /Ch /T (department) /Ff 131072 "
            b"/Rect [300 560 460 584] /P 3 0 R /F 4 /DA (/Helv 12 Tf 0 g) "
            b"/Opt [(Engineering) (Design) (Sales)] >>"
        ),
        # Page 1 fields.
        22: (
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (notes) /Ff 4096 "
            b"/Rect [72 600 400 680] /P 12 0 R /F 4 /DA (/Helv 12 Tf 0 g) >>"
        ),
        23: (
            b"<< /Type /Annot /Subtype /Widget /FT /Btn /T (agree) /Ff 2 "
            b"/V /Off /AS /Off /Rect [72 560 88 576] /P 12 0 R /F 4 "
            b"/AP << /N << /Off 9 0 R /Yes 10 0 R >> >> >>"
        ),
        24: (
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (js_guard) "
            b"/Rect [72 500 320 524] /P 12 0 R /F 4 /DA (/Helv 12 Tf 0 g) "
            b"/AA << /K " + javascript_guard + b" >> >>"
        ),
        25: (
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (readonly_note) /Ff 1 "
            b"/V (Read only) /Rect [72 440 320 464] /P 12 0 R /F 4 "
            b"/DA (/Helv 12 Tf 0 g) >>"
        ),
        26: b"<< /Title (PDFX form fixture) /Author (PDFX QA) >>",
    }
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=b"26 0 R")


def fill_appearance_pdf() -> bytes:
    """Two-page AcroForm for appearance-fidelity tests.

    Every widget carries a deliberately stale red appearance whose drawing does
    not match its `/V`. The form exercises the field shapes PDFium's annotation
    API cannot rewrite directly: a terminal text field whose value lives on the
    parent while the widget sits in `/Kids` (`membership_id`), and a field
    nested under another field (`person.name`). Page 1 is rotated 90 degrees so
    a filled appearance must compensate for `/Rotate`. `/NM` is present on two
    widgets so copying the page can be checked for independent annotation
    names.
    """
    page_one_content = content_lines(
        [
            "PDFX fill appearance fixture",
            "Membership id:",
            "Person name:",
            "Subscribe:",
            "Gender:",
            "Country:",
        ],
        size=16,
    )
    page_two_content = content_lines(
        [
            "PDFX fill appearance fixture - page 2 (rotated)",
            "Comments:",
            "Department:",
        ],
        size=16,
    )
    # Every stale appearance is red so a saved page can be scanned for leftover
    # source pixels without relying on text extraction.
    stale_text = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 248 24] "
        b"/Resources << /Font << /Helv 4 0 R >> >>",
        b"q 1 0 0 rg BT /Helv 12 Tf 2 6 Td (STALE) Tj ET Q\n",
    )
    stale_checkbox_off = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        b"q 1 0 0 RG 1 w 1 1 14 14 re S Q\n",
    )
    stale_checkbox_on = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        b"q 1 0 0 rg 2 2 12 12 re f Q\n",
    )
    stale_radio_on = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        b"q 1 0 0 rg 3 3 10 10 re f Q\n",
    )
    stale_radio_off = stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 16 16]",
        b"q 1 0 0 RG 1 w 1 1 14 14 re S Q\n",
    )

    objects = {
        1: (
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields ["
            b"6 0 R 9 0 R 12 0 R 15 0 R 21 0 R 25 0 R 26 0 R"
            b"] /DA (/Helv 10 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> >> >>"
        ),
        2: b"<< /Type /Pages /Kids [3 0 R 23 0 R] /Count 2 >>",
        # Page 0, unrotated.
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R "
            b"/Annots [7 0 R 11 0 R 12 0 R 16 0 R 17 0 R 18 0 R 21 0 R] >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", page_one_content),
        # Terminal text field with the value on the parent and the widget in
        # /Kids: Pdfium writes /V to the annotation, which would be a no-op.
        6: (
            b"<< /FT /Tx /T (membership_id) /V (OLD-0001) "
            b"/DA (/Helv 12 Tf 0 g) /Kids [7 0 R] >>"
        ),
        7: (
            b"<< /Type /Annot /Subtype /Widget /Parent 6 0 R "
            b"/Rect [72 656 320 680] /P 3 0 R /F 4 /NM (w-membership) "
            b"/AP << /N 8 0 R >> >>"
        ),
        8: stale_text,
        # Nested field: /Fields -> person -> name -> widget. The value lives on
        # the child; the widget's /Parent chain carries the dotted name.
        9: b"<< /T (person) /FT /Tx /DA (/Helv 10 Tf 0 g) /Kids [10 0 R] >>",
        10: (
            b"<< /T (name) /Parent 9 0 R /V (Grace Hopper) /Kids [11 0 R] >>"
        ),
        11: (
            b"<< /Type /Annot /Subtype /Widget /Parent 10 0 R "
            b"/Rect [72 606 320 630] /P 3 0 R /F 4 /NM (w-person-name) "
            b"/AP << /N 8 0 R >> >>"
        ),
        12: (
            b"<< /Type /Annot /Subtype /Widget /FT /Btn /T (subscribe) "
            b"/V /Off /AS /Off /Rect [72 556 88 572] /P 3 0 R /F 4 "
            b"/AP << /N << /Off 13 0 R /Yes 14 0 R >> >> >>"
        ),
        13: stale_checkbox_off,
        14: stale_checkbox_on,
        15: (
            b"<< /FT /Btn /T (gender) /Ff 32770 /Kids [16 0 R 17 0 R 18 0 R] "
            b"/V /Off >>"
        ),
        16: (
            b"<< /Type /Annot /Subtype /Widget /Parent 15 0 R "
            b"/Rect [72 506 88 522] /P 3 0 R /F 4 /NM (w-gender-male) "
            b"/AP << /N << /male 19 0 R /Off 20 0 R >> >> /AS /Off >>"
        ),
        17: (
            b"<< /Type /Annot /Subtype /Widget /Parent 15 0 R "
            b"/Rect [96 506 112 522] /P 3 0 R /F 4 "
            b"/AP << /N << /female 19 0 R /Off 20 0 R >> >> /AS /Off >>"
        ),
        18: (
            b"<< /Type /Annot /Subtype /Widget /Parent 15 0 R "
            b"/Rect [120 506 136 522] /P 3 0 R /F 4 "
            b"/AP << /N << /other 19 0 R /Off 20 0 R >> >> /AS /Off >>"
        ),
        19: stale_radio_on,
        20: stale_radio_off,
        21: (
            b"<< /Type /Annot /Subtype /Widget /FT /Ch /T (country) "
            b"/Rect [300 480 460 560] /P 3 0 R /F 4 /DA (/Helv 12 Tf 0 g) "
            b"/Opt [(China) (Japan) (Germany)] /V (China) /I [0] "
            b"/AP << /N 22 0 R >> >>"
        ),
        22: stale_text,
        # Page 1, rotated 90 degrees.
        23: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Rotate 90 "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 24 0 R "
            b"/Annots [25 0 R 26 0 R] >>"
        ),
        24: stream_object(b"", page_two_content),
        25: (
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (comments) /Ff 4096 "
            b"/Rect [72 600 400 680] /P 23 0 R /F 4 /DA (/Helv 12 Tf 0 g) "
            b"/V (Old comment) /AP << /N 8 0 R >> >>"
        ),
        26: (
            b"<< /Type /Annot /Subtype /Widget /FT /Ch /T (department) /Ff 131072 "
            b"/Rect [72 500 300 524] /P 23 0 R /F 4 /DA (/Helv 12 Tf 0 g) "
            b"/Opt [(Engineering) (Design) (Sales)] /AP << /N 22 0 R >> >>"
        ),
        27: b"<< /Title (PDFX fill appearance fixture) /Author (PDFX QA) >>",
    }
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=b"27 0 R")
def _signature_appearance(width: int, height: int, label: bytes) -> bytes:
    return stream_object(
        b"/Type /XObject /Subtype /Form /BBox [0 0 "
        + str(width).encode("ascii")
        + b" "
        + str(height).encode("ascii")
        + b"] /Resources << /Font << /Helv 4 0 R >> >>",
        b"BT /Helv 12 Tf 4 14 Td " + label + b" Tj ET\n",
    )


def _signature_dictionary() -> bytes:
    """A structurally valid placeholder signature dictionary.

    The contents are not a real PKCS#7 signature. `/Name` and `/Reason` carry
    canary text the editor and its logs must never surface.
    """
    contents = bytes(range(64))
    return (
        b"<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached "
        b"/ByteRange [0 1234 5678 9012] /Contents "
        + hex_string(contents)
        + b" /Name (PDFX SIGNER CANARY) /Reason (PDFX REASON CANARY) "
        b"/M (D:20260101000000Z) >>"
    )


def signature_signed_pdf() -> bytes:
    """One signed signature field and one empty hierarchical signature field.

    The signed widget stores `/V` as an indirect signature dictionary. The
    second field is a non-terminal `/FT /Sig` parent whose widget kid inherits
    `/FT` through `/Parent`; the text field guards against over-counting.
    """
    content = content_lines(
        [
            "PDFX digital signature fixture",
            "This document contains a signature and a signature field.",
            "PDFX only warns; it never signs and never edits signature objects.",
        ],
        size=16,
    )
    objects = {
        1: (
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 9 0 R 12 0 R] "
            b"/DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> /SigFlags 1 >> >>"
        ),
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R "
            b"/Annots [6 0 R 10 0 R 12 0 R] >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", content),
        # Merged signed signature field; /V points at the signature dictionary.
        6: (
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T "
            + hex_string(b"Approval")
            + b" /V 7 0 R /Rect [72 650 300 690] /P 3 0 R /F 4 "
            b"/AP << /N 8 0 R >> >>"
        ),
        7: _signature_dictionary(),
        8: _signature_appearance(228, 40, b"(Signed)"),
        # Non-terminal empty signature field; the widget inherits /FT but has
        # no /T, so it is part of this field rather than a second one.
        9: (
            b"<< /FT /Sig /T "
            + hex_string(b"countersign_group")
            + b" /Kids [10 0 R] >>"
        ),
        10: (
            b"<< /Type /Annot /Subtype /Widget /Parent 9 0 R "
            b"/Rect [72 560 300 600] /P 3 0 R /F 4 >>"
        ),
        11: b"<< /Title (PDFX signature fixture) /Author (PDFX QA) >>",
        # An ordinary text field must not be mistaken for a signature field.
        12: (
            b"<< /Type /Annot /Subtype /Widget /FT /Tx /T "
            + hex_string(b"name")
            + b" /Rect [72 480 300 504] /P 3 0 R /F 4 /DA (/Helv 12 Tf 0 g) >>"
        ),
    }
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=b"11 0 R")


def signature_field_only_pdf() -> bytes:
    """Two empty signature fields, one merged and one inherited from /Parent."""
    content = content_lines(
        [
            "PDFX signature field fixture",
            "This document has signature fields but no signature value.",
        ],
        size=16,
    )
    objects = {
        1: (
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 9 0 R] "
            b"/DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> /SigFlags 1 >> >>"
        ),
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R "
            b"/Annots [6 0 R 10 0 R] >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", content),
        6: (
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T "
            + hex_string(b"sign_here")
            + b" /Rect [72 650 300 690] /P 3 0 R /F 4 /AP << /N 8 0 R >> >>"
        ),
        8: _signature_appearance(228, 40, b"(Sign here)"),
        9: (
            b"<< /FT /Sig /T "
            + hex_string(b"witness_group")
            + b" /Kids [10 0 R] >>"
        ),
        10: (
            b"<< /Type /Annot /Subtype /Widget /Parent 9 0 R "
            b"/Rect [72 560 300 600] /P 3 0 R /F 4 >>"
        ),
        11: b"<< /Title (PDFX signature field fixture) /Author (PDFX QA) >>",
    }
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=b"11 0 R")


def encrypted_signature_restricted_pdf() -> bytes:
    """An owner-restricted encrypted document with a signed signature field.

    The empty user password lets the editor open it read-only; Save As must
    still confirm both the signature and ADR-0007 unencrypted-output warnings.
    Strings are encrypted with the Standard Security Handler, matching
    `encrypted_text_pdf`.
    """
    handler = SecurityHandler(b"", b"pdfx-owner", -3900, DOC_ID)
    content = content_lines(
        [
            "PDFX encrypted signature fixture.",
            "Owner restrictions deny modification, copying and annotation.",
        ],
        size=16,
    )
    contents = bytes(range(1, 65))
    signature_number = 7
    objects = {
        1: (
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 9 0 R] "
            b"/DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> /SigFlags 1 >> >>"
        ),
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R "
            b"/Annots [6 0 R 10 0 R] >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", handler.encrypt(5, content)),
        6: (
            b"<< /Type /Annot /Subtype /Widget /FT /Sig /T "
            + hex_string(handler.encrypt(6, b"approval"))
            + b" /V 7 0 R /Rect [72 650 300 690] /P 3 0 R /F 4 >>"
        ),
        7: (
            b"<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached "
            b"/ByteRange [0 1234 5678 9012] /Contents "
            + hex_string(handler.encrypt(signature_number, contents))
            + b" /Name "
            + hex_string(handler.encrypt(signature_number, b"PDFX SIGNER CANARY"))
            + b" >>"
        ),
        9: (
            b"<< /FT /Sig /T "
            + hex_string(handler.encrypt(9, b"witness_group"))
            + b" /Kids [10 0 R] >>"
        ),
        10: (
            b"<< /Type /Annot /Subtype /Widget /Parent 9 0 R "
            b"/Rect [72 560 300 600] /P 3 0 R /F 4 >>"
        ),
        11: (
            b"<< /Title "
            + hex_string(
                handler.encrypt(11, b"PDFX encrypted signature fixture")
            )
            + b" /Author " + hex_string(handler.encrypt(11, b"PDFX QA"))
            + b" >>"
        ),
        12: handler.dictionary(),
    }
    return build_pdf(
        objects,
        root_ref=b"1 0 R",
        info_ref=b"11 0 R",
        doc_id=DOC_ID,
        extra_trailer=b" /Encrypt 12 0 R",
    )


def structure_pdf() -> bytes:
    page_content = content_lines(
        ["PDFX structure fixture", "Bookmarks, a link, metadata and an attachment."],
        size=16,
    )
    xmp = (
        b'<?xpacket begin="\xef\xbb\xbf" id="W5M0MpCehiHzreSzNTczkc9d"?>\n'
        b'<x:xmpmeta xmlns:x="adobe:ns:meta/">\n'
        b' <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">\n'
        b'  <rdf:Description rdf:about="https://example.com/pdfx"\n'
        b'    xmlns:dc="http://purl.org/dc/elements/1.1/">\n'
        b"   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">PDFX structure fixture</rdf:li></rdf:Alt></dc:title>\n"
        b"  </rdf:Description>\n"
        b" </rdf:RDF>\n"
        b"</x:xmpmeta>\n"
        b'<?xpacket end="w"?>\n'
    )
    attachment = b"PDFX fixture attachment.\n"

    objects = {
        1: (
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R "
            b"/Names << /EmbeddedFiles << /Names [(notes.txt) 11 0 R] >> >> "
            b"/Metadata 13 0 R >>"
        ),
        2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R "
            b"/Annots [6 0 R] >>"
        ),
        4: HELVETICA,
        5: stream_object(b"", page_content),
        6: (
            b"<< /Type /Annot /Subtype /Link /Rect [72 600 320 622] /Border [0 0 1] "
            b"/A << /S /URI /URI (https://example.com/pdfx) >> >>"
        ),
        7: b"<< /Title (PDFX structure fixture) /Author (PDFX QA) /Subject (Bookmarks, links, metadata, attachment) >>",
        8: b"<< /Type /Outlines /First 9 0 R /Last 10 0 R /Count 2 >>",
        9: b"<< /Title (Introduction) /Parent 8 0 R /Next 10 0 R /Dest [3 0 R /Fit] >>",
        10: b"<< /Title (Details) /Parent 8 0 R /Prev 9 0 R /Dest [3 0 R /Fit] >>",
        11: b"<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 12 0 R >> >>",
        12: stream_object(b"/Type /EmbeddedFile /Subtype /text#2Fplain", attachment),
        13: stream_object(b"/Type /Metadata /Subtype /XML", xmp),
    }
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=b"7 0 R")


def layers_pdf() -> bytes:
    """Two pages with three UTF-16-named optional content groups.

    Page 1 paints a red rectangle for 红色图层, with a blue rectangle for
    蓝色图层 nested inside the same marked-content block; page 2 paints the
    red rectangle and a separate green rectangle for 绿色图层. 红色图层 is
    on by default and spans both pages, 绿色图层 only exists on page 2, and
    蓝色图层 is off by default.  The marked-content property names are
    resolved through /Resources /Properties, and both /OCProperties and its
    /D configuration are indirect objects.
    """
    resources = b"<< /Properties << /MC0 10 0 R /MC1 11 0 R /MC2 12 0 R >> >>"
    page_one_content = (
        b"/OC /MC0 BDC\n"
        b"q 1 0 0 rg 72 500 200 100 re f Q\n"
        b"/OC /MC2 BDC\n"
        b"q 0 0 1 rg 300 500 200 100 re f Q\n"
        b"EMC\n"
        b"EMC\n"
    )
    page_two_content = (
        b"/OC /MC0 BDC\n"
        b"q 1 0 0 rg 72 500 200 100 re f Q\n"
        b"EMC\n"
        b"/OC /MC1 BDC\n"
        b"q 0 1 0 rg 300 500 200 100 re f Q\n"
        b"EMC\n"
    )
    objects = {
        1: b"<< /Type /Catalog /Pages 2 0 R /OCProperties 8 0 R >>",
        2: b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
        3: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources " + resources + b" /Contents 4 0 R >>"
        ),
        4: stream_object(b"", page_one_content),
        5: (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources " + resources + b" /Contents 6 0 R >>"
        ),
        6: stream_object(b"", page_two_content),
        8: b"<< /OCGs [10 0 R 11 0 R 12 0 R] /D 9 0 R >>",
        9: (
            b"<< /Type /OCProperties /ListMode /VisiblePages "
            b"/Order [10 0 R 11 0 R 12 0 R] /ON [10 0 R 11 0 R] "
            b"/OFF [12 0 R] /BaseState /OFF "
            b"/AS [<< /Event /View /Category [/View] /OCGs [10 0 R 11 0 R] >>] >>"
        ),
        10: b"<< /Type /OCG /Name " + utf16be_hex("红色图层") + b" >>",
        11: b"<< /Type /OCG /Name " + utf16be_hex("绿色图层") + b" >>",
        12: b"<< /Type /OCG /Name " + utf16be_hex("蓝色图层") + b" >>",
        13: b"<< /Title (PDFX layers fixture) /Author (PDFX QA) >>",
    }
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=b"13 0 R")


def rotated_pdf() -> bytes:
    pages: List[Tuple[int, int, int]] = [(612, 792, 90), (792, 612, 0), (288, 432, 270)]
    objects: Dict[int, bytes] = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        3: HELVETICA,
    }
    kids: List[bytes] = []
    next_number = 4
    for index, (width, height, rotate) in enumerate(pages, start=1):
        page_number = next_number
        content_number = next_number + 1
        next_number += 2
        kids.append(f"{page_number} 0 R".encode("ascii"))
        objects[page_number] = (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 "
            + str(width).encode("ascii")
            + b" "
            + str(height).encode("ascii")
            + b"] /Rotate "
            + str(rotate).encode("ascii")
            + b" /Resources << /Font << /F1 3 0 R >> >> /Contents "
            + str(content_number).encode("ascii")
            + b" 0 R >>"
        )
        objects[content_number] = stream_object(
            b"",
            content_lines([f"Rotated page {index} (rotate {rotate})"], size=16),
        )

    objects[2] = (
        b"<< /Type /Pages /Kids ["
        + b" ".join(kids)
        + b"] /Count "
        + str(len(pages)).encode("ascii")
        + b" >>"
    )
    objects[next_number] = b"<< /Title (PDFX rotated fixture) /Author (PDFX QA) >>"
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=f"{next_number} 0 R".encode("ascii"))


def large_pdf(page_count: int) -> bytes:
    objects: Dict[int, bytes] = {
        1: b"<< /Type /Catalog /Pages 2 0 R >>",
        3: HELVETICA,
    }
    kids: List[bytes] = []
    next_number = 4
    for index in range(1, page_count + 1):
        page_number = next_number
        content_number = next_number + 1
        next_number += 2
        kids.append(f"{page_number} 0 R".encode("ascii"))
        objects[page_number] = (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 3 0 R >> >> /Contents "
            + str(content_number).encode("ascii")
            + b" 0 R >>"
        )
        objects[content_number] = stream_object(
            b"",
            content_lines([f"PDFX large fixture: page {index} of {page_count}"], size=12),
        )

    objects[2] = (
        b"<< /Type /Pages /Kids ["
        + b" ".join(kids)
        + b"] /Count "
        + str(page_count).encode("ascii")
        + b" >>"
    )
    objects[next_number] = b"<< /Title (PDFX large fixture) /Author (PDFX QA) >>"
    return build_pdf(objects, root_ref=b"1 0 R", info_ref=f"{next_number} 0 R".encode("ascii"))


def malformed_fixtures(base: bytes) -> Dict[str, bytes]:
    truncated = base[: int(len(base) * 0.55)]

    startxref = base.rfind(b"startxref\n")
    eof = base.find(b"\n%%EOF", startxref)
    bad_xref = base[: startxref] + b"startxref\n999999" + base[eof:]

    return {
        "malformed-truncated.pdf": truncated,
        "malformed-bad-xref.pdf": bad_xref,
    }


# ---------------------------------------------------------------------------
# Manifest
# ---------------------------------------------------------------------------


@dataclass
class FixtureSpec:
    filename: str
    category: str
    description: str
    page_count: int
    valid: bool
    password: Optional[str] = None
    owner_password: Optional[str] = None


def build_all(large_pages: int) -> Tuple[Dict[str, bytes], List[FixtureSpec]]:
    text = text_latin_pdf()
    files: Dict[str, bytes] = {
        "text-latin.pdf": text,
        "scanned-image-only.pdf": scanned_pdf(),
        "encrypted-user-password.pdf": encrypted_text_pdf(
            user_password="pdfx-user",
            owner_password="pdfx-owner",
            permissions=-4,
            title="PDFX encrypted fixture (user password)",
        ),
        "encrypted-owner-restrictions.pdf": encrypted_text_pdf(
            user_password="",
            owner_password="pdfx-owner",
            permissions=-3900,  # printing allowed; modification, copying, annotation denied
            title="PDFX encrypted fixture (owner restrictions)",
        ),
        "encrypted-print-denied.pdf": encrypted_text_pdf(
            user_password="",
            owner_password="pdfx-owner",
            permissions=-8,  # printing denied; editing, copying and annotation allowed
            title="PDFX encrypted fixture (print denied)",
        ),
        "form-acroform.pdf": form_pdf(),
        "form-fill-appearance.pdf": fill_appearance_pdf(),
        "signature-signed.pdf": signature_signed_pdf(),
        "signature-field-only.pdf": signature_field_only_pdf(),
        "encrypted-signature-restricted.pdf": encrypted_signature_restricted_pdf(),
        "structure-metadata.pdf": structure_pdf(),
        "layers.pdf": layers_pdf(),
        f"large-{large_pages}-pages.pdf": large_pdf(large_pages),
        "rotated-mixed-size.pdf": rotated_pdf(),
    }
    files.update(malformed_fixtures(text))

    specs = [
        FixtureSpec(
            "text-latin.pdf",
            "text",
            "单页拉丁文本，使用 Helvetica；文字可被提取。",
            1,
            True,
        ),
        FixtureSpec(
            "scanned-image-only.pdf",
            "scanned",
            "纯图片页（DeviceGray 图像），没有文本层。",
            1,
            True,
        ),
        FixtureSpec(
            "encrypted-user-password.pdf",
            "encrypted",
            "需要用户密码 pdfx-user 才能打开；所有者密码为 pdfx-owner。",
            1,
            True,
            password="pdfx-user",
            owner_password="pdfx-owner",
        ),
        FixtureSpec(
            "encrypted-owner-restrictions.pdf",
            "encrypted",
            "用户密码为空可直接打开；所有者密码 pdfx-owner 限制修改、复制与注释。",
            1,
            True,
            password="",
            owner_password="pdfx-owner",
        ),
        FixtureSpec(
            "encrypted-print-denied.pdf",
            "encrypted",
            "用户密码为空可直接打开；所有者密码 pdfx-owner 仅禁止打印，编辑仍允许。",
            1,
            True,
            password="",
            owner_password="pdfx-owner",
        ),
        FixtureSpec(
            "form-acroform.pdf",
            "form",
            "两页 AcroForm：文本、复选框、单选组、列表框、下拉框，含必填与只读字段；"
            "js_guard 字段带 JavaScript 击键动作（PDFX 不执行）。",
            2,
            True,
        ),
        FixtureSpec(
            "form-fill-appearance.pdf",
            "form-fill",
            "两页 AcroForm：字段值位于带 /Kids 的终端字段、嵌套 /Parent 字段、"
            "复选框、单选组、列表框与下拉框；第二页旋转 90°，所有控件带故意过期的"
            "红色外观流，用于填写后外观保真测试。",
            2,
            True,
        ),
        FixtureSpec(
            "signature-signed.pdf",
            "signature",
            "一个已签名的签名字段（/V 指向含 /ByteRange、/Contents 的签名字典）"
            "与一个通过 /Parent 继承 /FT 的空签名字段；另含普通文本字段。",
            1,
            True,
        ),
        FixtureSpec(
            "signature-field-only.pdf",
            "signature",
            "两个空签名字段：终态 widget 与通过 /Parent 继承 /FT 的嵌套字段；无 /V。",
            1,
            True,
        ),
        FixtureSpec(
            "encrypted-signature-restricted.pdf",
            "signature",
            "用户密码为空可直接打开；所有者密码 pdfx-owner 限制修改、复制与注释；"
            "含已签名与空签名字段，用于只读 + Save As 的签名确认组合。",
            1,
            True,
            password="",
            owner_password="pdfx-owner",
        ),
        FixtureSpec(
            "structure-metadata.pdf",
            "structure",
            "书签、URI 链接、Info/XMP 元数据与内嵌附件。",
            1,
            True,
        ),
        FixtureSpec(
            "layers.pdf",
            "layers",
            "两页三个可选内容组：红色图层跨两页并默认显示，绿色图层仅第二页，"
            "蓝色图层默认隐藏且嵌套在红色图层的标记内容块中；组名使用 UTF-16 中文。",
            2,
            True,
        ),
        FixtureSpec(
            f"large-{large_pages}-pages.pdf",
            "large",
            f"{large_pages} 页的最小“大文件”样例；完整 2000 页压力样例可用 --large-pages 2000 生成。",
            large_pages,
            True,
        ),
        FixtureSpec(
            "rotated-mixed-size.pdf",
            "rotated",
            "三页，旋转角 90/0/270，混合页面尺寸。",
            3,
            True,
        ),
        FixtureSpec(
            "malformed-truncated.pdf",
            "malformed",
            "在文件中部截断，缺少 xref 与 trailer。",
            1,
            False,
        ),
        FixtureSpec(
            "malformed-bad-xref.pdf",
            "malformed",
            "结构完整但 startxref 指向不存在的偏移。",
            1,
            False,
        ),
    ]
    return files, specs


def manifest_document(files: Dict[str, bytes], specs: List[FixtureSpec]) -> dict:
    return {
        "version": 1,
        "generated_by": "fixtures/generate.py",
        "notes": (
            "Deterministic fixture corpus. Regenerate with python3 fixtures/generate.py. "
            "Passwords are documented in fixtures/README.md."
        ),
        "fixtures": [
            {
                "id": spec.filename.removesuffix(".pdf"),
                "category": spec.category,
                "path": f"pdfs/{spec.filename}",
                "sha256": hashlib.sha256(files[spec.filename]).hexdigest(),
                "size_bytes": len(files[spec.filename]),
                "page_count": spec.page_count,
                "valid": spec.valid,
                "password": spec.password,
                "owner_password": spec.owner_password,
                "description": spec.description,
            }
            for spec in specs
        ],
    }


def manifest_bytes(document: dict) -> bytes:
    return (json.dumps(document, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--large-pages",
        type=int,
        default=500,
        help="page count for the committed large fixture (default: 500)",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify committed fixtures match a fresh generation instead of writing",
    )
    args = parser.parse_args()

    files, specs = build_all(args.large_pages)
    manifest = manifest_bytes(manifest_document(files, specs))

    if args.check:
        problems: List[str] = []
        for filename, expected in files.items():
            path = PDFS_DIR / filename
            if not path.exists():
                problems.append(f"missing {path}")
            elif path.read_bytes() != expected:
                problems.append(f"stale {path}")
        if not MANIFEST_PATH.exists():
            problems.append(f"missing {MANIFEST_PATH}")
        elif MANIFEST_PATH.read_bytes() != manifest:
            problems.append(f"stale {MANIFEST_PATH}")
        if problems:
            print("fixture check failed:", file=sys.stderr)
            for problem in problems:
                print(f"  - {problem}", file=sys.stderr)
            return 1
        print(f"fixtures OK ({len(files)} PDFs)")
        return 0

    PDFS_DIR.mkdir(parents=True, exist_ok=True)
    for filename, data in files.items():
        (PDFS_DIR / filename).write_bytes(data)
    MANIFEST_PATH.write_bytes(manifest)
    print(f"wrote {len(files)} PDFs to {PDFS_DIR}")
    print(f"wrote {MANIFEST_PATH}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
