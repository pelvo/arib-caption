//! Bytes a broadcaster would send, built rather than captured.
//!
//! These were once slices of an off-air recording, which is where every
//! assumption in this crate first failed: an ISDB PMT does not fit in one TS
//! packet, and superimpose arrives as private_stream_2 with no PTS. Both were
//! found that way and both are still asserted here — the streams are now
//! constructed in `tests/support/` so the crate ships no third-party broadcast
//! content. What the constructed set does not cover is in the README.

mod support;

use arib_caption::model::{Caption, CaptionKind, Duration};
use arib_caption::pes::{self, DataGroup, DataUnitKind, Tmd};
use arib_caption::render::ass;
use arib_caption::render::timeline::Timeline;
use arib_caption::ts::{PacketSplitter, PesAssembler, ServiceScanner, TS_PACKET_SIZE};
use arib_caption::{Decoder, Options};
use support::fixtures;

fn packets(bytes: &[u8]) -> Vec<[u8; TS_PACKET_SIZE]> {
    let mut splitter = PacketSplitter::new();
    splitter.feed(bytes);
    let mut out = Vec::new();
    while let Some(p) = splitter.next_packet() {
        out.push(p);
    }
    out
}

fn pes_packets(bytes: &[u8], pid: u16) -> Vec<arib_caption::ts::PesPacket> {
    let mut asm = PesAssembler::new(pid);
    let mut out = Vec::new();
    for packet in packets(bytes) {
        if let Some(p) = asm.push(&packet) {
            out.push(p);
        }
    }
    if let Some(p) = asm.flush() {
        out.push(p);
    }
    assert_eq!(asm.discontinuities, 0, "fixture should be a clean stream");
    out
}

/// The caption PID and the superimpose PID are told apart by the PMT's
/// component tag, and the PMT does not fit in one TS packet.
#[test]
fn finds_caption_pids_in_the_pmt() {
    let psi = fixtures::psi_ts();
    let mut scanner = ServiceScanner::new();
    let mut pmt_packets = 0usize;
    for packet in packets(&psi) {
        let pid = (((packet[1] & 0x1F) as u16) << 8) | packet[2] as u16;
        if pid == fixtures::PMT_PID {
            pmt_packets += 1;
        }
        scanner.push(&packet);
    }
    assert!(pmt_packets > 1, "the PMT must not fit in one packet");

    assert_eq!(scanner.caption_pid(), Some(fixtures::CAPTION_PID));
    assert_eq!(scanner.superimpose_pid(), Some(fixtures::SUPERIMPOSE_PID));
    let streams = scanner.streams();
    assert_eq!(streams.len(), 2);
    assert_eq!(streams[0].service_id, fixtures::SERVICE_ID);
    assert_eq!(
        streams[0].component_tag,
        Some(fixtures::CAPTION_COMPONENT_TAG)
    );
    assert_eq!(
        streams[1].component_tag,
        Some(fixtures::SUPERIMPOSE_COMPONENT_TAG)
    );
}

/// Every PES on the caption PID parses, carries a PTS, and belongs to the
/// Japanese caption service.
#[test]
fn every_caption_pes_parses() {
    let pes_list = pes_packets(&fixtures::caption_ts(), fixtures::CAPTION_PID);
    assert!(pes_list.len() > 20, "got {} PES packets", pes_list.len());

    let mut management = 0usize;
    let mut statements = 0usize;
    let mut last_pts = i64::MIN;

    for packet in &pes_list {
        let pts = packet.pts_ms().expect("caption PES carries a PTS");
        assert!(pts >= last_pts, "PTS went backwards: {last_pts} → {pts}");
        last_pts = pts;

        let parsed = pes::parse(&packet.payload).expect("parses");
        assert_eq!(parsed.kind, CaptionKind::Caption);
        match parsed.group {
            DataGroup::Management(m) => {
                management += 1;
                assert_eq!(m.tmd, Tmd::Free);
                assert_eq!(m.languages.len(), 1);
                let lang = m.languages[0];
                assert_eq!(&lang.iso639, b"jpn");
                assert_eq!(lang.language_id, 1);
                // Writing format 8 → the 960x540 horizontal plane, which is
                // what full-seg captions use.
                assert_eq!(lang.format, 8);
                assert_eq!(lang.tcs, 0, "JIS coding, not UCS");
            }
            DataGroup::Statement(s) => {
                statements += 1;
                assert_eq!(s.language_id, 1);
                let units: Vec<_> = s.units().map(|u| u.expect("unit parses")).collect();
                assert!(!units.is_empty(), "a statement carries at least one unit");
                assert!(units.iter().all(|u| u.kind == DataUnitKind::StatementBody
                    || matches!(u.kind, DataUnitKind::Drcs1 | DataUnitKind::Drcs2)));
            }
        }
    }

    assert_eq!(management, fixtures::CAPTION_SCRIPT.len());
    assert_eq!(statements, fixtures::CAPTION_SCRIPT.len());
}

/// Superimpose arrives as private_stream_2, which has no PES header extension
/// and therefore no PTS. Parsing it as private_stream_1 loses the first bytes
/// of the data group and the stream reads as empty.
#[test]
fn superimpose_is_private_stream_2() {
    let pes_list = pes_packets(&fixtures::superimpose_ts(), fixtures::SUPERIMPOSE_PID);
    assert!(!pes_list.is_empty(), "no superimpose PES reassembled");

    for packet in &pes_list {
        assert_eq!(packet.pts_ms(), None, "private_stream_2 has no PTS");
        let parsed = pes::parse(&packet.payload).expect("parses");
        assert_eq!(parsed.kind, CaptionKind::Superimpose);
        if let DataGroup::Management(m) = parsed.group {
            assert_eq!(&m.languages[0].iso639, b"jpn");
        }
    }
}

/// Decode every caption in the fixture with the options a text renderer would
/// use.
fn decode_fixture(options: Options) -> Vec<Caption> {
    let bytes = fixtures::caption_ts();
    let mut decoder = Decoder::new(CaptionKind::Caption, options);
    let mut captions = Vec::new();
    for packet in pes_packets(&bytes, fixtures::CAPTION_PID) {
        match decoder.decode(&packet.payload, packet.pts_ms()) {
            Ok(Some(caption)) => captions.push(caption),
            Ok(None) => {}
            Err(e) => panic!("decode failed: {e}"),
        }
    }
    captions
}

#[test]
fn decodes_captions_to_japanese_text() {
    let captions = decode_fixture(Options::default());
    assert_eq!(
        captions.len(),
        fixtures::CAPTION_SCRIPT.len(),
        "caption count changed"
    );

    let first = &captions[0];
    assert_eq!(first.text, "合成字幕の一行目➡");
    assert_eq!(first.language_str(), "jpn");
    // 960x540 is the plane writing format 8 selects.
    assert_eq!((first.plane_width, first.plane_height), (960, 540));
    // Every caption in this stream starts by clearing the previous one.
    assert!(first.clear_screen);
    assert_eq!(
        first.pts_ms,
        Some((fixtures::FIRST_STATEMENT_PTS_90K / 90) as i64)
    );

    // The continuation arrow (➡, U+27A1) is an ARIB additional symbol: it only
    // decodes if the gaiji table is aligned.
    let arrows = captions.iter().filter(|c| c.text.ends_with('➡')).count();
    assert_eq!(arrows, 9, "lines ending with the continuation arrow");
    // ⁉ (U+2049) is another, from a different row of the same table.
    assert!(captions.iter().any(|c| c.text.contains('⁉')));

    // Timing: most captions run until the next one, one carries a duration.
    assert_eq!(captions[2].duration, Duration::Millis(2300));
    assert!(
        captions
            .iter()
            .filter(|c| matches!(c.duration, Duration::Indefinite))
            .count()
            > 20
    );

    // Ordering is the PES order, and PTS never goes backwards.
    let mut last = i64::MIN;
    for c in &captions {
        let pts = c.pts_ms.expect("caption has a PTS");
        assert!(pts >= last);
        last = pts;
    }
}

/// Ruby is furigana: a half-size region riding above the line it annotates.
/// It must stay out of `text`, or a reading gets spliced into the sentence.
#[test]
fn ruby_is_placed_but_not_spoken() {
    let captions = decode_fixture(Options::default());
    let with_ruby: Vec<_> = captions
        .iter()
        .filter(|c| c.regions.iter().any(|r| r.is_ruby))
        .collect();
    assert!(!with_ruby.is_empty(), "fixture has no ruby");

    for caption in with_ruby {
        for region in caption.regions.iter().filter(|r| r.is_ruby) {
            let ruby: String = region
                .chars
                .iter()
                .filter_map(|c| c.text())
                .collect::<Vec<_>>()
                .join("");
            assert!(!ruby.is_empty(), "a ruby region with no characters");
            assert!(
                !caption.text.contains(&ruby),
                "ruby {ruby:?} leaked into the text {:?}",
                caption.text
            );
            // Ruby is drawn at half size, which is how it was recognized.
            let ch = &region.chars[0];
            assert!(ch.char_horizontal_scale <= 0.5 || ch.char_width == 18);
        }
    }
}

/// Every character lands inside the caption plane, and a region's width is the
/// sum of the cells in it — the two invariants a renderer relies on.
#[test]
fn geometry_stays_inside_the_plane() {
    for caption in decode_fixture(Options::default()) {
        for region in &caption.regions {
            let sum: i32 = region.chars.iter().map(|c| c.section_width()).sum();
            assert_eq!(region.width, sum, "region width does not match its chars");
            assert!(region.height > 0);
            for ch in &region.chars {
                assert!(
                    ch.x >= 0 && ch.x < caption.plane_width,
                    "x {} off plane",
                    ch.x
                );
                assert!(
                    ch.y >= 0 && ch.y < caption.plane_height,
                    "y {} off plane",
                    ch.y
                );
                assert!(ch.section_width() > 0 && ch.section_height() > 0);
            }
        }
    }
}

/// The MSZ substitution is a rendering choice, not a decoding one: the same
/// bytes give fullwidth punctuation for a text track and halfwidth for a
/// renderer drawing real half-width cells.
#[test]
fn msz_substitution_is_a_rendering_choice() {
    let fullwidth = decode_fixture(Options::default());
    let halfwidth = decode_fixture(Options {
        replace_msz_fullwidth_japanese: true,
        ..Options::default()
    });

    let fw = &fullwidth[1].text;
    let hw = &halfwidth[1].text;
    assert_eq!(fw, "テスト。");
    assert_eq!(hw, "テスト｡");
    assert_ne!(fw, hw);
}

/// The ASS renderer against the same bytes: a script libass can draw.
///
/// The interesting property is not the format but the layout. ARIB positions
/// every cell, so a region's characters must abut exactly — and where a region
/// mixes half-width and full-width cells, the two runs it splits into have to
/// meet where the first one ends. A run whose `\fsp` were wrong would land the
/// second one on top of the first, or leave a hole; nothing in the file would
/// look wrong, and it would only show up on screen.
#[test]
fn renders_an_ass_script_whose_runs_meet_on_the_grid() {
    // The option a text track wants and an ASS script does not: half-width
    // cells are drawn as squeezed fullwidth glyphs, not halfwidth characters.
    let captions = decode_fixture(Options {
        replace_msz_fullwidth_ascii: false,
        ..Options::default()
    });
    let mut line: Timeline<Caption> = Timeline::new();
    let mut events = Vec::new();
    for caption in &captions {
        let shown = (!caption.is_empty()).then(|| caption.clone());
        if let Some(event) = line.push(caption, shown) {
            events.push(event);
        }
    }
    if let Some(event) = line.flush() {
        events.push(event);
    }
    assert!(events.len() > 20, "{} events", events.len());

    let script = ass::to_file(&events, &ass::Options::default());
    assert!(script.contains("PlayResX: 960"));
    assert!(script.contains("PlayResY: 540"));
    assert!(script.contains(ass::DEFAULT_FONT));
    assert!(script.contains("合成字幕の一行目"));
    // Half-width is the norm in a Japanese caption, so the squeeze had better
    // be there.
    assert!(script.contains("\\fscx50"), "no half-width cells rendered");

    // Walk the model the same way the renderer does and check the pen: every
    // cell starts where the one before it ended.
    let mut mixed_regions = 0usize;
    for caption in &captions {
        for region in &caption.regions {
            let mut pen = region.x;
            let mut widths = region.chars.iter().map(|c| c.section_width());
            if let Some(first_width) = widths.next() {
                if widths.any(|w| w != first_width) {
                    mixed_regions += 1;
                }
            }
            for ch in &region.chars {
                assert_eq!(ch.x, pen, "cell out of step in region at {}", region.y);
                pen += ch.section_width();
            }
            assert_eq!(pen, region.x + region.width);
        }
    }
    // Without a region that genuinely mixes cell widths, the pen walk above
    // only ever advances by a uniform stride and can't catch a run whose
    // `\fsp` is wrong relative to the run before it.
    assert!(mixed_regions > 0, "no region mixes cell widths");

    // And the events are in order, none of them of zero length.
    let mut last_end: Option<i64> = None;
    for event in &events {
        if let Some(last_end) = last_end {
            assert!(event.start_ms >= last_end, "events overlap");
        }
        assert!(event.end_ms > event.start_ms);
        last_end = Some(event.end_ms);
    }
}

/// A DRCS glyph: a character this stream defined as a bitmap rather than send
/// as a character, and which reads as 〓 in every text form.
///
/// Two things are being checked. That the bitmap is unpacked the way it was
/// transmitted — 2 bits per pixel, MSB first, no row padding — which nothing
/// else in the crate would notice getting wrong, because a wrong bit order is
/// still a plausible-looking `Vec<u8>`. And that the ASS renderer draws it
/// instead of substituting it.
#[test]
fn draws_a_drcs_glyph_that_no_font_has() {
    let bytes = fixtures::drcs_ts();
    let mut decoder = Decoder::new(
        CaptionKind::Caption,
        Options {
            replace_msz_fullwidth_ascii: false,
            ..Options::default()
        },
    );
    let mut captions = Vec::new();
    for packet in pes_packets(&bytes, fixtures::CAPTION_PID) {
        if let Ok(Some(caption)) = decoder.decode(&packet.payload, packet.pts_ms()) {
            captions.push(caption);
        }
    }
    assert_eq!(captions.len(), 1);
    let caption = &captions[0];
    assert_eq!(caption.text, "〓のテスト。");
    assert_eq!(caption.drcs.len(), 1);

    let glyph = caption.drcs.values().next().unwrap();
    assert_eq!((glyph.width, glyph.height), (36, 36));
    assert_eq!((glyph.depth, glyph.depth_bits), (4, 2));
    // The key a replacement table would be written against. It is over the
    // bytes as transmitted, so it must not move when the unpacking changes.
    assert_eq!(glyph.md5, fixtures::DRCS_GLYPH_MD5);

    // An empty first column and ink in the fifth — the case that says the bits
    // are being read in the right order and from the right end. Every row is
    // sent twice, which is what the renderer's rectangle merge pays for.
    assert_eq!(glyph.level(0, 0), 0);
    assert_eq!(glyph.level(4, 0), 3, "the left stroke");
    assert_eq!(glyph.level(4, 1), 3, "the row below repeats it");
    assert!(
        (0..36).all(|x| glyph.level(x, 0) == glyph.level(x, 1)),
        "rows are sent in pairs"
    );
    // Only the two extreme levels are used: a stencil sent at 2 bits.
    assert!((0..36).all(|y| (0..36).all(|x| matches!(glyph.level(x, y), 0 | 3))));

    let mut line: Timeline<Caption> = Timeline::new();
    let event = {
        line.push(caption, Some(caption.clone()));
        line.flush().expect("one event")
    };
    let script = ass::to_file(&[event], &ass::Options::default());
    assert!(
        !script.contains('〓'),
        "the glyph was substituted, not drawn"
    );
    // Drawn into a 36-wide cell from a 36-wide bitmap, so 1:1.
    assert!(script.contains("\\fscx100\\fscy100"), "{script}");
    // And the words around it are still words.
    assert!(script.contains("のテスト"), "{script}");
}

/// A non-default background box and foreground colour, the way the real
/// capture sends both on every statement: `COL` palette-select then
/// background-select, and a direct single-byte colour code. Before this was
/// found, every character in this fixture kept the CLUT default —
/// `decoder.rs`'s `BKF..=WHF` and `COL` branches had no integration coverage
/// at all.
#[test]
fn colour_controls_set_a_non_default_foreground_and_background() {
    use arib_caption::b24::tables::CLUT;

    let captions = decode_fixture(Options::default());
    let default_text = CLUT[0][7];
    let default_back = CLUT[0][8];

    let mut saw_non_default_text = false;
    let mut saw_non_default_back = false;
    for caption in &captions {
        for ch in caption.regions.iter().flat_map(|r| &r.chars) {
            saw_non_default_text |= ch.text_color != default_text;
            saw_non_default_back |= ch.back_color != default_back;
        }
    }
    assert!(
        saw_non_default_text,
        "no character used a non-default foreground colour"
    );
    assert!(
        saw_non_default_back,
        "no character used a non-default background colour (COL)"
    );
}
