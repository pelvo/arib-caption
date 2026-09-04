//! The caption model: what a decoded caption *is*, with no opinion about how
//! it gets drawn.
//!
//! This is the seam the whole crate is built around. The decoder produces
//! [`Caption`] values and nothing else; a renderer consumes them and emits
//! WebVTT, ASS, or pixels. Ported from libaribcaption's `caption.hpp`, which
//! keeps the same split.
//!
//! Coordinates are in the caption plane the broadcast declared
//! ([`Caption::plane_width`] × [`Caption::plane_height`], typically 960×540
//! for full-seg), origin top-left. A renderer scales that plane onto whatever
//! it is drawing into; nothing here knows the video's real resolution.

/// Colour with straight (non-premultiplied) alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba::new(0, 0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 0xAARRGGBB, the form the B24 CLUT tables are written in.
    pub const fn from_argb(argb: u32) -> Self {
        Self {
            r: ((argb >> 16) & 0xff) as u8,
            g: ((argb >> 8) & 0xff) as u8,
            b: (argb & 0xff) as u8,
            a: ((argb >> 24) & 0xff) as u8,
        }
    }

    pub const fn is_transparent(&self) -> bool {
        self.a == 0
    }
}

/// Which stream the caption came from.
///
/// The two live on different PIDs and are decoded by separate instances: a
/// superimpose (emergency crawl, sports score flash) is not a subtitle and
/// must not be merged into the subtitle track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionKind {
    /// `data_identifier` 0x80 — closed caption proper.
    Caption,
    /// `data_identifier` 0x81 — superimpose.
    Superimpose,
}

/// Caption profile, which decides the default code sets and the plane size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Full-seg (ARIB STD-B24 profile A): 960×540 or 720×480 planes.
    A,
    /// One-seg (profile C): a 320×180 plane, DRCS-heavy.
    C,
}

/// How the statement bytes are encoded.
///
/// Japanese ISDB is always [`Encoding::JisB24`]; the others exist because the
/// same standard was adopted in Brazil (Latin) and the Philippines (UTF-8),
/// and the management data says which is in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// ARIB STD-B24 JIS — the 8-bit ISO/IEC 2022 variant.
    JisB24,
    /// ARIB STD-B24 with a UTF-8 code set (TCS = 1).
    Utf8,
    /// ABNT NBR 15606-1 Latin (SBTVD).
    Latin,
}

/// Per-character styling flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CharStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Outlined text (ARIB "ornament" / stroke), drawn in [`CaptionChar::stroke_color`].
    pub stroke: bool,
}

impl CharStyle {
    pub fn is_default(&self) -> bool {
        *self == CharStyle::default()
    }
}

/// Which sides of the character cell carry a rule (ARIB enclosure).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Enclosure {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl Enclosure {
    pub fn is_none(&self) -> bool {
        *self == Enclosure::default()
    }
}

/// A DRCS glyph: a bitmap the broadcast defined on the fly, for a character
/// that is not in any code set.
///
/// Renderers that draw pixels use [`Drcs::pixels`]; renderers that emit text
/// use [`Drcs::alternative`], which comes from matching [`Drcs::md5`] against
/// a table of glyphs seen in the wild.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drcs {
    pub width: u32,
    pub height: u32,
    /// Bits per pixel as transmitted (`depth` in the standard is the number of
    /// distinct levels; `depth_bits` is how many bits each pixel occupies).
    pub depth: u32,
    pub depth_bits: u32,
    /// The bitmap exactly as transmitted: `depth_bits` per pixel, packed
    /// most-significant-bit first, row-major, with no padding between rows.
    ///
    /// Kept packed rather than expanded because [`Drcs::md5`] is taken over
    /// these bytes, and that hash is the key a replacement table is written
    /// against. Use [`Drcs::level`] to read a pixel.
    pub pixels: Vec<u8>,
    /// Lowercase hex MD5 of `pixels` — the key the replacement table uses.
    pub md5: String,
    /// The Unicode character this glyph is known to stand for, if known.
    pub alternative: Option<char>,
}

impl Drcs {
    /// The level of one pixel, 0 (background) to `depth - 1` (full ink).
    ///
    /// Most Japanese DRCS is two-level, where this is 0 or 1 and the glyph is a
    /// stencil. Deeper glyphs use the extra levels as coverage, which a
    /// renderer can draw as partial alpha.
    pub fn level(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height || self.depth_bits == 0 {
            return 0;
        }
        let bit = (y * self.width + x) * self.depth_bits;
        let mut value = 0u32;
        for i in 0..self.depth_bits {
            let at = bit + i;
            let byte = match self.pixels.get((at / 8) as usize) {
                Some(&b) => b,
                None => return 0,
            };
            let set = (byte >> (7 - (at % 8))) & 1;
            value = (value << 1) | set as u32;
        }
        value as u8
    }
}

/// What a [`CaptionChar`] carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CharBody {
    /// An ordinary character. `pua` is the Private Use Area codepoint for ARIB
    /// additional symbols (gaiji): most map into Unicode 5.2, but fonts that
    /// predate it only carry the PUA code, so both are kept.
    Text { text: String, pua: Option<char> },
    /// A DRCS glyph, keyed into [`Caption::drcs`] by this code.
    Drcs { code: u32 },
    /// A DRCS glyph a replacement table resolved to a real character. The code
    /// is kept so a pixel renderer can still draw the original.
    DrcsReplaced { text: String, code: u32 },
}

/// One character cell, positioned in the caption plane.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptionChar {
    pub body: CharBody,
    pub x: i32,
    pub y: i32,
    pub char_width: i32,
    pub char_height: i32,
    pub char_horizontal_spacing: i32,
    pub char_vertical_spacing: i32,
    pub char_horizontal_scale: f32,
    pub char_vertical_scale: f32,
    pub text_color: Rgba,
    pub back_color: Rgba,
    pub stroke_color: Rgba,
    pub style: CharStyle,
    pub enclosure: Enclosure,
}

impl CaptionChar {
    /// Width of the cell including spacing and scaling — what advances the pen.
    pub fn section_width(&self) -> i32 {
        (((self.char_width + self.char_horizontal_spacing) as f32) * self.char_horizontal_scale)
            .floor() as i32
    }

    /// Height of the cell including spacing and scaling.
    pub fn section_height(&self) -> i32 {
        (((self.char_height + self.char_vertical_spacing) as f32) * self.char_vertical_scale)
            .floor() as i32
    }

    /// The text this cell contributes to a plain-text rendering, if any.
    pub fn text(&self) -> Option<&str> {
        match &self.body {
            CharBody::Text { text, .. } | CharBody::DrcsReplaced { text, .. } => Some(text),
            CharBody::Drcs { .. } => None,
        }
    }
}

/// The character and line progression of one caption region.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WritingMode {
    /// Characters progress left-to-right; lines progress top-to-bottom.
    #[default]
    HorizontalTb,
    /// Characters progress top-to-bottom; columns progress right-to-left.
    VerticalRl,
}

/// A contiguous run of characters on one character path — the unit a renderer
/// positions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptionRegion {
    pub chars: Vec<CaptionChar>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub writing_mode: WritingMode,
    /// Ruby (furigana): half-height text riding above another region. Text
    /// renderers drop these — inlining them corrupts the sentence — while a
    /// pixel renderer draws them where they were sent.
    pub is_ruby: bool,
}

/// How long a caption stays up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Duration {
    /// The broadcast gave no end time: show it until the next caption arrives.
    /// This is the common case, and it is why a renderer cannot emit a cue the
    /// moment it decodes one.
    Indefinite,
    Millis(u32),
}

/// A decoded caption: everything needed to present one screen of text.
#[derive(Clone, Debug, PartialEq)]
pub struct Caption {
    pub kind: CaptionKind,
    /// ISO 639-2 code as transmitted, e.g. `*b"jpn"`.
    pub language: [u8; 3],
    /// Plain text of the caption, ruby excluded, lines joined by `\n`.
    pub text: String,
    pub regions: Vec<CaptionRegion>,
    /// DRCS glyphs defined for this caption, keyed by [`CharBody::Drcs::code`].
    pub drcs: std::collections::HashMap<u32, Drcs>,
    /// Presentation timestamp in milliseconds, taken from the PES that carried
    /// it. `None` when the caller had no PTS to give.
    pub pts_ms: Option<i64>,
    pub duration: Duration,
    /// Clear whatever is on screen before presenting this one (CS).
    pub clear_screen: bool,
    pub plane_width: i32,
    pub plane_height: i32,
    /// The caption asked for a built-in sound to be played.
    pub builtin_sound: Option<u8>,
}

impl Caption {
    /// True when this caption carries nothing to show — a bare clear-screen,
    /// or management data with no statement. Renderers still care: a clear is
    /// how an indefinite caption ends.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// The language code as a string, for output formats that want it.
    pub fn language_str(&self) -> String {
        String::from_utf8_lossy(&self.language).into_owned()
    }
}

impl Default for Caption {
    fn default() -> Self {
        Self {
            kind: CaptionKind::Caption,
            language: *b"jpn",
            text: String::new(),
            regions: Vec::new(),
            drcs: std::collections::HashMap::new(),
            pts_ms: None,
            duration: Duration::Indefinite,
            clear_screen: false,
            plane_width: 960,
            plane_height: 540,
            builtin_sound: None,
        }
    }
}
