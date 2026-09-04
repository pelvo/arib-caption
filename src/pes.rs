//! The independent-PES layer: caption bytes as they arrive on the wire.
//!
//! An ISDB caption PES payload is not the statement text — it is a *data
//! group* (ARIB STD-B24 part 3), which is either the management data that says
//! what languages and screen format are in use, or the statement data holding
//! the text and the DRCS glyph definitions. This module gets from the payload
//! down to the individual data units and stops there; turning a statement body
//! into characters is [`crate::decoder`]'s job.
//!
//! Everything here borrows the input. Nothing is copied, and nothing is
//! decoded, so a caller can cheaply look at the structure — which is what
//! `arib-caption dump` does.

use crate::model::CaptionKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer bytes than the structure being read requires.
    Truncated,
    /// `data_identifier` was neither 0x80 (caption) nor 0x81 (superimpose).
    BadDataIdentifier(u8),
    /// `private_stream_id` must be 0xFF.
    BadPrivateStreamId(u8),
    /// A data unit must start with the 0x1F separator.
    BadUnitSeparator(u8),
    /// `num_languages` outside 1..=2.
    BadLanguageCount(u8),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Truncated => write!(f, "truncated"),
            ParseError::BadDataIdentifier(v) => write!(f, "bad data_identifier 0x{v:02X}"),
            ParseError::BadPrivateStreamId(v) => write!(f, "bad private_stream_id 0x{v:02X}"),
            ParseError::BadUnitSeparator(v) => write!(f, "bad unit_separator 0x{v:02X}"),
            ParseError::BadLanguageCount(v) => write!(f, "bad num_languages {v}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Which of the two interleaved data-group sequences this belongs to.
///
/// A and B alternate so a receiver can tell a fresh management block from the
/// retransmission of the one it already applied: same group means the same
/// block sent again (ARIB TR-B14 4.2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    A,
    B,
}

/// Presentation time mode of a data group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tmd {
    /// 00 — free: present when it arrives (the ordinary case; timing comes
    /// from the PES PTS).
    Free,
    /// 01 — real time: an STM accompanies the statement.
    RealTime,
    /// 10 — offset time: an OTM gives an offset to apply.
    OffsetTime,
    /// 11 — reserved.
    Reserved,
}

impl Tmd {
    fn from_bits(bits: u8) -> Tmd {
        match bits {
            0b00 => Tmd::Free,
            0b01 => Tmd::RealTime,
            0b10 => Tmd::OffsetTime,
            _ => Tmd::Reserved,
        }
    }
}

/// A 5-byte BCD timecode (OTM / STM): hh:mm:ss.mmm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timecode {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millis: u16,
}

impl Timecode {
    fn parse(b: &[u8]) -> Timecode {
        // Nine BCD digits packed into the first 4.5 bytes: HH MM SS mmm, with
        // the low nibble of the last byte reserved.
        let d = |byte: u8, high: bool| -> u16 {
            if high {
                (byte >> 4) as u16
            } else {
                (byte & 0x0f) as u16
            }
        };
        Timecode {
            hour: (d(b[0], true) * 10 + d(b[0], false)) as u8,
            minute: (d(b[1], true) * 10 + d(b[1], false)) as u8,
            second: (d(b[2], true) * 10 + d(b[2], false)) as u8,
            millis: d(b[3], true) * 100 + d(b[3], false) * 10 + d(b[4], true),
        }
    }

    pub fn as_millis(&self) -> i64 {
        (self.hour as i64) * 3_600_000
            + (self.minute as i64) * 60_000
            + (self.second as i64) * 1_000
            + self.millis as i64
    }
}

/// Per-language entry of the caption management data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanguageInfo {
    /// 1-based language id, matching the `data_group_id` of that language's
    /// statement data.
    pub language_id: u8,
    /// Display mode (`DMF`): whether the caption is presented automatically,
    /// on request, or selectively.
    pub dmf: u8,
    /// Display condition, present only for the DMF values that carry one.
    pub display_condition: Option<u8>,
    /// ISO 639-2, e.g. `*b"jpn"`.
    pub iso639: [u8; 3],
    /// Raw writing-format value from management data (Table 9-7). Values 0..=4
    /// map directly to CSI SWF; values 6..=13 map to SWF 5..=12. Value 5 and
    /// values above 13 are reserved.
    pub format: u8,
    /// Character coding: 0 = 8-bit B24, 1 = UCS (UTF-8).
    pub tcs: u8,
    /// Roll-up mode.
    pub rollup: u8,
}

/// Header common to both kinds of data group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataGroupHeader {
    pub group: Group,
    /// 0 for management data, 1..=8 for a language's statement data.
    pub id: u8,
    pub version: u8,
    pub link_number: u8,
    pub last_link_number: u8,
    pub size: usize,
}

/// Caption management data: what the following statements will look like.
#[derive(Clone, Debug)]
pub struct Management<'a> {
    pub tmd: Tmd,
    pub otm: Option<Timecode>,
    pub languages: Vec<LanguageInfo>,
    units: &'a [u8],
}

/// Caption statement data: one screen of caption, as control codes and text.
#[derive(Clone, Debug)]
pub struct Statement<'a> {
    /// Which language's statement this is (the data group id, 1..=8).
    pub language_id: u8,
    pub tmd: Tmd,
    pub stm: Option<Timecode>,
    units: &'a [u8],
}

impl<'a> Management<'a> {
    pub fn units(&self) -> DataUnits<'a> {
        DataUnits { rest: self.units }
    }
}

impl<'a> Statement<'a> {
    pub fn units(&self) -> DataUnits<'a> {
        DataUnits { rest: self.units }
    }
}

#[derive(Clone, Debug)]
pub enum DataGroup<'a> {
    Management(Management<'a>),
    Statement(Statement<'a>),
}

/// A parsed caption PES payload.
#[derive(Clone, Debug)]
pub struct Parsed<'a> {
    pub kind: CaptionKind,
    pub header: DataGroupHeader,
    pub group: DataGroup<'a>,
}

/// What a data unit holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataUnitKind {
    /// 0x20 — the statement body: control codes and character bytes.
    StatementBody,
    /// 0x28 — geometric shape.
    GeometricShape,
    /// 0x2C — additional sound (built-in sound data).
    AdditionalSound,
    /// 0x30 — DRCS with 1-byte codes.
    Drcs1,
    /// 0x31 — DRCS with 2-byte codes.
    Drcs2,
    /// 0x34 — colour map.
    ColorMap,
    /// 0x35 — bitmap.
    Bitmap,
    Unknown(u8),
}

impl DataUnitKind {
    fn from_parameter(p: u8) -> DataUnitKind {
        match p {
            0x20 => DataUnitKind::StatementBody,
            0x28 => DataUnitKind::GeometricShape,
            0x2c => DataUnitKind::AdditionalSound,
            0x30 => DataUnitKind::Drcs1,
            0x31 => DataUnitKind::Drcs2,
            0x34 => DataUnitKind::ColorMap,
            0x35 => DataUnitKind::Bitmap,
            other => DataUnitKind::Unknown(other),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DataUnit<'a> {
    pub kind: DataUnitKind,
    pub bytes: &'a [u8],
}

/// Iterator over the data units of a data group.
///
/// Yields a `ParseError` and then stops if the loop is malformed: a data group
/// arrives whole or not at all, so there is nothing to resynchronize to.
#[derive(Clone, Debug)]
pub struct DataUnits<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for DataUnits<'a> {
    type Item = Result<DataUnit<'a>, ParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        if self.rest.len() < 5 {
            self.rest = &[];
            return Some(Err(ParseError::Truncated));
        }
        let separator = self.rest[0];
        if separator != 0x1f {
            self.rest = &[];
            return Some(Err(ParseError::BadUnitSeparator(separator)));
        }
        let kind = DataUnitKind::from_parameter(self.rest[1]);
        let size = be24(&self.rest[2..5]);
        if 5 + size > self.rest.len() {
            self.rest = &[];
            return Some(Err(ParseError::Truncated));
        }
        let bytes = &self.rest[5..5 + size];
        self.rest = &self.rest[5 + size..];
        Some(Ok(DataUnit { kind, bytes }))
    }
}

fn be24(b: &[u8]) -> usize {
    ((b[0] as usize) << 16) | ((b[1] as usize) << 8) | (b[2] as usize)
}

/// Parse one caption PES payload (starting at `data_identifier`).
pub fn parse(payload: &[u8]) -> Result<Parsed<'_>, ParseError> {
    if payload.len() < 3 {
        return Err(ParseError::Truncated);
    }

    let kind = match payload[0] {
        0x80 => CaptionKind::Caption,
        0x81 => CaptionKind::Superimpose,
        other => return Err(ParseError::BadDataIdentifier(other)),
    };
    if payload[1] != 0xff {
        return Err(ParseError::BadPrivateStreamId(payload[1]));
    }

    // PES_data_packet_header_length is the low nibble; the header itself is
    // of no interest to us.
    let begin = 3 + (payload[2] & 0x0f) as usize;
    if begin + 5 > payload.len() {
        return Err(ParseError::Truncated);
    }

    let data_group_id = (payload[begin] & 0b1111_1100) >> 2;
    // Bit 5 selects the interleaved sequence; the low 5 bits are the id
    // proper. libaribcaption reads the group as `(id & 0xF0) >> 8`, which is
    // always 0 — harmless there, but it means a mid-stream management change
    // is taken for a retransmission and dropped. We read the bit.
    let group = if data_group_id & 0x20 != 0 {
        Group::B
    } else {
        Group::A
    };
    let id = data_group_id & 0x1f;
    let size = ((payload[begin + 3] as usize) << 8) | payload[begin + 4] as usize;
    let body_start = begin + 5;
    if body_start + size > payload.len() {
        return Err(ParseError::Truncated);
    }
    let header = DataGroupHeader {
        group,
        id,
        version: payload[begin] & 0x03,
        link_number: payload[begin + 1],
        last_link_number: payload[begin + 2],
        size,
    };
    let body = &payload[body_start..body_start + size];

    let group = if id == 0 {
        DataGroup::Management(parse_management(body)?)
    } else {
        DataGroup::Statement(parse_statement(id, body)?)
    };

    Ok(Parsed {
        kind,
        header,
        group,
    })
}

fn parse_management(body: &[u8]) -> Result<Management<'_>, ParseError> {
    if body.is_empty() {
        return Err(ParseError::Truncated);
    }
    let tmd = Tmd::from_bits((body[0] & 0b1100_0000) >> 6);
    let mut off = 1usize;

    let otm = if tmd == Tmd::OffsetTime {
        if off + 5 > body.len() {
            return Err(ParseError::Truncated);
        }
        let tc = Timecode::parse(&body[off..off + 5]);
        off += 5;
        Some(tc)
    } else {
        None
    };

    if off >= body.len() {
        return Err(ParseError::Truncated);
    }
    let num_languages = body[off];
    off += 1;
    if num_languages == 0 || num_languages > 2 {
        return Err(ParseError::BadLanguageCount(num_languages));
    }

    let mut languages = Vec::with_capacity(num_languages as usize);
    for _ in 0..num_languages {
        if off + 5 > body.len() {
            return Err(ParseError::Truncated);
        }
        let language_tag = (body[off] & 0b1110_0000) >> 5;
        let dmf = body[off] & 0b0000_1111;
        off += 1;

        // These three display modes are followed by a display-condition byte.
        let display_condition = if matches!(dmf, 0b1100..=0b1110) {
            if off >= body.len() {
                return Err(ParseError::Truncated);
            }
            let dc = body[off];
            off += 1;
            Some(dc)
        } else {
            None
        };

        if off + 4 > body.len() {
            return Err(ParseError::Truncated);
        }
        let iso639 = [body[off], body[off + 1], body[off + 2]];
        off += 3;
        let format = (body[off] & 0b1111_0000) >> 4;
        let tcs = (body[off] & 0b0000_1100) >> 2;
        let rollup = body[off] & 0b0000_0011;
        off += 1;

        languages.push(LanguageInfo {
            language_id: language_tag + 1,
            dmf,
            display_condition,
            iso639,
            format,
            tcs,
            rollup,
        });
    }

    let units = data_unit_loop(body, off)?;
    Ok(Management {
        tmd,
        otm,
        languages,
        units,
    })
}

fn parse_statement(language_id: u8, body: &[u8]) -> Result<Statement<'_>, ParseError> {
    if body.is_empty() {
        return Err(ParseError::Truncated);
    }
    let tmd = Tmd::from_bits((body[0] & 0b1100_0000) >> 6);
    let mut off = 1usize;

    // Both real-time and offset-time statements carry a 5-byte STM. We keep it
    // for the record; presentation time comes from the PES PTS, which is what
    // every player has and what the segmenter must align to.
    let stm = if matches!(tmd, Tmd::RealTime | Tmd::OffsetTime) {
        if off + 5 > body.len() {
            return Err(ParseError::Truncated);
        }
        let tc = Timecode::parse(&body[off..off + 5]);
        off += 5;
        Some(tc)
    } else {
        None
    };

    let units = data_unit_loop(body, off)?;
    Ok(Statement {
        language_id,
        tmd,
        stm,
        units,
    })
}

fn data_unit_loop(body: &[u8], off: usize) -> Result<&[u8], ParseError> {
    if off + 3 > body.len() {
        return Err(ParseError::Truncated);
    }
    let len = be24(&body[off..off + 3]);
    let start = off + 3;
    if start + len > body.len() {
        return Err(ParseError::Truncated);
    }
    Ok(&body[start..start + len])
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal management data group: TMD free, one language "jpn" with
    // writing format 7 (960x540 horizontal), and an empty data unit loop.
    #[test]
    fn parses_management_data() {
        let payload = [
            0x80, 0xff, 0xf0, // data_identifier, private_stream_id, no header
            0x00, 0x00, 0x00, 0x00, 0x0a, // data_group_id 0 (mgmt), size 10
            0x00, // TMD free
            0x01, // one language
            0x00, // language_tag 0, DMF 0
            b'j', b'p', b'n', 0x70, // format 7, TCS 0, rollup 0
            0x00, 0x00, 0x00, // data_unit_loop_length 0
        ];
        let parsed = parse(&payload).expect("parse");
        assert_eq!(parsed.kind, CaptionKind::Caption);
        assert_eq!(parsed.header.group, Group::A);
        assert_eq!(parsed.header.id, 0);
        let DataGroup::Management(m) = parsed.group else {
            panic!("expected management data");
        };
        assert_eq!(m.tmd, Tmd::Free);
        assert_eq!(m.otm, None);
        assert_eq!(m.languages.len(), 1);
        assert_eq!(m.languages[0].iso639, *b"jpn");
        assert_eq!(m.languages[0].format, 7);
        assert_eq!(m.languages[0].language_id, 1);
        assert_eq!(m.units().count(), 0);
    }

    #[test]
    fn parses_statement_data_units() {
        let payload = [
            0x80, 0xff, 0xf0, //
            0x04, 0x00, 0x00, 0x00, 0x0b, // data_group_id 1 (statement), size 11
            0x00, // TMD free
            0x00, 0x00, 0x07, // data_unit_loop_length 7 (5 header + 2 body)
            0x1f, 0x20, 0x00, 0x00, 0x02, // statement body, 2 bytes
            0x41, 0x42,
        ];
        let parsed = parse(&payload).expect("parse");
        let DataGroup::Statement(s) = parsed.group else {
            panic!("expected statement data");
        };
        assert_eq!(s.language_id, 1);
        let units: Vec<_> = s.units().map(|u| u.unwrap()).collect();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].kind, DataUnitKind::StatementBody);
        assert_eq!(units[0].bytes, &[0x41, 0x42]);
    }

    #[test]
    fn rejects_foreign_payload() {
        assert_eq!(
            parse(&[0x82, 0xff, 0xf0]).unwrap_err(),
            ParseError::BadDataIdentifier(0x82)
        );
        assert_eq!(
            parse(&[0x80, 0x00, 0xf0]).unwrap_err(),
            ParseError::BadPrivateStreamId(0x00)
        );
        assert_eq!(parse(&[0x80, 0xff]).unwrap_err(), ParseError::Truncated);
    }

    #[test]
    fn group_b_is_not_group_a() {
        // data_group_id 0x20 → management data, sequence B.
        let payload = [
            0x80, 0xff, 0xf0, //
            0x80, 0x00, 0x00, 0x00, 0x0a, //
            0x00, 0x01, 0x00, b'j', b'p', b'n', 0x70, 0x00, 0x00, 0x00,
        ];
        let parsed = parse(&payload).expect("parse");
        assert_eq!(parsed.header.group, Group::B);
        assert_eq!(parsed.header.id, 0);
    }
}
