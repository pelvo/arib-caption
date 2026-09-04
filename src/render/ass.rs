//! ASS: where the caption was, what colour it was, how wide the cells were —
//! the sidecar form for a recording, where a player will honour all of it.
//!
//! The trick that makes this tractable is `PlayResX`/`PlayResY`. Set them to
//! the caption plane the broadcast declared (960×540 for full-seg) and every
//! coordinate in the model goes out unchanged, with libass scaling the whole
//! plane onto whatever the video really is. Nothing here needs to know the
//! video's resolution, which is just as well — the decoder does not either.
//!
//! Two things follow from ARIB laying text out on a fixed cell grid rather than
//! flowing it:
//!
//! - A region is emitted as one `Dialogue` positioned with `\pos`, never as a
//!   line the player is left to place. Letter spacing (`\fsp`) is derived from
//!   the cell advance so the run lands on the grid the broadcast used.
//! - MSZ (half-width) text must be drawn as fullwidth glyphs squeezed to 50%
//!   (`\fscx50`), which is what a television does, and *not* as the halfwidth
//!   characters [`crate::Options::replace_msz_fullwidth_ascii`] substitutes for
//!   text output. A halfwidth glyph advances half an em where the cell expects
//!   a full one, and the line walks off the grid. Decode with that option off
//!   when rendering to ASS.
//!
//! Vertical placement is `\an4` — middle-left — against the centre of the cell,
//! rather than `\an7` against its top. Both are the same for a font whose line
//! box is exactly one em; only `\an4` stays right for a font where it is not.
//!
//! And no font is: `\fs` asks for the *line box*, which a player divides by the
//! font's own `usWinAscent + usWinDescent` to reach an em. A cell's height
//! therefore has to be asked for multiplied back out — see
//! [`DEFAULT_FONT_SIZE_RATIO`], which is the one number here that depends on
//! which font renders the script.
//!
//! DRCS glyphs are drawn rather than substituted. A DRCS character is a bitmap
//! the broadcast defined on the fly for something no code set has, and ASS can
//! draw a bitmap: one `\p1` rectangle per run of set pixels, scaled into the
//! cell. So a caption using them is *right* here without any of the machinery
//! a text renderer would need — no replacement table, no font that has the
//! character — where WebVTT can only show 〓.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::model::{Caption, CaptionChar, CaptionRegion, CharBody, Drcs, Rgba};
use crate::render::timeline::Timed;

/// The font a script names when nothing else is asked for.
///
/// A rounded gothic, because that is what ARIB specifies and what a television
/// draws. This one is the Google Fonts release of the Rounded M+ lineage every
/// ARIB tool used to recommend, which matters because the fonts those tools
/// linked to were on file hosts that have since closed. Glyphs it lacks — the
/// ARIB additional symbols above the BMP, 🈑 and friends — libass fills in from
/// fontconfig's fallback.
///
/// The name is the trap. Google Fonts lists the family as *M PLUS Rounded 1c*
/// and its webfont CSS asks for that, but the `.ttf` you install carries
/// `Rounded Mplus 1c` in its name table, which is what fontconfig matches on
/// and therefore what an ASS `Fontname` has to say. Asking for the other one
/// renders identically to asking for a font that does not exist — measured, by
/// rendering all three.
pub const DEFAULT_FONT: &str = "Rounded Mplus 1c";

/// How much taller than an em [`DEFAULT_FONT`]'s own line box is.
///
/// `Fontsize` in ASS is not the em. libass, following VSFilter, scales the face
/// so that the font's `usWinAscent + usWinDescent` comes out at the size asked
/// for — so a 36-unit em has to be *asked for* as 36 × this ratio, and a script
/// that asks for 36 puts a 26-unit glyph in the 36-unit cell it drew a
/// background for. That is what this renderer did until it was measured against
/// libass: text a quarter short of the pitch the broadcast laid out, sitting
/// small and left inside its own box.
///
/// 1.395 is this font's: `usWinAscent` 1075 + `usWinDescent` 320 over an
/// `unitsPerEm` of 1000, read out of the Google Fonts release. Another font
/// wants [`Options::font_size_ratio`] — though the CJK gothics a system
/// substitutes when this one is absent sit around 1.45, close enough to leave a
/// caption a few percent narrow rather than a quarter.
pub const DEFAULT_FONT_SIZE_RATIO: f32 = 1.395;

/// What the caption plane defaults to when a script has no caption to read it
/// from: ARIB STD-B24 profile A.
const DEFAULT_PLANE: (i32, i32) = (960, 540);

/// What a DRCS character falls back to when its glyph was never transmitted.
///
/// GETA is what every ARIB decoder shows for a character it cannot draw. It
/// should be rare here: a glyph that *was* transmitted is drawn as a vector
/// outline rather than substituted, so this is only for a stream that used a
/// DRCS code without defining it.
const GETA: &str = "〓";

#[derive(Clone, Debug)]
pub struct Options {
    pub font: String,
    /// The font's line box over its em — see [`DEFAULT_FONT_SIZE_RATIO`]. Every
    /// `\fs` is multiplied by it, because that is what a player divides by.
    pub font_size_ratio: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            font: DEFAULT_FONT.to_string(),
            font_size_ratio: DEFAULT_FONT_SIZE_RATIO,
        }
    }
}

/// A whole ASS script: the header plus every caption. For a recording sidecar,
/// where all the captions are known.
///
/// Times are whatever the caller put in [`Timed`] — for a sidecar they must
/// already be relative to the start of the recording, not broadcast PTS. See
/// [`crate::render::timeline::rebase`].
pub fn to_file(captions: &[Timed<Caption>], options: &Options) -> String {
    let plane = plane_of(captions);
    let mut out = header(plane, options);
    for timed in captions {
        write_caption(
            &mut out,
            &timed.value,
            timed.start_ms,
            timed.end_ms,
            plane,
            options.font_size_ratio,
        );
    }
    out
}

/// The script header, declaring the caption plane as the coordinate system.
pub fn header(plane: (i32, i32), options: &Options) -> String {
    let mut out = String::new();
    out.push_str("[Script Info]\n");
    out.push_str("; ARIB STD-B24 captions, rendered by arib-caption\n");
    out.push_str("ScriptType: v4.00+\n");
    let _ = writeln!(out, "PlayResX: {}", plane.0);
    let _ = writeln!(out, "PlayResY: {}", plane.1);
    // 2: no wrapping of any kind. Every line here is positioned on the grid the
    // broadcast sent; a player breaking one would put it somewhere else.
    out.push_str("WrapStyle: 2\n");
    out.push_str("ScaledBorderAndShadow: yes\n");
    // The colours below are the B24 CLUT's own RGB. Naming a matrix would have
    // libass convert them as if they were TV-range luma.
    out.push_str("YCbCr Matrix: None\n\n");

    out.push_str("[V4+ Styles]\n");
    out.push_str(
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
         BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
         BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
    );
    // Everything that matters is overridden per line; this only has to be a
    // style that adds nothing of its own — no outline, no shadow, no margins.
    // Its size is the profile's standard body asked for the way ASS wants it
    // (see DEFAULT_FONT_SIZE_RATIO), so a player that somehow drew a line
    // without the overrides would still draw it at a cell's height.
    let _ = writeln!(
        out,
        "Style: Default,{},{},&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,\
         0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1\n",
        options.font,
        num(36.0 * options.font_size_ratio)
    );

    out.push_str("[Events]\n");
    out.push_str(
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
    );
    out
}

/// Every `Dialogue` line one caption produces.
fn write_caption(
    out: &mut String,
    caption: &Caption,
    start_ms: i64,
    end_ms: i64,
    plane: (i32, i32),
    font_size_ratio: f32,
) {
    let scale = Scale::to(plane, caption);
    let (start, end) = (timestamp(start_ms), timestamp(end_ms));
    for region in &caption.regions {
        // Background first, on the lower layer: ARIB fills the whole cell
        // behind the text, which is what makes a caption readable over the
        // picture, and an outline is not the same shape.
        write_backgrounds(out, region, &start, &end, scale);
        write_text(
            out,
            region,
            &start,
            &end,
            scale,
            &caption.drcs,
            font_size_ratio,
        );
    }
}

/// One drawing per run of cells sharing a background colour.
fn write_backgrounds(
    out: &mut String,
    region: &CaptionRegion,
    start: &str,
    end: &str,
    scale: Scale,
) {
    for run in runs(&region.chars, |a, b| a.back_color == b.back_color) {
        let first = &run[0];
        if first.back_color.is_transparent() {
            continue;
        }
        let x = scale.x(first.x);
        let y = scale.y(first.y);
        let w = scale.x(run.iter().map(|c| c.section_width()).sum());
        let h = scale.y(run.iter().map(|c| c.section_height()).max().unwrap_or(0));
        let _ = writeln!(
            out,
            "Dialogue: 0,{start},{end},Default,,0,0,0,,{{\\an7\\pos({x},{y})\\p1\\bord0\\shad0\
             \\1c{}\\1a{}}}m 0 0 l {w} 0 l {w} {h} l 0 {h}{{\\p0}}",
            colour(first.back_color),
            alpha(first.back_color),
        );
    }
}

/// One `Dialogue` per run of cells sharing the metrics that decide where the
/// next character lands; colour and emphasis change inline within a run.
///
/// A DRCS cell breaks the run and becomes a drawing of its own. It has to: the
/// pen inside a `Dialogue` advances by glyph, and a drawing does not advance it
/// at all, so anything after one in the same line would slide left by a cell.
fn write_text(
    out: &mut String,
    region: &CaptionRegion,
    start: &str,
    end: &str,
    scale: Scale,
    drcs: &HashMap<u32, Drcs>,
    ratio: f32,
) {
    for run in runs(&region.chars, |a, b| {
        Metrics::of(a, scale, ratio) == Metrics::of(b, scale, ratio)
            && glyph_of(a, drcs).is_none()
            && glyph_of(b, drcs).is_none()
    }) {
        let first = &run[0];
        if let Some(glyph) = glyph_of(first, drcs) {
            write_glyph(out, first, glyph, start, end, scale);
            continue;
        }
        // The first character's appearance belongs in the same override block
        // as the position; only a later change needs one of its own.
        let mut head = String::new();
        let mut body = String::new();
        let mut prev: Option<&CaptionChar> = None;
        for ch in run {
            let tags = appearance(ch, prev);
            if prev.is_none() {
                head = tags;
            } else if !tags.is_empty() {
                let _ = write!(body, "{{{tags}}}");
            }
            body.push_str(&escape(text_of(ch)));
            prev = Some(ch);
        }
        let _ = writeln!(
            out,
            "Dialogue: 1,{start},{end},Default,,0,0,0,,{{\\an4\\pos({},{}){}{head}}}{body}",
            scale.x(first.x),
            // The middle of the cell, not its top: see the module docs.
            scale.yf(first.y as f32 + first.section_height() as f32 / 2.0),
            Metrics::of(first, scale, ratio),
        );
    }
}

/// The DRCS glyph a cell draws, if it draws one.
///
/// A glyph the replacement table resolved is *not* one of these: once the
/// character is known, a font draws it better than a 36×36 stencil does.
fn glyph_of<'a>(ch: &CaptionChar, drcs: &'a HashMap<u32, Drcs>) -> Option<&'a Drcs> {
    let CharBody::Drcs { code } = ch.body else {
        return None;
    };
    drcs.get(&code)
        .filter(|g| g.width > 0 && g.height > 0 && !g.pixels.is_empty())
}

/// A DRCS glyph as a vector outline: one rectangle per run of set pixels.
///
/// This is what keeps a broadcast's own characters on screen without a font
/// that has them and without a replacement table that names them — the bitmap
/// *was* transmitted, so there is nothing to look up, only something to draw.
///
/// The path is in the bitmap's own pixel coordinates and `\fscx`/`\fscy` scale
/// it into the cell, which keeps the numbers small and exact. Both of those
/// hold for libass by measurement: it scales drawings, and `\an7\pos` anchors
/// the path's own origin rather than the ink's bounding box, so a glyph with an
/// empty left column still lands where the cell is.
fn write_glyph(
    out: &mut String,
    ch: &CaptionChar,
    glyph: &Drcs,
    start: &str,
    end: &str,
    scale: Scale,
) {
    let cell_w = ch.char_width as f32 * ch.char_horizontal_scale * scale.sx;
    let cell_h = ch.char_height as f32 * ch.char_vertical_scale * scale.sy;
    let fscx = num(100.0 * cell_w / glyph.width as f32);
    let fscy = num(100.0 * cell_h / glyph.height as f32);
    let x = scale.x(ch.x);
    // The same seat a text cell gets: the character box centred in the section,
    // with the line spacing half above and half below. Drawing it at the top of
    // the section instead — which is where the coordinate is — leaves a DRCS
    // character riding above the words either side of it, by half the spacing.
    let y = scale.yf(ch.y as f32 + (ch.section_height() as f32 - cell_h / scale.sy) / 2.0);

    // A two-level glyph is a stencil and this runs once. A deeper one uses its
    // extra levels as coverage, which becomes one drawing per level at a
    // proportion of the text colour's alpha — ASS fills a path with one colour,
    // so partial coverage has to be partial transparency.
    // The standard allows a depth this loop could not index — `level` is a
    // byte — so it is clamped rather than cast, where a cast would wrap to an
    // empty range and draw nothing at all.
    let levels = glyph.depth.clamp(2, 256);
    for level in 1..levels {
        let rects = ink(glyph, level as u8);
        if rects.is_empty() {
            continue;
        }
        let mut path = String::new();
        for (rx, ry, rw, rh) in rects {
            let (x2, y2) = (rx + rw, ry + rh);
            let _ = write!(path, "m {rx} {ry} l {x2} {ry} l {x2} {y2} l {rx} {y2} ");
        }
        let opacity = ch.text_color.a as u32 * level / (levels - 1);
        let _ = writeln!(
            out,
            "Dialogue: 1,{start},{end},Default,,0,0,0,,{{\\an7\\pos({x},{y})\\p1\\bord0\\shad0\
             \\fscx{fscx}\\fscy{fscy}\\1c{}\\1a&H{:02X}&}}{}{{\\p0}}",
            colour(ch.text_color),
            255 - opacity.min(255) as u8,
            path.trim_end(),
        );
    }
}

/// The pixels at one level, merged into as few rectangles as possible.
///
/// Row by row, extending a rectangle downwards while the row below repeats it.
/// Worth the few lines: a DRCS glyph is often sent at double height with every
/// row duplicated, and a rectangle per row would double the path for nothing.
fn ink(glyph: &Drcs, level: u8) -> Vec<(u32, u32, u32, u32)> {
    let mut done: Vec<(u32, u32, u32, u32)> = Vec::new();
    // (x, width, first row) for the rectangles still growing downwards.
    let mut open: Vec<(u32, u32, u32)> = Vec::new();

    for y in 0..glyph.height {
        let mut row: Vec<(u32, u32)> = Vec::new();
        let mut x = 0;
        while x < glyph.width {
            if glyph.level(x, y) != level {
                x += 1;
                continue;
            }
            let from = x;
            while x < glyph.width && glyph.level(x, y) == level {
                x += 1;
            }
            row.push((from, x - from));
        }

        let mut still_open = Vec::with_capacity(row.len());
        for (rx, rw) in row {
            match open.iter().position(|&(ox, ow, _)| ox == rx && ow == rw) {
                Some(i) => {
                    let (_, _, top) = open.remove(i);
                    still_open.push((rx, rw, top));
                }
                None => still_open.push((rx, rw, y)),
            }
        }
        for (ox, ow, top) in open.drain(..) {
            done.push((ox, top, ow, y - top));
        }
        open = still_open;
    }
    for (ox, ow, top) in open {
        done.push((ox, top, ow, glyph.height - top));
    }

    // Reading order, so the same glyph always produces the same path.
    done.sort_unstable_by_key(|&(x, y, _, _)| (y, x));
    done
}

/// The override tags that turn the previous character's appearance into this
/// one's, unbraced — everything the style does not already give on the first
/// character, only the differences after it.
fn appearance(ch: &CaptionChar, prev: Option<&CaptionChar>) -> String {
    let mut tags = String::new();
    if prev.map(|p| p.text_color) != Some(ch.text_color) {
        let _ = write!(
            tags,
            "\\1c{}\\1a{}",
            colour(ch.text_color),
            alpha(ch.text_color)
        );
    }
    // Stroke is ARIB's outlined text. Where it is off, the cell's background is
    // what makes the text readable, so the border goes back to nothing rather
    // than to whatever the style carries.
    let stroked = ch.style.stroke && !ch.stroke_color.is_transparent();
    let was_stroked = prev.is_some_and(|p| p.style.stroke && !p.stroke_color.is_transparent());
    let stroke_changed = stroked && prev.map(|p| p.stroke_color) != Some(ch.stroke_color);
    if stroked {
        if !was_stroked || stroke_changed {
            let _ = write!(
                tags,
                "\\bord2\\3c{}\\3a{}",
                colour(ch.stroke_color),
                alpha(ch.stroke_color)
            );
        }
    } else if was_stroked {
        tags.push_str("\\bord0");
    }
    // The style the script declares is the plain one, so the first character
    // only has to say what is *not* plain — writing \b0\i0\u0 on every line
    // would be three quarters of the file saying nothing.
    let was = prev.map(|p| p.style).unwrap_or_default();
    if was.bold != ch.style.bold {
        let _ = write!(tags, "\\b{}", u8::from(ch.style.bold));
    }
    if was.italic != ch.style.italic {
        let _ = write!(tags, "\\i{}", u8::from(ch.style.italic));
    }
    if was.underline != ch.style.underline {
        let _ = write!(tags, "\\u{}", u8::from(ch.style.underline));
    }
    tags
}

/// The size and spacing of one cell, expressed the way ASS expresses them.
///
/// The em is the cell's height, and for a CJK glyph that is also its advance, so
/// the rest follows from the cell: the horizontal scale squeezes a square glyph
/// into a cell that may not be square, and the letter spacing makes up the
/// difference between the glyph and the pitch the broadcast advanced by. libass
/// scales `\fsp` by `\fscx` — measured, not assumed — which is why the spacing
/// below is in unscaled em units.
///
/// What ASS will not let you say is the em itself: `\fs` asks for the *line box*
/// and the player divides by the font's own proportions to get there. So the em
/// is what everything here is computed from, and `\fs` is that em multiplied
/// back out by [`Options::font_size_ratio`] at the last moment.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Metrics {
    em: f32,
    ratio: f32,
    fscx: f32,
    fsp: f32,
}

impl Metrics {
    fn of(ch: &CaptionChar, scale: Scale, ratio: f32) -> Metrics {
        let em = ch.char_height as f32 * ch.char_vertical_scale * scale.sy;
        let glyph_width = ch.char_width as f32 * ch.char_horizontal_scale * scale.sx;
        let fscx = if em > 0.0 {
            100.0 * glyph_width / em
        } else {
            100.0
        };
        let fsp = if ch.char_width > 0 {
            ch.char_horizontal_spacing as f32 * em / ch.char_width as f32
        } else {
            0.0
        };
        Metrics {
            em,
            ratio,
            fscx,
            fsp,
        }
    }
}

impl std::fmt::Display for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\\fs{}\\fscx{}\\fsp{}",
            num(self.em * self.ratio),
            num(self.fscx),
            num(self.fsp)
        )
    }
}

/// Maps a caption's own plane onto the one the script declares.
///
/// Both are normally the same and this is the identity. It is not free to
/// assume so: a service that changes between HD and SD mid-recording changes
/// the plane with it, and one script can only declare one `PlayRes`.
#[derive(Clone, Copy, Debug)]
struct Scale {
    sx: f32,
    sy: f32,
}

impl Scale {
    fn to(plane: (i32, i32), caption: &Caption) -> Scale {
        let sx = if caption.plane_width > 0 {
            plane.0 as f32 / caption.plane_width as f32
        } else {
            1.0
        };
        let sy = if caption.plane_height > 0 {
            plane.1 as f32 / caption.plane_height as f32
        } else {
            1.0
        };
        Scale { sx, sy }
    }

    fn x(&self, v: i32) -> String {
        num(v as f32 * self.sx)
    }

    fn y(&self, v: i32) -> String {
        num(v as f32 * self.sy)
    }

    fn yf(&self, v: f32) -> String {
        num(v * self.sy)
    }
}

/// The plane every caption in the script is written against: the first one's.
fn plane_of(captions: &[Timed<Caption>]) -> (i32, i32) {
    captions
        .iter()
        .map(|c| (c.value.plane_width, c.value.plane_height))
        .find(|&(w, h)| w > 0 && h > 0)
        .unwrap_or(DEFAULT_PLANE)
}

/// Consecutive characters for which `same` holds, in order.
fn runs<F>(chars: &[CaptionChar], same: F) -> Vec<&[CaptionChar]>
where
    F: Fn(&CaptionChar, &CaptionChar) -> bool,
{
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        if !same(&chars[i - 1], &chars[i]) {
            out.push(&chars[start..i]);
            start = i;
        }
    }
    if start < chars.len() {
        out.push(&chars[start..]);
    }
    out
}

fn text_of(ch: &CaptionChar) -> &str {
    ch.text().unwrap_or(GETA)
}

/// `&HBBGGRR&` — ASS orders the channels backwards from everything else.
fn colour(c: Rgba) -> String {
    format!("&H{:02X}{:02X}{:02X}&", c.b, c.g, c.r)
}

/// `&HAA&`, where ASS counts *transparency*: 00 is opaque.
fn alpha(c: Rgba) -> String {
    format!("&H{:02X}&", 255 - c.a)
}

/// `H:MM:SS.cc` — ASS keeps hundredths and no more.
fn timestamp(ms: i64) -> String {
    let cs = (ms.max(0) + 5) / 10;
    format!(
        "{}:{:02}:{:02}.{:02}",
        cs / 360_000,
        (cs / 6_000) % 60,
        (cs / 100) % 60,
        cs % 100
    )
}

/// A number with no more precision than it needs — `36`, not `36.00`.
fn num(v: f32) -> String {
    let rounded = (v * 100.0).round() / 100.0;
    if rounded.fract() == 0.0 {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

/// Braces open an override block, so a caption containing one would be read as
/// markup. Nothing else in ASS text is special.
fn escape(text: &str) -> String {
    if text.contains(['{', '}']) {
        text.replace('{', "\\{").replace('}', "\\}")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CharBody, CharStyle};

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
            // MSZ: half width, full height — how a Japanese caption fits ~34
            // characters on a line, and the common case by far.
            char_horizontal_scale: 0.5,
            char_vertical_scale: 1.0,
            text_color: Rgba::new(255, 255, 255, 255),
            back_color: Rgba::new(0, 0, 0, 128),
            stroke_color: Rgba::TRANSPARENT,
            style: CharStyle::default(),
            enclosure: Default::default(),
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
                writing_mode: Default::default(),
                is_ruby: false,
            }],
            pts_ms: Some(0),
            ..Default::default()
        }
    }

    /// Rendered with a font whose line box *is* its em, so every size below
    /// reads in the cell units the broadcast sent. The multiplier a real font
    /// needs has a test of its own.
    fn render(caption: Caption) -> String {
        to_file(
            &[Timed {
                start_ms: 1_500,
                end_ms: 4_250,
                value: caption,
            }],
            &Options {
                font_size_ratio: 1.0,
                ..Options::default()
            },
        )
    }

    #[test]
    fn the_script_declares_the_caption_plane_as_its_coordinate_system() {
        let script = render(caption_of(vec![cell(120, 400, "あ")]));
        assert!(script.contains("PlayResX: 960"), "{script}");
        assert!(script.contains("PlayResY: 540"), "{script}");
        assert!(script.contains(DEFAULT_FONT), "{script}");
    }

    /// The whole point of ASS over WebVTT: the caption lands where it was sent.
    #[test]
    fn a_region_is_positioned_at_its_own_cell() {
        let script = render(caption_of(vec![cell(120, 400, "あ")]));
        // x as sent; y through the middle of a 60-tall cell (36 + 24 spacing).
        assert!(script.contains("\\an4\\pos(120,430)"), "{script}");
    }

    /// A half-width cell is a fullwidth glyph squeezed, and the pen advances by
    /// the cell — which is `\fsp`'s job, and it has to be in unscaled em units
    /// because libass scales it by `\fscx`.
    #[test]
    fn half_width_cells_squeeze_the_glyph_and_keep_the_pitch() {
        let script = render(caption_of(vec![cell(120, 400, "あ"), cell(140, 400, "い")]));
        assert!(script.contains("\\fs36\\fscx50\\fsp4"), "{script}");
        // Both cells are one Dialogue: same metrics, contiguous.
        assert_eq!(script.matches("Dialogue: 1,").count(), 1, "{script}");
        assert!(script.contains("あい"), "{script}");
    }

    /// `\fs` is the font's line box and not its em, so a cell's height has to be
    /// asked for multiplied by the font's own proportions. Getting this wrong is
    /// not subtle: at 1.395 the glyphs come out at 72% of the cell they have a
    /// background drawn for, short of the pitch and hugging the left of it.
    #[test]
    fn the_font_size_asks_for_the_line_box_not_the_em() {
        let script = to_file(
            &[Timed {
                start_ms: 0,
                end_ms: 1_000,
                value: caption_of(vec![{
                    let mut c = cell(120, 400, "あ");
                    c.char_horizontal_scale = 1.0;
                    c
                }]),
            }],
            &Options::default(),
        );
        // 36 × 1.395, while everything derived from the em is untouched: the
        // glyph still fills its cell and the pen still advances by the pitch.
        assert!(script.contains("\\fs50.22\\fscx100\\fsp4"), "{script}");
        // And the style's own size follows the same convention.
        assert!(
            script.contains(&format!("{},50.22,", DEFAULT_FONT)),
            "{script}"
        );
    }

    #[test]
    fn the_background_is_a_filled_cell_not_an_outline() {
        let script = render(caption_of(vec![cell(120, 400, "あ"), cell(140, 400, "い")]));
        // Two 20-wide cells, 60 tall, at the region's own corner.
        assert!(
            script.contains(
                "{\\an7\\pos(120,400)\\p1\\bord0\\shad0\\1c&H000000&\\1a&H7F&}m 0 0 l 40 0 l 40 60 l 0 60"
            ),
            "{script}"
        );
        assert!(
            script.contains("Dialogue: 0,"),
            "the background is on the lower layer"
        );
    }

    #[test]
    fn a_colour_change_is_inline_and_does_not_break_the_run() {
        let mut second = cell(140, 400, "い");
        second.text_color = Rgba::new(255, 255, 0, 255);
        let script = render(caption_of(vec![cell(120, 400, "あ"), second]));
        assert_eq!(script.matches("Dialogue: 1,").count(), 1, "{script}");
        // BGR, and opaque is alpha 00.
        assert!(script.contains("\\1c&HFFFFFF&\\1a&H00&"), "{script}");
        assert!(script.contains("あ{\\1c&H00FFFF&\\1a&H00&}い"), "{script}");
    }

    /// Ruby is the other thing ASS keeps that WebVTT cannot: it is a region
    /// like any other, at its own size, above the line it belongs to.
    #[test]
    fn ruby_is_kept_at_its_own_size() {
        let mut ruby = cell(120, 370, "あ");
        ruby.char_width = 18;
        ruby.char_height = 18;
        ruby.char_horizontal_spacing = 2;
        ruby.char_vertical_spacing = 12;
        ruby.char_horizontal_scale = 1.0;
        let mut caption = caption_of(vec![cell(120, 400, "字")]);
        caption.regions.push(CaptionRegion {
            x: 120,
            y: 370,
            width: ruby.section_width(),
            height: ruby.section_height(),
            chars: vec![ruby],
            writing_mode: Default::default(),
            is_ruby: true,
        });
        let script = render(caption);
        assert_eq!(script.matches("Dialogue: 1,").count(), 2, "{script}");
        assert!(script.contains("\\fs18\\fscx100"), "{script}");
    }

    /// A glyph that was never transmitted is all a text renderer ever has.
    #[test]
    fn a_drcs_code_with_no_glyph_shows_geta() {
        let mut drcs = cell(120, 400, "");
        drcs.body = CharBody::Drcs { code: 0x41 };
        let script = render(caption_of(vec![drcs]));
        assert!(script.contains(GETA), "{script}");
    }

    /// A four-pixel-wide glyph: a filled top row, then a two-pixel column down
    /// the middle for three rows. Packed one bit per pixel, MSB first.
    fn stencil() -> Drcs {
        let rows = [0b1111u8, 0b0110, 0b0110, 0b0110];
        let mut bits = 0u16;
        for (i, row) in rows.iter().enumerate() {
            bits |= (*row as u16) << (12 - i * 4);
        }
        Drcs {
            width: 4,
            height: 4,
            depth: 2,
            depth_bits: 1,
            pixels: bits.to_be_bytes().to_vec(),
            md5: "test".into(),
            alternative: None,
        }
    }

    fn drcs_caption(code: u32, chars: Vec<CaptionChar>) -> Caption {
        let mut caption = caption_of(chars);
        caption.drcs.insert(code, stencil());
        caption
    }

    /// The point of the whole exercise: a character no font has, on screen.
    #[test]
    fn a_drcs_glyph_is_drawn_as_an_outline() {
        let mut drcs = cell(120, 400, "");
        drcs.body = CharBody::Drcs { code: 0x41 };
        let script = render(drcs_caption(0x41, vec![drcs]));

        assert!(!script.contains(GETA), "drawn, not substituted:\n{script}");
        // The cell is 18 wide (36 × 0.5) and 36 tall, the bitmap 4 × 4.
        assert!(
            script.contains("\\p1\\bord0\\shad0\\fscx450\\fscy900"),
            "{script}"
        );
        // Two rectangles, in reading order: the full top row, then the column
        // below it merged down three rows rather than drawn as three.
        assert!(
            script.contains("}m 0 0 l 4 0 l 4 1 l 0 1 m 1 1 l 3 1 l 3 4 l 1 4{\\p0}"),
            "{script}"
        );
    }

    /// A drawing does not advance the pen, so a cell after one has to start a
    /// `Dialogue` of its own or it slides a cell to the left.
    #[test]
    fn a_glyph_breaks_the_run_around_it() {
        let mut drcs = cell(140, 400, "");
        drcs.body = CharBody::Drcs { code: 0x41 };
        let script = render(drcs_caption(
            0x41,
            vec![cell(120, 400, "あ"), drcs, cell(160, 400, "い")],
        ));
        // Text before, glyph, text after — three lines, each positioned.
        assert_eq!(script.matches("Dialogue: 1,").count(), 3, "{script}");
        assert!(script.contains("\\pos(120,430)"), "{script}");
        assert!(script.contains("\\pos(140,412)"), "{script}");
        assert!(script.contains("\\pos(160,430)"), "{script}");
    }

    /// Deeper glyphs use their extra levels as coverage. ASS fills a path with
    /// one colour, so each level becomes its own drawing at its own alpha.
    #[test]
    // The literal below is grouped by pixel, two bits each, not by nibble.
    #[allow(clippy::unusual_byte_groupings)]
    fn a_multi_level_glyph_becomes_one_drawing_per_level() {
        let mut glyph = stencil();
        glyph.depth = 4;
        glyph.depth_bits = 2;
        // Two pixels wide: levels 1 and 3 on the first row, nothing after.
        glyph.width = 2;
        glyph.height = 1;
        glyph.pixels = vec![0b01_11_0000];

        let mut drcs = cell(120, 400, "");
        drcs.body = CharBody::Drcs { code: 0x41 };
        let mut caption = caption_of(vec![drcs]);
        caption.drcs.insert(0x41, glyph);
        let script = render(caption);

        // A third of the way opaque, and fully opaque. Nothing for level 2.
        assert!(
            script.contains("\\1a&HAA&}m 0 0 l 1 0 l 1 1 l 0 1"),
            "{script}"
        );
        assert!(
            script.contains("\\1a&H00&}m 1 0 l 2 0 l 2 1 l 1 1"),
            "{script}"
        );
        // \fscy only ever appears on a glyph drawing, so it counts them.
        assert_eq!(script.matches("\\fscy").count(), 2, "one drawing per level");
    }

    #[test]
    fn times_are_hundredths_and_a_brace_cannot_open_a_tag() {
        let script = render(caption_of(vec![cell(120, 400, "{x}")]));
        assert!(script.contains("0:00:01.50,0:00:04.25"), "{script}");
        assert!(script.contains("\\{x\\}"), "{script}");
    }

    /// A caption from a smaller plane still has to land in the right place.
    #[test]
    fn a_plane_change_mid_script_is_scaled_onto_the_declared_one() {
        let mut sd = caption_of(vec![cell(360, 240, "あ")]);
        sd.plane_width = 720;
        sd.plane_height = 480;
        let script = to_file(
            &[
                Timed {
                    start_ms: 0,
                    end_ms: 1_000,
                    value: caption_of(vec![cell(120, 400, "い")]),
                },
                Timed {
                    start_ms: 1_000,
                    end_ms: 2_000,
                    value: sd,
                },
            ],
            &Options::default(),
        );
        assert!(script.contains("PlayResX: 960"), "{script}");
        // 360 × 960/720 = 480; the cell's middle, 240 + 30, × 540/480 = 303.75.
        assert!(script.contains("\\pos(480,303.75)"), "{script}");
    }
}
