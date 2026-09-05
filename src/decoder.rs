//! The statement-body state machine: control codes and character bytes in,
//! [`Caption`] out.
//!
//! A port of libaribcaption's `decoder_impl.cpp`. The shape is kept — same
//! control-code semantics, same region derivation, same positioning arithmetic
//! — because those are the parts that took someone years of watching real
//! broadcasts to get right.
//!
//! Two deliberate departures:
//!
//! - A statement that runs out of bytes mid-control-code stops and keeps what
//!   it decoded, where upstream discards the whole caption. On live TV a PES
//!   arrives truncated now and then, and half a line of subtitle beats none.
//! - The interleave group is read as bit 5 of `data_group_id` (see
//!   [`crate::pes`]), so a mid-stream management change is applied instead of
//!   being taken for a retransmission.

use std::collections::HashMap;

use crate::b24::charset::{self, SizeContext, GETA};
use crate::b24::codesets::{self, Codeset, GraphicSet};
use crate::b24::controls::{c0, c1, csi, esc};
use crate::b24::tables::CLUT;
use crate::b24::DEFAULT_MACROS;
use crate::model::{
    Caption, CaptionChar, CaptionKind, CaptionRegion, CharBody, CharStyle, Drcs, Duration,
    Enclosure, Encoding, Profile, Rgba, WritingMode,
};
use crate::pes::{self, DataGroup, DataUnitKind, Group, ParseError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    Pes(ParseError),
    /// The data group parsed but its contents contradict the standard in a way
    /// that leaves nothing to salvage.
    Malformed(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Pes(e) => write!(f, "{e}"),
            DecodeError::Malformed(m) => write!(f, "malformed: {m}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<ParseError> for DecodeError {
    fn from(e: ParseError) -> Self {
        DecodeError::Pes(e)
    }
}

/// Convert the raw management `format` field (Table 9-7) to the value used by
/// CSI SWF (Table 7-17). The gap at raw value 5 is reserved, not an offset form
/// of SWF 4.
fn management_format_to_swf(raw: u8) -> Option<u8> {
    match raw {
        0..=4 => Some(raw),
        6..=13 => Some(raw - 1),
        _ => None,
    }
}

/// Direction selected by every defined SWF value in Table 7-17.
/// Largest dimension, in dots, that any real ARIB caption plane uses.
///
/// ARIB STD-B24's display formats top out far below this; the cap exists only
/// so that a malformed stream cannot drive the layout arithmetic into an
/// overflow. Every CSI geometry parameter passes through [`plane_dots`].
const MAX_PLANE_DOTS: i32 = 8192;

/// Clamps a CSI geometry parameter to a dimension a caption plane could have.
///
/// The parameters arrive as decimal digits in the statement body, so they are
/// entirely stream-controlled: `param as i32` was both a silently narrowing
/// cast and the source of the downstream multiply overflows. Anything beyond
/// `MAX_PLANE_DOTS` cannot describe a real plane, so it is pinned there rather
/// than rejected -- the caption is still worth rendering as best it can be.
fn plane_dots(param: u32) -> i32 {
    i32::try_from(param)
        .unwrap_or(MAX_PLANE_DOTS)
        .clamp(0, MAX_PLANE_DOTS)
}

fn writing_mode_for_swf(swf: u8) -> Option<WritingMode> {
    match swf {
        0 | 2 | 4 | 5 | 7 | 9 | 11 => Some(WritingMode::HorizontalTb),
        1 | 3 | 6 | 8 | 10 | 12 => Some(WritingMode::VerticalRl),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Full-seg (A) or one-seg (C). Decides the default graphic sets and plane.
    pub profile: Profile,
    /// Which language's statements to decode. 1 is the main caption service;
    /// a second language, when present, is 2.
    pub language_id: u8,
    /// Under MSZ (half-width) mode, emit halfwidth ASCII rather than the
    /// fullwidth forms the code set nominally maps to. Wanted for text output
    /// (WebVTT, ASS): a fullwidth `Ａ` in a half-width cell reads as a typo.
    pub replace_msz_fullwidth_ascii: bool,
    /// The same for Japanese punctuation and kana (。「」、・ー).
    ///
    /// Off by default, unlike upstream. Japanese captions are written in MSZ
    /// almost throughout — that is how ~34 characters fit on a line — so
    /// switching this on rewrites ordinary prose: 「キャラクター」 comes out as
    /// 「キャラクタｰ」, a halfwidth long-vowel mark inside fullwidth kana. In a
    /// bitmap renderer, where the cell really is half as wide, it is the right
    /// glyph; in a browser drawing a WebVTT cue it just looks broken. A pixel
    /// renderer should turn it on.
    pub replace_msz_fullwidth_japanese: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            profile: Profile::A,
            language_id: 1,
            replace_msz_fullwidth_ascii: true,
            replace_msz_fullwidth_japanese: false,
        }
    }
}

/// Decodes one caption service.
///
/// Feed it the PES payloads of a single PID in order. State persists between
/// calls, which is required rather than incidental: the management data that
/// sets the plane size arrives in its own PES, and the graphic sets a statement
/// designates hold until the next one changes them.
pub struct Decoder {
    kind: CaptionKind,
    options: Options,
    encoding: Encoding,

    // ── code set state ──
    gx: [Codeset; 4],
    gl: usize,
    gr: usize,
    drcs_maps: Vec<HashMap<u16, Drcs>>,

    // ── writing format ──
    swf: u8,
    plane_width: i32,
    plane_height: i32,
    area_x: i32,
    area_y: i32,
    area_width: i32,
    area_height: i32,
    char_width: i32,
    char_height: i32,
    char_h_spacing: i32,
    char_v_spacing: i32,
    char_h_scale: f32,
    char_v_scale: f32,

    // ── pen ──
    pos_inited: bool,
    pos_x: i32,
    pos_y: i32,

    // ── styling ──
    palette: usize,
    text_color: Rgba,
    back_color: Rgba,
    stroke_color: Rgba,
    style: CharStyle,
    enclosure: Enclosure,

    // ── per-caption accumulation ──
    regions: Vec<CaptionRegion>,
    text: String,
    drcs_used: HashMap<u32, Drcs>,
    clear_screen: bool,
    wait_ms: Option<u32>,
    builtin_sound: Option<u8>,

    language: [u8; 3],
    prev_management_group: Option<Group>,
}

impl Decoder {
    pub fn new(kind: CaptionKind, options: Options) -> Self {
        let mut decoder = Self {
            kind,
            options,
            encoding: Encoding::JisB24,
            gx: [codesets::KANJI; 4],
            gl: 0,
            gr: 2,
            drcs_maps: vec![HashMap::new(); 16],
            swf: 7,
            plane_width: 960,
            plane_height: 540,
            area_x: 0,
            area_y: 0,
            area_width: 960,
            area_height: 540,
            char_width: 36,
            char_height: 36,
            char_h_spacing: 4,
            char_v_spacing: 24,
            char_h_scale: 1.0,
            char_v_scale: 1.0,
            pos_inited: false,
            pos_x: 0,
            pos_y: 0,
            palette: 0,
            text_color: CLUT[0][7],
            back_color: CLUT[0][8],
            stroke_color: Rgba::TRANSPARENT,
            style: CharStyle::default(),
            enclosure: Enclosure::default(),
            regions: Vec::new(),
            text: String::new(),
            drcs_used: HashMap::new(),
            clear_screen: false,
            wait_ms: None,
            builtin_sound: None,
            language: *b"jpn",
            prev_management_group: None,
        };
        decoder.reset_state();
        decoder
    }

    /// Forget everything: called on a discontinuity, or when seeking.
    pub fn flush(&mut self) {
        self.prev_management_group = None;
        for map in &mut self.drcs_maps {
            map.clear();
        }
        self.reset_state();
    }

    /// Decode one caption PES payload, with the PTS of the PES that carried it.
    ///
    /// Returns `None` for a payload that legitimately produces no caption:
    /// management data, a retransmission, or another language's statement.
    pub fn decode(
        &mut self,
        payload: &[u8],
        pts_ms: Option<i64>,
    ) -> Result<Option<Caption>, DecodeError> {
        let parsed = pes::parse(payload)?;
        if parsed.kind != self.kind {
            return Err(DecodeError::Malformed(
                "data_identifier is not the stream this decoder was made for",
            ));
        }

        self.regions.clear();
        self.text.clear();
        self.drcs_used.clear();
        self.clear_screen = false;
        self.wait_ms = None;

        match &parsed.group {
            DataGroup::Management(m) => {
                // Management data repeats within its interleave group; only the
                // first copy means anything (ARIB TR-B14 4.2.4).
                if self.prev_management_group == Some(parsed.header.group) {
                    return Ok(None);
                }

                // Validate the selected language before committing any of the
                // management state. In particular, a reserved format must not
                // poison duplicate suppression or replace the prior epoch's
                // usable writing format.
                let selected = m
                    .languages
                    .iter()
                    .find(|lang| lang.language_id == self.options.language_id)
                    .map(|lang| {
                        let swf = management_format_to_swf(lang.format)
                            .ok_or(DecodeError::Malformed("reserved management writing format"))?;
                        let encoding = if lang.tcs == 1 {
                            Encoding::Utf8
                        } else {
                            Encoding::JisB24
                        };
                        Ok::<_, DecodeError>((lang.iso639, swf, encoding))
                    })
                    .transpose()?;

                self.prev_management_group = Some(parsed.header.group);
                if let Some((language, swf, encoding)) = selected {
                    self.language = language;
                    self.swf = swf;
                    self.encoding = encoding;
                }
                self.reset_state();

                // DRCS definitions may arrive with the management data, and
                // apply to the statements that follow.
                for unit in m.units() {
                    let unit = unit?;
                    self.handle_unit(unit.kind, unit.bytes);
                }
            }
            DataGroup::Statement(s) => {
                if s.language_id != self.options.language_id {
                    return Ok(None);
                }
                for unit in s.units() {
                    let unit = unit?;
                    self.handle_unit(unit.kind, unit.bytes);
                }
            }
        }

        if self.regions.is_empty() && !self.clear_screen && self.wait_ms.is_none() {
            return Ok(None);
        }

        Ok(Some(Caption {
            kind: self.kind,
            language: self.language,
            text: std::mem::take(&mut self.text),
            regions: std::mem::take(&mut self.regions),
            drcs: std::mem::take(&mut self.drcs_used),
            pts_ms,
            duration: match self.wait_ms.take() {
                Some(ms) => Duration::Millis(ms),
                None => Duration::Indefinite,
            },
            clear_screen: self.clear_screen,
            plane_width: self.plane_width,
            plane_height: self.plane_height,
            builtin_sound: self.builtin_sound.take(),
        }))
    }

    fn handle_unit(&mut self, kind: DataUnitKind, bytes: &[u8]) {
        match kind {
            DataUnitKind::StatementBody => self.parse_statement_body(bytes),
            DataUnitKind::Drcs1 => self.parse_drcs(bytes, 1),
            DataUnitKind::Drcs2 => self.parse_drcs(bytes, 2),
            // Bitmap, geometric shape and colour map units are presentation
            // data no text or ASS rendering can carry; a pixel renderer may
            // want them one day.
            _ => {}
        }
    }

    // ── state resets ────────────────────────────────────────────────

    fn reset_graphic_sets(&mut self) {
        self.gx = match (self.encoding, self.options.profile) {
            (Encoding::Latin, _) => [
                codesets::ALPHANUMERIC,
                codesets::ALPHANUMERIC,
                codesets::LATIN_EXTENSION,
                codesets::LATIN_SPECIAL,
            ],
            // One-seg leads with DRCS because its captions are largely drawn
            // glyphs, not text.
            (_, Profile::C) => [
                codesets::DRCS_1,
                codesets::ALPHANUMERIC,
                codesets::KANJI,
                codesets::MACRO,
            ],
            (_, Profile::A) => [
                codesets::KANJI,
                codesets::ALPHANUMERIC,
                codesets::HIRAGANA,
                codesets::MACRO,
            ],
        };
        self.gl = 0;
        self.gr = 2;
    }

    fn reset_writing_format(&mut self) {
        match self.options.profile {
            Profile::A => {
                let (w, h, hs, vs) = match self.swf {
                    // The legacy formats keep the crate's historical fallback
                    // geometry. Their exact density metrics remain deferred.
                    0..=4 => (960, 540, 4, 24),
                    5 => (1920, 1080, 4, 24),
                    // Newly recognized modes reuse their density family's
                    // established spacing; this is not a Windows-parity claim.
                    6 => (1920, 1080, 4, 24),
                    7 => (960, 540, 4, 24),
                    8 => (960, 540, 12, 24),
                    9 => (720, 480, 4, 16),
                    10 => (720, 480, 8, 24),
                    11 | 12 => (1280, 720, 4, 24),
                    _ => (960, 540, 4, 24),
                };
                self.plane_width = w;
                self.plane_height = h;
                self.area_width = w;
                self.area_height = h;
                self.char_width = 36;
                self.char_height = 36;
                self.char_h_spacing = hs;
                self.char_v_spacing = vs;
            }
            Profile::C => {
                self.plane_width = 320;
                self.plane_height = 180;
                self.area_width = 320;
                self.area_height = 180;
                self.char_width = 18;
                self.char_height = 18;
                self.char_h_spacing = 2;
                self.char_v_spacing = 6;
            }
        }
        if self.encoding == Encoding::Latin {
            self.char_h_spacing = 2;
            self.char_v_spacing = 16;
        }
    }

    fn reset_state(&mut self) {
        self.reset_graphic_sets();
        self.reset_writing_format();
        self.area_x = 0;
        self.area_y = 0;
        self.pos_inited = false;
        self.pos_x = 0;
        self.pos_y = 0;
        if self.encoding == Encoding::Latin {
            // Latin captions are written in MSZ by default.
            self.char_h_scale = 0.5;
            self.char_v_scale = 1.0;
        } else {
            self.char_h_scale = 1.0;
            self.char_v_scale = 1.0;
        }
        self.style = CharStyle::default();
        self.stroke_color = Rgba::TRANSPARENT;
        self.enclosure = Enclosure::default();
        self.builtin_sound = None;
        self.palette = 0;
        self.text_color = CLUT[0][7];
        self.back_color = CLUT[0][8];
    }

    // ── statement body ──────────────────────────────────────────────

    fn parse_statement_body(&mut self, data: &[u8]) {
        let mut offset = 0usize;
        while offset < data.len() {
            let rest = &data[offset..];
            let ch = rest[0];
            let consumed = if self.encoding == Encoding::Utf8 {
                if ch <= 0x1f {
                    self.handle_c0(rest)
                } else if ch == 0x7f {
                    self.handle_c1(rest)
                } else if ch == 0xc2
                    && matches!(rest.get(1), Some(next) if (0x80..=0x9f).contains(next))
                {
                    // C1 codes appear as their UTF-8 two-byte form.
                    self.handle_c1(&rest[1..]).map(|n| n + 1)
                } else {
                    self.handle_utf8(rest)
                }
            } else if ch <= 0x20 {
                self.handle_c0(rest)
            } else if ch < 0x7f {
                self.handle_glgr(rest, self.gx[self.gl])
            } else if ch <= 0xa0 {
                self.handle_c1(rest)
            } else if ch < 0xff {
                self.handle_glgr(rest, self.gx[self.gr])
            } else {
                Some(1)
            };

            // None means the body ended inside a control code. Keep what was
            // decoded — the alternative is dropping a whole subtitle because
            // its last byte went missing.
            match consumed {
                Some(0) | None => return,
                Some(n) => offset += n,
            }
        }
    }

    fn handle_c0(&mut self, data: &[u8]) -> Option<usize> {
        Some(match data[0] {
            c0::NUL | c0::BEL | c0::CAN | c0::RS | c0::US => 1,
            c0::APB => {
                self.move_character_path(-1);
                1
            }
            c0::APF => {
                self.move_character_path(1);
                1
            }
            c0::APD => {
                self.move_line_direction(1);
                1
            }
            c0::APU => {
                self.move_line_direction(-1);
                1
            }
            c0::CS => {
                // Clear screen: the caption that carries it ends whatever was
                // on display, which is how an indefinite caption terminates.
                self.reset_state();
                self.clear_screen = true;
                1
            }
            c0::APR => {
                self.text.push('\n');
                self.move_to_newline();
                1
            }
            c0::LS1 => {
                self.gl = 1;
                1
            }
            c0::LS0 => {
                self.gl = 0;
                1
            }
            c0::PAPF => {
                let step = (*data.get(1)? & 0b0011_1111) as i32;
                self.move_character_path(step);
                2
            }
            c0::SS2 => {
                let n = self.handle_glgr(data.get(1..)?, self.gx[2])?;
                1 + n
            }
            c0::SS3 => {
                let n = self.handle_glgr(data.get(1..)?, self.gx[3])?;
                1 + n
            }
            c0::ESC => {
                let n = self.handle_esc(data.get(1..)?)?;
                1 + n
            }
            c0::APS => {
                let line = (*data.get(1)? & 0b0011_1111) as i32;
                let character = (*data.get(2)? & 0b0011_1111) as i32;
                self.set_active_position(line, character);
                3
            }
            c0::SP => {
                // A space is ideographic unless the text is Latin, UTF-8, or in
                // half-width mode — where a fullwidth space would open a gap
                // twice the size of the surrounding characters.
                let ideographic = self.encoding == Encoding::JisB24
                    && !(self.options.replace_msz_fullwidth_ascii && self.is_msz());
                self.push_text(if ideographic { "\u{3000}" } else { " " }, None);
                self.move_character_path(1);
                1
            }
            _ => 1,
        })
    }

    fn handle_esc(&mut self, data: &[u8]) -> Option<usize> {
        Some(match data[0] {
            esc::LS2 => {
                self.gl = 2;
                1
            }
            esc::LS3 => {
                self.gl = 3;
                1
            }
            esc::LS1R => {
                self.gr = 1;
                1
            }
            esc::LS2R => {
                self.gr = 2;
                1
            }
            esc::LS3R => {
                self.gr = 3;
                1
            }
            // Two-byte set designation: ESC 0x24 [0x28..0x2B] F
            0x24 => {
                let second = *data.get(1)?;
                if (0x28..=0x2b).contains(&second) {
                    let index = (second - 0x28) as usize;
                    let third = *data.get(2)?;
                    if third == 0x20 {
                        // Two-byte DRCS.
                        let f = *data.get(3)?;
                        if let Some(set) = codesets::drcs_set_by_final(f) {
                            self.gx[index] = Codeset { bytes: 2, ..set };
                        }
                        4
                    } else {
                        if let Some(set) = codesets::g_set_by_final(third) {
                            self.gx[index] = set;
                        }
                        3
                    }
                } else {
                    // ESC 0x24 F designates into G0.
                    if let Some(set) = codesets::g_set_by_final(second) {
                        self.gx[0] = set;
                    }
                    2
                }
            }
            // One-byte set designation: ESC [0x28..0x2B] F
            0x28..=0x2b => {
                let index = (data[0] - 0x28) as usize;
                let second = *data.get(1)?;
                if second == 0x20 {
                    let f = *data.get(2)?;
                    if let Some(set) = codesets::drcs_set_by_final(f) {
                        self.gx[index] = set;
                    }
                    3
                } else {
                    if let Some(set) = codesets::g_set_by_final(second) {
                        self.gx[index] = set;
                    }
                    2
                }
            }
            _ => 1,
        })
    }

    fn handle_c1(&mut self, data: &[u8]) -> Option<usize> {
        Some(match data[0] {
            c1::DEL => 1,
            c1::BKF..=c1::WHF => {
                self.text_color = CLUT[self.palette][(data[0] - c1::BKF) as usize];
                1
            }
            c1::COL => {
                let p1 = *data.get(1)?;
                if p1 == 0x20 {
                    // Palette select. Indexes above 7 are unused in practice.
                    self.palette = (*data.get(2)? & 0x07) as usize;
                    3
                } else if (0x48..=0x7f).contains(&p1) {
                    let index = (p1 & 0x0f) as usize;
                    match p1 & 0xf0 {
                        0x40 => self.text_color = CLUT[self.palette][index],
                        0x50 => self.back_color = CLUT[self.palette][index],
                        _ => {}
                    }
                    2
                } else {
                    return None;
                }
            }
            c1::SSZ => {
                self.char_h_scale = 0.5;
                self.char_v_scale = 0.5;
                1
            }
            c1::MSZ => {
                self.char_h_scale = 0.5;
                self.char_v_scale = 1.0;
                1
            }
            c1::NSZ => {
                self.char_h_scale = 1.0;
                self.char_v_scale = 1.0;
                1
            }
            c1::SZX => {
                match *data.get(1)? {
                    0x41 => self.char_v_scale = 2.0,
                    0x44 => self.char_h_scale = 2.0,
                    0x45 => {
                        self.char_h_scale = 2.0;
                        self.char_v_scale = 2.0;
                    }
                    // Other values are unused per ARIB TR-B14.
                    _ => {}
                }
                2
            }
            c1::FLC | c1::POL | c1::WMM => {
                data.get(1)?;
                2
            }
            c1::CDC => {
                if *data.get(1)? == 0x20 {
                    data.get(2)?;
                    3
                } else {
                    2
                }
            }
            c1::TIME => {
                let p1 = *data.get(1)?;
                let p2 = *data.get(2)?;
                if p1 == 0x20 {
                    // Wait, in units of 100 ms, accumulating across commands.
                    let add = ((p2 & 0b0011_1111) as u32) * 100;
                    self.wait_ms = Some(self.wait_ms.unwrap_or(0) + add);
                }
                3
            }
            // MACRO as a C1 command is unused per ARIB TR-B14; the macro
            // *character set* (handled in handle_glgr) is what captions use.
            c1::MACRO => 1,
            c1::RPC => {
                data.get(1)?;
                2
            }
            c1::STL => {
                self.style.underline = true;
                1
            }
            c1::SPL => {
                self.style.underline = false;
                1
            }
            c1::HLC => {
                let bits = *data.get(1)? & 0x0f;
                self.enclosure = Enclosure {
                    bottom: bits & 0b0001 != 0,
                    right: bits & 0b0010 != 0,
                    top: bits & 0b0100 != 0,
                    left: bits & 0b1000 != 0,
                };
                2
            }
            c1::CSI => {
                let n = self.handle_csi(data.get(1..)?)?;
                1 + n
            }
            _ => 1,
        })
    }

    fn handle_csi(&mut self, data: &[u8]) -> Option<usize> {
        let mut offset = 0usize;
        let mut param1 = 0u32;
        let mut param2 = 0u32;
        let mut param_count = 0usize;

        // Parameters are decimal digits separated by 0x3B and terminated by
        // 0x20, e.g. CSI "840;480" SP 'V'.
        while offset < data.len() {
            let b = data[offset];
            if (0x30..=0x39).contains(&b) {
                if param_count <= 1 {
                    // Saturating: the digit run is stream-controlled and has no
                    // length limit, so a plain `* 10 +` overflows u32 before
                    // any range check downstream can reject the value.
                    param2 = param2.saturating_mul(10).saturating_add((b & 0x0f) as u32);
                }
            } else if b == 0x20 {
                if param_count == 0 {
                    param1 = param2;
                }
                param_count += 1;
                break;
            } else if b == 0x3b {
                if param_count == 0 {
                    param1 = param2;
                    param2 = 0;
                }
                param_count += 1;
            }
            offset += 1;
        }

        offset += 1; // the F byte follows the intermediate
        let f = *data.get(offset)?;
        match f {
            csi::SWF => {
                if param_count == 1 {
                    if let Ok(swf) = u8::try_from(param1) {
                        if writing_mode_for_swf(swf).is_some() {
                            self.swf = swf;
                            self.reset_writing_format();
                        }
                    }
                }
            }
            csi::SDF => {
                self.area_width = plane_dots(param1);
                self.area_height = plane_dots(param2);
            }
            csi::SSM => {
                self.char_width = plane_dots(param1);
                self.char_height = plane_dots(param2);
            }
            csi::SHS => self.char_h_spacing = plane_dots(param1),
            csi::SVS => self.char_v_spacing = plane_dots(param1),
            csi::SDP => {
                self.area_x = plane_dots(param1);
                if param_count >= 2 {
                    self.area_y = plane_dots(param2);
                }
                if !self.pos_inited {
                    // The pen starts at the first cell on the active character
                    // path: top-left horizontally, top-right vertically.
                    self.set_active_position(0, 0);
                }
            }
            // Clamped like every other geometry parameter: an unbounded ACPS
            // reached the renderers, where `region.y * 3` then overflowed.
            csi::ACPS => self.set_absolute_pos_dots(plane_dots(param1), plane_dots(param2)),
            csi::ORN => {
                if param1 == 0 {
                    self.style.stroke = false;
                } else if param1 == 1 && param_count >= 2 {
                    let palette = (param2 / 100) as usize;
                    let index = (param2 % 100) as usize;
                    if palette >= 8 || index >= 16 {
                        return None;
                    }
                    self.style.stroke = true;
                    self.stroke_color = CLUT[palette][index];
                }
            }
            csi::MDF => match param1 {
                0 => {
                    self.style.bold = false;
                    self.style.italic = false;
                }
                1 => self.style.bold = true,
                2 => self.style.italic = true,
                3 => {
                    self.style.bold = true;
                    self.style.italic = true;
                }
                _ => {}
            },
            csi::PRA => self.builtin_sound = Some(param1 as u8),
            // GSM, CCC, PLD, PLU, GAA, SRC, TCC, CFS, XCS, SCR, ACS, UED, RCS,
            // SCS: presentation details a text or ASS rendering has no use for.
            _ => {}
        }
        Some(offset + 1)
    }

    fn handle_utf8(&mut self, data: &[u8]) -> Option<usize> {
        // One character at a time, since the statement may end mid-sequence.
        let mut found = None;
        for len in 1..=data.len().min(4) {
            if let Ok(s) = std::str::from_utf8(&data[..len]) {
                found = s.chars().next();
                if found.is_some() {
                    break;
                }
            }
        }
        let text = found?;
        let len = text.len_utf8();
        let cp = text as u32;
        if (0xec00..=0xf8ff).contains(&cp) {
            // STD-B24 maps DRCS into the private use area starting at U+EC00.
            self.push_drcs(0, cp as u16);
        } else {
            self.push_text(&text.to_string(), None);
        }
        self.move_character_path(1);
        Some(len)
    }

    fn handle_glgr(&mut self, data: &[u8], entry: Codeset) -> Option<usize> {
        let ch = data[0] & 0x7f;
        if !(0x21..0x7f).contains(&ch) {
            return None;
        }
        let ch2 = if entry.bytes == 2 {
            let b = *data.get(1)? & 0x7f;
            if !(0x21..0x7f).contains(&b) {
                return None;
            }
            b
        } else {
            0
        };

        let size = self.size_context();
        match entry.set {
            GraphicSet::Macro => {
                // A macro is a canned statement body: designate a whole set of
                // graphic sets in one character.
                if (0x60..=0x6f).contains(&ch) {
                    let body = DEFAULT_MACROS[(ch & 0x0f) as usize];
                    self.parse_statement_body(body);
                }
            }
            GraphicSet::Drcs(map_index) => {
                let key = if entry.bytes == 2 {
                    ((ch as u16) << 8) | ch2 as u16
                } else {
                    ch as u16
                };
                self.push_drcs(map_index, key);
                self.move_character_path(1);
            }
            GraphicSet::Kanji
            | GraphicSet::JisX0213Kanji1
            | GraphicSet::JisX0213Kanji2
            | GraphicSet::AdditionalSymbols => {
                let resolved = charset::resolve_double(
                    entry.set,
                    (ch - 0x21) as u32,
                    (ch2 - 0x21) as u32,
                    size,
                );
                self.push_text(&resolved.text, resolved.pua);
                self.move_character_path(1);
            }
            other => {
                if let Some(resolved) = charset::resolve_single(other, ch, size) {
                    self.push_text(&resolved.text, resolved.pua);
                    self.move_character_path(1);
                }
                // A set with no text form (mosaic) consumes its byte and draws
                // nothing, rather than aborting the statement.
            }
        }
        Some(entry.bytes as usize)
    }

    // ── DRCS ────────────────────────────────────────────────────────

    fn parse_drcs(&mut self, data: &[u8], byte_count: usize) {
        let Some(&number_of_code) = data.first() else {
            return;
        };
        let mut offset = 1usize;

        for _ in 0..number_of_code {
            if offset + 3 > data.len() {
                return;
            }
            let character_code = ((data[offset] as u16) << 8) | data[offset + 1] as u16;
            let number_of_font = data[offset + 2];
            offset += 3;

            for _ in 0..number_of_font {
                if offset >= data.len() {
                    return;
                }
                let mode = data[offset] & 0x0f;
                offset += 1;

                if mode == 0b0000 || mode == 0b0001 {
                    if offset + 3 > data.len() {
                        return;
                    }
                    let depth = data[offset] as u32 + 2;
                    let width = data[offset + 1] as u32;
                    let height = data[offset + 2] as u32;
                    offset += 3;

                    // depth is a count of levels; the bits per pixel is how
                    // many bits it takes to index them.
                    let depth_bits = (32 - (depth - 1).leading_zeros()).max(1);
                    let bitmap_size = (width * height * depth_bits).div_ceil(8) as usize;
                    if depth < 2 || offset + bitmap_size > data.len() {
                        return;
                    }

                    let pixels = data[offset..offset + bitmap_size].to_vec();
                    offset += bitmap_size;
                    let md5 = format!("{:x}", md5::compute(&pixels));
                    let drcs = Drcs {
                        width,
                        height,
                        depth,
                        depth_bits,
                        pixels,
                        md5,
                        // Resolving a glyph to a character needs a table of
                        // known MD5s, which is a separate piece of work; until
                        // then a text renderer shows GETA and a pixel renderer
                        // draws the real thing.
                        alternative: None,
                    };

                    if byte_count == 1 {
                        // The set the code belongs to is encoded in the code
                        // itself: high nibble picks DRCS-1..15.
                        let final_byte = ((character_code & 0x0f00) >> 8) as u8 + 0x40;
                        let Some(set) = codesets::drcs_set_by_final(final_byte) else {
                            continue;
                        };
                        if let GraphicSet::Drcs(index) = set.set {
                            let key = (character_code & 0x00ff) & 0x7f;
                            self.drcs_maps[index as usize].insert(key, drcs);
                        }
                    } else {
                        let key = if (0xec00..=0xf8ff).contains(&character_code) {
                            character_code
                        } else {
                            character_code & 0x7f7f
                        };
                        self.drcs_maps[0].insert(key, drcs);
                    }
                } else {
                    // Geometric font data — not a bitmap, and nothing here
                    // draws it.
                    if offset + 4 > data.len() {
                        return;
                    }
                    let length = ((data[offset + 2] as usize) << 8) | data[offset + 3] as usize;
                    offset += 4 + length;
                }
            }
        }
    }

    fn push_drcs(&mut self, map_index: u8, key: u16) {
        let Some(drcs) = self.drcs_maps[map_index as usize].get(&key).cloned() else {
            // A DRCS character whose glyph was never transmitted. GETA is what
            // every ARIB decoder shows.
            self.push_text(&GETA.to_string(), None);
            return;
        };
        let code = ((map_index as u32) << 16) | key as u32;
        let body = match &drcs.alternative {
            Some(c) => {
                if !self.is_ruby() {
                    self.text.push(*c);
                }
                CharBody::DrcsReplaced {
                    text: c.to_string(),
                    code,
                }
            }
            None => {
                if !self.is_ruby() {
                    self.text.push(GETA);
                }
                CharBody::Drcs { code }
            }
        };
        self.drcs_used.entry(code).or_insert(drcs);
        let ch = self.make_char(body);
        self.push_char(ch);
    }

    // ── character placement ─────────────────────────────────────────

    fn push_text(&mut self, text: &str, pua: Option<char>) {
        if !self.is_ruby() {
            self.text.push_str(text);
        }
        let ch = self.make_char(CharBody::Text {
            text: text.to_string(),
            pua,
        });
        self.push_char(ch);
    }

    fn make_char(&mut self, body: CharBody) -> CaptionChar {
        self.ensure_position();
        CaptionChar {
            body,
            x: self.pos_x,
            y: self.pos_y - self.section_height(),
            char_width: self.char_width,
            char_height: self.char_height,
            char_horizontal_spacing: self.char_h_spacing,
            char_vertical_spacing: self.char_v_spacing,
            char_horizontal_scale: self.char_h_scale,
            char_vertical_scale: self.char_v_scale,
            text_color: self.text_color,
            back_color: self.back_color,
            stroke_color: if self.style.stroke {
                self.stroke_color
            } else {
                Rgba::TRANSPARENT
            },
            style: self.style,
            enclosure: self.enclosure,
        }
    }

    fn push_char(&mut self, ch: CaptionChar) {
        if self.needs_new_region(&ch) {
            self.regions.push(CaptionRegion {
                x: ch.x,
                y: ch.y,
                width: ch.section_width(),
                height: ch.section_height(),
                writing_mode: self.writing_mode(),
                is_ruby: self.is_ruby(),
                chars: Vec::new(),
            });
        } else {
            let ch_right = ch.x + ch.section_width();
            let ch_bottom = ch.y + ch.section_height();
            let region = self
                .regions
                .last_mut()
                .expect("a region exists after needs_new_region");
            let right = (region.x + region.width).max(ch_right);
            let bottom = (region.y + region.height).max(ch_bottom);
            region.x = region.x.min(ch.x);
            region.y = region.y.min(ch.y);
            region.width = right - region.x;
            region.height = bottom - region.y;
        }
        let region = self
            .regions
            .last_mut()
            .expect("a region exists after needs_new_region");
        region.chars.push(ch);
    }

    /// A region is a contiguous run along one character path. A position jump,
    /// cross-path size change, or writing-direction change starts another one.
    fn needs_new_region(&self, next: &CaptionChar) -> bool {
        let Some(region) = self.regions.last() else {
            return true;
        };
        let writing_mode = self.writing_mode();
        if region.writing_mode != writing_mode {
            return true;
        }
        let Some(prev) = region.chars.last() else {
            return false;
        };
        match writing_mode {
            WritingMode::HorizontalTb => {
                next.x != prev.x + prev.section_width()
                    || next.y != prev.y
                    || next.section_height() != prev.section_height()
            }
            WritingMode::VerticalRl => {
                next.x != prev.x
                    || next.y != prev.y + prev.section_height()
                    || next.section_width() != prev.section_width()
            }
        }
    }

    fn size_context(&self) -> SizeContext {
        SizeContext {
            is_msz: self.is_msz(),
            replace_msz_ascii: self.options.replace_msz_fullwidth_ascii,
            replace_msz_japanese: self.options.replace_msz_fullwidth_japanese,
        }
    }

    /// Half width against full height — the mode Japanese captions use for
    /// narrow text, and the one where a fullwidth glyph looks wrong.
    fn is_msz(&self) -> bool {
        self.char_h_scale * 2.0 == self.char_v_scale
    }

    /// Ruby (furigana) is sent as half-size text, or on profile A as text in an
    /// 18×18 cell. Text renderings drop it; pixel renderings draw it.
    fn is_ruby(&self) -> bool {
        if self.encoding != Encoding::JisB24 {
            return false;
        }
        (self.char_h_scale == 0.5 && self.char_v_scale == 0.5)
            || (self.options.profile == Profile::A
                && self.char_width == 18
                && self.char_height == 18)
    }

    fn section_width(&self) -> i32 {
        (((self.char_width + self.char_h_spacing) as f32) * self.char_h_scale).floor() as i32
    }

    fn section_height(&self) -> i32 {
        (((self.char_height + self.char_v_spacing) as f32) * self.char_v_scale).floor() as i32
    }

    fn writing_mode(&self) -> WritingMode {
        writing_mode_for_swf(self.swf).unwrap_or_default()
    }

    /// Set APS coordinates. Its first parameter counts in the line direction;
    /// its second counts along the character path.
    ///
    /// Saturating throughout: `plane_dots` already bounds every geometry
    /// parameter this multiplies, so an overflow here would mean that bound
    /// was bypassed. Saturating rather than wrapping keeps a malformed stream
    /// pinned to the edge of the plane instead of wrapping to its opposite
    /// side.
    fn set_active_position(&mut self, line: i32, character: i32) {
        self.pos_inited = true;
        match self.writing_mode() {
            WritingMode::HorizontalTb => {
                self.pos_x = self
                    .area_x
                    .saturating_add(character.saturating_mul(self.section_width()));
                self.pos_y = self
                    .area_y
                    .saturating_add(line.saturating_add(1).saturating_mul(self.section_height()));
            }
            WritingMode::VerticalRl => {
                self.pos_x = self
                    .area_x
                    .saturating_add(self.area_width)
                    .saturating_sub(line.saturating_add(1).saturating_mul(self.section_width()));
                self.pos_y = self.area_y.saturating_add(
                    character
                        .saturating_add(1)
                        .saturating_mul(self.section_height()),
                );
            }
        }
    }

    fn set_absolute_pos_dots(&mut self, x: i32, y: i32) {
        self.pos_inited = true;
        self.pos_x = x;
        self.pos_y = y;
    }

    fn ensure_position(&mut self) {
        if !self.pos_inited || self.pos_x < 0 || self.pos_y < 0 {
            self.set_active_position(0, 0);
        }
    }

    fn move_character_path(&mut self, mut steps: i32) {
        self.ensure_position();
        match self.writing_mode() {
            WritingMode::HorizontalTb => {
                while steps < 0 {
                    self.pos_x -= self.section_width();
                    steps += 1;
                    if self.pos_x < self.area_x {
                        self.pos_x = self.area_x + self.area_width - self.section_width();
                        self.move_line_direction(-1);
                    }
                }
                while steps > 0 {
                    self.pos_x += self.section_width();
                    steps -= 1;
                    if self.pos_x >= self.area_x + self.area_width {
                        self.pos_x = self.area_x;
                        self.move_line_direction(1);
                    }
                }
            }
            WritingMode::VerticalRl => {
                while steps < 0 {
                    self.pos_y -= self.section_height();
                    steps += 1;
                    if self.pos_y < self.area_y + self.section_height() {
                        self.pos_y = self.area_y + self.area_height;
                        self.move_line_direction(-1);
                    }
                }
                while steps > 0 {
                    self.pos_y += self.section_height();
                    steps -= 1;
                    if self.pos_y > self.area_y + self.area_height {
                        self.pos_y = self.area_y + self.section_height();
                        self.move_line_direction(1);
                    }
                }
            }
        }
    }

    fn move_line_direction(&mut self, mut steps: i32) {
        self.ensure_position();
        match self.writing_mode() {
            WritingMode::HorizontalTb => {
                while steps < 0 {
                    self.pos_y -= self.section_height();
                    steps += 1;
                    if self.pos_y < self.area_y + self.section_height() {
                        self.pos_y = self.area_y + self.area_height;
                    }
                }
                while steps > 0 {
                    self.pos_y += self.section_height();
                    steps -= 1;
                    if self.pos_y > self.area_y + self.area_height {
                        self.pos_y = self.area_y + self.section_height();
                    }
                }
            }
            WritingMode::VerticalRl => {
                while steps < 0 {
                    self.pos_x += self.section_width();
                    steps += 1;
                    if self.pos_x >= self.area_x + self.area_width {
                        self.pos_x = self.area_x;
                    }
                }
                while steps > 0 {
                    self.pos_x -= self.section_width();
                    steps -= 1;
                    if self.pos_x < self.area_x {
                        self.pos_x = self.area_x + self.area_width - self.section_width();
                    }
                }
            }
        }
    }

    fn move_to_newline(&mut self) {
        self.ensure_position();
        self.move_line_direction(1);
        match self.writing_mode() {
            WritingMode::HorizontalTb => self.pos_x = self.area_x,
            WritingMode::VerticalRl => self.pos_y = self.area_y + self.section_height(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap a statement body in the data group and PES framing a decoder wants.
    fn statement_pes(body: &[u8]) -> Vec<u8> {
        let unit_len = body.len();
        let mut group = vec![0x00]; // TMD free
        let loop_len = 5 + unit_len;
        group.extend_from_slice(&[
            (loop_len >> 16) as u8,
            (loop_len >> 8) as u8,
            loop_len as u8,
        ]);
        group.extend_from_slice(&[
            0x1f,
            0x20,
            (unit_len >> 16) as u8,
            (unit_len >> 8) as u8,
            unit_len as u8,
        ]);
        group.extend_from_slice(body);

        let mut payload = vec![0x80, 0xff, 0xf0];
        // data_group_id 1 (statement, language 1), version 0, links 0.
        payload.extend_from_slice(&[0x04, 0x00, 0x00]);
        payload.extend_from_slice(&[(group.len() >> 8) as u8, group.len() as u8]);
        payload.extend_from_slice(&group);
        payload
    }

    fn management_pes(format: u8, group: Group) -> Vec<u8> {
        let body = [
            0x00, // TMD free
            0x01, // one language
            0x00, // language tag 0, DMF 0
            b'j',
            b'p',
            b'n',
            format << 4, // format, TCS 0, roll-up 0
            0x00,
            0x00,
            0x00, // empty data-unit loop
        ];
        let group_byte = match group {
            Group::A => 0x00,
            Group::B => 0x80,
        };
        let mut payload = vec![0x80, 0xff, 0xf0];
        payload.extend_from_slice(&[group_byte, 0x00, 0x00, 0x00, body.len() as u8]);
        payload.extend_from_slice(&body);
        payload
    }

    fn vertical_decoder() -> Decoder {
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        decoder.swf = 8;
        decoder.reset_state();
        decoder
    }

    fn active_cell_position(decoder: &mut Decoder) -> (i32, i32) {
        let ch = decoder.make_char(CharBody::Text {
            text: "x".into(),
            pua: None,
        });
        (ch.x, ch.y)
    }

    #[test]
    fn management_formats_map_to_swf_without_treating_all_values_as_one_based() {
        for raw in 0..=4 {
            assert_eq!(management_format_to_swf(raw), Some(raw));
        }
        for raw in 6..=13 {
            assert_eq!(management_format_to_swf(raw), Some(raw - 1));
        }
        for reserved in [5, 14, 15] {
            assert_eq!(management_format_to_swf(reserved), None);
        }
    }

    #[test]
    fn a_reserved_management_format_does_not_mutate_prior_state() {
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        assert!(matches!(
            decoder.decode(&management_pes(10, Group::A), None),
            Ok(None)
        ));
        let before = (
            decoder.prev_management_group,
            decoder.swf,
            decoder.writing_mode(),
            decoder.plane_width,
            decoder.plane_height,
            decoder.char_h_spacing,
            decoder.char_v_spacing,
            decoder.encoding,
            decoder.language,
        );

        assert!(matches!(
            decoder.decode(&management_pes(5, Group::B), None),
            Err(DecodeError::Malformed("reserved management writing format"))
        ));
        assert_eq!(
            (
                decoder.prev_management_group,
                decoder.swf,
                decoder.writing_mode(),
                decoder.plane_width,
                decoder.plane_height,
                decoder.char_h_spacing,
                decoder.char_v_spacing,
                decoder.encoding,
                decoder.language,
            ),
            before
        );

        // Group B was not marked as consumed by the rejected management group.
        assert!(matches!(
            decoder.decode(&management_pes(9, Group::B), None),
            Ok(None)
        ));
        assert_eq!(decoder.prev_management_group, Some(Group::B));
        assert_eq!(decoder.swf, 8);
        assert_eq!(decoder.writing_mode(), WritingMode::VerticalRl);
    }

    #[test]
    fn every_defined_swf_has_an_explicit_direction() {
        let expected = [
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
            WritingMode::HorizontalTb,
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
        ];
        for (swf, writing_mode) in expected.into_iter().enumerate() {
            assert_eq!(writing_mode_for_swf(swf as u8), Some(writing_mode));
        }
        assert_eq!(writing_mode_for_swf(13), None);
    }

    #[test]
    fn modern_swf_dimensions_and_established_metrics_are_explicit() {
        let cases = [
            (5, 1920, 1080, 4, 24),
            (6, 1920, 1080, 4, 24),
            (7, 960, 540, 4, 24),
            (8, 960, 540, 12, 24),
            (9, 720, 480, 4, 16),
            (10, 720, 480, 8, 24),
            (11, 1280, 720, 4, 24),
            (12, 1280, 720, 4, 24),
        ];
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());

        for (swf, width, height, horizontal_spacing, vertical_spacing) in cases {
            decoder.swf = swf;
            decoder.reset_state();
            assert_eq!(
                (
                    decoder.plane_width,
                    decoder.plane_height,
                    decoder.area_width,
                    decoder.area_height,
                    decoder.char_h_spacing,
                    decoder.char_v_spacing,
                ),
                (
                    width,
                    height,
                    width,
                    height,
                    horizontal_spacing,
                    vertical_spacing,
                ),
                "SWF {swf}"
            );
        }
    }

    #[test]
    fn legacy_swf_keeps_fallback_geometry_while_honouring_direction() {
        let modes = [
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
            WritingMode::HorizontalTb,
            WritingMode::VerticalRl,
            WritingMode::HorizontalTb,
        ];
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());

        for (swf, writing_mode) in modes.into_iter().enumerate() {
            decoder.swf = swf as u8;
            decoder.reset_state();
            assert_eq!(decoder.writing_mode(), writing_mode);
            assert_eq!((decoder.plane_width, decoder.plane_height), (960, 540));
            assert_eq!((decoder.char_h_spacing, decoder.char_v_spacing), (4, 24));
        }
    }

    #[test]
    fn ordinary_vertical_glyphs_advance_downward_and_form_column_bounds() {
        let body = [
            0x9b, b'8', 0x20, b'S', // CSI SWF 8: 960x540 vertical
            0x1b, 0x24, 0x42, 0x0f, // kanji in G0, LS0
            0x30, 0x21, 0x30, 0x22, // two ordinary glyphs
        ];
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let caption = decoder
            .decode(&statement_pes(&body), None)
            .expect("decodes")
            .expect("caption");

        assert_eq!(caption.regions.len(), 1);
        let region = &caption.regions[0];
        assert_eq!(region.writing_mode, WritingMode::VerticalRl);
        assert_eq!(
            (region.x, region.y, region.width, region.height),
            (912, 0, 48, 120)
        );
        assert_eq!((region.chars[0].x, region.chars[0].y), (912, 0));
        assert_eq!((region.chars[1].x, region.chars[1].y), (912, 60));
    }

    #[test]
    fn vertical_controls_follow_character_and_line_axes() {
        let mut decoder = vertical_decoder();

        assert_eq!(decoder.handle_c0(&[c0::APS, 0x40 | 1, 0x40 | 2]), Some(3));
        assert_eq!(active_cell_position(&mut decoder), (864, 120));

        assert_eq!(decoder.handle_c0(&[c0::APF]), Some(1));
        assert_eq!(active_cell_position(&mut decoder), (864, 180));
        assert_eq!(decoder.handle_c0(&[c0::APB]), Some(1));
        assert_eq!(active_cell_position(&mut decoder), (864, 120));

        assert_eq!(decoder.handle_c0(&[c0::APD]), Some(1));
        assert_eq!(active_cell_position(&mut decoder), (816, 120));
        assert_eq!(decoder.handle_c0(&[c0::APU]), Some(1));
        assert_eq!(active_cell_position(&mut decoder), (864, 120));

        assert_eq!(decoder.handle_c0(&[c0::PAPF, 0x40 | 3]), Some(2));
        assert_eq!(active_cell_position(&mut decoder), (864, 300));

        assert_eq!(decoder.handle_c0(&[c0::APR]), Some(1));
        assert_eq!(active_cell_position(&mut decoder), (816, 0));
    }

    #[test]
    fn vertical_character_and_line_steps_wrap_in_their_own_axes() {
        let mut decoder = vertical_decoder();

        decoder.handle_c0(&[c0::APS, 0x40, 0x40 | 8]);
        assert_eq!(active_cell_position(&mut decoder), (912, 480));
        decoder.handle_c0(&[c0::APF]);
        assert_eq!(active_cell_position(&mut decoder), (864, 0));
        decoder.handle_c0(&[c0::APB]);
        assert_eq!(active_cell_position(&mut decoder), (912, 480));

        decoder.handle_c0(&[c0::APS, 0x40 | 19, 0x40]);
        assert_eq!(active_cell_position(&mut decoder), (0, 0));
        decoder.handle_c0(&[c0::APD]);
        assert_eq!(active_cell_position(&mut decoder), (912, 0));
        decoder.handle_c0(&[c0::APU]);
        assert_eq!(active_cell_position(&mut decoder), (0, 0));
    }

    #[test]
    fn acps_remains_a_physical_coordinate_in_vertical_mode() {
        let mut decoder = vertical_decoder();

        assert_eq!(decoder.handle_csi(b"123;456 a"), Some(9));

        assert_eq!(active_cell_position(&mut decoder), (123, 396));
    }

    #[test]
    fn changing_direction_splits_regions_within_one_density() {
        let body = [
            0x9b,
            b'7',
            0x20,
            b'S', // 960x540 horizontal
            0x1b,
            0x24,
            0x42,
            0x0f, // kanji in G0, LS0
            c0::APS,
            0x40,
            0x40,
            0x30,
            0x21,
            0x9b,
            b'8',
            0x20,
            b'S', // same density, vertical
            c0::APS,
            0x40,
            0x40,
            0x30,
            0x22,
        ];
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let caption = decoder
            .decode(&statement_pes(&body), None)
            .expect("decodes")
            .expect("caption");

        assert_eq!(caption.regions.len(), 2);
        assert_eq!(caption.regions[0].writing_mode, WritingMode::HorizontalTb);
        assert_eq!(caption.regions[1].writing_mode, WritingMode::VerticalRl);
        assert_eq!((caption.regions[0].x, caption.regions[0].y), (0, 0));
        assert_eq!((caption.regions[1].x, caption.regions[1].y), (912, 0));
    }

    /// Positioning is the part with no room for interpretation: APS counts
    /// character cells, and the pen's y is the *bottom* of the row, so a
    /// character's own y sits one section height above it.
    #[test]
    fn aps_positions_a_character_by_cell() {
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let body = [
            0x1b,
            0x24,
            0x42, // designate kanji into G0
            0x0f, // LS0
            0x1c,
            0x40 | 2,
            0x40 | 3, // APS: row 2, column 3
            0x30,
            0x21, // ku 15, ten 0 → 亜
        ];
        let caption = decoder
            .decode(&statement_pes(&body), Some(1_000))
            .expect("decodes")
            .expect("a caption");

        assert_eq!(caption.text, "亜");
        assert_eq!(caption.pts_ms, Some(1_000));
        assert_eq!(caption.duration, Duration::Indefinite);
        assert_eq!(caption.regions.len(), 1);

        let region = &caption.regions[0];
        // Default profile A metrics: 36x36 cells, 4 and 24 of spacing → a
        // section of 40x60.
        assert_eq!(region.x, 3 * 40);
        assert_eq!(region.y, 3 * 60 - 60);
        assert_eq!(region.height, 60);
        assert_eq!(region.width, 40);
        assert!(!region.is_ruby);

        let ch = &region.chars[0];
        assert_eq!(ch.x, 120);
        assert_eq!(ch.y, 120);
        assert_eq!(ch.section_width(), 40);
        assert_eq!(ch.section_height(), 60);
    }

    /// A run of characters on one line is one region; a jump starts another.
    #[test]
    fn a_position_jump_starts_a_new_region() {
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let body = [
            0x1b,
            0x24,
            0x42,
            0x0f, //
            0x1c,
            0x40,
            0x40, // APS(0, 0)
            0x30,
            0x21,
            0x30,
            0x22, // two characters, contiguous
            0x1c,
            0x40 | 3,
            0x40 | 10, // APS elsewhere
            0x30,
            0x23,
        ];
        let caption = decoder
            .decode(&statement_pes(&body), None)
            .expect("decodes")
            .expect("a caption");
        assert_eq!(caption.regions.len(), 2);
        assert_eq!(caption.regions[0].chars.len(), 2);
        assert_eq!(caption.regions[1].chars.len(), 1);
        assert_eq!(caption.text.chars().count(), 3);
    }

    /// TIME with the 0x20 parameter sets how long the caption stays up, in
    /// units of 100 ms, accumulating across commands.
    #[test]
    fn time_control_sets_the_duration() {
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let body = [
            0x1b,
            0x24,
            0x42,
            0x0f,
            0x30,
            0x21, //
            0x9d,
            0x20,
            0x40 | 23, // TIME: 2.3 s
        ];
        let caption = decoder
            .decode(&statement_pes(&body), None)
            .expect("decodes")
            .expect("a caption");
        assert_eq!(caption.duration, Duration::Millis(2300));
    }

    /// Clear-screen carries no text but must still be reported: it is how the
    /// caption before it stops being shown.
    #[test]
    fn clear_screen_alone_is_still_a_caption() {
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let caption = decoder
            .decode(&statement_pes(&[0x0c]), Some(500))
            .expect("decodes")
            .expect("a caption");
        assert!(caption.clear_screen);
        assert!(caption.is_empty());
        assert_eq!(caption.pts_ms, Some(500));
    }

    /// Colour controls apply to the characters that follow them, not the ones
    /// already placed.
    #[test]
    fn foreground_colour_applies_from_where_it_appears() {
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let body = [
            0x1b, 0x24, 0x42, 0x0f, //
            0x30, 0x21, // white by default
            0x83, // YLF: yellow foreground
            0x30, 0x22,
        ];
        let caption = decoder
            .decode(&statement_pes(&body), None)
            .expect("decodes")
            .expect("a caption");
        let chars: Vec<_> = caption.regions.iter().flat_map(|r| &r.chars).collect();
        assert_eq!(chars.len(), 2);
        assert_eq!(chars[0].text_color, Rgba::new(255, 255, 255, 255));
        assert_eq!(chars[1].text_color, Rgba::new(255, 255, 0, 255));
    }

    /// A statement that ends inside a control code keeps what it decoded.
    /// Upstream drops the caption; on live TV a truncated PES is not rare
    /// enough to pay for that.
    #[test]
    fn a_truncated_statement_keeps_what_it_got() {
        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let body = [
            0x1b, 0x24, 0x42, 0x0f, 0x30, 0x21, //
            0x1c, 0x40, // APS with its second parameter missing
        ];
        let caption = decoder
            .decode(&statement_pes(&body), None)
            .expect("decodes")
            .expect("a caption");
        assert_eq!(caption.text, "亜");
    }

    #[test]
    fn a_csi_geometry_parameter_beyond_any_caption_plane_cannot_overflow_the_layout() {
        // CSI SSM assigned char_height straight from a stream parameter, and
        // APS then evaluated (line + 1) * section_height() in i32. A cell size
        // above roughly 33 million overflowed that multiply. The parameter is
        // attacker-controlled: it arrives in the caption statement.
        let mut body = vec![0x9b];
        body.extend_from_slice(b"0;2000000000");
        body.extend_from_slice(&[0x20, b'W']); // SP, SSM
        body.extend_from_slice(&[c0::APS, 0x7f, 0x21]); // APS, line 63
        body.extend_from_slice(&[0x1b, 0x24, 0x42, 0x0f, 0x30, 0x21]);

        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        // The only requirement is that a malformed plane geometry does not
        // panic; whether it yields a caption is not this test's business.
        let _ = decoder.decode(&statement_pes(&body), None);
    }

    #[test]
    fn a_csi_parameter_with_a_long_digit_run_cannot_overflow_the_accumulator() {
        // The decimal accumulator was `param2 * 10 + digit` on a u32, so a
        // sufficiently long digit run overflowed before any range check saw it.
        let mut body = vec![0x9b];
        body.extend_from_slice(b"0;99999999999999999999");
        body.extend_from_slice(&[0x20, b'W']);
        body.extend_from_slice(&[0x1b, 0x24, 0x42, 0x0f, 0x30, 0x21]);

        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let _ = decoder.decode(&statement_pes(&body), None);
    }

    #[test]
    fn an_absolute_position_beyond_the_plane_is_clamped_before_it_reaches_a_renderer() {
        // CSI ACPS set the caption position straight from two stream
        // parameters. An unbounded value survived into Caption::regions, where
        // the WebVTT renderer's `region.y * 3` overflowed i32.
        let mut body = vec![0x9b];
        body.extend_from_slice(b"2000000000;2000000000");
        body.extend_from_slice(&[0x20, b'a']); // SP, ACPS
        body.extend_from_slice(&[0x1b, 0x24, 0x42, 0x0f, 0x30, 0x21]);

        let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
        let decoded = decoder.decode(&statement_pes(&body), None);

        if let Ok(Some(caption)) = decoded {
            for region in &caption.regions {
                assert!(
                    region.y <= MAX_PLANE_DOTS,
                    "region.y {} escaped the plane clamp",
                    region.y
                );
                assert!(region.x <= MAX_PLANE_DOTS, "region.x {} escaped", region.x);
            }
        }
    }
}
