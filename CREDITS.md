# Credits

`arib-caption` builds on the work of the following projects and standards.

## xqq/libaribcaption

The B24 decoder, caption model, and renderer design in this crate trace back
to [`xqq/libaribcaption`](https://github.com/xqq/libaribcaption), licensed
under the MIT License.

- Copyright (c) 2022 magicxqq

The character conversion tables in `src/b24/tables.rs` carry an additional,
ISC-form permission notice from the same author. See
[LICENSE-MIT](LICENSE-MIT) for both notices in full.

## DuckFeather10086/libaribcaption-rs

This crate began as a fork of
[`DuckFeather10086/libaribcaption-rs`](https://github.com/DuckFeather10086/libaribcaption-rs),
a pure-Rust port of libaribcaption's decoder, vendored at revision `5485697`.
See [VENDORING.md](VENDORING.md) for the fork's provenance and local changes,
including the fact that upstream ships no LICENSE file of its own.

## Standards

- **ARIB STD-B24**: Data coding and transmission specification for digital
  broadcasting, published by the Association of Radio Industries and
  Businesses (ARIB). Defines the caption/superimpose data structures and the
  B24 coded character set this crate decodes.
