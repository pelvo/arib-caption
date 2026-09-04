//! Fixture construction shared by the integration tests.
//!
//! `arib-caption` used to test against slices of a real off-air recording.
//! Those bytes were third-party broadcast content and could not be published,
//! so the streams are built here instead: TS framing, PES framing, PSI tables
//! and ARIB STD-B24 statement bodies, written the way a broadcaster sends them.
//!
//! Cargo builds no target from a `tests/` subdirectory without a `main.rs`, so
//! this compiles once into each test binary that declares `mod support;`. Each
//! binary uses a different part of it, hence the blanket dead-code allowance.

#![allow(dead_code)]

pub mod b24;
pub mod fixtures;
pub mod ts;
