//! JSON: the whole model, for a renderer that is not in this process.
//!
//! [`vtt`](crate::render::vtt) keeps the words and [`ass`](crate::render::ass)
//! keeps where they were — but both are *files*, written once for a recording
//! somebody will play later. A live stream has a third consumer: a browser,
//! drawing the caption itself over the picture as it arrives. It wants the same
//! thing `ass` wants and cannot use an ASS script, because ASS is a rendering
//! and this is the model.
//!
//! So this renderer renders nothing. It serialises [`Caption`] whole — regions,
//! cells, colours, styles, enclosure, ruby, the declared plane, and the DRCS
//! bitmaps a text form can only spell `〓` — and leaves every decision about
//! pixels to whoever reads it. In particular the times are **raw broadcast
//! PTS**: rebasing them is the consumer's job, and a value that has already been
//! rebased once cannot be rebased again.
//!
//! Two things shape the encoding:
//!
//! - **Defaults are omitted.** A caption is 40-odd cells and almost all of them
//!   share their spacing, scale, style and enclosure with the cell before. The
//!   reader is expected to know what a missing field means (see
//!   [`Caption::default`] and the field docs below), which halves a segment.
//! - **DRCS pixels go out packed, base64, exactly as transmitted.** They are the
//!   one part of a caption that is genuinely a bitmap; anything else would be a
//!   rendering decision made here. The unpacking rule is
//!   [`Drcs::level`](crate::model::Drcs::level).
//!
//! There is no serde here for the same reason there is no serde anywhere in this
//! crate: the output is a handful of shapes that are written once and read by
//! something in another language, and a derive would not make either end
//! simpler.

use std::fmt::Write as _;

use crate::model::{Caption, CaptionChar, CaptionRegion, CharBody, Drcs, Rgba, WritingMode};

/// One caption as a JSON object, ready to be embedded in a larger document.
///
/// ```text
/// {"plane_width":960,"plane_height":540,
///  "regions":[{"x":190,"y":404,"width":580,"height":60,
///              "chars":[{"text":"あ","x":190,"y":404,"char_width":36,
///                        "char_height":36,"char_horizontal_spacing":4,
///                        "char_vertical_spacing":24,"char_horizontal_scale":0.5,
///                        "text_color":"#FFFFFFFF","back_color":"#000000CC"}]}]}
/// ```
///
/// A region without `writing_mode` is horizontal-tb. Vertical regions carry
/// `"writing_mode":"vertical-rl"` explicitly.
pub fn caption(caption: &Caption) -> String {
    let mut out = String::with_capacity(1024);
    out.push('{');
    let _ = write!(
        out,
        r#""plane_width":{},"plane_height":{}"#,
        caption.plane_width, caption.plane_height
    );
    out.push_str(r#","regions":["#);
    for (i, region) in caption.regions.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_region(&mut out, region);
    }
    out.push(']');
    // Keyed by the code a `{"drcs":N}` cell names. Absent when the caption used
    // none, which is the common case.
    if !caption.drcs.is_empty() {
        out.push_str(r#","drcs":{"#);
        // Sorted, so the same caption always serialises the same way — a
        // HashMap's order is not a fact about the broadcast.
        let mut codes: Vec<&u32> = caption.drcs.keys().collect();
        codes.sort_unstable();
        for (i, code) in codes.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let _ = write!(out, "\"{code}\":");
            write_drcs(&mut out, &caption.drcs[code]);
        }
        out.push('}');
    }
    out.push('}');
    out
}

fn write_region(out: &mut String, region: &CaptionRegion) {
    let _ = write!(
        out,
        r#"{{"x":{},"y":{},"width":{},"height":{}"#,
        region.x, region.y, region.width, region.height
    );
    if region.writing_mode == WritingMode::VerticalRl {
        out.push_str(r#","writing_mode":"vertical-rl""#);
    }
    // Ruby (furigana) rides above the line it annotates at half size. Absent
    // means an ordinary region; a text renderer drops these and a pixel
    // renderer draws them where they were sent.
    if region.is_ruby {
        out.push_str(r#","is_ruby":true"#);
    }
    out.push_str(r#","chars":["#);
    for (i, ch) in region.chars.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_char(out, ch);
    }
    out.push_str("]}");
}

fn write_char(out: &mut String, ch: &CaptionChar) {
    out.push('{');
    match &ch.body {
        // A DRCS glyph a replacement table resolved is text like any other:
        // once the character is known, a font draws it better than a 36×36
        // stencil does.
        CharBody::Text { text, .. } | CharBody::DrcsReplaced { text, .. } => {
            let _ = write!(out, r#""text":"{}""#, escape(text));
        }
        // Not text at all: the glyph is in the caption's `drcs` map under this
        // code, and a renderer that cannot draw a bitmap shows 〓.
        CharBody::Drcs { code } => {
            let _ = write!(out, r#""drcs":{code}"#);
        }
    }
    let _ = write!(
        out,
        r#","x":{},"y":{},"char_width":{},"char_height":{}"#,
        ch.x, ch.y, ch.char_width, ch.char_height
    );
    // The pen advances by the cell *plus* the spacing, so a reader that leaves
    // these out lays the line out on the wrong pitch. Zero is common enough to
    // be worth omitting.
    if ch.char_horizontal_spacing != 0 {
        let _ = write!(
            out,
            r#","char_horizontal_spacing":{}"#,
            ch.char_horizontal_spacing
        );
    }
    if ch.char_vertical_spacing != 0 {
        let _ = write!(
            out,
            r#","char_vertical_spacing":{}"#,
            ch.char_vertical_spacing
        );
    }
    // MSZ — half width, full height — is how a Japanese caption fits ~34
    // characters on a line, so the horizontal one is present more often than
    // not. Missing means 1.
    if ch.char_horizontal_scale != 1.0 {
        let _ = write!(
            out,
            r#","char_horizontal_scale":{}"#,
            num(ch.char_horizontal_scale)
        );
    }
    if ch.char_vertical_scale != 1.0 {
        let _ = write!(
            out,
            r#","char_vertical_scale":{}"#,
            num(ch.char_vertical_scale)
        );
    }
    let _ = write!(out, r#","text_color":"{}""#, colour(ch.text_color));
    // ARIB fills the whole cell behind the text, which is what makes a caption
    // readable over the picture — but a transparent background is what a
    // caption over a black band uses, and there is no shape to draw for it.
    if !ch.back_color.is_transparent() {
        let _ = write!(out, r#","back_color":"{}""#, colour(ch.back_color));
    }
    // Only meaningful with `style.stroke`, and only sent with it.
    if ch.style.stroke && !ch.stroke_color.is_transparent() {
        let _ = write!(out, r#","stroke_color":"{}""#, colour(ch.stroke_color));
    }
    if !ch.style.is_default() {
        out.push_str(r#","style":{"#);
        let mut flags = Flags::new();
        flags.set(out, "bold", ch.style.bold);
        flags.set(out, "italic", ch.style.italic);
        flags.set(out, "underline", ch.style.underline);
        flags.set(out, "stroke", ch.style.stroke);
        out.push('}');
    }
    if !ch.enclosure.is_none() {
        out.push_str(r#","enclosure":{"#);
        let mut flags = Flags::new();
        flags.set(out, "top", ch.enclosure.top);
        flags.set(out, "right", ch.enclosure.right);
        flags.set(out, "bottom", ch.enclosure.bottom);
        flags.set(out, "left", ch.enclosure.left);
        out.push('}');
    }
    out.push('}');
}

/// Only the flags that are set, comma-separated — `{"bold":true}` rather than
/// four fields of which three say nothing.
struct Flags(bool);

impl Flags {
    fn new() -> Self {
        Flags(false)
    }

    fn set(&mut self, out: &mut String, name: &str, value: bool) {
        if !value {
            return;
        }
        if self.0 {
            out.push(',');
        }
        self.0 = true;
        let _ = write!(out, "\"{name}\":true");
    }
}

/// A DRCS glyph: its dimensions and its bitmap, packed as transmitted.
///
/// `depth_bits` per pixel, most-significant-bit first, row-major, no padding
/// between rows — the same bytes the MD5 is taken over, so a consumer holding a
/// replacement table can key into it without unpacking anything.
fn write_drcs(out: &mut String, glyph: &Drcs) {
    let _ = write!(
        out,
        r#"{{"width":{},"height":{},"depth":{},"depth_bits":{},"md5":"{}","pixels":"{}""#,
        glyph.width,
        glyph.height,
        glyph.depth,
        glyph.depth_bits,
        glyph.md5,
        base64(&glyph.pixels),
    );
    if let Some(alternative) = glyph.alternative {
        let _ = write!(
            out,
            r#","alternative":"{}""#,
            escape(&alternative.to_string())
        );
    }
    out.push('}');
}

/// `#RRGGBBAA` — the CSS form, since that is what reads it. Straight alpha, as
/// [`Rgba`] carries it: `FF` is opaque.
fn colour(c: Rgba) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, c.a)
}

/// A number with no more precision than it needs — `0.5`, not `0.50000`.
fn num(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// Standard base64, padded. Small enough to write out rather than take a
/// dependency for — the crate's whole point is that it has almost none.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b1 = chunk[0] as u32;
        let b2 = *chunk.get(1).unwrap_or(&0) as u32;
        let b3 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b1 << 16) | (b2 << 8) | b3;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// A JSON string body: what the grammar requires escaped, and nothing else.
///
/// Public because the `cues` command wraps these objects in a line of its own
/// and needs the same rules for the fields it writes itself.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CharStyle, Enclosure, WritingMode};

    fn cell(x: i32, y: i32, text: &str) -> CaptionChar {
        CaptionChar {
            body: CharBody::Text {
                text: text.to_string(),
                pua: None,
            },
            x,
            y,
            char_width: 36,
            char_height: 36,
            char_horizontal_spacing: 4,
            char_vertical_spacing: 24,
            char_horizontal_scale: 0.5,
            char_vertical_scale: 1.0,
            text_color: Rgba::new(255, 255, 255, 255),
            back_color: Rgba::new(0, 0, 0, 204),
            stroke_color: Rgba::TRANSPARENT,
            style: CharStyle::default(),
            enclosure: Enclosure::default(),
        }
    }

    fn caption_of(chars: Vec<CaptionChar>) -> Caption {
        let width = chars.iter().map(|c| c.section_width()).sum();
        let (x, y) = (chars[0].x, chars[0].y);
        let height = chars[0].section_height();
        Caption {
            regions: vec![CaptionRegion {
                chars,
                x,
                y,
                width,
                height,
                writing_mode: WritingMode::HorizontalTb,
                is_ruby: false,
            }],
            pts_ms: Some(0),
            ..Default::default()
        }
    }

    #[test]
    fn a_cell_carries_its_place_on_the_plane_and_its_colours() {
        let json = caption(&caption_of(vec![cell(190, 404, "あ")]));
        assert!(
            json.contains(r#""plane_width":960,"plane_height":540"#),
            "{json}"
        );
        assert!(
            json.contains(r#""x":190,"y":404,"char_width":36,"char_height":36"#),
            "{json}"
        );
        assert!(json.contains(r#""char_horizontal_spacing":4"#), "{json}");
        assert!(json.contains(r#""char_horizontal_scale":0.5"#), "{json}");
        assert!(json.contains(r##""text_color":"#FFFFFFFF""##), "{json}");
        assert!(json.contains(r##""back_color":"#000000CC""##), "{json}");
    }

    /// Almost every cell is plain, and saying so four times a cell is most of
    /// the document. What is missing is what [`Caption::default`] says.
    #[test]
    fn the_defaults_are_left_out() {
        let json = caption(&caption_of(vec![cell(190, 404, "あ")]));
        assert!(!json.contains("char_vertical_scale"), "{json}");
        assert!(!json.contains("stroke_color"), "{json}");
        assert!(!json.contains("\"style\""), "{json}");
        assert!(!json.contains("enclosure"), "{json}");
        assert!(!json.contains("is_ruby"), "{json}");
        assert!(!json.contains("drcs"), "{json}");
        assert!(!json.contains("writing_mode"), "{json}");
    }

    #[test]
    fn horizontal_json_stays_byte_for_byte_compatible() {
        let json = caption(&caption_of(vec![cell(190, 404, "あ")]));
        assert_eq!(
            json,
            concat!(
                r##"{"plane_width":960,"plane_height":540,"regions":[{"x":190,"y":404,"width":20,"height":60,"chars":["##,
                r##"{"text":"あ","x":190,"y":404,"char_width":36,"char_height":36,"char_horizontal_spacing":4,"char_vertical_spacing":24,"char_horizontal_scale":0.5,"text_color":"#FFFFFFFF","back_color":"#000000CC"}"##,
                "]}]}"
            )
        );
    }

    #[test]
    fn vertical_regions_name_their_writing_mode() {
        let mut caption = caption_of(vec![cell(912, 0, "縦")]);
        caption.regions[0].writing_mode = WritingMode::VerticalRl;

        let json = super::caption(&caption);

        assert!(json.contains(r#""writing_mode":"vertical-rl""#), "{json}");
    }

    #[test]
    fn only_the_styles_that_are_set_are_named() {
        let mut ch = cell(190, 404, "あ");
        ch.style = CharStyle {
            bold: true,
            stroke: true,
            ..Default::default()
        };
        ch.stroke_color = Rgba::new(0, 0, 255, 255);
        let json = caption(&caption_of(vec![ch]));
        assert!(json.contains(r##""stroke_color":"#0000FFFF""##), "{json}");
        assert!(
            json.contains(r#""style":{"bold":true,"stroke":true}"#),
            "{json}"
        );
    }

    /// A stroke colour with the flag off would have a reader draw an outline
    /// the broadcast did not ask for.
    #[test]
    fn a_stroke_colour_without_the_flag_is_not_sent() {
        let mut ch = cell(190, 404, "あ");
        ch.stroke_color = Rgba::new(0, 0, 255, 255);
        let json = caption(&caption_of(vec![ch]));
        assert!(!json.contains("stroke_color"), "{json}");
    }

    #[test]
    fn ruby_is_a_region_flag() {
        let mut caption_ = caption_of(vec![cell(190, 404, "字")]);
        caption_.regions.push(CaptionRegion {
            x: 190,
            y: 374,
            width: 20,
            height: 30,
            chars: vec![cell(190, 374, "じ")],
            writing_mode: WritingMode::HorizontalTb,
            is_ruby: true,
        });
        let json = caption(&caption_);
        assert!(json.contains(r#""is_ruby":true"#), "{json}");
    }

    /// The one thing in a caption that is genuinely a bitmap goes out as one,
    /// packed exactly as transmitted so the MD5 still keys a replacement table.
    #[test]
    fn a_drcs_glyph_goes_out_packed_and_base64() {
        let mut ch = cell(190, 404, "");
        ch.body = CharBody::Drcs { code: 0x41 };
        let mut caption_ = caption_of(vec![ch]);
        caption_.drcs.insert(
            0x41,
            Drcs {
                width: 4,
                height: 4,
                depth: 2,
                depth_bits: 1,
                // 1111 0110 0110 0110
                pixels: vec![0xF6, 0x66],
                md5: "abc".into(),
                alternative: None,
            },
        );
        let json = caption(&caption_);
        assert!(
            json.contains(r#""drcs":65"#),
            "the cell names the code: {json}"
        );
        assert!(
            json.contains(
                r#""65":{"width":4,"height":4,"depth":2,"depth_bits":1,"md5":"abc","pixels":"9mY=""#
            ),
            "{json}"
        );
    }

    #[test]
    fn base64_pads_every_tail_length() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_quote_in_a_caption_cannot_close_the_string() {
        let json = caption(&caption_of(vec![cell(0, 0, "\"\\\n")]));
        assert!(json.contains(r#""text":"\"\\\n""#), "{json}");
    }
}
