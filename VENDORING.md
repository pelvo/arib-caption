# Fork provenance

Forked from: https://github.com/DuckFeather10086/libaribcaption-rs
Revision at vendoring time: 5485697
Vendored: 2026-08-09

Upstream ships no LICENSE file at any revision (a raw fetch of `LICENSE` from
the repository returns 404), though its `Cargo.toml` declares
`license = "MIT OR Apache-2.0"` and its README states the same licence — an
informal, affirmative signal of licensing intent that a LICENSE file would
normally back up. Revision `5485697` is not present in that repository's
current history — it may have been rewritten or force-pushed since
vendoring. None of this is discoverable from the vendored tree alone, so it
is recorded here. This fork does not rely on that declared intent, or on any
other grant from the intermediate Rust port; its License section (see
README.md) carries forward xqq/libaribcaption's original MIT grant instead.

Copied rather than referenced by path.
Keep the list below current so a future re-sync knows what it would overwrite.

## Local changes

- Replaced the fork's crate-local MPEG-TS PES assembler with `tuner-codec`'s
  shared engine behind a compatibility adapter.
- Preserved the public `PesAssembler` and `PesPacket` API, including
  discontinuity accounting and finite-stream flushing behavior.
- Removed the duplicate PES implementation and its temporary selection
  feature after parity was established.
- Added compatibility tests and a 70-record self-golden of the shared engine,
  generated against synthesized input. An earlier 199-record golden,
  captured from the retired engine against a real broadcast fixture, was
  deleted along with that fixture — see README.md's Test coverage section.
- Added the required dependency on `tuner-codec`.

The previous version of this document additionally claimed "three
behavior-preserving Rust idiom cleanups" in `decoder.rs`, `pes.rs`, and
`ts.rs`, implying the rest of the fork's source is unmodified beyond
mechanical PES-engine integration. That claim is not settled either way by
anything this document can currently point to:

- `decoder.rs`'s own module doc documents two behavioral departures from
  xqq's C++ upstream — a statement that runs out of bytes mid-control-code
  now stops and keeps what it decoded, where the C++ discards the whole
  caption; and the interleave group is read from bit 5 of `data_group_id`
  so a mid-stream management change is applied instead of being taken for a
  retransmission. Fetched live, `DuckFeather10086/libaribcaption-rs`'s
  current `main` branch carries this same module doc byte-for-byte, "Two
  deliberate departures" text included. These departures predate vendoring:
  they belong to the intermediate Rust port's own divergence from xqq's
  C++, not to anything this fork introduced.
- `model.rs`'s module doc is likewise byte-for-byte identical to upstream's
  current tree. Its description of itself as a port of libaribcaption's
  `caption.hpp` that "keeps the same split" is upstream's own
  self-description, not evidence of what this fork changed.
- What is actually true, and narrower: `model.rs` and `render/json.rs` are
  not accounted for in the "Local changes" list above at all. Whether this
  fork touched either of them beyond the mechanical PES-engine swap is
  simply unknown — not contradicted by anything cited here, since neither
  module doc speaks to this fork's own changes at all.

No line-by-line audit of the `decoder.rs` / `model.rs` / `json.rs` delta
between this fork and the vendored revision has been done. That hedge is
about changes this fork may have made that go unlisted above — it is not a
claim that anything above is misattributed. The two departures cited above
are correctly attributed to the intermediate Rust port, not to this fork,
precisely because they were checked against that port's own live tree.
