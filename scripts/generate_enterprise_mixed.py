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


VP8L_FRAME = bytes.fromhex("2f00000000cd55202202d1ff8804")
XMP_PACKET = (
    b'<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>'
    b'<x:xmpmeta xmlns:x="adobe:ns:meta/">'
    b'<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
    b'<rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/">'
    b"<dc:title>Synthetic</dc:title>"
    b"</rdf:Description></rdf:RDF></x:xmpmeta>"
    b'<?xpacket end="w"?>'
)


def riff_chunk(name: bytes, data: bytes) -> bytes:
    pad = b"\x00" if len(data) % 2 else b""
    return name + struct.pack("<I", len(data)) + data + pad


def chunk_overhead(size: int) -> int:
    padded = size + (size % 2)
    return padded + 8


def webp_with_exif(tiff: bytes) -> bytes:
    """1x1 lossless WebP. The VP8L frame is a fixed image-webp encoding."""
    frame = VP8L_FRAME
    total = (
        22
        + chunk_overhead(len(frame))
        + chunk_overhead(len(tiff))
        + chunk_overhead(len(XMP_PACKET))
    )
    flags = (1 << 2) | (1 << 3)
    vp8x = bytes([flags, 0, 0, 0, 0, 0, 0, 0, 0, 0])
    body = b"WEBP" + riff_chunk(b"VP8X", vp8x)
    body += riff_chunk(b"VP8L", frame)
    body += riff_chunk(b"EXIF", tiff)
    body += riff_chunk(b"XMP ", XMP_PACKET)
    return b"RIFF" + struct.pack("<I", total) + body


def psd_with_layer() -> bytes:
    """1x1 grayscale PSD with one layer named Ink. No flattened scene."""
    header = b"8BPS" + struct.pack(">H", 1) + b"\x00" * 6
    header += struct.pack(">H", 1)
    header += struct.pack(">II", 1, 1)
    header += struct.pack(">HH", 8, 1)
    record = struct.pack(">iiii", 0, 0, 1, 1)
    record += struct.pack(">H", 1)
    record += struct.pack(">hI", 0, 3)
    record += b"8BIMnorm"
    record += bytes([255, 0, 0, 0])
    record += struct.pack(">III", 12, 0, 0)
    record += bytes([3]) + b"Ink"
    channel = b"\x00\x00\x11\x00"
    layer_info = struct.pack(">H", 1) + record + channel
    layer_body = struct.pack(">I", len(layer_info)) + layer_info
    layer = struct.pack(">I", len(layer_body)) + layer_body
    image = struct.pack(">H", 0) + b"\x80"
    return header + struct.pack(">II", 0, 0) + layer + image


class _Bits:
    def __init__(self) -> None:
        self.bits = []

    def write(self, value: int, width: int) -> None:
        for shift in range(width - 1, -1, -1):
            self.bits.append((value >> shift) & 1)

    def to_bytes(self) -> bytes:
        bits = list(self.bits)
        while len(bits) % 8:
            bits.append(0)
        out = bytearray()
        for index in range(0, len(bits), 8):
            byte = 0
            for bit in bits[index : index + 8]:
                byte = (byte << 1) | bit
            out.append(byte)
        return bytes(out)


def avif_still() -> bytes:
    """Minimal still AVIF: sequence-header dimensions, no pixel decode."""
    bits = _Bits()
    bits.write(0, 3)
    bits.write(1, 1)
    bits.write(1, 1)
    bits.write(0, 5)
    bits.write(0, 4)
    bits.write(0, 4)
    bits.write(0, 1)
    bits.write(0, 1)
    for _ in range(6):
        bits.write(0, 1)
    bits.write(0, 1)
    bits.write(1, 1)
    bits.write(0, 1)
    bits.write(1, 1)
    bits.write(0, 1)
    obu = bytes([0x08]) + bits.to_bytes()

    def box(tag: bytes, payload: bytes) -> bytes:
        return struct.pack(">I", 8 + len(payload)) + tag + payload

    def fullbox(tag: bytes, version: int, payload: bytes) -> bytes:
        return box(tag, bytes([version, 0, 0, 0]) + payload)

    ftyp = box(b"ftyp", b"avif" + struct.pack(">I", 0) + b"avif")
    mdat = box(b"mdat", obu)
    offset = len(ftyp) + 8
    iloc = bytes([0x44, 0x00]) + struct.pack(">HHHHII", 1, 1, 0, 1, offset, len(obu))
    pitm = fullbox(b"pitm", 0, struct.pack(">H", 1))
    infe = fullbox(b"infe", 2, struct.pack(">HH", 1, 0) + b"av01")
    iinf = fullbox(b"iinf", 0, struct.pack(">H", 1) + infe)
    meta = fullbox(b"meta", 0, pitm + iinf + fullbox(b"iloc", 0, iloc))
    return ftyp + mdat + meta


def heic_still(tiff: bytes) -> bytes:
    """HEIC container with an Exif item and an ispe size. No HEVC frame."""

    def box(tag: bytes, payload: bytes) -> bytes:
        return struct.pack(">I", 8 + len(payload)) + tag + payload

    def fullbox(tag: bytes, version: int, payload: bytes) -> bytes:
        return box(tag, bytes([version, 0, 0, 0]) + payload)

    payload = struct.pack(">I", 0) + b"Exif\x00\x00" + tiff
    infe = fullbox(b"infe", 2, struct.pack(">HH", 1, 0) + b"Exif" + b"\x00")
    iinf = fullbox(b"iinf", 0, struct.pack(">H", 1) + infe)
    ispe = fullbox(b"ispe", 0, struct.pack(">II", 1, 1))
    ftyp = box(b"ftyp", b"heic" + struct.pack(">I", 0) + b"heic")
    iloc_inner = bytes([0x44, 0x00]) + struct.pack(">HHHHII", 1, 1, 0, 1, 0, len(payload))
    meta = fullbox(b"meta", 0, iinf + fullbox(b"iloc", 0, iloc_inner) + ispe)
    offset = len(ftyp) + len(meta) + 8
    iloc_inner = bytes([0x44, 0x00]) + struct.pack(">HHHHII", 1, 1, 0, 1, offset, len(payload))
    meta = fullbox(b"meta", 0, iinf + fullbox(b"iloc", 0, iloc_inner) + ispe)
    return ftyp + meta + box(b"mdat", payload)


def mov_clip() -> bytes:
    """MOV movie header and video track. No sample frames."""

    def box(tag: bytes, payload: bytes) -> bytes:
        return struct.pack(">I", 8 + len(payload)) + tag + payload

    def fullbox(tag: bytes, version: int, payload: bytes) -> bytes:
        return box(tag, bytes([version, 0, 0, 0]) + payload)

    mvhd = fullbox(
        b"mvhd",
        0,
        struct.pack(">IIII", 0, 0, 1000, 1500) + (b"\x00" * 76) + struct.pack(">I", 2),
    )
    tkhd = fullbox(
        b"tkhd",
        0,
        struct.pack(">IIIII", 0, 0, 1, 0, 1500)
        + (b"\x00" * 8)
        + struct.pack(">HHHH", 0, 0, 0, 0)
        + (b"\x00" * 36)
        + struct.pack(">HHHH", 320, 0, 240, 0),
    )
    hdlr = fullbox(b"hdlr", 0, b"\x00\x00\x00\x00vide")
    trak = box(b"trak", tkhd + box(b"mdia", hdlr))
    xyz = box(b"\xa9xyz", b"\x00\x00\x00\x00+27.1281+100.2508+000.000/")
    moov = box(b"moov", mvhd + trak + box(b"udta", xyz))
    ftyp = box(b"ftyp", b"qt  " + struct.pack(">I", 0) + b"qt  ")
    return ftyp + moov


WEBM_CLIP = bytes.fromhex(
    "1a45dfa3884282857765626d0018538067a91549a966932ad7b184000f4240"
    "44898840c77000000000001654ae6b8cae8ae088b0820280ba8201e0"
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
    write("images/photo.webp", webp_with_exif(tiff_from_jpeg(jpeg)))
    write("images/still.avif", avif_still())
    write("images/art.psd", psd_with_layer())
    write("images/IMG_0001.heic", heic_still(tiff_from_jpeg(jpeg)))
    write("images/IMG_0001.mov", mov_clip())
    write("images/clip.webm", WEBM_CLIP)
    write(
        "office/brief.docx",
        opc(
            {
                "[Content_Types].xml": CONTENT_TYPES.format(
                    overrides="".join(
                        [
                            '<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>',
                            '<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>',
                            '<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>',
                        ]
                    )
                ),
                "_rels/.rels": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
  <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>
""",
                "docProps/core.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/">
  <dc:title>Synthetic brief</dc:title>
  <dc:creator>Gnosis</dc:creator>
  <dc:subject>Catalog</dc:subject>
  <cp:revision>3</cp:revision>
  <dcterms:modified>2026-01-02T03:04:05Z</dcterms:modified>
</cp:coreProperties>
""",
                "docProps/app.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>Gnosis</Application></Properties>
""",
                "word/document.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Overview</w:t></w:r></w:p></w:body>
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
                    overrides="".join(
                        [
                            '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>',
                            '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>',
                        ]
                    )
                ),
                "_rels/.rels": RELS.format(
                    rel_type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
                    target="xl/workbook.xml",
                ),
                "xl/_rels/workbook.xml.rels": RELS.format(
                    rel_type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet",
                    target="worksheets/sheet1.xml",
                ),
                "xl/workbook.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <sheets><sheet name="Prices" sheetId="1" r:id="rId1"/></sheets>
</workbook>
""",
                "xl/worksheets/sheet1.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <sheetData><row r="2"><c r="B2"><f>Prices!A1</f></c></row></sheetData>
</worksheet>
""",
            }
        ),
    )
    write(
        "office/deck.pptx",
        opc(
            {
                "[Content_Types].xml": CONTENT_TYPES.format(
                    overrides="".join(
                        [
                            '<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>',
                            '<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>',
                        ]
                    )
                ),
                "_rels/.rels": RELS.format(
                    rel_type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
                    target="ppt/presentation.xml",
                ),
                "ppt/_rels/presentation.xml.rels": RELS.format(
                    rel_type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide",
                    target="slides/slide1.xml",
                ),
                "ppt/presentation.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst>
</p:presentation>
""",
                "ppt/slides/slide1.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <p:cSld><p:spTree><p:sp>
    <p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
    <p:txBody><a:p><a:r><a:t>Synthetic</a:t></a:r></a:p></p:txBody>
  </p:sp></p:spTree></p:cSld>
</p:sld>
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
