// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Differential and Property Fuzzing Harness.
//!
//! Enforces:
//! - Shell quoting safety invariant: `shell_quote` output MUST ALWAYS satisfy `verify_shell_safety`.
//! - ANSI policy enforcement: `filter_sgr_only` strictly rejects OSC/CSI cursor-manipulation payloads.
//! - Truncation invariant: `visible_width(&truncate_visible_width(s, w)) <= w`.
//! - `Config::parse_toml` resilience: arbitrary malformed TOML inputs never panic or loop.

use std::collections::HashMap;
use std::fmt::Write as _;
use tigrs_core::Config;
use tigrs_core::ansi::{
    filter_sgr_only, strip_control_chars, truncate_visible_width, visible_width,
};
use tigrs_core::quote::{
    interpolate_command, shell_quote, verify_shell_argument_safety, verify_shell_safety,
};

/// Fast deterministic pseudo-random number generator (Xorshift64).
struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xdead_beef_cafe_babe
            } else {
                seed
            },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_range(&mut self, min: usize, max: usize) -> usize {
        if min >= max {
            return min;
        }
        let span = max - min;
        min + ((self.next_u64() as usize) % span)
    }

    /// Generates a randomized string containing printable characters, control chars,
    /// ANSI sequences, quotes, and newlines.
    fn random_string(&mut self, max_len: usize) -> String {
        let len = self.next_range(0, max_len);
        let mut s = String::with_capacity(len);
        let candidates = [
            'a', 'b', 'c', '1', '2', ' ', '-', '_', '/', '.', '\\', '\'', '"', '`', '$', ';', '&',
            '|', '<', '>', '(', ')', '\n', '\r', '\t', '\x1b', '[', ']', 'm', '0', '7', ';', '?',
            '\0',
        ];
        for _ in 0..len {
            let idx = (self.next_u64() as usize) % candidates.len();
            s.push(candidates[idx]);
        }
        s
    }
}

#[test]
fn test_fuzz_shell_quoting_and_safety_invariant() {
    let mut rng = SimpleRng::new(0x1337_c0de);

    for _ in 0..10_000 {
        let raw = rng.random_string(64);

        // Invariant 1: verify_shell_safety never panics on arbitrary input
        let _ = verify_shell_safety(&raw);

        // Invariant 2: verify_shell_argument_safety never panics
        let _ = verify_shell_argument_safety(&raw);

        // Invariant 3: ANY string quoted by shell_quote MUST pass verify_shell_safety!
        let quoted = shell_quote(&raw);
        assert!(
            verify_shell_safety(&quoted).is_ok(),
            "shell_quote output failed verify_shell_safety: raw={raw:?}, quoted={quoted:?}"
        );

        // Invariant 4: interpolate_command never panics on arbitrary templates
        let mut dict = HashMap::new();
        dict.insert("commit", raw.as_str());
        dict.insert("repo", quoted.as_str());
        let _ = interpolate_command(&raw, &dict);
    }
}

#[test]
fn test_fuzz_ansi_sanitization_and_width_invariants() {
    let mut rng = SimpleRng::new(0xcafe_babe);

    for _ in 0..10_000 {
        let raw = rng.random_string(80);

        // Invariant 1: strip_control_chars never panics and eliminates control characters
        let stripped = strip_control_chars(&raw);
        for c in stripped.chars() {
            if c != '\t' && c != '\n' {
                assert!(!c.is_control(), "Found unstripped control character {c:?}");
            }
        }

        // Invariant 2: filter_sgr_only never panics and permits only SGR sequences
        let filtered = filter_sgr_only(&raw);
        assert!(
            !filtered.contains("\x1b]"),
            "OSC sequence leaked into filtered output"
        );
        assert!(
            !filtered.contains("\x1bP"),
            "DCS sequence leaked into filtered output"
        );
        assert!(
            !filtered.contains("\x1b_"),
            "APC sequence leaked into filtered output"
        );
        assert!(
            !filtered.contains("\x1b^"),
            "PM sequence leaked into filtered output"
        );

        // Invariant 3: visible_width never panics
        let w = visible_width(&raw);
        let max_target = rng.next_range(0, w + 10);

        // Invariant 4: truncate_visible_width never exceeds target width
        let truncated = truncate_visible_width(&raw, max_target);
        let trunc_w = visible_width(&truncated);
        assert!(
            trunc_w <= max_target,
            "truncate_visible_width exceeded max_target: trunc_w={trunc_w}, max_target={max_target}"
        );
    }
}

#[test]
fn test_fuzz_config_toml_parser_resilience() {
    let mut rng = SimpleRng::new(0xdead_f00d);

    for _ in 0..2_000 {
        let num_lines = rng.next_range(1, 8);
        let mut file_content = String::new();

        for _ in 0..num_lines {
            let cmd_type = rng.next_range(0, 4);
            match cmd_type {
                0 => {
                    let var_name = rng.random_string(10);
                    let val_content = rng.random_string(20);
                    let _ = writeln!(file_content, "[general]\n{var_name} = {val_content:?}");
                }
                1 => {
                    let km = rng.random_string(8);
                    let key = rng.random_string(5);
                    let act = rng.random_string(15);
                    let _ = writeln!(file_content, "[keybindings.{km}]\n{key:?} = {act:?}");
                }
                2 => {
                    let area = rng.random_string(10);
                    let spec = rng.random_string(15);
                    let _ = writeln!(file_content, "[colors]\n{area:?} = {spec:?}");
                }
                _ => {
                    let garbage = rng.random_string(30);
                    let _ = writeln!(file_content, "{garbage}");
                }
            }
        }

        // Invariant: parser never panics on arbitrary randomized TOML syntax
        let _ = Config::parse_toml(&file_content);
    }
}
