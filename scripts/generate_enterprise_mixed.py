#!/usr/bin/env python3
"""Rebuild the synthetic fixtures/enterprise-mixed tree.

The files are redistributable stand-ins, not client documents. Office
packages are minimal OPC zip files. The JPEG and TIFF carry the same EXIF
segment as fixtures/images/with-exif.jpg. Run from the repository root.
"""

import struct
import zlib
import zipfile
from pathlib import Path
from typing import Union

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "fixtures" / "enterprise-mixed"
JPEG = ROOT / "fixtures" / "images" / "with-exif.jpg"


def chunk(tag: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(tag + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)


def write(rel: str, data: Union[bytes, str]) -> None:
    path = OUT / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(data, str):
        path.write_text(data, encoding="utf-8")
    else:
        path.write_bytes(data)


def tiff_from_jpeg(jpeg: bytes) -> bytes:
    if jpeg[:4] != b"\xff\xd8\xff\xe1":
        raise SystemExit("with-exif.jpg is not an APP1 JPEG")
    length = int.from_bytes(jpeg[4:6], "big")
    segment = jpeg[6 : 4 + length]
    if not segment.startswith(b"Exif\x00\x00"):
        raise SystemExit("APP1 segment has no Exif header")
    return segment[6:]


def png_with_text() -> bytes:
    ihdr = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)
    # Filter byte plus one RGB pixel.
    idat = zlib.compress(b"\x00\xff\x00\x00")
    text = b"Description\x00Synthetic caption"
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"tEXt", text)
        + chunk(b"IDAT", idat)
        + chunk(b"IEND", b"")
    )


def pdf_note() -> bytes:
    objects = [
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Length 55 >>\nstream\nBT /F1 12 Tf 72 72 Td (Synthetic note) Tj ET\nendstream\nendobj\n",
    ]
    body = b"%PDF-1.4\n"
    offsets = [0]
    for obj in objects:
        offsets.append(len(body))
        body += obj
    xref = len(body)
    body += f"xref\n0 {len(offsets)}\n".encode()
    body += b"0000000000 65535 f \n"
    for offset in offsets[1:]:
        body += f"{offset:010d} 00000 n \n".encode()
    body += f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return body


def opc(parts: dict[str, str]) -> bytes:
    path = OUT / "_tmp.zip"
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, text in parts.items():
            archive.writestr(name, text)
    data = path.read_bytes()
    path.unlink()
    return data


CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  {overrides}
</Types>
"""

RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="{rel_type}" Target="{target}"/>
</Relationships>
"""


def main() -> None:
    jpeg = JPEG.read_bytes()
    write("src/pricing.rs", "pub fn price(amount: f64) -> f64 {\n    amount\n}\n")
    write("src/quote.rs", "pub fn price(amount: f64) -> f64 {\n    amount * 2.0\n}\n")
    write(
        "elixir/entry.ex",
        "defmodule Ledger.Entry do\n  def post(amount), do: amount\nend\n",
    )
    write(
        "cpp/rule.hpp",
        "#pragma once\nnamespace ledger {\nint apply(int amount);\n}\n",
    )
    write(
        "cpp/rule.cpp",
        '#include "rule.hpp"\nnamespace ledger {\nint apply(int amount) { return amount; }\n}\n',
    )
    write(
        "js/price.js",
        "import ledger from './ledger.js';\nfunction price(x) { return x; }\nclass Quote {}\n",
    )
    write(
        "ts/price.ts",
        "export function price(x: number) { return x; }\nexport class Quote {}\n",
    )
    write(
        "python/price.py",
        "import ledger\n\ndef price(x):\n    return x\n\nclass Quote:\n    pass\n",
    )
    write(
        "java/Quote.java",
        "import ledger.Entry;\nclass Quote { int price(int x) { return x; } }\n",
    )
    write(
        "csharp/Quote.cs",
        "using Ledger;\nclass Quote { int Price(int x) { return x; } }\n",
    )
    write(
        "docs/overview.md",
        "# Overview\n\nSynthetic catalog for the baseline harness.\n",
    )
    write("data/prices.json", '{"sku":"A1","amount":10}\n')
    write("data/prices.csv", "sku,amount\nA1,10\n")
    write("images/camera.jpg", jpeg)
    write("images/scan.tif", tiff_from_jpeg(jpeg))
    write("images/caption.png", png_with_text())
    write(
        "office/brief.docx",
        opc(
            {
                "[Content_Types].xml": CONTENT_TYPES.format(
                    overrides='<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>'
                ),
                "_rels/.rels": RELS.format(
                    rel_type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
                    target="word/document.xml",
                ),
                "word/document.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:r><w:t>Synthetic brief</w:t></w:r></w:p></w:body>
</w:document>
""",
            }
        ),
    )
    write(
        "office/book.xlsx",
        opc(
            {
                "[Content_Types].xml": CONTENT_TYPES.format(
                    overrides='<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
                ),
                "_rels/.rels": RELS.format(
                    rel_type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
                    target="xl/workbook.xml",
                ),
                "xl/workbook.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheets><sheet name="Prices" sheetId="1" r:id="rId1" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"/></sheets>
</workbook>
""",
            }
        ),
    )
    write(
        "office/deck.pptx",
        opc(
            {
                "[Content_Types].xml": CONTENT_TYPES.format(
                    overrides='<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>'
                ),
                "_rels/.rels": RELS.format(
                    rel_type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
                    target="ppt/presentation.xml",
                ),
                "ppt/presentation.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:sldIdLst><p:sldId id="256" r:id="rId1" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"/></p:sldIdLst>
</p:presentation>
""",
            }
        ),
    )
    write("office/note.pdf", pdf_note())
    write("archives/broken.zip", b"PK\x03\x04" + b"\x00" * 16)
    write("proprietary/widget.bin", b"GNOS\x00widget")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
