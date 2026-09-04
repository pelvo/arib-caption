//! The fixture builder, checked against the crate it builds fixtures for.
//!
//! A synthesized transport stream is only worth anything if the production
//! parsers accept it, so every writer here is asserted through
//! `PacketSplitter`, `PesAssembler` and `ServiceScanner` rather than against a
//! second copy of the same arithmetic.

mod support;

use arib_caption::ts::{
    CaptionStream, PacketSplitter, PesAssembler, ServiceScanner, TS_PACKET_SIZE,
};
use support::ts::{
    caption_pes, pat_section, pmt_section, superimpose_pes, ElementaryStream, TsWriter,
};

fn packets(bytes: &[u8]) -> Vec<[u8; TS_PACKET_SIZE]> {
    let mut splitter = PacketSplitter::new();
    splitter.feed(bytes);
    let mut out = Vec::new();
    while let Some(packet) = splitter.next_packet() {
        out.push(packet);
    }
    out
}

/// Checks that `packet` is exactly what `TsWriter::null_packet` promises:
/// sync byte, the reserved PID `0x1FFF`, no adaptation field (AFC = `01`),
/// and a payload of straight `0xFF` stuffing. `PesAssembler` and
/// `ServiceScanner` both drop a wrong-PID or malformed packet silently, so
/// nothing else in this file would notice if `null_packet` emitted garbage;
/// this is the direct check.
fn assert_is_null_packet(packet: &[u8; TS_PACKET_SIZE]) {
    assert_eq!(packet[0], 0x47, "null packet keeps the sync byte");
    let pid = ((packet[1] as u16 & 0x1F) << 8) | packet[2] as u16;
    assert_eq!(pid, 0x1FFF, "null packets carry the reserved PID 0x1FFF");
    assert_eq!(
        packet[3] & 0x30,
        0x10,
        "AFC = 01: no adaptation field, payload only"
    );
    assert_eq!(
        &packet[4..],
        [0xFFu8; TS_PACKET_SIZE - 4].as_slice(),
        "a null packet's payload is 0xFF stuffing"
    );
}

#[test]
fn the_writer_emits_whole_packets_the_splitter_accepts() {
    let mut writer = TsWriter::new();
    writer.packet(0x0130, true, &[0x11, 0x22, 0x33]);
    writer.packet(0x0130, false, &[0x44; 184]);
    let bytes = writer.finish();

    assert_eq!(bytes.len(), 2 * TS_PACKET_SIZE);
    let packets = packets(&bytes);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0][0], 0x47);
    assert_eq!(packets[0][1] & 0x40, 0x40, "first packet carries PUSI");
    assert_eq!(packets[1][1] & 0x40, 0x00);
    assert_eq!(packets[0][3] & 0x0F, 0);
    assert_eq!(packets[1][3] & 0x0F, 1, "continuity counter advances");

    // State the stuffing property outright rather than inferring it from a
    // reassembled payload matching: a 3-byte payload must carry an
    // adaptation field (AFC = 11) whose length pads out to exactly 184
    // bytes of packet body, not filler appended after the payload.
    assert_eq!(
        packets[0][3] & 0x30,
        0x30,
        "a 3-byte payload needs adaptation-field stuffing (AFC = 11)"
    );
    assert_eq!(
        packets[0][4],
        (TS_PACKET_SIZE - 5 - 3) as u8,
        "adaptation_field_length pads exactly to the packet boundary"
    );
    // The second packet's payload fills all 184 available bytes, so it
    // carries no adaptation field at all (AFC = 01) -- the other half of the
    // same property.
    assert_eq!(
        packets[1][3] & 0x30,
        0x10,
        "a full 184-byte payload carries no adaptation field (AFC = 01)"
    );
}

#[test]
fn a_caption_pes_reassembles_with_its_pts() {
    let body = [0x80u8, 0xFF, 0xF0, 0x04, 0x00, 0x00, 0x00, 0x00];
    let mut writer = TsWriter::new();
    writer.payload(0x0130, &caption_pes(&body, 900_000));
    // The PES fits in one packet; PacketSplitter needs a second sync byte
    // 188 bytes later before it will trust the first one, so pad with a
    // null packet the way a real muxer would rather than leave the stream
    // one packet long -- see TsWriter::null_packet.
    writer.null_packet();
    let bytes = writer.finish();

    let ts_packets = packets(&bytes);
    assert_eq!(
        ts_packets.len(),
        2,
        "one caption-PES packet plus the null pad"
    );
    assert_is_null_packet(&ts_packets[1]);

    let mut assembler = PesAssembler::new(0x0130);
    let mut out = Vec::new();
    for packet in &ts_packets {
        if let Some(pes) = assembler.push(packet) {
            out.push(pes);
        }
    }
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].pts_90k, Some(900_000));
    assert_eq!(out[0].pts_ms(), Some(10_000));
    assert_eq!(out[0].payload, body);
    assert_eq!(assembler.discontinuities, 0);
}

#[test]
fn a_superimpose_pes_reassembles_without_one() {
    let body = [0x81u8, 0xFF, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00];
    let mut writer = TsWriter::new();
    writer.payload(0x0138, &superimpose_pes(&body));
    // Same one-packet-stream shape as the caption PES test above; pad with a
    // null packet so PacketSplitter has a second sync byte to confirm on.
    writer.null_packet();
    let bytes = writer.finish();

    let ts_packets = packets(&bytes);
    assert_eq!(
        ts_packets.len(),
        2,
        "one superimpose-PES packet plus the null pad"
    );
    assert_is_null_packet(&ts_packets[1]);

    let mut assembler = PesAssembler::new(0x0138);
    let mut out = Vec::new();
    for packet in &ts_packets {
        if let Some(pes) = assembler.push(packet) {
            out.push(pes);
        }
    }
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].pts_90k, None, "private_stream_2 has no PTS");
    assert_eq!(out[0].payload, body);
}

#[test]
fn a_pes_longer_than_one_packet_reassembles_whole() {
    let mut body = vec![0x80u8, 0xFF, 0xF0];
    body.extend(std::iter::repeat(0x5A).take(400));
    let mut writer = TsWriter::new();
    writer.payload(0x0130, &caption_pes(&body, 90_000));
    let bytes = writer.finish();

    assert_eq!(
        bytes.len(),
        3 * TS_PACKET_SIZE,
        "412 payload bytes need 3 packets"
    );
    let mut assembler = PesAssembler::new(0x0130);
    let mut out = Vec::new();
    for packet in packets(&bytes) {
        if let Some(pes) = assembler.push(&packet) {
            out.push(pes);
        }
    }
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].payload, body);
    assert_eq!(assembler.discontinuities, 0);
}

#[test]
fn the_pmt_spans_more_than_one_packet_and_the_scanner_still_reads_it() {
    let mut writer = TsWriter::new();
    writer.section(0x0000, &pat_section(1024, 0x01F0));
    writer.section(
        0x01F0,
        &pmt_section(
            1024,
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
                    pid: 0x0130,
                    component_tag: 0x30,
                },
                ElementaryStream {
                    stream_type: 0x06,
                    pid: 0x0138,
                    component_tag: 0x38,
                },
            ],
        ),
    );
    let bytes = writer.finish();

    let packets = packets(&bytes);
    assert_eq!(packets.len(), 3, "one PAT packet and a PMT split over two");

    let mut scanner = ServiceScanner::new();
    for packet in &packets {
        scanner.push(packet);
    }
    assert_eq!(
        scanner.streams(),
        &[
            CaptionStream {
                pid: 0x0130,
                service_id: 1024,
                component_tag: Some(0x30)
            },
            CaptionStream {
                pid: 0x0138,
                service_id: 1024,
                component_tag: Some(0x38)
            },
        ]
    );
}

#[test]
fn the_section_crc_is_the_mpeg_2_one() {
    // MPEG-2 systems CRC-32 of "123456789" is 0x0376E6E7.
    assert_eq!(support::ts::mpeg_crc32(b"123456789"), 0x0376_E6E7);
}

/// `mpeg_crc32` being the right algorithm (tested above) says nothing about
/// whether `pat_section`/`pmt_section` actually append it correctly:
/// `ServiceScanner` never validates a section's CRC (it only skips the
/// trailing 4 bytes, see `section_body` in `crates/arib-caption/src/ts.rs`),
/// so a wrong byte order or a CRC computed over the wrong span would pass
/// every other test in this file while producing a non-conformant stream.
/// This proves the property directly instead.
#[test]
fn the_pat_and_pmt_crc_bytes_are_the_big_endian_crc_of_everything_before_them() {
    for section in [
        pat_section(1024, 0x01F0),
        pmt_section(
            1024,
            0x0100,
            162,
            &[
                ElementaryStream {
                    stream_type: 0x02,
                    pid: 0x0100,
                    component_tag: 0x00,
                },
                ElementaryStream {
                    stream_type: 0x06,
                    pid: 0x0130,
                    component_tag: 0x30,
                },
            ],
        ),
    ] {
        assert!(section.len() >= 4, "a section always carries a 4-byte CRC");
        let (body, crc_bytes) = section.split_at(section.len() - 4);
        let expected = support::ts::mpeg_crc32(body);
        let actual = u32::from_be_bytes(crc_bytes.try_into().unwrap());
        assert_eq!(
            actual, expected,
            "the trailing 4 bytes must be the big-endian CRC-32 of everything before them"
        );
    }
}

// ── ARIB STD-B24 body encoders ──────────────────────────────────────

use arib_caption::model::CaptionKind;
use arib_caption::pes::{DataGroup, Tmd};
use arib_caption::{Decoder, Options};
use support::b24;
use support::fixtures;

#[test]
fn kanji_bytes_are_the_euc_jp_bytes_with_the_high_bit_cleared() {
    // 。 is row 1 column 3 of JIS X 0208, which EUC-JP sends as A1 A3.
    assert_eq!(b24::kanji("。"), vec![0x21, 0x23]);
    // テスト is katakana in the kanji plane, row 5.
    assert_eq!(
        b24::kanji("テスト"),
        vec![0x25, 0x46, 0x25, 0x39, 0x25, 0x48]
    );
}

#[test]
fn the_decoder_reads_back_what_the_encoder_wrote() {
    let mut body = vec![b24::CS];
    body.extend_from_slice(&b24::DESIGNATE_KANJI_G0);
    body.push(b24::LS0);
    body.extend_from_slice(&b24::aps(1, 2));
    body.extend_from_slice(&b24::kanji("字幕"));
    body.extend_from_slice(&b24::ARROW);
    body.extend_from_slice(&b24::INTERROBANG);
    let units = b24::data_unit(b24::UNIT_STATEMENT_BODY, &body);
    let payload = b24::statement_payload(0x80, 1, &units);

    let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
    let caption = decoder
        .decode(&payload, Some(1_234))
        .expect("decodes")
        .expect("a caption");
    assert_eq!(caption.text, "字幕➡⁉");
    assert!(caption.clear_screen);
    assert_eq!(caption.pts_ms, Some(1_234));
    // APS row 1, column 2, against profile A's 40x60 section.
    assert_eq!((caption.regions[0].x, caption.regions[0].y), (80, 60));
}

#[test]
fn management_data_declares_one_japanese_language() {
    let payload = b24::management_payload(0x80, false, fixtures::WRITING_FORMAT);
    let parsed = arib_caption::pes::parse(&payload).expect("parses");
    let DataGroup::Management(management) = parsed.group else {
        panic!("expected management data");
    };
    assert_eq!(management.tmd, Tmd::Free);
    assert_eq!(management.languages.len(), 1);
    assert_eq!(&management.languages[0].iso639, b"jpn");
    assert_eq!(management.languages[0].language_id, 1);
    assert_eq!(management.languages[0].format, 8);
    assert_eq!(management.languages[0].tcs, 0);
}

#[test]
fn the_drcs_stencil_packs_to_its_pinned_md5() {
    let levels = b24::drcs_stencil();
    assert_eq!(levels.len(), 36 * 36);
    let packed = b24::pack_two_bit(&levels);
    // 36 x 36 pixels at 2 bits, no row padding: exactly 324 bytes.
    assert_eq!(packed.len(), 324);
    assert_eq!(
        format!("{:x}", md5::compute(&packed)),
        fixtures::DRCS_GLYPH_MD5
    );
}

/// Decode the actual synthesized DRCS stream through the real TS/PES/decoder
/// path — not just far enough to see the right data-unit kind, but all the
/// way to the glyph `parse_drcs` records and the character the statement
/// then draws with it. A test that stopped at `pes::parse` and checked only
/// `units.len()` and `kind == Drcs1` would pass on garbage
/// `number_of_code`/`depth`/`width`/`height`, since `DataUnits` never parses
/// the DRCS body — only `Decoder::parse_drcs` does.
#[test]
fn the_drcs_unit_defines_a_glyph_the_statement_can_draw() {
    let bytes = fixtures::drcs_ts();
    let mut assembler = PesAssembler::new(fixtures::CAPTION_PID);
    let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
    let mut caption = None;
    for packet in packets(&bytes) {
        if let Some(pes) = assembler.push(&packet) {
            if let Some(c) = decoder.decode(&pes.payload, pes.pts_ms()).expect("decodes") {
                caption = Some(c);
            }
        }
    }
    let caption = caption.expect("the statement PES produced a caption");

    // character_code 0x4121 is DRCS-1 (high nibble 1) code 0x21; parse_drcs
    // keys it as (map_index << 16) | code, so this key only comes out right
    // if character_code was decomposed correctly.
    assert_eq!(
        caption.drcs.len(),
        1,
        "number_of_code was 1 — not zero from a truncated parse, not more from \
         reading past the unit"
    );
    let drcs = caption
        .drcs
        .get(&0x1_0021)
        .expect("DRCS-1 code 0x21 was recorded under its (map_index, code) key");
    assert_eq!(drcs.width, 36, "the width byte parse_drcs read");
    assert_eq!(drcs.height, 36, "the height byte parse_drcs read");
    assert_eq!(
        drcs.depth, 4,
        "the transmitted depth field (0x02) plus the standard's +2 offset"
    );
    assert_eq!(drcs.depth_bits, 2, "4 levels need 2 bits per pixel");
    assert_eq!(
        drcs.md5,
        fixtures::DRCS_GLYPH_MD5,
        "the MD5 the decoder computed over the bitmap bytes it actually sliced \
         out of the wire — not over the bytes pack_two_bit produced locally"
    );
    assert!(
        caption.text.starts_with('〓'),
        "no alternative is registered for this glyph, so it draws as GETA — \
         proof the glyph was usable, not just present in the map"
    );
}

#[test]
fn the_synthesized_streams_are_whole_packets() {
    for (name, bytes) in [
        ("caption", fixtures::caption_ts()),
        ("superimpose", fixtures::superimpose_ts()),
        ("psi", fixtures::psi_ts()),
        ("drcs", fixtures::drcs_ts()),
    ] {
        assert_eq!(
            bytes.len() % TS_PACKET_SIZE,
            0,
            "{name} is not whole packets"
        );
        assert!(
            bytes.len() >= 2 * TS_PACKET_SIZE,
            "{name} needs two packets for PacketSplitter to sync"
        );
    }
}

/// `b24::crc16` is the algorithm verified here against ground truth
/// (CRC-16/CCITT, polynomial 0x1021, init 0, residue 0x0000 on all 199
/// data groups reassembled from a real off-air recording — not distributed
/// with this crate), pinned here against the standard's own published check
/// value the same way `the_section_crc_is_the_mpeg_2_one` pins
/// `ts::mpeg_crc32`.
#[test]
fn the_data_group_crc_is_the_ccitt_one() {
    assert_eq!(b24::crc16(b"123456789"), 0x31C3);
}

/// ARIB's data group ends in a CRC_16 that `src/pes.rs` never reads (it
/// bounds the body by `data_group_size` and stops), so nothing short of an
/// external validator — or reproducing that validator's check here — would
/// ever catch its absence. This is that check: recompute CRC-16 over the
/// *whole* group, trailing CRC bytes included, and require the standard
/// division-remainder property (append the remainder, recompute over
/// everything, and it must drain to zero) to hold. This is deliberately not
/// `assert_eq!(extracted_crc, crc16(group_without_crc))` — that would just
/// assert a value equals itself, true for any polynomial. Checking the
/// residue instead exercises the actual byte range and placement `data_group`
/// emits: get the CRC's position, width, or endianness wrong and this fails
/// even though the "equals itself" version would not.
#[test]
fn every_emitted_data_group_ends_in_a_crc16_whose_residue_is_zero() {
    let mut payloads = vec![
        b24::management_payload(0x80, false, fixtures::WRITING_FORMAT),
        b24::management_payload(0x80, true, fixtures::WRITING_FORMAT),
        b24::management_payload(0x81, false, fixtures::WRITING_FORMAT),
    ];
    for entry in fixtures::CAPTION_SCRIPT {
        let units = b24::data_unit(b24::UNIT_STATEMENT_BODY, &fixtures::statement_body(entry));
        payloads.push(b24::statement_payload(0x80, 1, &units));
    }
    let drcs_unit = b24::data_unit(
        b24::UNIT_DRCS_1BYTE,
        &b24::drcs_unit_body(fixtures::DRCS_CHARACTER_CODE, 36, 36, &b24::drcs_stencil()),
    );
    payloads.push(b24::statement_payload(0x80, 1, &drcs_unit));

    for payload in payloads {
        // payload[0..3] is data_identifier / private_stream_id /
        // PES_data_packet_header_length — not part of the data group.
        // payload[3..] is data_group_id through the trailing CRC_16 itself:
        // exactly the span CRC_16 covers, plus the two bytes it produced.
        assert_eq!(
            b24::crc16(&payload[3..]),
            0,
            "CRC-16 residue over the whole data group must be zero"
        );
    }
}

/// Real ARIB statements open with a CSI preamble (SWF/SDF/SDP/SHS/SVS/SSM)
/// and a macro invocation, not a bare `ESC 24 42` designation — see
/// `b24::preamble`. That leaves `decoder.rs::handle_csi` (~100 lines) and the
/// `DEFAULT_MACROS` path entirely unexercised unless something decodes a
/// statement built with it and checks for an effect only CSI parsing
/// produces. SDP's (58, 29) is that effect: it moves the region's origin off
/// the (0, 0) every other path in this file would produce.
#[test]
fn the_csi_preamble_reaches_display_position_and_format_controls() {
    let entry = &fixtures::CAPTION_SCRIPT[6]; // line(0, 0, "普通の行"): no ruby, no MSZ
    assert_eq!(entry.text, "普通の行");
    assert_eq!((entry.row, entry.column), (0, 0));

    let units = b24::data_unit(b24::UNIT_STATEMENT_BODY, &fixtures::statement_body(entry));
    let payload = b24::statement_payload(0x80, 1, &units);
    let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());
    let caption = decoder
        .decode(&payload, None)
        .expect("decodes")
        .expect("a caption");

    assert_eq!(
        caption.text, "普通の行",
        "the whole preamble parsed without truncating the statement"
    );
    assert_eq!(caption.regions.len(), 1);
    let region = &caption.regions[0];
    // CSI SDP put the display area's origin at (58, 29); APS(0, 0) then
    // placed the character at that origin. Every other statement in this
    // file (built without the preamble) sits at (0, 0), so this value can
    // only come from handle_csi's SDP branch actually running.
    assert_eq!((region.x, region.y), (58, 29), "CSI SDP set area_x/area_y");
    // 4 kanji-plane characters, each a 40x60 section (36 + 4 h-spacing by
    // 36 + 24 v-spacing — CSI SHS/SVS/SSM re-asserting profile A's own
    // defaults, the same values a real broadcast resends after every CS).
    assert_eq!(
        (region.width, region.height),
        (4 * 40, 60),
        "CSI SHS/SVS/SSM sized the cell"
    );
    assert_eq!(region.chars.len(), 4);
}
