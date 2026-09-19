# Third-Party Notices & Attribution

`tigrs` is dual-licensed under the **MIT License** ([`LICENSE-MIT`](LICENSE-MIT)) and the **Apache License, Version 2.0** ([`LICENSE-APACHE`](LICENSE-APACHE)) (`MIT OR Apache-2.0`).

---

## 1. Rust Dependencies

`tigrs` is built in 100% safe Rust (`#![forbid(unsafe_code)]`) with zero C dependencies and links against pure-Rust crates from the Rust community under compatible open-source licenses:

- **MIT OR Apache-2.0**: `gix` (`gitoxide` suite), `gix-commitgraph`, `crossterm`, `clap`, `clap_complete`, `clap_mangen`, `syntect`, `two-face`, `rustix`, `signal-hook`, `rayon`, `crossbeam-channel`, `regex`, `thiserror`, `unicode-width`, `toml`, `toml_edit`, `serde`, `parking_lot`, `hashbrown`, `indexmap`, `smallvec`, `arrayvec`, `bitflags`, `itoa`, `memchr`, `notify`, `scopeguard`, `shlex`, `tempfile`.
- **MIT**: `clru`.
- **MPL-2.0**: `uluru` (transitive dependency via `gix-pack`).
- **Zlib**: `zlib-rs` (pure-Rust DEFLATE decompression via `gix`).
- **BSD-3-Clause**: `encoding_rs` (character encoding conversions).

---

## 2. Historical & Design Inspiration

`tigrs` is an independent, clean-room pure-Rust terminal user interface lightly inspired by the [tig](https://github.com/jonas/tig) project by Jonas Fonseca. No source code from `tig` or `git` is included in `tigrs`.
