//! A pure-Rust decoder for ARIB STD-B24 — the Association of Radio
//! Industries and Businesses standard governing closed captions on Japanese
//! digital TV.
//!
//! A port of the decoder half of [libaribcaption](https://github.com/xqq/libaribcaption)
//! with the model and the renderers kept apart on purpose:
//!
//! ```text
//! TS ──► ts::PesAssembler ──► pes::parse ──► decoder::Decoder ──► model::Caption
//!                                                                     │
//!                                              render::vtt (live) ◄────┤
//!                                         render::ass (recordings) ◄───┤
//!                                        render::json (a renderer  ◄───┤
//!                                         in another process)          │
//!                             render::bitmap (not yet implemented) ◄───┘
//! ```
//!
//! [`model::Caption`] is the contract. It carries positions, colours, sizes and
//! DRCS glyphs — everything the broadcast sent — so a renderer chooses how much
//! of that to keep: WebVTT keeps the text and the timing, ASS keeps the
//! placement too, and JSON keeps all of it by rendering none of it — the
//! model itself, for a consumer that draws the caption somewhere this crate
//! cannot reach.
//!
//! This crate depends on the sibling crate
//! [`tuner-codec`](https://github.com/pelvo/tuner-codec) for its MPEG-TS/PES
//! (Packetized Elementary Stream) framing.
//!
//! There are no C dependencies, which is the point: the stack this belongs to
//! cross-compiles to arm64 as a one-liner and would rather not stop doing that
//! for a subtitle track.

mod shared_pes_adapter;

pub mod b24;
pub mod decoder;
pub mod model;
pub mod pes;
pub mod render;
pub mod ts;

pub use decoder::{DecodeError, Decoder, Options};
pub use model::{
    Caption, CaptionChar, CaptionKind, CaptionRegion, Drcs, Duration, Profile, Rgba, WritingMode,
};
