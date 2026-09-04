//! ARIB STD-B24 statement bodies and data groups, written rather than captured.
//!
//! The fixtures have to *encode* what `crate::decoder` decodes, so every byte
//! sequence here is the inverse of a specific branch of that state machine, and
//! the constants are the ones `src/b24/controls.rs` names.

/// Clear screen (C0, 0x0C). Every statement in the fixture opens with it, which
/// is how the caption before it stops being shown.
pub const CS: u8 = 0x0C;
/// Locking shift 0 — invoke G0 into GL (C0, 0x0F).
pub const LS0: u8 = 0x0F;
/// Active position set (C0, 0x1C): two 6-bit parameters, line then character.
pub const APS: u8 = 0x1C;
/// Small size — half width, half height (C1, 0x88). This is how ruby is sent.
pub const SSZ: u8 = 0x88;
/// Middle size — half width, full height (C1, 0x89).
pub const MSZ: u8 = 0x89;
/// Normal size (C1, 0x8A).
pub const NSZ: u8 = 0x8A;
/// Time control (C1, 0x9D).
pub const TIME: u8 = 0x9D;
/// Colour controls (C1, 0x90): select a palette, or set the foreground or
/// background colour within the current one. Real off-air captures send this
/// before every statement (see `col_select_palette` / `col_set_background`).
pub const COL: u8 = 0x90;
/// Yellow foreground, one of the eight direct single-byte colour codes
/// (C1, 0x80..=0x87, BKF..=WHF). Real captures use this on 19 of 34
/// statements; the rest use `WHF` (white, redundant with the default but
/// still an explicit control code).
pub const YLF: u8 = 0x83;

/// `ESC 0x24 0x42` designates the two-byte kanji set into G0.
pub const DESIGNATE_KANJI_G0: [u8; 3] = [0x1B, 0x24, 0x42];
/// `ESC 0x28 0x20 0x41` designates the one-byte DRCS-1 set into G0.
pub const DESIGNATE_DRCS1_G0: [u8; 4] = [0x1B, 0x28, 0x20, 0x41];

/// Single shift 3 (C0, 0x1D) — invoke G3 for exactly the next character. Real
/// captures use this to fire a default macro at the top of a statement.
pub const SS3: u8 = 0x1D;
/// Control sequence introducer (C1, 0x9B): parameters (decimal digits,
/// `;`-separated), a space, then a final byte naming the command (ARIB
/// STD-B24 part 1, table 7-16).
pub const CSI: u8 = 0x9B;
/// CSI final byte: Set Writing Format (ARIB STD-B24 part 1, table 7-17).
pub const CSI_SWF: u8 = 0x53;
/// CSI final byte: Set Display Format — the active display area's size.
pub const CSI_SDF: u8 = 0x56;
/// CSI final byte: Set Display Position — the active display area's origin.
pub const CSI_SDP: u8 = 0x5F;
/// CSI final byte: Set Horizontal Spacing.
pub const CSI_SHS: u8 = 0x58;
/// CSI final byte: Set Vertical Spacing.
pub const CSI_SVS: u8 = 0x59;
/// CSI final byte: character composition dot designation — the character
/// cell's own width and height, independent of spacing.
pub const CSI_SSM: u8 = 0x57;

/// The continuation arrow ➡ (U+27A1): ARIB additional symbols, ku 92 ten 1.
/// It only decodes if the gaiji table is aligned, which is why it is here.
pub const ARROW: [u8; 2] = [0x7C, 0x21];
/// ⁉ (U+2049): additional symbols, ku 93 ten 79 — a different row of the same
/// table, so the two together catch a table off by a row.
pub const INTERROBANG: [u8; 2] = [0x7D, 0x6F];

/// Data unit parameter for the statement body.
pub const UNIT_STATEMENT_BODY: u8 = 0x20;
/// Data unit parameter for DRCS with one-byte codes.
pub const UNIT_DRCS_1BYTE: u8 = 0x30;

/// Japanese text as two-byte kanji-plane characters.
///
/// Rows 1-83 of the ARIB kanji set are JIS X 0208, which is exactly what
/// EUC-JP's two-byte form encodes, so the ARIB bytes are the EUC bytes with the
/// high bit cleared. Encoding this way rather than tabulating code points keeps
/// the fixture readable and keeps it honest: the decoder reaches the same table
/// from the other direction, through `encoding_rs`'s EUC-JP *decoder*.
pub fn kanji(text: &str) -> Vec<u8> {
    let (encoded, _, had_errors) = encoding_rs::EUC_JP.encode(text);
    assert!(!had_errors, "{text:?} is not representable in EUC-JP");
    assert_eq!(
        encoded.len() % 2,
        0,
        "{text:?} did not encode to two bytes per character"
    );
    encoded
        .iter()
        .map(|byte| {
            assert!(
                *byte >= 0xA1,
                "{text:?} used a single-byte or three-byte EUC form"
            );
            byte - 0x80
        })
        .collect()
}

/// Active position set: `line` counts in the line direction, `character` along
/// the character path, both 6-bit.
pub fn aps(row: u8, column: u8) -> [u8; 3] {
    assert!(row < 0x40 && column < 0x40, "APS parameters are 6-bit");
    [APS, 0x40 | row, 0x40 | column]
}

/// TIME with the 0x20 parameter: how long the caption stays up, in 100 ms units.
pub fn wait(units_of_100ms: u8) -> [u8; 3] {
    assert!(units_of_100ms < 0x40, "the wait parameter is 6-bit");
    [TIME, 0x20, 0x40 | units_of_100ms]
}

/// A CSI command: `CSI`, 1 or 2 decimal parameters separated by `;`, a space,
/// then the final byte.
pub fn csi(params: &[u32], final_byte: u8) -> Vec<u8> {
    assert!(
        !params.is_empty() && params.len() <= 2,
        "the CSI commands this fixture uses take 1 or 2 parameters"
    );
    let mut bytes = vec![CSI];
    for (index, param) in params.iter().enumerate() {
        if index > 0 {
            bytes.push(b';');
        }
        bytes.extend_from_slice(param.to_string().as_bytes());
    }
    bytes.push(0x20);
    bytes.push(final_byte);
    bytes
}

/// Invoke one of the 16 default macros (ARIB STD-B24 part 2, table 7-19) via
/// SS3: `number` selects `DEFAULT_MACROS[number]`.
pub fn macro_invoke(number: u8) -> [u8; 2] {
    assert!(number < 16, "16 default macros, 0..=15");
    [SS3, 0x60 | number]
}

/// `COL SP Pn`: select a palette (`decoder.rs`'s `handle_c1` masks `Pn` to its
/// low 3 bits). Extracted from the real capture, which sends `90 20 44`
/// before every statement — palette 4 — as the first half of a pair with
/// `col_set_background`.
pub fn col_select_palette(palette: u8) -> [u8; 3] {
    assert!(palette < 8, "COL palette select is 3 bits");
    [COL, 0x20, 0x40 | palette]
}

/// `COL Pn`: set the background colour to `index` within the current
/// palette. `decoder.rs`'s `handle_c1` requires `Pn` in `0x48..=0x7f`, which
/// `0x50 | index` always satisfies. Extracted from the real capture, which
/// sends `90 51` right after `col_select_palette(4)` on every statement:
/// index 1 in palette 4 is `CLUT[4][1]`, a half-tone black box behind the
/// text — the standard broadcast background for readability.
pub fn col_set_background(index: u8) -> [u8; 2] {
    assert!(index < 16, "COL colour index is 4 bits");
    [COL, 0x50 | index]
}

/// The preamble every statement in the fixture opens with: clear screen, then
/// the writing-format CSI commands a real broadcast resends after every clear
/// (SWF, SDF, SDP, SHS, SVS, SSM — the values a real off-air capture uses),
/// then macro 1 to designate the graphic sets. Macro 1 puts kanji in G0 and
/// invokes it into GL on its own (see `DEFAULT_MACROS[1]` in `src/b24/mod.rs`
/// — `ESC 24 42`, `LS0`, among the sets it designates), which is why nothing
/// here repeats `DESIGNATE_KANJI_G0`.
pub fn preamble() -> Vec<u8> {
    let mut body = vec![CS];
    body.extend_from_slice(&csi(&[7], CSI_SWF));
    body.extend_from_slice(&csi(&[840, 480], CSI_SDF));
    body.extend_from_slice(&csi(&[58, 29], CSI_SDP));
    body.extend_from_slice(&csi(&[4], CSI_SHS));
    body.extend_from_slice(&csi(&[24], CSI_SVS));
    body.extend_from_slice(&csi(&[36, 36], CSI_SSM));
    body.extend_from_slice(&macro_invoke(1));
    body
}

/// CRC_16 (ARIB STD-B24 part 3 section 9.3, and the same convention as the
/// PSI table CRC in `ts::mpeg_crc32`, at half the width): CCITT polynomial
/// 0x1021, initial value 0, no reflection, no final XOR. Every data group
/// ends with two bytes of this, computed over the data group's own bytes —
/// `data_group_id` through the end of `data_group_data_byte` — and appended
/// big-endian. It is not covered by `data_group_size`, so a decoder that
/// slices the body by that length and stops (as this crate's does) never
/// looks at it; that a validator built against the real off-air captures
/// would reject its absence is exactly why it belongs here regardless.
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0x0000;
    for &byte in bytes {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A 24-bit big-endian length, as the data unit loop and data units use.
pub fn be24(value: usize) -> [u8; 3] {
    assert!(value <= 0xFF_FFFF, "length overflows 24 bits");
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

/// One data unit: the 0x1F separator, its parameter, a 24-bit length, the body.
pub fn data_unit(parameter: u8, body: &[u8]) -> Vec<u8> {
    let mut unit = vec![0x1F, parameter];
    unit.extend_from_slice(&be24(body.len()));
    unit.extend_from_slice(body);
    unit
}

/// A PES payload: `data_identifier`, `private_stream_id`, an empty PES data
/// packet header, then the data group header, body, and its CRC_16.
///
/// `data_group_id` is 0 for management data and the language id for statement
/// data; bit 5 selects the interleaved sequence, so 0x20 is that same group in
/// sequence B.
pub fn data_group(data_identifier: u8, data_group_id: u8, body: &[u8]) -> Vec<u8> {
    assert!(
        matches!(data_identifier, 0x80 | 0x81),
        "data_identifier is 0x80 (caption) or 0x81 (superimpose)"
    );
    assert!(body.len() <= 0xFFFF, "data group size overflows");
    let mut group = vec![data_group_id << 2]; // data_group_id, version 0
    group.push(0x00); // data_group_link_number
    group.push(0x00); // last_data_group_link_number
    group.push((body.len() >> 8) as u8);
    group.push((body.len() & 0xFF) as u8);
    group.extend_from_slice(body);
    // CRC_16 covers everything above (not `data_group_size`'s own count, but
    // the bytes it describes) and is appended after them, outside the size.
    let crc = crc16(&group);
    group.push((crc >> 8) as u8);
    group.push((crc & 0xFF) as u8);

    let mut payload = vec![data_identifier, 0xFF, 0xF0];
    payload.extend_from_slice(&group);
    payload
}

/// Statement data: TMD free, then the data unit loop.
pub fn statement_payload(data_identifier: u8, data_group_id: u8, units: &[u8]) -> Vec<u8> {
    let mut body = vec![0x00]; // TMD free — timing comes from the PES PTS
    body.extend_from_slice(&be24(units.len()));
    body.extend_from_slice(units);
    data_group(data_identifier, data_group_id, &body)
}

/// Management data for a single Japanese caption service.
pub fn management_payload(data_identifier: u8, group_b: bool, writing_format: u8) -> Vec<u8> {
    assert!(writing_format < 0x10, "writing format is 4-bit");
    let body = [
        0x00, // TMD free
        0x01, // one language
        0x00, // language_tag 0 → language_id 1, DMF 0
        b'j',
        b'p',
        b'n',
        writing_format << 4, // writing format, TCS 0 (JIS coding), roll-up 0
        0x00,
        0x00,
        0x00, // empty data unit loop
    ];
    data_group(data_identifier, if group_b { 0x20 } else { 0x00 }, &body)
}

/// Pack pixel levels at 2 bits each, most-significant-bit first, row-major,
/// with no padding between rows — the way the standard transmits them and the
/// way `model::Drcs::level` reads them back.
pub fn pack_two_bit(levels: &[u8]) -> Vec<u8> {
    let mut packed = vec![0u8; levels.len().div_ceil(4)];
    for (index, level) in levels.iter().enumerate() {
        let bit = index * 2;
        packed[bit / 8] |= (level & 0x03) << (6 - (bit % 8));
    }
    packed
}

/// A synthetic 36x36 stencil with the properties a wrong bit order or a wrong
/// row stride would break: an empty first column, ink starting at x = 4, a
/// crossbar, and every row sent twice.
///
/// The vertical stroke is 2 pixels wide (`x ∈ 4..6`), not 4: at 2 bits per
/// pixel, 4 pixels pack to one byte, and a 4-pixel-aligned run (`x ∈ 4..8`)
/// packs to a byte with all four 2-bit groups identical — reversing the pixel
/// order within that byte is then a complete no-op, and a decoder reading the
/// bits in the wrong order would still pass every probe below. `x ∈ 4..6`
/// packs the byte covering `x ∈ 4..8` as `11 11 00 00` (0xF0): reading that
/// byte's four 2-bit groups in reverse order swaps `level(4, _)` from 3 to 0,
/// so a wrong bit order is now something the probes actually catch.
///
/// Only levels 0 and 3 are used. Real Japanese DRCS is two-level sent at a
/// depth that could carry more, and reading it as coverage rather than as a
/// stencil is one of the ways this goes wrong.
pub fn drcs_stencil() -> Vec<u8> {
    let mut levels = vec![0u8; 36 * 36];
    for row_pair in 0..18usize {
        for x in 0..36usize {
            let ink = (4..6).contains(&x) || (row_pair == 4 && (4..32).contains(&x));
            if ink {
                levels[row_pair * 2 * 36 + x] = 3;
                levels[(row_pair * 2 + 1) * 36 + x] = 3;
            }
        }
    }
    levels
}

/// A DRCS data unit body defining one code with one bitmap font.
///
/// The set a one-byte code belongs to is carried in the code itself: the low
/// nibble of `character_code`'s high byte picks DRCS-1..15, so 0x4121 is code
/// 0x21 of DRCS-1.
pub fn drcs_unit_body(character_code: u16, width: u8, height: u8, levels: &[u8]) -> Vec<u8> {
    assert_eq!(
        levels.len(),
        width as usize * height as usize,
        "one level per pixel"
    );
    let mut body = vec![
        0x01, // number_of_code
        (character_code >> 8) as u8,
        (character_code & 0xFF) as u8,
        0x01, // number_of_font
        0x00, // font_id 0, mode 0 → a bitmap follows
        0x02, // depth — the field carries depth - 2, so this is 4 levels
        width,
        height,
    ];
    body.extend_from_slice(&pack_two_bit(levels));
    body
}
