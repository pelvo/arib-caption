//! `arib-caption` — decode the captions of a transport stream.
//!
//! Usage:
//!   arib-caption text  [--pid 0x130] [--limit N] < stream.ts
//!   arib-caption vtt   [--pid 0x130] [--anchor A] < stream.ts > subs.vtt
//!   arib-caption ass   [--pid 0x130] [--anchor A] [--font NAME] [--font-ratio R]
//!                                                  < rec.ts > rec.ass
//!   arib-caption cues  [--pid 0x130] [--regions] < stream.ts
//!   arib-caption dump  [--pid 0x130] [--limit N] < stream.ts
//!   arib-caption drcs  [--pid 0x130]             < stream.ts
//!   arib-caption pids                            < stream.ts
//!
//! `text` is for reading, `vtt` and `ass` write a subtitle file to sit beside a
//! recording, `cues` streams one JSON object per cue for a program that is
//! segmenting a live stream, `drcs` draws the glyphs a stream defined for
//! itself, and `dump` shows the data groups underneath — which is what to look
//! at when the text comes out empty.
//!
//! `vtt` keeps the words; `ass` keeps where they were, what colour, and how
//! wide the cells were, which is what a player with libass can draw back.
//! `cues --regions` keeps all of that too, but as the model rather than as a
//! rendering — for a consumer that is going to draw the caption itself.
//!
//! With no `--pid` the caption PID comes from the PMT: it differs per service,
//! and the superimpose stream looks identical apart from its component tag.

use std::collections::VecDeque;
use std::io::{self, Read, Write};

use arib_caption::model::{Caption, CaptionKind, Duration};
use arib_caption::pes::{self, DataGroup, DataUnitKind};
use arib_caption::render::ass;
use arib_caption::render::json;
use arib_caption::render::timeline::{self, Timed, Timeline};
use arib_caption::render::vtt::{self, Cue, CueStream};
use arib_caption::ts::{self, PacketSplitter, PesAssembler, PesPacket, ServiceScanner};
use arib_caption::{Decoder, Options};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("text");
    let mut pid: Option<u16> = None;
    let mut limit: Option<usize> = None;
    let mut anchor = Anchor::Auto;
    let mut font = ass::DEFAULT_FONT.to_string();
    let mut font_ratio = ass::DEFAULT_FONT_SIZE_RATIO;
    let mut regions = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pid" => {
                let Some(v) = args.get(i + 1).and_then(|v| parse_u16(v)) else {
                    fail("--pid wants a number, e.g. --pid 0x130");
                };
                pid = Some(v);
                i += 2;
            }
            "--limit" => {
                let Some(v) = args.get(i + 1).and_then(|v| v.parse().ok()) else {
                    fail("--limit wants a count");
                };
                limit = Some(v);
                i += 2;
            }
            "--anchor" => {
                let Some(v) = args.get(i + 1).and_then(|v| Anchor::parse(v)) else {
                    fail("--anchor wants auto, caption, raw, or a number of milliseconds");
                };
                anchor = v;
                i += 2;
            }
            "--font" => {
                let Some(v) = args.get(i + 1) else {
                    fail("--font wants a family name");
                };
                font = v.clone();
                i += 2;
            }
            // The one thing a script cannot say about the font it names: ASS
            // sizes are line boxes, so the em a cell wants has to be asked for
            // multiplied by the font's own usWinAscent+usWinDescent over its
            // unitsPerEm. --font without this leaves the text a few percent off
            // the grid; the default is right for the default font.
            "--font-ratio" => {
                let Some(v) = args
                    .get(i + 1)
                    .and_then(|v| v.parse::<f32>().ok())
                    .filter(|v| *v > 0.0)
                else {
                    fail("--font-ratio wants the font's line box over its em, e.g. 1.395");
                };
                font_ratio = v;
                i += 2;
            }
            // Off by default: a consumer that only wants the words should not
            // pay several kilobytes a caption for the ones it will throw away.
            "--regions" => {
                regions = true;
                i += 1;
            }
            "-h" | "--help" => {
                usage();
                return;
            }
            other => fail(&format!("unknown argument {other}")),
        }
    }

    match command {
        "text" => text(pid, limit),
        "vtt" => write_vtt(pid, anchor),
        "ass" => write_ass(pid, anchor, font, font_ratio),
        "cues" => cues(pid, regions),
        "dump" => dump(pid, limit),
        "drcs" => drcs(pid),
        "pids" => pids(),
        "-h" | "--help" => usage(),
        other => fail(&format!("unknown command {other}")),
    }
}

fn usage() {
    println!("arib-caption text [--pid 0x130] [--limit N] < stream.ts");
    println!("arib-caption vtt  [--pid 0x130] [--anchor A] < stream.ts > subs.vtt");
    println!(
        "arib-caption ass  [--pid 0x130] [--anchor A] [--font NAME] [--font-ratio R] \
         < rec.ts > rec.ass"
    );
    println!("arib-caption cues [--pid 0x130] [--regions] < stream.ts");
    println!("arib-caption dump [--pid 0x130] [--limit N] < stream.ts");
    println!("arib-caption drcs [--pid 0x130] < stream.ts");
    println!("arib-caption pids < stream.ts");
    println!();
    println!("--anchor decides what time zero is in a sidecar:");
    println!("  auto     the earliest audio/video PTS in the file (default) — what a player uses");
    println!("  caption  the first caption's own PTS");
    println!("  raw      none; times stay broadcast PTS, as the live `cues` form needs");
    println!("  <ms>     that many milliseconds of PTS");
    println!();
    println!("--regions adds the whole caption model to each `cues` line — where the");
    println!("  cells were, their colours, sizes and DRCS bitmaps — for a consumer");
    println!("  that draws the caption itself instead of showing the words.");
}

/// What a sidecar's time zero is measured from.
///
/// Caption times are broadcast PTS — hours into the day. A live HLS rendition
/// hands those to the player as they are and lets `X-TIMESTAMP-MAP` reconcile
/// them, but a file sitting beside a recording has no such mechanism: its zero
/// has to be the player's zero, which for a transport stream is the earliest
/// PTS the file holds.
#[derive(Clone, Copy, Debug)]
enum Anchor {
    Auto,
    Caption,
    Raw,
    Fixed(i64),
}

impl Anchor {
    fn parse(s: &str) -> Option<Anchor> {
        match s {
            "auto" => Some(Anchor::Auto),
            "caption" => Some(Anchor::Caption),
            "raw" | "none" => Some(Anchor::Raw),
            other => other.parse().ok().map(Anchor::Fixed),
        }
    }

    /// The milliseconds to subtract, given what the pass found.
    fn resolve(self, media_pts: Option<i64>, first_caption: Option<i64>) -> i64 {
        match self {
            // Falling back to the first caption is for a stream with no media
            // in it — a caption-PID tap, or a test fixture. It is worse than
            // the real start (a recording can run for minutes before anyone
            // speaks) but it is not hours out, which raw PTS would be.
            Anchor::Auto => media_pts.or(first_caption).unwrap_or(0),
            Anchor::Caption => first_caption.unwrap_or(0),
            Anchor::Raw => 0,
            Anchor::Fixed(ms) => ms,
        }
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("arib-caption: {msg}");
    std::process::exit(2);
}

fn parse_u16(s: &str) -> Option<u16> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse().ok()
    }
}

// ── commands ────────────────────────────────────────────────────────

/// Captions as lines a person can read, with their timing and what the
/// decoder found in them.
fn text(pid: Option<u16>, limit: Option<usize>) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut shown = 0usize;

    let stats = for_each_caption(pid, Options::default(), |caption| {
        let pts = caption
            .pts_ms
            .map(vtt::timestamp)
            .unwrap_or_else(|| "--:--:--.---".into());
        let dur = match caption.duration {
            Duration::Indefinite => "until next".to_string(),
            Duration::Millis(ms) => format!("{:.1}s", ms as f32 / 1000.0),
        };
        let mut flags = Vec::new();
        if caption.clear_screen {
            flags.push("clear".to_string());
        }
        if !caption.drcs.is_empty() {
            flags.push(format!("drcs={}", caption.drcs.len()));
        }
        let ruby = caption.regions.iter().filter(|r| r.is_ruby).count();
        if ruby > 0 {
            flags.push(format!("ruby={ruby}"));
        }
        let flags = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(" "))
        };
        // Where the text sits in the caption plane. Worth printing: everything
        // a renderer does with position depends on it, and it is invisible in
        // the text itself.
        let at = caption
            .regions
            .iter()
            .filter(|r| !r.is_ruby)
            .map(|r| format!("{},{}", r.x, r.y))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(
            out,
            "{pts} +{dur} {}x{} at={at}{flags} {}",
            caption.plane_width,
            caption.plane_height,
            caption.text.replace('\n', " / ")
        );
        shown += 1;
        !matches!(limit, Some(n) if shown >= n)
    });

    let _ = writeln!(out, "# {shown} captions, {} errors", stats.errors);
}

/// A WebVTT file for the whole stream — the sidecar form, for a recording.
fn write_vtt(pid: Option<u16>, anchor: Anchor) {
    let mut stream = CueStream::new();
    let mut cues: Vec<Cue> = Vec::new();
    let mut first_caption = None;
    let stats = for_each_caption(pid, Options::default(), |caption| {
        first_caption = first_caption.or(caption.pts_ms);
        if let Some(cue) = stream.push(&caption) {
            cues.push(cue);
        }
        true
    });
    if let Some(cue) = stream.flush() {
        cues.push(cue);
    }

    let base = anchor.resolve(stats.start_pts, first_caption);
    for cue in &mut cues {
        cue.start_ms = timeline::rebase(cue.start_ms, base);
        cue.end_ms = timeline::rebase(cue.end_ms, base);
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(vtt::to_file(&cues).as_bytes());
    eprintln!(
        "arib-caption: {} cues, time zero at PTS {base} ms",
        cues.len()
    );
}

/// An ASS script for the whole stream: the words where they were sent, in the
/// colours they were sent in.
fn write_ass(pid: Option<u16>, anchor: Anchor, font: String, font_ratio: f32) {
    // Half-width text has to stay fullwidth here and be squeezed by \fscx: a
    // halfwidth glyph advances half an em where the cell expects a full one,
    // and the line walks off the grid the broadcast laid it out on.
    let options = Options {
        replace_msz_fullwidth_ascii: false,
        ..Options::default()
    };

    let mut line: Timeline<Caption> = Timeline::new();
    let mut events: Vec<Timed<Caption>> = Vec::new();
    let mut first_caption = None;
    let stats = for_each_caption(pid, options, |caption| {
        first_caption = first_caption.or(caption.pts_ms);
        // A caption with no regions is a clear-screen: it shows nothing, but it
        // is what ends the caption before it.
        let shown = (!caption.is_empty()).then(|| caption.clone());
        if let Some(event) = line.push(&caption, shown) {
            events.push(event);
        }
        true
    });
    if let Some(event) = line.flush() {
        events.push(event);
    }

    let base = anchor.resolve(stats.start_pts, first_caption);
    for event in &mut events {
        event.start_ms = timeline::rebase(event.start_ms, base);
        event.end_ms = timeline::rebase(event.end_ms, base);
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    let _ = out.write_all(
        ass::to_file(
            &events,
            &ass::Options {
                font,
                font_size_ratio: font_ratio,
            },
        )
        .as_bytes(),
    );
    eprintln!(
        "arib-caption: {} captions, time zero at PTS {base} ms",
        events.len()
    );
}

/// One JSON object per cue, flushed as it is known.
///
/// This is the live form: a segmenter that owns the playlist reads these and
/// writes the WebVTT segments itself, because only it knows where the segment
/// boundaries are.
///
/// Each caption is printed **twice**: once the moment it appears, with
/// `"open":true` and a provisional end, and again with `"open":false` when the
/// next caption (or a clear-screen) says where it really ended. Both carry the
/// same `start_ms`, which is the key a consumer replaces on. Printing only the
/// closed form is what makes a live subtitle track late — the end arrives with
/// the next caption, and Japanese captions are 2 to 8 seconds apart, by which
/// time the segment the cue belonged in has already been fetched.
///
/// With `--regions` each line also carries the caption itself — `"caption":{…}`,
/// see [`render::json`] — for a consumer that draws it rather than showing the
/// words. Both forms come off **one** decode: a second child reading a second
/// copy of the same TS to produce the same captions twice is the thing this
/// flag exists to avoid.
fn cues(pid: Option<u16>, regions: bool) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut stream = CueStream::new();
    // The caption each open cue came from, keyed by its PTS — which is the
    // `start_ms` of every cue the stream will ever emit for it. Keeping them
    // beside the CueStream rather than running a second Timeline is what
    // guarantees the two forms describe the same caption: a Timeline of
    // `Caption` closes on `regions.is_empty()` where CueStream closes on the
    // *text* being empty, and a caption of undrawable DRCS is the case where
    // those two disagree.
    let mut recent: VecDeque<(i64, String)> = VecDeque::new();

    let emit = |out: &mut dyn Write, cue: &Cue, open: bool, recent: &VecDeque<(i64, String)>| {
        let _ = write!(
            out,
            r#"{{"start_ms":{},"end_ms":{},"open":{},"top":{},"text":"{}""#,
            cue.start_ms,
            cue.end_ms,
            open,
            cue.top,
            json::escape(&cue.text)
        );
        if let Some((_, caption)) = recent.iter().find(|(pts, _)| *pts == cue.start_ms) {
            let _ = write!(out, r#","caption":{caption}"#);
        }
        let _ = writeln!(out, "}}");
        // A consumer is waiting on this line to write a segment; buffering it
        // would put the subtitle behind the picture.
        let _ = out.flush();
    };

    for_each_caption(pid, Options::default(), |caption| {
        if regions {
            if let Some(pts) = caption.pts_ms {
                if !caption.is_empty() {
                    recent.push_back((pts, json::caption(&caption)));
                    // Two is all the protocol needs — a cue is emitted open at
                    // the caption that opened it and closed at the next one —
                    // and a handful covers a decoder that emits several before
                    // anything closes.
                    while recent.len() > 8 {
                        recent.pop_front();
                    }
                }
            }
        }
        if let Some(cue) = stream.push(&caption) {
            emit(&mut out, &cue, false, &recent);
        }
        if let Some(cue) = stream.pending() {
            emit(&mut out, &cue, true, &recent);
        }
        true
    });
    if let Some(cue) = stream.flush() {
        emit(&mut out, &cue, false, &recent);
    }
}

/// Every distinct DRCS glyph in the stream, drawn.
///
/// A DRCS glyph is a bitmap the broadcast defined on the fly for a character
/// that is in no code set, and there is nothing to read it as until its MD5 is
/// in a replacement table. This is how one gets there: see the glyph, decide
/// what character it is, write the pair down. It is also the check that the
/// bitmap is being unpacked correctly at all — a wrong bit order is noise, and
/// noise is obvious here and invisible everywhere else.
fn drcs(pid: Option<u16>) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut seen: Vec<String> = Vec::new();

    for_each_caption(pid, Options::default(), |caption| {
        for glyph in caption.drcs.values() {
            if seen.contains(&glyph.md5) {
                continue;
            }
            seen.push(glyph.md5.clone());
            let _ = writeln!(
                out,
                "md5={} {}x{} depth={} ({} bpp){}",
                glyph.md5,
                glyph.width,
                glyph.height,
                glyph.depth,
                glyph.depth_bits,
                match glyph.alternative {
                    Some(c) => format!(" → {c}"),
                    None => String::new(),
                }
            );
            for y in 0..glyph.height {
                let row: String = (0..glyph.width)
                    .map(|x| match glyph.level(x, y) {
                        0 => ' ',
                        // Two-level glyphs are the common case and want one
                        // ink character; deeper ones show their coverage.
                        l if l + 1 == glyph.depth as u8 => '#',
                        _ => '+',
                    })
                    .collect();
                let _ = writeln!(out, "  |{row}|");
            }
        }
        true
    });

    if seen.is_empty() {
        eprintln!("arib-caption: no DRCS glyphs in this stream");
    } else {
        eprintln!("arib-caption: {} distinct glyphs", seen.len());
    }
}

/// The caption and superimpose PIDs the PMT declares.
fn pids() {
    let mut scanner = ServiceScanner::new();
    for_each_packet(|packet| {
        scanner.push(packet);
        true
    });
    if scanner.streams().is_empty() {
        eprintln!("arib-caption: no caption stream in the PMT");
        std::process::exit(1);
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for s in scanner.streams() {
        let kind = if s.is_superimpose() {
            "superimpose"
        } else {
            "caption"
        };
        let tag = s
            .component_tag
            .map(|t| format!("0x{t:02x}"))
            .unwrap_or_else(|| "-".into());
        let _ = writeln!(
            out,
            "pid=0x{:04x} service=0x{:04x} component_tag={} {}",
            s.pid, s.service_id, tag, kind
        );
    }
}

/// One line per data group: what it is, when it plays, what it holds.
fn dump(pid: Option<u16>, limit: Option<usize>) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut groups = 0usize;
    let mut errors = 0usize;

    for_each_pes(pid, |packet| {
        let parsed = match pes::parse(&packet.payload) {
            Ok(p) => p,
            Err(e) => {
                let _ = writeln!(out, "!! {e} ({} bytes)", packet.payload.len());
                errors += 1;
                return true;
            }
        };
        groups += 1;
        let pts = packet
            .pts_ms()
            .map(|ms| format!("{}.{:03}", ms / 1000, ms % 1000))
            .unwrap_or_else(|| "-".into());
        let seq = match parsed.header.group {
            pes::Group::A => 'A',
            pes::Group::B => 'B',
        };
        match &parsed.group {
            DataGroup::Management(m) => {
                let langs: Vec<String> = m
                    .languages
                    .iter()
                    .map(|l| {
                        format!(
                            "{}(id={} fmt={} tcs={} dmf=0x{:x})",
                            String::from_utf8_lossy(&l.iso639),
                            l.language_id,
                            l.format,
                            l.tcs,
                            l.dmf
                        )
                    })
                    .collect();
                let _ = writeln!(
                    out,
                    "pts={pts} seq={seq} mgmt tmd={:?} langs=[{}] {}",
                    m.tmd,
                    langs.join(" "),
                    units(m.units())
                );
            }
            DataGroup::Statement(s) => {
                let _ = writeln!(
                    out,
                    "pts={pts} seq={seq} stmt lang={} tmd={:?} {}",
                    s.language_id,
                    s.tmd,
                    units(s.units())
                );
            }
        }
        !matches!(limit, Some(n) if groups >= n)
    });

    let _ = writeln!(out, "# {groups} data groups, {errors} unparseable");
}

fn units(iter: pes::DataUnits<'_>) -> String {
    let mut parts = Vec::new();
    for unit in iter {
        match unit {
            Ok(u) => {
                let name = match u.kind {
                    DataUnitKind::StatementBody => "body".to_string(),
                    DataUnitKind::Drcs1 => "drcs1".to_string(),
                    DataUnitKind::Drcs2 => "drcs2".to_string(),
                    DataUnitKind::Bitmap => "bitmap".to_string(),
                    DataUnitKind::ColorMap => "colormap".to_string(),
                    DataUnitKind::GeometricShape => "geometric".to_string(),
                    DataUnitKind::AdditionalSound => "sound".to_string(),
                    DataUnitKind::Unknown(p) => format!("unknown(0x{p:02x})"),
                };
                parts.push(format!("{name}({})", u.bytes.len()));
            }
            Err(e) => parts.push(format!("!{e}")),
        }
    }
    if parts.is_empty() {
        "units=-".into()
    } else {
        format!("units={}", parts.join(","))
    }
}

// ── plumbing ────────────────────────────────────────────────────────

#[derive(Default)]
struct Stats {
    errors: usize,
    /// The earliest audio/video PTS in the head of the stream — a player's
    /// zero. `None` for a stream carrying nothing but captions.
    start_pts: Option<i64>,
}

/// Decode captions from stdin, handing each to `f` until it returns false.
fn for_each_caption(
    pid: Option<u16>,
    options: Options,
    mut f: impl FnMut(Caption) -> bool,
) -> Stats {
    let mut decoder = Decoder::new(CaptionKind::Caption, options);
    let mut errors = 0usize;
    let start_pts = for_each_pes(pid, |packet| {
        match decoder.decode(&packet.payload, packet.pts_ms()) {
            Ok(Some(caption)) => f(caption),
            Ok(None) => true,
            Err(e) => {
                eprintln!("arib-caption: {e}");
                errors += 1;
                true
            }
        }
    });
    Stats { errors, start_pts }
}

/// How far into the stream to keep looking for the media streams' first PTS.
///
/// The answer is in the first packets by construction — a demuxer decides the
/// same way — and scanning the whole file for it would mean holding the head's
/// answer against timestamps from an hour later.
const START_PTS_PACKETS: usize = 20_000;

/// Reassemble the caption PID's PES packets from stdin, finding the PID in the
/// PMT if one was not given. Returns the earliest audio/video PTS seen in the
/// head of the stream, which is the zero a sidecar's times are measured from.
fn for_each_pes(pid: Option<u16>, mut f: impl FnMut(&PesPacket) -> bool) -> Option<i64> {
    let mut scanner = ServiceScanner::new();
    let mut assembler = pid.map(PesAssembler::new);
    let mut found_any = assembler.is_some();
    let mut start_pts: Option<i64> = None;
    let mut scanned = 0usize;

    for_each_packet(|packet| {
        if scanned < START_PTS_PACKETS {
            scanned += 1;
            if let Some(pts) = ts::pes_pts_ms(packet) {
                start_pts = Some(start_pts.map_or(pts, |seen: i64| seen.min(pts)));
            }
        }
        if assembler.is_none() {
            scanner.push(packet);
            if let Some(found) = scanner.caption_pid() {
                eprintln!("arib-caption: caption pid 0x{found:04x} (from PMT)");
                assembler = Some(PesAssembler::new(found));
                found_any = true;
            }
        }
        let Some(asm) = assembler.as_mut() else {
            return true;
        };
        match asm.push(packet) {
            Some(pes_packet) => f(&pes_packet),
            None => true,
        }
    });

    if let Some(asm) = assembler.as_mut() {
        if let Some(pes_packet) = asm.flush() {
            f(&pes_packet);
        }
    }
    if !found_any {
        eprintln!("arib-caption: no caption stream found (try --pid)");
        std::process::exit(1);
    }
    start_pts
}

/// Feed every TS packet on stdin to `f`, stopping early if it returns false.
fn for_each_packet(mut f: impl FnMut(&[u8]) -> bool) {
    let mut splitter = PacketSplitter::new();
    let mut stdin = io::stdin().lock();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("arib-caption: read: {e}");
                std::process::exit(1);
            }
        };
        splitter.feed(&buf[..n]);
        while let Some(packet) = splitter.next_packet() {
            if !f(&packet) {
                return;
            }
        }
    }
}
