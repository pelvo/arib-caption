#!/usr/bin/env python3
"""Emit libaribcaption-rs/src/b24/tables.rs from libaribcaption's C++ headers.

The tables are data, not code: transcribing ~4000 codepoints by hand would
introduce errors that no test would catch. This parses the upstream arrays and
writes the Rust equivalents, so a re-run against a newer upstream is a diff.
"""
import re, sys, pathlib

REF = pathlib.Path(sys.argv[1])          # libaribcaption checkout
OUT = pathlib.Path(sys.argv[2])          # tables.rs

def arrays(path):
    """{name: [ints]} for every `inline constexpr uint32_t name[] = {...};`"""
    text = path.read_text()
    out = {}
    for m in re.finditer(r"inline constexpr uint32_t (\w+)\[\] = \{(.*?)\};", text, re.S):
        name, body = m.group(1), m.group(2)
        body = re.sub(r"//[^\n]*", "", body)
        out[name] = [int(v, 0) for v in re.findall(r"0x[0-9a-fA-F]+|\b\d+\b", body)]
    return out

conv = arrays(REF / "src/decoder/b24_conv_tables.hpp")
gaiji = arrays(REF / "src/decoder/b24_gaiji_table.hpp")

clut_src = (REF / "src/decoder/b24_colors.cpp").read_text()
palettes = []
for block in re.findall(r"\{\s*((?:ColorRGBA\([^)]*\),?\s*)+)\}", clut_src):
    colors = [tuple(int(v) for v in c.split(",")) for c in re.findall(r"ColorRGBA\(([^)]*)\)", block)]
    palettes.append(colors)
assert len(palettes) == 8, f"expected 8 CLUT palettes, got {len(palettes)}"
assert all(len(p) == 16 for p in palettes), "each palette holds 16 colours"

def rust_u32_table(name, values, doc):
    lines = [f"/// {doc}", f"pub static {name}: [u32; {len(values)}] = ["]
    for i in range(0, len(values), 8):
        chunk = ", ".join(f"0x{v:04x}" for v in values[i:i + 8])
        lines.append(f"    {chunk},")
    lines.append("];")
    return "\n".join(lines)

WANT = [
    ("ALPHANUMERIC_HALFWIDTH", "kAlphanumericTable_Halfwidth", "Alphanumeric set as halfwidth ASCII (MSZ replacement)."),
    ("ALPHANUMERIC_FULLWIDTH", "kAlphanumericTable_Fullwidth", "Alphanumeric set as fullwidth forms — the default for Japanese."),
    ("ALPHANUMERIC_LATIN", "kAlphanumericTable_Latin", "Alphanumeric set for ABNT NBR 15606-1 (Latin)."),
    ("LATIN_EXTENSION", "kLatinExtensionTable", "Latin extension set (SBTVD)."),
    ("LATIN_SPECIAL", "kLatinSpecialTable", "Latin special set (SBTVD)."),
    ("HIRAGANA", "kHiraganaTable", "Hiragana set."),
    ("KATAKANA", "kKatakanaTable", "Katakana set."),
    ("KANA_SYMBOLS_HALFWIDTH", "kKanaSymbolsTable_Halfwidth", "Kana punctuation as halfwidth, for codes 0x79.. under MSZ."),
    ("JIS_X0201_KATAKANA", "kJISX0201KatakanaTable", "JIS X 0201 katakana mapped to fullwidth."),
    ("JIS_X0201_KATAKANA_HALFWIDTH", "kJISX0201KatakanaTable_Halfwidth", "JIS X 0201 katakana kept halfwidth."),
    ("KANJI_SYMBOLS_HALFWIDTH", "kKanjiSymbolsTable_Halfwidth", "Kanji-plane rows 1-2 (punctuation) as halfwidth, under MSZ."),
]

parts = ['''//! Character conversion tables, generated — do not edit by hand.
//!
//! Generated from libaribcaption's `b24_conv_tables.hpp`, `b24_gaiji_table.hpp`
//! and `b24_colors.cpp` (see `scripts/gen_tables.py`). The data is upstream's:
//!
//! > Copyright (C) 2021 magicxqq <xqq@xqq.im>. All rights reserved.
//! > Permission to use, copy, modify, and distribute this software for any
//! > purpose with or without fee is hereby granted, provided that the above
//! > copyright notice and this permission notice appear in all copies.
//!
//! The kanji plane itself is *not* here: rows 1-83 are decoded through EUC-JP
//! (`encoding_rs`) rather than an 7896-entry table, the same trick
//! `libaribb24-rs` uses for EIT text. Rows 84+ are the ARIB additional
//! symbols, which no standard encoding covers, so those are tabulated.

#![allow(clippy::all)]

use crate::model::Rgba;
''']

for rust_name, cpp_name, doc in WANT:
    assert cpp_name in conv, f"{cpp_name} missing from upstream headers"
    parts.append(rust_u32_table(rust_name, conv[cpp_name], doc))

parts.append(rust_u32_table(
    "ADDITIONAL_SYMBOLS_UNICODE", gaiji["kAdditionalSymbolsTable_Unicode"],
    "ARIB additional symbols (gaiji), kanji rows 84.., as Unicode 5.2."))
parts.append(rust_u32_table(
    "ADDITIONAL_SYMBOLS_PUA", gaiji["kAdditionalSymbolsTable_PUA"],
    "The same symbols as Private Use Area codes, for fonts predating Unicode 5.2."))

clut_lines = ["/// The B24 colour lookup table: 8 palettes of 16 colours.",
              "pub static CLUT: [[Rgba; 16]; 8] = ["]
for palette in palettes:
    clut_lines.append("    [")
    for (r, g, b, a) in palette:
        clut_lines.append(f"        Rgba::new({r:>3}, {g:>3}, {b:>3}, {a:>3}),")
    clut_lines.append("    ],")
clut_lines.append("];")
parts.append("\n".join(clut_lines))

OUT.write_text("\n\n".join(parts) + "\n")
sizes = {n: len(conv[c]) for n, c, _ in WANT}
sizes["ADDITIONAL_SYMBOLS_UNICODE"] = len(gaiji["kAdditionalSymbolsTable_Unicode"])
sizes["ADDITIONAL_SYMBOLS_PUA"] = len(gaiji["kAdditionalSymbolsTable_PUA"])
print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")
for k, v in sizes.items():
    print(f"  {k}: {v}")
