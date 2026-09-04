//! Code point → character, for every graphic set a caption can designate.
//!
//! The kanji plane is the one set that is *not* tabulated here. Rows 1-83 of
//! the ARIB kanji set are JIS X 0208, so they come out of `encoding_rs`'s
//! EUC-JP decoder by mapping (ku, ten) onto the two-byte form, and JIS X 0213
//! plane 2 onto the three-byte form — 8000 codepoints of table for two byte
//! additions. Rows 84 and up are the ARIB additional symbols, which no
//! standard encoding knows, and those are in [`super::tables`].

use super::codesets::GraphicSet;
use super::tables;

/// The character a slot resolves to, plus its Private Use Area alias when the
/// symbol has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub text: String,
    pub pua: Option<char>,
}

/// GETA MARK — what a slot with no mapping shows, matching every other ARIB
/// decoder's convention.
pub const GETA: char = '〓';

/// Size flags that decide the halfwidth substitutions.
#[derive(Clone, Copy, Debug)]
pub struct SizeContext {
    /// True under MSZ (half width, full height), which is where a fullwidth
    /// glyph would look wrong in a text rendering.
    pub is_msz: bool,
    pub replace_msz_ascii: bool,
    pub replace_msz_japanese: bool,
}

impl SizeContext {
    fn ascii(&self) -> bool {
        self.is_msz && self.replace_msz_ascii
    }

    fn japanese(&self) -> bool {
        self.is_msz && self.replace_msz_japanese
    }
}

/// An undefined slot. Upstream's tables spell one of these two ways — a zero,
/// or U+FFFD — and both mean the same thing: nothing is mapped here. GETA is
/// what goes on screen instead, since a replacement character reads as a
/// decoding failure when the truth is that the standard leaves the slot empty.
fn mapped(cp: u32) -> Option<char> {
    if cp == 0 || cp == 0xfffd {
        return None;
    }
    char::from_u32(cp)
}

fn from_table(table: &[u32], index: usize) -> Resolved {
    let cp = table.get(index).copied().unwrap_or(0);
    Resolved {
        text: mapped(cp).unwrap_or(GETA).into(),
        pua: None,
    }
}

/// Resolve a one-byte set's character. `code` is the 0x21..0x7E byte.
pub fn resolve_single(set: GraphicSet, code: u8, size: SizeContext) -> Option<Resolved> {
    let index = (code - 0x21) as usize;
    Some(match set {
        GraphicSet::Hiragana | GraphicSet::ProportionalHiragana => {
            // 0x79.. are the punctuation shared with katakana, which have real
            // halfwidth forms.
            if code >= 0x79 && size.japanese() {
                from_table(&tables::KANA_SYMBOLS_HALFWIDTH, (code - 0x79) as usize)
            } else {
                from_table(&tables::HIRAGANA, index)
            }
        }
        GraphicSet::Katakana | GraphicSet::ProportionalKatakana => {
            if code >= 0x79 && size.japanese() {
                from_table(&tables::KANA_SYMBOLS_HALFWIDTH, (code - 0x79) as usize)
            } else {
                from_table(&tables::KATAKANA, index)
            }
        }
        GraphicSet::JisX0201Katakana => {
            if size.japanese() {
                from_table(&tables::JIS_X0201_KATAKANA_HALFWIDTH, index)
            } else {
                from_table(&tables::JIS_X0201_KATAKANA, index)
            }
        }
        GraphicSet::Alphanumeric | GraphicSet::ProportionalAlphanumeric => {
            if size.ascii() {
                from_table(&tables::ALPHANUMERIC_HALFWIDTH, index)
            } else {
                from_table(&tables::ALPHANUMERIC_FULLWIDTH, index)
            }
        }
        GraphicSet::LatinExtension => from_table(&tables::LATIN_EXTENSION, index),
        GraphicSet::LatinSpecial => from_table(&tables::LATIN_SPECIAL, index),
        // Mosaic sets are block graphics for data broadcasting; captions do not
        // use them and there is nothing sensible to put in a text stream.
        _ => return None,
    })
}

/// Resolve a two-byte set's character from its (ku, ten) pair, both 0-based.
pub fn resolve_double(set: GraphicSet, ku: u32, ten: u32, size: SizeContext) -> Resolved {
    const GAIJI_BEGIN_KU: u32 = 84;

    match set {
        GraphicSet::JisX0213Kanji2 => euc_jp_plane2(ku, ten),
        GraphicSet::Kanji | GraphicSet::JisX0213Kanji1 | GraphicSet::AdditionalSymbols => {
            if ku >= GAIJI_BEGIN_KU {
                return additional_symbol(ku - GAIJI_BEGIN_KU, ten);
            }
            // Rows 1-2 are punctuation and fullwidth forms; under MSZ those get
            // halfwidth substitutes so a text rendering is not full of
            // double-width commas.
            if ku < 2 && size.japanese() {
                return from_table(&tables::KANJI_SYMBOLS_HALFWIDTH, (ku * 94 + ten) as usize);
            }
            let mut resolved = euc_jp_plane1(ku, ten);
            if size.ascii() {
                if let Some(c) = resolved.text.chars().next() {
                    let cp = c as u32;
                    // Ideographic space and fullwidth ASCII have exact
                    // halfwidth counterparts a fixed offset away.
                    if cp == 0x3000 || (0xff01..=0xff5e).contains(&cp) {
                        if let Some(half) = char::from_u32((cp & 0xff) + 0x20) {
                            resolved.text = half.into();
                        }
                    }
                }
            }
            resolved
        }
        _ => Resolved {
            text: GETA.into(),
            pua: None,
        },
    }
}

fn additional_symbol(row: u32, ten: u32) -> Resolved {
    let index = (row * 94 + ten) as usize;
    let ucs = tables::ADDITIONAL_SYMBOLS_UNICODE
        .get(index)
        .copied()
        .unwrap_or(0);
    let pua = tables::ADDITIONAL_SYMBOLS_PUA
        .get(index)
        .copied()
        .unwrap_or(0);
    // A PUA code equal to the character itself, or outside the BMP private
    // area, carries no extra information.
    let pua = if pua == ucs || !(0xe000..=0xf8ff).contains(&pua) {
        None
    } else {
        char::from_u32(pua)
    };
    Resolved {
        text: mapped(ucs).unwrap_or(GETA).into(),
        pua,
    }
}

/// JIS plane 1 (ku, ten) through EUC-JP's two-byte form.
fn euc_jp_plane1(ku: u32, ten: u32) -> Resolved {
    let bytes = [(ku + 0xa1) as u8, (ten + 0xa1) as u8];
    decode_euc_jp(&bytes)
}

/// JIS plane 2 (ku, ten) through EUC-JP's three-byte form (SS3 prefix).
fn euc_jp_plane2(ku: u32, ten: u32) -> Resolved {
    let bytes = [0x8f, (ku + 0xa1) as u8, (ten + 0xa1) as u8];
    decode_euc_jp(&bytes)
}

fn decode_euc_jp(bytes: &[u8]) -> Resolved {
    let (text, _, had_error) = encoding_rs::EUC_JP.decode(bytes);
    let c = if had_error {
        GETA
    } else {
        match text.chars().next() {
            Some('\u{fffd}') | None => GETA,
            Some(c) => c,
        }
    };
    Resolved {
        text: c.into(),
        pua: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: SizeContext = SizeContext {
        is_msz: false,
        replace_msz_ascii: true,
        replace_msz_japanese: true,
    };
    const MSZ: SizeContext = SizeContext {
        is_msz: true,
        replace_msz_ascii: true,
        replace_msz_japanese: true,
    };

    #[test]
    fn kanji_comes_out_of_euc_jp() {
        // ku 15, ten 1 (0-based 14/0) is 亜 — row 16 column 1 of JIS X 0208.
        let r = resolve_double(GraphicSet::Kanji, 15, 0, NORMAL);
        assert_eq!(r.text, "亜");
        // ku 3 (0-based) row 4 is hiragana in the kanji plane: あ.
        assert_eq!(resolve_double(GraphicSet::Kanji, 3, 0, NORMAL).text, "ぁ");
    }

    #[test]
    fn additional_symbols_carry_a_pua_alias() {
        // Kanji row 90 column 54 (1-based) is 🈑, mapped into the PUA as well.
        let r = resolve_double(GraphicSet::AdditionalSymbols, 89, 53, NORMAL);
        assert_eq!(r.text.chars().next().unwrap() as u32, 0x1f211);
        assert!(r.pua.is_some(), "expected a PUA alias");
    }

    #[test]
    fn hiragana_and_alphanumeric() {
        assert_eq!(
            resolve_single(GraphicSet::Hiragana, 0x22, NORMAL)
                .unwrap()
                .text,
            "あ"
        );
        // Alphanumeric is fullwidth by default, halfwidth under MSZ.
        assert_eq!(
            resolve_single(GraphicSet::Alphanumeric, 0x41, NORMAL)
                .unwrap()
                .text,
            "Ａ"
        );
        assert_eq!(
            resolve_single(GraphicSet::Alphanumeric, 0x41, MSZ)
                .unwrap()
                .text,
            "A"
        );
    }

    #[test]
    fn unmapped_slots_are_geta_not_panics() {
        // Mosaic sets have no text form at all.
        assert!(resolve_single(GraphicSet::MosaicA, 0x21, NORMAL).is_none());
        // A ten beyond the row is out of every table.
        assert_eq!(
            resolve_double(GraphicSet::AdditionalSymbols, 93, 93, NORMAL).text,
            GETA.to_string()
        );
    }
}
