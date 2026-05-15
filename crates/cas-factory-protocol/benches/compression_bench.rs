//! Compression benchmarks for LZ4 compression on terminal output.
//!
//! Run with: `cargo bench -p cas-factory-protocol`

use cas_factory_protocol::compression::{compress, decompress};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

/// Generate typical Claude output with ANSI codes and text.
fn generate_claude_output(size: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(size);

    // Typical Claude response patterns
    let patterns = [
        // Bold text with color
        "\x1b[1;34m❯\x1b[0m ",
        // Normal text
        "Let me help you with that task. ",
        // Code block start
        "\x1b[38;5;243m```rust\x1b[0m\n",
        // Code content with syntax highlighting
        "\x1b[38;5;208mfn\x1b[0m \x1b[38;5;33mmain\x1b[0m() {\n",
        "    \x1b[38;5;208mprintln!\x1b[0m(\x1b[38;5;28m\"Hello, world!\"\x1b[0m);\n",
        "}\n",
        // Code block end
        "\x1b[38;5;243m```\x1b[0m\n",
        // Bullet points
        "\x1b[1m•\x1b[0m First item in the list\n",
        "\x1b[1m•\x1b[0m Second item with more text\n",
        "\x1b[1m•\x1b[0m Third item explaining something\n",
        // Status messages
        "\x1b[32m✓\x1b[0m Task completed successfully\n",
        "\x1b[33m⚠\x1b[0m Warning: Check the output\n",
    ];

    let mut pattern_idx = 0;
    while output.len() < size {
        let pattern = patterns[pattern_idx % patterns.len()];
        output.extend_from_slice(pattern.as_bytes());
        pattern_idx += 1;
    }

    output.truncate(size);
    output
}

/// Generate code-heavy output (syntax highlighted source code).
fn generate_code_output(size: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(size);

    let code_patterns = [
        "\x1b[38;5;208muse\x1b[0m std::collections::HashMap;\n",
        "\x1b[38;5;208muse\x1b[0m std::sync::Arc;\n",
        "\n",
        "\x1b[38;5;243m/// Documentation comment\x1b[0m\n",
        "\x1b[38;5;208mpub struct\x1b[0m \x1b[38;5;33mMyStruct\x1b[0m {\n",
        "    \x1b[38;5;243m/// Field documentation\x1b[0m\n",
        "    \x1b[38;5;208mpub\x1b[0m field: \x1b[38;5;33mString\x1b[0m,\n",
        "    \x1b[38;5;208mpub\x1b[0m count: \x1b[38;5;33musize\x1b[0m,\n",
        "}\n",
        "\n",
        "\x1b[38;5;208mimpl\x1b[0m \x1b[38;5;33mMyStruct\x1b[0m {\n",
        "    \x1b[38;5;208mpub fn\x1b[0m \x1b[38;5;33mnew\x1b[0m() -> \x1b[38;5;33mSelf\x1b[0m {\n",
        "        \x1b[38;5;33mSelf\x1b[0m {\n",
        "            field: \x1b[38;5;33mString\x1b[0m::new(),\n",
        "            count: \x1b[38;5;33m0\x1b[0m,\n",
        "        }\n",
        "    }\n",
        "}\n",
    ];

    let mut pattern_idx = 0;
    while output.len() < size {
        let pattern = code_patterns[pattern_idx % code_patterns.len()];
        output.extend_from_slice(pattern.as_bytes());
        pattern_idx += 1;
    }

    output.truncate(size);
    output
}

/// Generate sparse terminal output (lots of whitespace and cursor movement).
fn generate_sparse_output(size: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(size);

    let sparse_patterns = [
        // Cursor movement
        "\x1b[H",      // Move to home
        "\x1b[2J",     // Clear screen
        "\x1b[10;20H", // Move to row 10, col 20
        "Status: OK",
        "\x1b[K",     // Clear to end of line
        "          ", // Spaces
        "\n",
        "\x1b[5A",  // Move up 5 lines
        "\x1b[10C", // Move right 10 columns
        "Progress: [##########          ] 50%",
        "\r", // Carriage return
    ];

    let mut pattern_idx = 0;
    while output.len() < size {
        let pattern = sparse_patterns[pattern_idx % sparse_patterns.len()];
        output.extend_from_slice(pattern.as_bytes());
        pattern_idx += 1;
    }

    output.truncate(size);
    output
}

fn bench_compression_ratio(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression_ratio");

    // Test sizes typical for terminal output
    let sizes = [512, 1024, 4096, 16384, 65536];

    for size in sizes {
        // Claude output
        let claude_data = generate_claude_output(size);
        let compressed_claude = compress(&claude_data);
        let ratio_claude = 1.0 - (compressed_claude.len() as f64 / claude_data.len() as f64);
        println!(
            "Claude output {}B: {}B compressed ({:.1}% reduction)",
            size,
            compressed_claude.len(),
            ratio_claude * 100.0
        );

        // Code output
        let code_data = generate_code_output(size);
        let compressed_code = compress(&code_data);
        let ratio_code = 1.0 - (compressed_code.len() as f64 / code_data.len() as f64);
        println!(
            "Code output {}B: {}B compressed ({:.1}% reduction)",
            size,
            compressed_code.len(),
            ratio_code * 100.0
        );

        // Sparse output
        let sparse_data = generate_sparse_output(size);
        let compressed_sparse = compress(&sparse_data);
        let ratio_sparse = 1.0 - (compressed_sparse.len() as f64 / sparse_data.len() as f64);
        println!(
            "Sparse output {}B: {}B compressed ({:.1}% reduction)",
            size,
            compressed_sparse.len(),
            ratio_sparse * 100.0
        );

        // Verify ratios meet targets for larger data (small data near threshold compresses less)
        // LZ4 achieves better ratios with more data to work with
        if size >= 1024 {
            assert!(
                ratio_claude >= 0.5,
                "Claude output {}B should compress 50%+, got {:.1}%",
                size,
                ratio_claude * 100.0
            );
            assert!(
                ratio_code >= 0.5,
                "Code output {}B should compress 50%+, got {:.1}%",
                size,
                ratio_code * 100.0
            );
        }

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::new("claude", size), &claude_data, |b, data| {
            b.iter(|| compress(black_box(data)))
        });
        group.bench_with_input(BenchmarkId::new("code", size), &code_data, |b, data| {
            b.iter(|| compress(black_box(data)))
        });
        group.bench_with_input(BenchmarkId::new("sparse", size), &sparse_data, |b, data| {
            b.iter(|| compress(black_box(data)))
        });
    }

    group.finish();
}

fn bench_decompression_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("decompression_throughput");

    let sizes = [1024, 4096, 16384, 65536];

    for size in sizes {
        let claude_data = generate_claude_output(size);
        let compressed = compress(&claude_data);

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("decompress", size),
            &compressed,
            |b, data| b.iter(|| decompress(black_box(data)).unwrap()),
        );
    }

    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("roundtrip");

    let sizes = [1024, 4096, 16384];

    for size in sizes {
        let data = generate_claude_output(size);

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(
            BenchmarkId::new("compress_decompress", size),
            &data,
            |b, data| {
                b.iter(|| {
                    let compressed = compress(black_box(data));
                    let decompressed = decompress(black_box(&compressed)).unwrap();
                    black_box(decompressed.as_ref().len());
                })
            },
        );
    }

    group.finish();
}

fn bench_overhead(c: &mut Criterion) {
    // Verify <1ms overhead per message for typical sizes
    let mut group = c.benchmark_group("overhead_check");

    // Typical message sizes in WebSocket protocol
    let message_sizes = [256, 512, 1024, 2048, 4096];

    for size in message_sizes {
        let data = generate_claude_output(size);

        group.bench_with_input(
            BenchmarkId::new("compress_overhead", size),
            &data,
            |b, data| b.iter(|| compress(black_box(data))),
        );
    }

    group.finish();

    // Print message about overhead verification
    println!("\nOverhead check: Run 'cargo bench' and verify times are <1ms per operation");
}

criterion_group!(
    benches,
    bench_compression_ratio,
    bench_decompression_throughput,
    bench_roundtrip,
    bench_overhead,
);
criterion_main!(benches);
