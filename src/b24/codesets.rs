//! The graphic sets a statement body can designate, and the escape-sequence
//! final bytes that select them (ARIB STD-B24 part 2, table 7-3).

/// A character repertoire that can be designated into G0..G3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicSet {
    Kanji,
    Alphanumeric,
    LatinExtension,
    LatinSpecial,
    Hiragana,
    Katakana,
    MosaicA,
    MosaicB,
    MosaicC,
    MosaicD,
    ProportionalAlphanumeric,
    ProportionalHiragana,
    ProportionalKatakana,
    JisX0201Katakana,
    JisX0213Kanji1,
    JisX0213Kanji2,
    AdditionalSymbols,
    /// DRCS set 0..15. Set 0 is two-byte, the rest are one-byte.
    Drcs(u8),
    Macro,
}

/// A designated set together with how many bytes one of its characters takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Codeset {
    pub set: GraphicSet,
    pub bytes: u8,
}

impl Codeset {
    const fn new(set: GraphicSet, bytes: u8) -> Self {
        Self { set, bytes }
    }
}

pub const KANJI: Codeset = Codeset::new(GraphicSet::Kanji, 2);
pub const ALPHANUMERIC: Codeset = Codeset::new(GraphicSet::Alphanumeric, 1);
pub const HIRAGANA: Codeset = Codeset::new(GraphicSet::Hiragana, 1);
pub const KATAKANA: Codeset = Codeset::new(GraphicSet::Katakana, 1);
pub const LATIN_EXTENSION: Codeset = Codeset::new(GraphicSet::LatinExtension, 1);
pub const LATIN_SPECIAL: Codeset = Codeset::new(GraphicSet::LatinSpecial, 1);
pub const MACRO: Codeset = Codeset::new(GraphicSet::Macro, 1);
/// DRCS-0 is the two-byte set; DRCS-1..15 are one byte each.
pub const DRCS_0: Codeset = Codeset::new(GraphicSet::Drcs(0), 2);
pub const DRCS_1: Codeset = Codeset::new(GraphicSet::Drcs(1), 1);

/// Graphic set for an escape sequence's final byte, e.g. `ESC 0x24 0x42` → kanji.
pub fn g_set_by_final(f: u8) -> Option<Codeset> {
    Some(match f {
        0x42 => KANJI,
        0x4a => ALPHANUMERIC,
        0x4b => LATIN_EXTENSION,
        0x4c => LATIN_SPECIAL,
        0x30 => HIRAGANA,
        0x31 => KATAKANA,
        0x32 => Codeset::new(GraphicSet::MosaicA, 1),
        0x33 => Codeset::new(GraphicSet::MosaicB, 1),
        0x34 => Codeset::new(GraphicSet::MosaicC, 1),
        0x35 => Codeset::new(GraphicSet::MosaicD, 1),
        0x36 => Codeset::new(GraphicSet::ProportionalAlphanumeric, 1),
        0x37 => Codeset::new(GraphicSet::ProportionalHiragana, 1),
        0x38 => Codeset::new(GraphicSet::ProportionalKatakana, 1),
        0x49 => Codeset::new(GraphicSet::JisX0201Katakana, 1),
        0x39 => Codeset::new(GraphicSet::JisX0213Kanji1, 2),
        0x3a => Codeset::new(GraphicSet::JisX0213Kanji2, 2),
        0x3b => Codeset::new(GraphicSet::AdditionalSymbols, 2),
        _ => return None,
    })
}

/// Graphic set for a DRCS designation's final byte (`ESC 0x28 0x20 F`).
pub fn drcs_set_by_final(f: u8) -> Option<Codeset> {
    match f {
        0x40 => Some(DRCS_0),
        0x41..=0x4f => Some(Codeset::new(GraphicSet::Drcs(f - 0x40), 1)),
        0x70 => Some(MACRO),
        _ => None,
    }
}
