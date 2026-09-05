//! WebVTT: the text-and-timing renderer, for a subtitle track a browser can
//! turn on.
//!
//! The hard part is not the format, it is knowing when a caption ends — which
//! is [`crate::render::timeline`]'s problem, shared with every other timed
//! renderer. What is left here is what WebVTT keeps of a caption: the words,
//! and whether it was at the top of the screen.

use std::fmt::Write as _;

use crate::model::Caption;
use crate::render::timeline::{Timed, Timeline};

#[doc(inline)]
pub use crate::render::timeline::{MAX_OPEN_MS, PROVISIONAL_MS};

/// A resolved subtitle cue: text with a start and an end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// The caption sat in the upper half of the plane. Japanese broadcasts move
    /// captions up when something at the bottom of the frame matters — a score
    /// bar, a name caption — so a renderer that always places at the bottom
    /// covers exactly what the broadcaster was avoiding.
    pub top: bool,
}

/// Turns decoded captions into cues, resolving the open-ended ones.
#[derive(Debug, Default)]
pub struct CueStream {
    timeline: Timeline<Body>,
}

/// What WebVTT keeps of a caption.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Body {
    text: String,
    top: bool,
}

impl CueStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one decoded caption. Returns the cue that this caption *closed*,
    /// if any — which is normally the one before it.
    pub fn push(&mut self, caption: &Caption) -> Option<Cue> {
        let text = visible_text(caption);
        // An empty one is a clear-screen, or a caption made only of DRCS with
        // no replacement: nothing to show, but it does end whatever was showing.
        let body = (!text.is_empty()).then(|| Body {
            text,
            top: is_top(caption),
        });
        self.timeline.push(caption, body).map(Cue::from)
    }

    /// Close the open cue, if the stream ends.
    pub fn flush(&mut self) -> Option<Cue> {
        self.timeline.flush().map(Cue::from)
    }

    /// The start time of the caption still waiting for its end, if any.
    pub fn open_since(&self) -> Option<i64> {
        self.timeline.open_since()
    }

    /// The caption currently on screen as a publishable cue, its end
    /// provisional ([`PROVISIONAL_MS`]) unless the broadcast stated one.
    pub fn pending(&self) -> Option<Cue> {
        self.timeline.pending().map(Cue::from)
    }
}

impl From<Timed<Body>> for Cue {
    fn from(timed: Timed<Body>) -> Self {
        Cue {
            start_ms: timed.start_ms,
            end_ms: timed.end_ms,
            text: timed.value.text,
            top: timed.value.top,
        }
    }
}

/// The text a subtitle track should show: the caption's text with ruby already
/// excluded by the decoder, trimmed of the trailing continuation arrow.
fn visible_text(caption: &Caption) -> String {
    // ➡ ends a line that continues in the next caption. It is a reading aid on
    // a TV screen and clutter in a subtitle track, where the next cue follows
    // immediately anyway.
    let text = caption.text.trim_end_matches('➡').trim();
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// True when every one of the caption's own regions sits in the top third of
/// the plane.
///
/// A third, not a half: this stream places ordinary dialogue anywhere from 28%
/// to 61% down the plane, because ARIB captions are positioned per line to dodge
/// whatever graphics are on screen. WebVTT can only say "up" or "down", so the
/// threshold has to be where a caption is unambiguously *not* subtitle-shaped.
fn is_top(caption: &Caption) -> bool {
    let mut any = false;
    let mut all_top = true;
    for region in caption.regions.iter().filter(|r| !r.is_ruby) {
        any = true;
        // Saturating: `Caption` is public, so a caller can hand this renderer
        // a region the decoder would never emit.
        if region.y.saturating_mul(3) >= caption.plane_height {
            all_top = false;
        }
    }
    any && all_top
}

/// Where a caption sits when the broadcast did not move it up.
///
/// Not the browser's own default, which is nowhere near the bottom of the
/// picture: a browser reserves room for its control bar whether or not the
/// controls are showing — measured at 13.6% of the picture height in Chromium
/// 1217 — and a subtitle floating in the lower third of the frame is what that
/// looks like. Snap-to-lines cannot go below that reservation (`line:-1` lands
/// in exactly the same place as `line:auto`), so the percentage form is the way
/// down: 94% leaves the caption clear of the progress bar, which sits at about
/// 96%, and is roughly where a television puts one. Measured there at 5.6% of
/// the picture height, and a caption of two, three or four lines grows *upward*
/// from it rather than off the bottom.
///
/// Bare, with no line alignment after it. WebVTT allows `line:94%,end` and
/// hls.js parses it, but Chromium's own parser throws the whole setting away
/// when it sees the comma (`cue.line` comes back `auto`, which is how this was
/// first shipped wrong) — and it has no `lineAlign` to align with in the first
/// place, since it places a percentage line by the box's bottom edge anyway.
///
/// `internal/caption` in ferrite writes the same setting for the live rendition;
/// the two have to agree or a channel's captions move when you record it.
const BOTTOM_LINE: &str = "line:94%";

/// `HH:MM:SS.mmm`, the only timestamp form WebVTT accepts for hours.
pub fn timestamp(ms: i64) -> String {
    let ms = ms.max(0);
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1000) % 60,
        ms % 1000
    )
}

fn escape(text: &str) -> String {
    // & first, or the escapes of the others get mangled.
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

impl Cue {
    /// The cue as it appears in a WebVTT file.
    pub fn to_block(&self) -> String {
        let mut out = String::new();
        let _ = write!(
            out,
            "{} --> {}",
            timestamp(self.start_ms),
            timestamp(self.end_ms)
        );
        if self.top {
            // Two lines down from the top, which clears the channel logos that
            // sit in the corner.
            let _ = write!(out, " line:1");
        } else {
            let _ = write!(out, " {BOTTOM_LINE}");
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "{}", escape(&self.text));
        out
    }
}

/// A whole WebVTT file: the header plus every cue. For a recording sidecar,
/// where all the cues are known.
pub fn to_file(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for cue in cues {
        out.push_str(&cue.to_block());
        out.push('\n');
    }
    out
}

/// One WebVTT segment of an HLS subtitle rendition.
///
/// Cue times are the caption's own presentation timestamps — the PTS of the TS
/// the captions came out of — and `X-TIMESTAMP-MAP` is what tells the player
/// so. `MPEGTS:0,LOCAL:00:00:00.000` declares "cue time zero is PTS zero",
/// which is exactly true when the times are PTS, and lets the player subtract
/// the stream's own initial PTS to reach its media timeline. It only holds if
/// the video segments keep their input timestamps (`ffmpeg -copyts`); without
/// that the video restarts at zero and every cue lands hours late.
pub fn to_segment(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000\n\n");
    for cue in cues {
        out.push_str(&cue.to_block());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CaptionRegion, Duration};

    fn caption(pts_ms: i64, text: &str, duration: Duration) -> Caption {
        Caption {
            text: text.into(),
            pts_ms: Some(pts_ms),
            duration,
            regions: vec![CaptionRegion {
                y: 480,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn an_open_caption_ends_where_the_next_one_starts() {
        let mut stream = CueStream::new();
        assert_eq!(
            stream.push(&caption(1_000, "ひとつめ", Duration::Indefinite)),
            None,
            "the first caption closes nothing"
        );
        let cue = stream
            .push(&caption(3_500, "ふたつめ", Duration::Indefinite))
            .expect("the first cue is now closed");
        assert_eq!(cue.start_ms, 1_000);
        assert_eq!(cue.end_ms, 3_500);
        assert_eq!(cue.text, "ひとつめ");
        assert_eq!(stream.open_since(), Some(3_500));
    }

    #[test]
    fn a_clear_screen_ends_the_caption_before_it() {
        let mut stream = CueStream::new();
        stream.push(&caption(1_000, "きえます", Duration::Indefinite));
        let mut clear = caption(2_000, "", Duration::Indefinite);
        clear.regions.clear();
        clear.clear_screen = true;
        let cue = stream.push(&clear).expect("closed by the clear");
        assert_eq!((cue.start_ms, cue.end_ms), (1_000, 2_000));
        assert_eq!(stream.open_since(), None);
    }

    #[test]
    fn an_explicit_duration_is_honoured_but_never_overruns() {
        let mut stream = CueStream::new();
        stream.push(&caption(1_000, "みじかい", Duration::Millis(2_000)));
        // Nothing follows for a while: the explicit end stands.
        let cue = stream
            .push(&caption(10_000, "つぎ", Duration::Indefinite))
            .unwrap();
        assert_eq!(cue.end_ms, 3_000);

        // Something follows sooner than the stated duration: it wins.
        let mut stream = CueStream::new();
        stream.push(&caption(1_000, "みじかい", Duration::Millis(5_000)));
        let cue = stream
            .push(&caption(2_000, "つぎ", Duration::Indefinite))
            .unwrap();
        assert_eq!(cue.end_ms, 2_000);
    }

    /// The caption on screen is publishable before its end is known — the
    /// point being that a live consumer never has to wait for the next caption.
    #[test]
    fn the_pending_caption_is_publishable_with_a_provisional_end() {
        let mut stream = CueStream::new();
        assert!(stream.pending().is_none());

        stream.push(&caption(1_000, "いま出ている", Duration::Indefinite));
        let open = stream.pending().expect("a pending cue");
        assert_eq!(open.start_ms, 1_000);
        assert_eq!(open.end_ms, 1_000 + PROVISIONAL_MS);
        assert_eq!(open.text, "いま出ている");

        // A stated duration is not provisional at all.
        let mut stream = CueStream::new();
        stream.push(&caption(1_000, "みじかい", Duration::Millis(2_000)));
        assert_eq!(stream.pending().unwrap().end_ms, 3_000);

        // Once it closes, the real end supersedes it and nothing is pending.
        let closed = stream
            .push(&caption(1_500, "つぎ", Duration::Indefinite))
            .expect("closed");
        assert_eq!(closed.end_ms, 1_500);
        assert_eq!(stream.pending().unwrap().start_ms, 1_500);
    }

    #[test]
    fn flush_bounds_a_cue_nothing_ever_closed() {
        let mut stream = CueStream::new();
        stream.push(&caption(1_000, "さいご", Duration::Indefinite));
        let cue = stream.flush().expect("flushed");
        assert_eq!(cue.end_ms, 1_000 + MAX_OPEN_MS);
        assert!(stream.flush().is_none());
    }

    #[test]
    fn the_continuation_arrow_is_dropped() {
        let mut stream = CueStream::new();
        stream.push(&caption(0, "つづく➡", Duration::Indefinite));
        let cue = stream
            .push(&caption(1_000, "つぎ", Duration::Indefinite))
            .unwrap();
        assert_eq!(cue.text, "つづく");
    }

    /// A caption the broadcast left at the bottom is placed by us and not left
    /// to the player's default, which reserves room for a control bar and puts
    /// the words in the lower third of the picture.
    #[test]
    fn ordinary_captions_are_placed_just_above_the_progress_bar() {
        let mut stream = CueStream::new();
        stream.push(&caption(0, "した", Duration::Indefinite));
        let cue = stream
            .push(&caption(1_000, "つぎ", Duration::Indefinite))
            .unwrap();
        assert!(!cue.top);
        let block = cue.to_block();
        assert!(block.contains("line:94%"), "{block}");
        // The percentage form is what turns snapping off, so the setting has to
        // carry the % — a bare `line:94` would be the 94th line from the top.
        assert!(!block.contains("line:94\n"), "{block}");
        // And nothing after the percentage: a line alignment there makes
        // Chromium discard the setting altogether.
        assert!(!block.contains("line:94%,"), "{block}");
    }

    #[test]
    fn top_captions_get_a_line_setting() {
        let mut top = caption(0, "うえ", Duration::Indefinite);
        top.regions[0].y = 40;
        let mut stream = CueStream::new();
        stream.push(&top);
        let cue = stream
            .push(&caption(1_000, "した", Duration::Indefinite))
            .unwrap();
        assert!(cue.top);
        assert!(cue.to_block().contains("line:1"), "{}", cue.to_block());
    }

    #[test]
    fn markup_in_caption_text_is_escaped() {
        let cue = Cue {
            start_ms: 0,
            end_ms: 1_000,
            text: "<b> & </b>".into(),
            top: false,
        };
        assert!(cue.to_block().contains("&lt;b&gt; &amp; &lt;/b&gt;"));
    }

    #[test]
    fn a_file_and_a_segment_differ_by_the_timestamp_map() {
        let cues = vec![Cue {
            start_ms: 13_427_175,
            end_ms: 13_429_243,
            text: "国内アーティスト楽曲の多くが".into(),
            top: false,
        }];
        let file = to_file(&cues);
        assert!(file.starts_with("WEBVTT\n\n"));
        assert!(file.contains("03:43:47.175 --> 03:43:49.243"));

        let segment = to_segment(&cues);
        assert!(segment.contains("X-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000"));
        assert!(segment.contains("03:43:47.175 --> 03:43:49.243"));
    }
}
