//! Renderers: what to do with a decoded [`crate::model::Caption`].
//!
//! Each one keeps a different amount of what the broadcast sent, because each
//! answers a different question:
//!
//! - [`vtt`] — text and timing, for a subtitle track a browser can toggle.
//!   Position survives only as "this one was at the top".
//! - [`ass`] — placement, colour and size as well, for a sidecar beside a
//!   recording where a player will honour them.
//! - [`json`] — no rendering at all: the model itself, for a consumer that will
//!   draw it somewhere this process cannot reach. The live overlay in a browser
//!   is the one that exists.
//! - `bitmap` (next) — every pixel, including DRCS glyphs no font has, for the
//!   recording that should look exactly like the broadcast.
//!
//! They share the model, and [`timeline`], which answers the one question none
//! of them can answer alone: when a caption that stated no duration ends. Past
//! that they share nothing — a decoder change is visible to all of them, a
//! renderer change to none of the others.

pub mod ass;
pub mod json;
pub mod timeline;
pub mod vtt;
