//! Microbenchmark for the `update_alt_screen` ESC scanner.
//!
//! This is the hot path on every `Pane::feed()` call — every chunk of PTY
//! output produced by Claude Code, factory worker shells, and director panes
//! is scanned for DEC private-mode toggle sequences. Most chunks are dense
//! ASCII with no escape bytes at all, so the steady-state cost is dominated
//! by "how fast can we sweep past N bytes without an ESC".
//!
//! The benchmark exposes the public `Pane::update_alt_screen` function via a
//! tiny re-export helper to avoid needing access to private items from a
//! bench harness.
//!
//! Workloads:
//! - `esc_free_64k`     — 64 KiB of pure ASCII, no ESC. Best case for the
//!                        memchr fast-path; this is what should show the
//!                        biggest delta vs a byte-by-byte scanner.
//! - `sparse_esc_64k`   — 64 KiB of ASCII with one harmless ESC every 200
//!                        bytes that does NOT match the DEC pattern (so the
//!                        scanner does the minimal recovery work).
//! - `dense_match_4k`   — 4 KiB containing many real DEC 1049 h/l toggles,
//!                        exercising the matching path (not optimized by
//!                        memchr, but must not regress).

use std::hint::black_box;

use cas_mux::Pane;
use criterion::{Criterion, criterion_group, criterion_main};

fn make_esc_free(n: usize) -> Vec<u8> {
    // Repeating printable ASCII, no 0x1b.
    let pattern = b"Lorem ipsum dolor sit amet, consectetur adipiscing elit. ";
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        let take = pattern.len().min(n - v.len());
        v.extend_from_slice(&pattern[..take]);
    }
    debug_assert!(!v.contains(&0x1b));
    v
}

fn make_sparse_esc(n: usize, esc_every: usize) -> Vec<u8> {
    let mut v = make_esc_free(n);
    let mut i = esc_every;
    while i < v.len() {
        // ESC followed by a non-'[' byte -> scanner rejects after one extra check.
        v[i] = 0x1b;
        i += esc_every;
    }
    v
}

fn make_dense_matches(n: usize) -> Vec<u8> {
    let toggle: &[u8] = b"some output\x1b[?1049hmore stuff\x1b[?1049l";
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        let take = toggle.len().min(n - v.len());
        v.extend_from_slice(&toggle[..take]);
    }
    v
}

fn bench_alt_screen(c: &mut Criterion) {
    let esc_free = make_esc_free(64 * 1024);
    let sparse = make_sparse_esc(64 * 1024, 200);
    let dense = make_dense_matches(4 * 1024);

    c.bench_function("update_alt_screen/esc_free_64k", |b| {
        b.iter(|| {
            let out = Pane::update_alt_screen_for_bench(black_box(&esc_free), false);
            black_box(out);
        });
    });

    c.bench_function("update_alt_screen/sparse_esc_64k", |b| {
        b.iter(|| {
            let out = Pane::update_alt_screen_for_bench(black_box(&sparse), false);
            black_box(out);
        });
    });

    c.bench_function("update_alt_screen/dense_match_4k", |b| {
        b.iter(|| {
            let out = Pane::update_alt_screen_for_bench(black_box(&dense), false);
            black_box(out);
        });
    });
}

criterion_group!(benches, bench_alt_screen);
criterion_main!(benches);
