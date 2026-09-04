# arib-caption

`arib-caption` is a pure-Rust decoder for ARIB STD-B24 — the Association of
Radio Industries and Businesses standard that governs closed captions on
ISDB (Integrated Services Digital Broadcasting, Japan's digital TV system).
It reads ISDB caption and superimpose PES (Packetized Elementary Stream —
the unit an MPEG transport stream carries one kind of payload in) and
produces a lossless caption model, with renderers for WebVTT, ASS, and JSON.

It began as a fork of
[`DuckFeather10086/libaribcaption-rs`](https://github.com/DuckFeather10086/libaribcaption-rs)
(vendored at revision `5485697`), itself a pure-Rust port of the decoder
half of [`xqq/libaribcaption`](https://github.com/xqq/libaribcaption). The
licence chain is not as simple as that sentence implies — see
[License](#license) below and [VENDORING.md](VENDORING.md) for the full
provenance, including the fact that the intermediate Rust port ships no
LICENSE file of its own.

## Changes in this fork

Compared with the vendored revision, this crate:

- replaced the crate-local MPEG-TS PES assembler with the shared engine from
  the sibling crate [`tuner-codec`](https://github.com/pelvo/tuner-codec),
  which supplies MPEG-TS/PES framing to several crates in the same family;
- added `shared_pes_adapter.rs` to preserve the existing `PesAssembler`,
  `PesPacket`, `push`, `flush`, `pts_ms`, and discontinuity-counting API;
- removed the duplicate PES engine and the temporary feature switch after
  parity was established;
- added compatibility tests and a self-golden for the shared PES engine (70
  records, generated against synthesized input). An earlier 199-record
  golden, captured from the retired engine against a real off-air caption
  fixture, was deleted along with that fixture — see Test coverage, below;
  and
- added `tuner-codec` as a required dependency.

The B24 decoder, caption model, and renderers otherwise remain based on the
vendored fork. `decoder.rs`'s own module doc documents two behavioral
departures from upstream's C++ — but those words are inherited unchanged
from the vendored Rust source (verified live against
`DuckFeather10086/libaribcaption-rs`'s current tree, byte-for-byte), so they
predate this fork and are not evidence of anything this fork itself changed.
Whether this fork modified `decoder.rs`, `model.rs`, or `render/json.rs`
beyond the PES-engine swap is an open question — see
[VENDORING.md](VENDORING.md).

```text
TS ──► ts::PesAssembler ──► pes::parse ──► decoder::Decoder ──► model::Caption
                                                                    │
                                             render::vtt (live) ◄────┤
                                        render::ass (recordings) ◄───┤
                                       render::json (a renderer  ◄───┤
                                        in another process)          │
                          render::bitmap (not yet implemented) ◄─────┘
```

`model::Caption` is the contract: positions, sizes, colours, ruby flags and
DRCS glyphs — everything the broadcast sent. Each renderer then decides how
much of that to keep. WebVTT keeps text and timing (a subtitle track a
browser can toggle), ASS keeps the placement as well (a sidecar beside a
recording), JSON keeps all of it by rendering none of it — the model itself,
for a consumer that will draw the caption somewhere this process cannot reach
— and a pixel renderer keeps all of it as pixels.

The crate has no C dependencies or native bindings. Its one dependency
beyond ordinary crates.io crates is the sibling crate
[`tuner-codec`](https://github.com/pelvo/tuner-codec), which supplies the
shared MPEG-TS/PES engine.

## Layout

```text
crates/arib-caption/
├── Cargo.toml
├── README.md
├── LICENSE-MIT
├── LICENSE-APACHE
├── CREDITS.md                 # Upstream projects and standards cited
├── VENDORING.md               # Upstream provenance and update notes
├── scripts/gen_tables.py      # Regenerates checked-in character tables
├── src/
│   ├── lib.rs                # Public decoder/model/render API
│   ├── bin/arib-caption.rs   # Streaming command-line tool
│   ├── ts.rs                 # TS packet splitting and service discovery
│   ├── shared_pes_adapter.rs # Adapter over tuner-codec's PES engine
│   ├── pes.rs                # ARIB caption data groups and data units
│   ├── decoder.rs            # Statement-body state machine
│   ├── model.rs              # Lossless caption model
│   ├── b24/
│   │   ├── mod.rs            # B24 coded-character-set boundary
│   │   ├── charset.rs        # Graphic-set decoding
│   │   ├── codesets.rs       # Designation/invocation state
│   │   ├── controls.rs       # C0, C1, and CSI controls
│   │   └── tables.rs         # Generated mapping tables
│   └── render/
│       ├── mod.rs            # Renderer exports
│       ├── timeline.rs       # Shared cue lifetime resolution
│       ├── vtt.rs            # WebVTT output
│       ├── ass.rs            # Positioned ASS output
│       └── json.rs           # Full-model JSON output
└── tests/
    ├── broadcast.rs          # Decoder-behavior regressions
    ├── pes_engines.rs        # Shared PES-engine parity golden
    ├── support_selftest.rs   # Self-tests for the fixture builders below
    └── support/              # Synthesizes the ARIB streams the tests read
        ├── mod.rs
        ├── b24.rs            # ARIB STD-B24 control-code and statement helpers
        ├── fixtures.rs       # The four caption/superimpose/PSI/DRCS streams
        └── ts.rs             # TS/PES/PSI framing
```

Data flows inward from `ts` through `pes` and `decoder` into `model`. Renderers
consume the model but never alter parsing or decoding. This separation lets a
consumer choose text-only, positioned, or lossless output without maintaining
another caption decoder.

## Status

Implemented:

- `ts` — enough MPEG-TS to work from a service stream: PES reassembly for one
  PID, PSI section reassembly across packets, and caption/superimpose PID
  discovery from the PMT's component tag.
- `pes` — the independent-PES layer: data group (management / statement),
  language info, writing format, TMD and OTM/STM timecodes, and the data unit
  loop (statement body, DRCS, bitmap, …).
- `decoder` — the statement-body state machine: graphic set designation and
  invocation, C0 / C1 / CSI control codes, the colour table, character sizes,
  DRCS glyph definition, positioning, and region / ruby derivation.
- `b24` — the coded character set: code sets, control codes, the CLUT, and the
  conversion to Unicode including the ARIB additional symbols and their PUA
  aliases.
- `model` — what all of the above produces.
- `render::timeline` — when a caption ends, which most of them do not say.
  Shared by every timed renderer, because two copies of it would drift.
- `render::vtt` — text and timing, as a file or as one HLS segment.
- `render::ass` — position, colour, cell size and ruby as well: `PlayRes` is
  the caption plane, so the model's coordinates are written out unchanged.
  DRCS glyphs are drawn as vector outlines, one rectangle per run of set
  pixels, so a character no font has still appears.
- `render::json` — no rendering at all: `model::Caption` serialised whole, DRCS
  bitmaps base64'd exactly as transmitted, defaults omitted.

Not yet:

- `render::bitmap` — every pixel, for a rendition that needs no player support
  at all. ASS now covers what it was mainly wanted for.
- DRCS glyph → Unicode replacement (a table keyed by the glyph's MD5). It still
  matters for the *text* forms, where a DRCS character reads as 〓;
  `arib-caption drcs` prints each glyph so a table can be built from what a
  stream actually sends.
- Enclosures (ARIB's ruled boxes around a cell) are in the model. The ASS
  renderer does not draw them; `render::json` passes them on.

## Using it

Build the CLI with `cargo build --release`; the binary lands at
`target/release/arib-caption`.

```bash
# what caption streams does this service carry?
arib-caption pids < recording.ts

# the captions themselves
arib-caption text < recording.ts

# a sidecar for a recording: words only, or words where they were sent
arib-caption vtt < recording.ts > recording.vtt
arib-caption ass < recording.ts > recording.ass

# the live form: one JSON cue per line, flushed as it is known. --regions adds
# the whole caption model to each line — cells, colours, sizes, DRCS bitmaps —
# for a consumer that will draw the caption itself rather than show the words.
arib-caption cues --regions < stream.ts

# the glyphs this stream defined for itself, drawn
arib-caption drcs < recording.ts

# and the structure underneath, for when text comes out empty
arib-caption dump --pid 0x130 --limit 20 < recording.ts
```

A sidecar's times have to be the player's, and a caption's PTS is not: it is
hours into the broadcast day. `--anchor` decides what zero means — by default
the earliest audio/video PTS in the file, which is how a demuxer decides it
too. `--anchor raw` keeps broadcast PTS, which is what the live `cues` form
needs, since there `X-TIMESTAMP-MAP` does the reconciling instead.

`ass` wants a rounded gothic, as ARIB specifies and a television draws. The
default is `Rounded Mplus 1c` — note the name: Google Fonts lists that family
as *M PLUS Rounded 1c*, but the installed `.ttf` says `Rounded Mplus 1c`, and
fontconfig matches the latter. `--font` overrides it.

```text
$ arib-caption text < recording.ts
03:43:47.175 +until next 960x540 regions=1 [clear] 国内アーティスト楽曲の多くが➡
03:43:49.243 +until next 960x540 regions=1 [clear] アニメ関連。
03:43:51.379 +2.3s 960x540 regions=2 [clear] いまやアニソンは…
03:44:06.194 +until next 960x540 regions=2 [clear ruby=1] 「MUSIC AWARDS JAPAN ２０２６」。

$ arib-caption dump < recording.ts
pts=13162.043 seq=A mgmt tmd=Free langs=[jpn(id=1 fmt=8 tcs=0 dmf=0xa)] units=-
pts=13162.177 seq=A stmt lang=1 tmd=Free units=body(1),body(92)
```

`+until next` is the common case and the reason a renderer cannot emit a cue
the moment it decodes one: most ARIB captions have no end time and run until
the next caption replaces them.

### As a library

The CLI is a thin wrapper over the same public API:

```rust
use arib_caption::{CaptionKind, Decoder, Options};
use arib_caption::ts::{PacketSplitter, PesAssembler};

let mut splitter = PacketSplitter::new();
let mut pes = PesAssembler::new(0x0130); // the caption PID, from the PMT
let mut decoder = Decoder::new(CaptionKind::Caption, Options::default());

splitter.feed(&transport_stream_bytes);
while let Some(packet) = splitter.next_packet() {
    if let Some(pes_packet) = pes.push(&packet) {
        if let Ok(Some(caption)) = decoder.decode(&pes_packet.payload, pes_packet.pts_ms()) {
            // caption.text, caption.regions, caption.drcs, …
        }
    }
}
```

## Tests

```bash
cargo test -p arib-caption
cargo doc -p arib-caption --no-deps
```

This repository ships no broadcast captures. The four transport streams the
integration tests read (caption, superimpose, PSI, DRCS) are not files under
`tests/`; they are built at test time by `tests/support/fixtures.rs` from
explicit ARIB STD-B24 control-code sequences, and are designed to exercise
the same decoder behaviours the real off-air recordings they replaced were
originally chosen to cover: DRCS glyph handling, ruby, positioning/size
control codes, and the superimpose (`private_stream_2`, no PTS) path.

The former second gate for the vendored PES engine was removed when that
duplicate engine was deleted. `tests/pes_engines.rs` now carries a 70-record
self-golden of the shared engine over the synthesized caption stream: it
still guards against behavioural drift, but — unlike the 199-record golden
it replaces, which was captured from the retired engine against a real
broadcast and has been deleted along with that broadcast fixture — it no
longer attests to parity with the engine it replaced. See Test coverage,
below.

### Test coverage

Where a real off-air recording exercised a decoder behaviour the synthetic
streams cannot reproduce faithfully, the corresponding assertion was dropped
rather than weakened. This was measured directly rather than assumed: before
the real fixtures were deleted, a one-time coverage tool (since removed,
along with the recordings it read) decoded both stream sets side by side and
profiled twelve decoder dimensions (PES packets, management and
statement groups, captions, regions, ruby regions, DRCS glyphs, clear
screens, explicit durations, half-width and quarter-size cells, additional
symbols) that had to be non-zero on the synthetic side wherever they were
non-zero on the real side — the synthetic set covers all twelve. Four more
dimensions were added and held to the same bar, because a scalar count alone
had already been shown insufficient to catch a real defect on this crate:
regions that mix cell widths within one region, DRCS packed bytes that are
not `0x00`/`0xFF` (the property that makes a glyph sensitive to pixel bit
order at all), and characters with a non-default foreground or background
colour — the synthetic set covers all four as well.

What the synthetic set does not cover, kept here rather than silently
dropped:

- **Single-byte kana graphic sets.** The real captures designated hiragana
  and katakana into G2 and reached them through GR and SS2. Every Japanese
  character in the synthetic set is written in the two-byte kanji plane;
  `b24::charset`'s own unit tests are the only coverage of `resolve_single`.
- **Display-mode conditions in management data.** The real streams sent
  `DMF = 0xA`; the synthetic ones send `DMF = 0`. `LanguageInfo`'s
  `display_condition` branch — set only when `dmf` is `0b1100..=0b1110`
  (`pes.rs`) — has no test coverage at all, at any level: no fixture, real
  or synthetic, ever set a `DMF` in that range, and no unit test constructs
  one either. Closing this is follow-up work, not a claim this crate makes
  today.
- **The real DRCS glyph and its MD5.** The recording defined 逢 as a 36×36
  bitmap; that specific hash is gone. The property the check exists for —
  that the hash is taken over the bytes as transmitted, not the unpacked
  form — is kept against a synthetic stencil of the same shape instead.
- **Irregular broadcast timing.** Real statements arrived at uneven
  intervals with management data interleaved between them; the synthetic
  stream sends one management group and one statement on a fixed 2 s grid.
  `pes_engines.rs`'s hand-built continuity and discontinuity cases still
  cover the reassembly mechanism itself.
- **The incumbent-engine golden.** The deleted 199-record golden attested to
  parity with the PES engine this crate retired. That attestation cannot be
  reconstructed from a synthetic stream and is not claimed by the golden
  that replaced it.
- **`WHF` as a direct colour code, and `COL` shapes beyond the one real
  stream sent.** The colour gap above is closed for the shapes the real
  stream actually sent: 15 of its 34 colour-setting statements used `WHF`
  (white) rather than `YLF` (yellow) as the direct single-byte foreground
  code — a no-op against the CLUT default, but still a distinct control-code
  path (`decoder.rs`'s `BKF..=WHF` match) — while the synthetic script's one
  colour-carrying line uses only `YLF`. Likewise, every real `COL`
  occurrence used the same pair (palette-select to palette 4, then
  background-select to index 1), so that is the only `COL` shape the
  synthetic set encodes, and the only one
  `colour_controls_set_a_non_default_foreground_and_background` exercises.
  `COL`'s foreground-select form (`0x40 | index`, `index >= 8`) and every
  palette other than 4 have no test coverage at all, at any level — not
  even `decoder.rs`'s own unit tests, which construct the direct
  single-byte colour code but never a `COL` sequence. Closing this is
  follow-up work, not a claim this crate makes today.

## License

MIT OR Apache-2.0.

The licence chain, stated in full because it is not obvious from the SPDX
expression alone: [`xqq/libaribcaption`](https://github.com/xqq/libaribcaption)
is MIT-licensed, with an additional ISC-form notice covering some files (the
character conversion tables this crate carries in `src/b24/tables.rs` are
one of them) — both notices are reproduced in full in
[LICENSE-MIT](LICENSE-MIT). MIT permits sublicensing, which is what makes
`MIT OR Apache-2.0` legitimate here. The ISC-form notice's own text never
uses the word "sublicense" — it grants "use, copy, modify, and distribute
… for any purpose, with or without fee". That grant is permissive in the
same way MIT's is, places no further condition on redistribution, and is
conventionally treated as equivalent to MIT/BSD for relicensing purposes;
this fork proceeds on that basis for `tables.rs` too.
`DuckFeather10086/libaribcaption-rs`, the pure-Rust port this crate began as
a fork of, ships **no LICENSE file** of its own — see
[VENDORING.md](VENDORING.md). Its `Cargo.toml` does declare
`license = "MIT OR Apache-2.0"`, and its README states the same licence: an
informal but affirmative signal of licensing intent, even without a LICENSE
file to back it up. This fork does not rely on that signal, or on any other
grant from the intermediate repository; it carries xqq's original MIT
copyright and permission notice forward unchanged (see
[LICENSE-MIT](LICENSE-MIT)) and adds [LICENSE-APACHE](LICENSE-APACHE) as the
second half of the dual licence.
