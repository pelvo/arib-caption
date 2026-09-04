//! The four transport streams the integration tests read.
//!
//! These replace `tests/fixtures/{caption,superimpose,psi,drcs}.ts`, which were
//! slices of a real recording. What each stream must still exercise is written
//! into it deliberately: a PMT that does not fit one packet, superimpose as
//! private_stream_2 with no PTS, the ARIB additional symbols, ruby, MSZ, cell
//! positioning, and a DRCS glyph.

use super::b24;
use super::ts::{self, ElementaryStream, TsWriter};

pub const CAPTION_PID: u16 = 0x0130;
pub const SUPERIMPOSE_PID: u16 = 0x0138;
pub const PMT_PID: u16 = 0x01F0;
pub const SERVICE_ID: u16 = 1024;
/// TR-B14 gives 0x30..0x37 to captions and 0x38..0x3F to superimpose, which is
/// the only thing telling the two PIDs apart.
pub const CAPTION_COMPONENT_TAG: u8 = 0x30;
pub const SUPERIMPOSE_COMPONENT_TAG: u8 = 0x38;
/// Management writing format 8 → SWF 7 → the 960x540 plane full-seg uses.
pub const WRITING_FORMAT: u8 = 8;

/// The PTS of the first statement, in 90 kHz ticks (10.000 s).
pub const FIRST_STATEMENT_PTS_90K: u64 = 90_000 * 10;
/// The gap between consecutive statements, in 90 kHz ticks (2.000 s).
pub const STATEMENT_INTERVAL_90K: u64 = 90_000 * 2;
/// How far ahead of its statement the management data is sent (0.500 s).
pub const MANAGEMENT_LEAD_90K: u64 = 45_000;

/// MD5 of `b24::drcs_stencil()` packed for transmission. It is taken over the
/// bytes as transmitted, so it must not move when the unpacking changes — that
/// is the whole point of hashing the packed form.
pub const DRCS_GLYPH_MD5: &str = "c01f2da36bc6d70445b836a6e41dccff";
/// DRCS-1, code 0x21.
pub const DRCS_CHARACTER_CODE: u16 = 0x4121;

/// One statement of the synthetic caption stream.
#[derive(Clone, Copy, Debug)]
pub struct Line {
    /// APS line parameter for the text.
    pub row: u8,
    /// APS character parameter for the text.
    pub column: u8,
    /// Kanji-plane text, ruby excluded.
    pub text: &'static str,
    /// Append the ARIB continuation arrow ➡.
    pub arrow: bool,
    /// Append ⁉.
    pub interrobang: bool,
    /// Write the text under MSZ (half width, full height).
    pub half_width: bool,
    /// Ruby placed at half size on the row above the text.
    pub ruby: Option<&'static str>,
    /// A TIME wait, in units of 100 ms.
    pub wait_100ms: Option<u8>,
    /// Half-width text appended right after `text`, in the same region (no
    /// intervening APS): MSZ before it, NSZ after. Unlike `half_width`, which
    /// makes the whole line half width, this makes one region genuinely mix
    /// full-width and half-width cells — the case a wrong `\fsp` on the
    /// second run would only show up on screen, never in the file.
    pub mixed_suffix: Option<&'static str>,
    /// Emit the colour sequence the real capture sends on every statement,
    /// found uncovered before this: `COL` palette-select to palette 4,
    /// `COL` background-select to `CLUT[4][1]` (a half-tone black box),
    /// then `YLF` for a yellow foreground. See `statement_body`.
    pub color: bool,
}

const fn line(row: u8, column: u8, text: &'static str) -> Line {
    Line {
        row,
        column,
        text,
        arrow: false,
        interrobang: false,
        half_width: false,
        ruby: None,
        wait_100ms: None,
        mixed_suffix: None,
        color: false,
    }
}

const fn arrow_line(row: u8, column: u8, text: &'static str) -> Line {
    Line {
        row,
        column,
        text,
        arrow: true,
        interrobang: false,
        half_width: false,
        ruby: None,
        wait_100ms: None,
        mixed_suffix: None,
        color: false,
    }
}

const fn symbol_line(row: u8, column: u8, text: &'static str) -> Line {
    Line {
        row,
        column,
        text,
        arrow: false,
        interrobang: true,
        half_width: false,
        ruby: None,
        wait_100ms: None,
        mixed_suffix: None,
        color: false,
    }
}

const fn half_width_line(row: u8, column: u8, text: &'static str) -> Line {
    Line {
        row,
        column,
        text,
        arrow: false,
        interrobang: false,
        half_width: true,
        ruby: None,
        wait_100ms: None,
        mixed_suffix: None,
        color: false,
    }
}

const fn ruby_line(row: u8, column: u8, text: &'static str, ruby: &'static str) -> Line {
    Line {
        row,
        column,
        text,
        arrow: false,
        interrobang: false,
        half_width: false,
        ruby: Some(ruby),
        wait_100ms: None,
        mixed_suffix: None,
        color: false,
    }
}

const fn timed_line(row: u8, column: u8, text: &'static str, wait_100ms: u8) -> Line {
    Line {
        row,
        column,
        text,
        arrow: false,
        interrobang: false,
        half_width: false,
        ruby: None,
        wait_100ms: Some(wait_100ms),
        mixed_suffix: None,
        color: false,
    }
}

/// A line whose region genuinely mixes cell widths: `text` at normal width,
/// then MSZ, `mixed_suffix` at half width, then NSZ — all in one region,
/// since no APS separates them.
const fn mixed_width_line(
    row: u8,
    column: u8,
    text: &'static str,
    mixed_suffix: &'static str,
) -> Line {
    Line {
        row,
        column,
        text,
        arrow: false,
        interrobang: false,
        half_width: false,
        ruby: None,
        wait_100ms: None,
        mixed_suffix: Some(mixed_suffix),
        color: false,
    }
}

/// A line that also emits the colour sequence real captures send on every
/// statement: a non-default background box and an explicit foreground
/// colour, neither of which any other line in this script exercises. See
/// `Line::color` and `statement_body`.
const fn color_line(row: u8, column: u8, text: &'static str) -> Line {
    Line {
        row,
        column,
        text,
        arrow: false,
        interrobang: false,
        half_width: false,
        ruby: None,
        wait_100ms: None,
        mixed_suffix: None,
        color: true,
    }
}

/// The 35 statements of the caption stream.
///
/// Nine end with the continuation arrow, one carries ⁉, one is written at half
/// width, one carries ruby, one carries an explicit 2.3 s duration, one mixes
/// full-width and half-width cells within a single region, one carries the
/// colour sequence (`COL` palette- and background-select, plus `YLF`) the
/// real capture sends on every statement, and several are positioned away
/// from the top-left so region derivation has something to derive.
pub const CAPTION_SCRIPT: &[Line] = &[
    arrow_line(0, 0, "合成字幕の一行目"),
    half_width_line(0, 0, "テスト。"),
    timed_line(0, 0, "三行目の字幕", 23),
    ruby_line(1, 0, "本日の放送", "ほんじつ"),
    symbol_line(0, 0, "驚きの結果"),
    arrow_line(0, 0, "続きの行がある"),
    line(0, 0, "普通の行"),
    arrow_line(0, 0, "次に続く行"),
    line(1, 2, "位置を変えた行"),
    line(0, 0, "短い行"),
    arrow_line(0, 0, "四つ目の矢印"),
    line(0, 0, "静かな夜"),
    line(2, 4, "下の段の行"),
    arrow_line(0, 0, "五つ目の矢印"),
    line(0, 0, "遠くの音"),
    half_width_line(1, 0, "半角の行。"),
    arrow_line(0, 0, "六つ目の矢印"),
    line(0, 0, "白い雲"),
    line(0, 0, "青い空"),
    arrow_line(0, 0, "七つ目の矢印"),
    line(3, 0, "四段目の行"),
    line(0, 0, "赤い花"),
    arrow_line(0, 0, "八つ目の矢印"),
    line(0, 0, "緑の森"),
    line(0, 0, "黒い石"),
    arrow_line(0, 0, "九つ目の矢印"),
    line(0, 0, "黄色い光"),
    line(0, 0, "銀の月"),
    line(0, 0, "金の星"),
    line(0, 0, "水の音"),
    line(0, 0, "風の道"),
    line(0, 0, "山の影"),
    line(0, 0, "終わりの行"),
    mixed_width_line(0, 0, "全角に続く", "半角部分"),
    color_line(0, 0, "色つきの行"),
];

/// The statement body for one line.
///
/// Order matters twice. SSZ comes before the ruby's APS because APS counts in
/// section widths, which the size control changes; and MSZ comes before the
/// text's APS for the same reason. The colour sequence, when present, comes
/// right after the preamble and before everything else — the same position
/// the real capture puts it in, immediately after the macro invoke and
/// before the first APS.
pub fn statement_body(entry: &Line) -> Vec<u8> {
    let mut body = b24::preamble();
    if entry.color {
        body.extend_from_slice(&b24::col_select_palette(4));
        body.extend_from_slice(&b24::col_set_background(1));
        body.push(b24::YLF);
    }
    if let Some(ruby) = entry.ruby {
        assert!(entry.row > 0, "ruby rides on the row above its text");
        body.push(b24::SSZ);
        body.extend_from_slice(&b24::aps(entry.row - 1, entry.column));
        body.extend_from_slice(&b24::kanji(ruby));
        body.push(b24::NSZ);
    }
    if entry.half_width {
        body.push(b24::MSZ);
    }
    body.extend_from_slice(&b24::aps(entry.row, entry.column));
    body.extend_from_slice(&b24::kanji(entry.text));
    if let Some(suffix) = entry.mixed_suffix {
        body.push(b24::MSZ);
        body.extend_from_slice(&b24::kanji(suffix));
        body.push(b24::NSZ);
    }
    if entry.arrow {
        body.extend_from_slice(&b24::ARROW);
    }
    if entry.interrobang {
        body.extend_from_slice(&b24::INTERROBANG);
    }
    if let Some(units) = entry.wait_100ms {
        body.extend_from_slice(&b24::wait(units));
    }
    body
}

/// The caption PID: management data half a second ahead of each statement, in
/// alternating interleave groups, and 34 statements two seconds apart.
pub fn caption_ts() -> Vec<u8> {
    let mut writer = TsWriter::new();
    for (index, entry) in CAPTION_SCRIPT.iter().enumerate() {
        let statement_pts = FIRST_STATEMENT_PTS_90K + STATEMENT_INTERVAL_90K * index as u64;
        let management = b24::management_payload(0x80, index % 2 == 1, WRITING_FORMAT);
        writer.payload(
            CAPTION_PID,
            &ts::caption_pes(&management, statement_pts - MANAGEMENT_LEAD_90K),
        );
        let units = b24::data_unit(b24::UNIT_STATEMENT_BODY, &statement_body(entry));
        writer.payload(
            CAPTION_PID,
            &ts::caption_pes(&b24::statement_payload(0x80, 1, &units), statement_pts),
        );
    }
    writer.finish()
}

/// The superimpose PID: private_stream_2, so no PES header extension and no PTS.
pub fn superimpose_ts() -> Vec<u8> {
    let mut writer = TsWriter::new();
    writer.payload(
        SUPERIMPOSE_PID,
        &ts::superimpose_pes(&b24::management_payload(0x81, false, WRITING_FORMAT)),
    );
    for text in ["速報", "気象情報"] {
        let mut body = b24::preamble();
        body.extend_from_slice(&b24::aps(0, 0));
        body.extend_from_slice(&b24::kanji(text));
        let units = b24::data_unit(b24::UNIT_STATEMENT_BODY, &body);
        writer.payload(
            SUPERIMPOSE_PID,
            &ts::superimpose_pes(&b24::statement_payload(0x81, 1, &units)),
        );
    }
    writer.finish()
}

/// PAT and a PMT that spans two TS packets, carrying both caption PIDs.
pub fn psi_ts() -> Vec<u8> {
    let mut writer = TsWriter::new();
    writer.section(0x0000, &ts::pat_section(SERVICE_ID, PMT_PID));
    // 162 bytes of private descriptor put the section at 210 bytes, which is
    // what an NHK PMT measures and is past what one packet carries.
    writer.section(
        PMT_PID,
        &ts::pmt_section(
            SERVICE_ID,
            0x0100,
            162,
            &[
                ElementaryStream {
                    stream_type: 0x02,
                    pid: 0x0100,
                    component_tag: 0x00,
                },
                ElementaryStream {
                    stream_type: 0x0F,
                    pid: 0x0110,
                    component_tag: 0x10,
                },
                ElementaryStream {
                    stream_type: 0x06,
                    pid: CAPTION_PID,
                    component_tag: CAPTION_COMPONENT_TAG,
                },
                ElementaryStream {
                    stream_type: 0x06,
                    pid: SUPERIMPOSE_PID,
                    component_tag: SUPERIMPOSE_COMPONENT_TAG,
                },
            ],
        ),
    );
    writer.finish()
}

/// One caption whose first character is a glyph no font has.
pub fn drcs_ts() -> Vec<u8> {
    let mut writer = TsWriter::new();
    writer.payload(
        CAPTION_PID,
        &ts::caption_pes(
            &b24::management_payload(0x80, false, WRITING_FORMAT),
            FIRST_STATEMENT_PTS_90K - MANAGEMENT_LEAD_90K,
        ),
    );

    let mut units = b24::data_unit(
        b24::UNIT_DRCS_1BYTE,
        &b24::drcs_unit_body(DRCS_CHARACTER_CODE, 36, 36, &b24::drcs_stencil()),
    );
    let mut body = b24::preamble();
    body.extend_from_slice(&b24::DESIGNATE_DRCS1_G0);
    body.push(b24::LS0);
    body.extend_from_slice(&b24::aps(0, 0));
    body.push(0x21); // the code the DRCS unit above defined
    body.extend_from_slice(&b24::DESIGNATE_KANJI_G0);
    body.push(b24::LS0);
    body.extend_from_slice(&b24::kanji("のテスト。"));
    units.extend_from_slice(&b24::data_unit(b24::UNIT_STATEMENT_BODY, &body));

    writer.payload(
        CAPTION_PID,
        &ts::caption_pes(
            &b24::statement_payload(0x80, 1, &units),
            FIRST_STATEMENT_PTS_90K,
        ),
    );
    writer.finish()
}
