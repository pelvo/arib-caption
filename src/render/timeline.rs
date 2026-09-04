//! When a caption ends — the one thing every timed renderer needs and none of
//! them can work out alone.
//!
//! Most ARIB captions carry no duration ([`Duration::Indefinite`]): they stay up
//! until the next caption replaces them, so nothing can be written the moment a
//! caption is decoded. Only the *next* caption — or a clear-screen, which is how
//! the last one ends when nothing follows — says where the previous one stopped.
//!
//! [`Timeline`] holds that one caption back and closes it when the answer
//! arrives. It is generic over what a renderer keeps: WebVTT keeps text and a
//! position hint, ASS keeps the whole caption. The timing is the same either
//! way, and having it in two places is how the two would drift apart.

use crate::model::{Caption, Duration};

/// A renderer's value together with the span it is on screen for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timed<T> {
    pub start_ms: i64,
    pub end_ms: i64,
    pub value: T,
}

/// How long a caption with no end time may stay up when nothing follows it.
///
/// A stream can simply stop — the programme ends, the tuner is preempted —
/// leaving the last caption open forever. Somebody has to choose a number;
/// this is it.
pub const MAX_OPEN_MS: i64 = 30_000;

/// The end given to a caption that is still on screen.
///
/// Waiting for the real end is what makes a live subtitle track late: the end
/// only arrives with the *next* caption, and Japanese captions are 2 to 8
/// seconds apart. A consumer that must publish now — an HLS segmenter, whose
/// segment will be fetched within a second or two of being written — needs the
/// cue at its correct start with an approximate end, not a correct cue that
/// arrives after the moment has passed. Typical caption is 2-3 s, so this errs
/// slightly long; the real end replaces it as soon as it is known.
pub const PROVISIONAL_MS: i64 = 5_000;

/// Turns decoded captions into spans, resolving the open-ended ones.
#[derive(Debug)]
pub struct Timeline<T> {
    pending: Option<Pending<T>>,
}

impl<T> Default for Timeline<T> {
    fn default() -> Self {
        Self { pending: None }
    }
}

#[derive(Debug)]
struct Pending<T> {
    start_ms: i64,
    /// Set only when the caption carried an explicit duration.
    end_ms: Option<i64>,
    value: T,
}

impl<T: Clone> Timeline<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one decoded caption along with what a renderer would show for it,
    /// or `None` when it shows nothing — a clear-screen puts nothing on screen
    /// but still ends what was there.
    ///
    /// Returns the span this caption *closed*, which is normally the one before
    /// it.
    pub fn push(&mut self, caption: &Caption, value: Option<T>) -> Option<Timed<T>> {
        // A caption with no PTS cannot be timed, and must not close anything
        // either: superimpose arrives that way (private_stream_2 has no PTS)
        // and it is not part of the subtitle timeline at all.
        let pts = caption.pts_ms?;

        let closed = self.close_at(pts);
        if let Some(value) = value {
            self.pending = Some(Pending {
                start_ms: pts,
                end_ms: match caption.duration {
                    Duration::Millis(ms) => Some(pts + ms as i64),
                    Duration::Indefinite => None,
                },
                value,
            });
        }
        closed
    }

    /// Close the open span, for when the stream ends.
    pub fn flush(&mut self) -> Option<Timed<T>> {
        let pending = self.pending.take()?;
        let end_ms = pending.end_ms.unwrap_or(pending.start_ms + MAX_OPEN_MS);
        Some(Timed {
            start_ms: pending.start_ms,
            end_ms,
            value: pending.value,
        })
    }

    /// The start time of the caption still waiting for its end, if any.
    pub fn open_since(&self) -> Option<i64> {
        self.pending.as_ref().map(|p| p.start_ms)
    }

    /// The caption currently on screen as a publishable span, its end
    /// provisional ([`PROVISIONAL_MS`]) unless the broadcast stated one.
    ///
    /// This is what a live consumer emits immediately and then supersedes with
    /// the closed span. The alternative — waiting for the real end — is a
    /// subtitle track that runs seconds behind the picture.
    pub fn pending(&self) -> Option<Timed<T>> {
        let pending = self.pending.as_ref()?;
        Some(Timed {
            start_ms: pending.start_ms,
            end_ms: pending.end_ms.unwrap_or(pending.start_ms + PROVISIONAL_MS),
            value: pending.value.clone(),
        })
    }

    fn close_at(&mut self, at_ms: i64) -> Option<Timed<T>> {
        let pending = self.pending.take()?;
        // An explicit duration wins, but never past the caption that replaced
        // it: a broadcast that says "3 s" and then sends a new line after 1 s
        // means the new line.
        let end_ms = match pending.end_ms {
            Some(explicit) => explicit.min(at_ms),
            None => at_ms,
        };
        // Guard against a PTS that did not advance: a zero-length span is
        // invisible and some players reject it.
        let end_ms = end_ms.max(pending.start_ms + 1);
        Some(Timed {
            start_ms: pending.start_ms,
            end_ms,
            value: pending.value,
        })
    }
}

/// Where a recording's clock starts, so a sidecar's times are the player's.
///
/// Caption times are broadcast PTS: hours into the day, not seconds into the
/// file. A live HLS rendition can hand a player the raw values and let
/// `X-TIMESTAMP-MAP` reconcile them, but an `.ass` or `.vtt` sitting beside a
/// recording has no such mechanism — its zero has to be the same zero the
/// player uses, which for a transport stream is the earliest PTS in it.
///
/// `anchor_ms` is that value; [`rebase`] applies it.
pub fn rebase(pts_ms: i64, anchor_ms: i64) -> i64 {
    // PTS is 33 bits at 90 kHz, so it wraps every ~26.5 hours. A recording that
    // crosses the wrap has captions numerically *before* its own start; the
    // only reading that makes sense is that the clock went round once.
    const WRAP_MS: i64 = (1 << 33) / 90;
    let t = pts_ms - anchor_ms;
    if t < -WRAP_MS / 2 {
        t + WRAP_MS
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CaptionRegion;

    fn caption(pts_ms: i64, duration: Duration) -> Caption {
        Caption {
            pts_ms: Some(pts_ms),
            duration,
            regions: vec![CaptionRegion::default()],
            ..Default::default()
        }
    }

    #[test]
    fn a_span_ends_where_the_next_one_starts() {
        let mut line: Timeline<&str> = Timeline::new();
        assert_eq!(
            line.push(&caption(1_000, Duration::Indefinite), Some("one")),
            None
        );
        let closed = line
            .push(&caption(3_500, Duration::Indefinite), Some("two"))
            .expect("the first span is now closed");
        assert_eq!(
            (closed.start_ms, closed.end_ms, closed.value),
            (1_000, 3_500, "one")
        );
        assert_eq!(line.open_since(), Some(3_500));
    }

    /// A caption that shows nothing still ends what was showing — that is what
    /// a clear-screen is for.
    #[test]
    fn a_caption_with_nothing_to_show_closes_but_does_not_open() {
        let mut line: Timeline<&str> = Timeline::new();
        line.push(&caption(1_000, Duration::Indefinite), Some("gone"));
        let closed = line
            .push(&caption(2_000, Duration::Indefinite), None)
            .expect("closed by the clear");
        assert_eq!((closed.start_ms, closed.end_ms), (1_000, 2_000));
        assert_eq!(line.open_since(), None);
    }

    #[test]
    fn a_caption_with_no_pts_is_not_on_the_timeline_at_all() {
        let mut line: Timeline<&str> = Timeline::new();
        line.push(&caption(1_000, Duration::Indefinite), Some("held"));
        let mut no_pts = caption(0, Duration::Indefinite);
        no_pts.pts_ms = None;
        assert_eq!(line.push(&no_pts, Some("superimpose")), None);
        assert_eq!(line.open_since(), Some(1_000), "the held span survives it");
    }

    #[test]
    fn a_recording_that_crosses_the_pts_wrap_stays_monotonic() {
        const WRAP_MS: i64 = (1 << 33) / 90;
        let anchor = WRAP_MS - 10_000; // ten seconds before the clock goes round
        assert_eq!(rebase(anchor, anchor), 0);
        assert_eq!(rebase(WRAP_MS - 1_000, anchor), 9_000);
        // The same instant expressed after the wrap: 1 s past it is 11 s in.
        assert_eq!(rebase(1_000, anchor), 11_000);
    }
}
