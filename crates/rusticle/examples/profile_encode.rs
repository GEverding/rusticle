//! Profile GIF encoding to measure time spent in each stage.
//!
//! Loads sample GIFs and measures:
//! - LUT construction time
//! - Per-frame quantization time (fast path vs imagequant)
//! - Per-frame write time (LZW encoding)
//! - Total encode time
//!
//! Run with: cargo run --example profile_encode --release

use std::fs;
use std::path::Path;
use std::time::Instant;

use rusticle::Gif;

fn main() {
    println!("=== GIF Encoding Profile ===\n");

    // Find all sample GIFs
    let sample_dir = "samples";
    let mut gifs = Vec::new();

    if let Ok(entries) = fs::read_dir(sample_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "gif") {
                gifs.push(path);
            }
        }
    }

    if gifs.is_empty() {
        eprintln!("No sample GIFs found in {}/", sample_dir);
        eprintln!("Run: python scripts/download_test_gifs.py");
        return;
    }

    gifs.sort();

    // Group by content type
    let mut by_type: std::collections::BTreeMap<&str, Vec<_>> = std::collections::BTreeMap::new();
    for gif_path in &gifs {
        let name = gif_path.file_name().unwrap().to_string_lossy();
        let content_type = if name.starts_with("cartoon") {
            "cartoon"
        } else if name.starts_with("photo") {
            "photo"
        } else if name.starts_with("pixel_art") {
            "pixel_art"
        } else if name.starts_with("transparent") {
            "transparent"
        } else {
            "other"
        };
        by_type.entry(content_type).or_default().push(gif_path);
    }

    // Profile each content type
    for (content_type, paths) in by_type {
        println!("=== {} ===", content_type.to_uppercase());

        for gif_path in paths {
            profile_gif(gif_path);
        }
        println!();
    }
}

fn profile_gif(path: &Path) {
    let name = path.file_name().unwrap().to_string_lossy();

    // Load GIF
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to read {}: {}", name, e);
            return;
        }
    };

    let gif = match Gif::from_bytes(&data) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Failed to decode {}: {}", name, e);
            return;
        }
    };

    let frame_count = gif.frames.len();
    let width = gif.width;
    let height = gif.height;
    let has_palette = gif.original_palette.is_some();

    // Encode with stats
    let start = Instant::now();
    let (encoded_bytes, stats) = match gif.to_bytes_with_stats() {
        Ok((b, s)) => (b, s),
        Err(e) => {
            eprintln!("Failed to encode {}: {}", name, e);
            return;
        }
    };
    let total_elapsed = start.elapsed();

    // Calculate metrics
    let lut_ms = stats.lut_build_ns as f64 / 1_000_000.0;
    let quantize_ms = stats.quantize_ns as f64 / 1_000_000.0;
    let write_ms = stats.write_ns as f64 / 1_000_000.0;
    let total_ms = total_elapsed.as_secs_f64() * 1000.0;

    let quantize_per_frame = if frame_count > 0 {
        quantize_ms / frame_count as f64
    } else {
        0.0
    };
    let write_per_frame = if frame_count > 0 {
        write_ms / frame_count as f64
    } else {
        0.0
    };

    let output_size_kb = encoded_bytes.len() as f64 / 1024.0;

    // Calculate percentages
    let total_work_ns = stats.lut_build_ns + stats.quantize_ns + stats.write_ns;
    let lut_pct = if total_work_ns > 0 {
        (stats.lut_build_ns as f64 / total_work_ns as f64) * 100.0
    } else {
        0.0
    };
    let quantize_pct = if total_work_ns > 0 {
        (stats.quantize_ns as f64 / total_work_ns as f64) * 100.0
    } else {
        0.0
    };
    let write_pct = if total_work_ns > 0 {
        (stats.write_ns as f64 / total_work_ns as f64) * 100.0
    } else {
        0.0
    };

    // Print results
    println!("{}", name);
    println!("  Dimensions:       {}x{}", width, height);
    println!("  Frames:           {}", frame_count);
    println!(
        "  Has palette:      {}",
        if has_palette { "yes" } else { "no" }
    );
    println!();
    println!("  LUT build:        {:.2}ms ({:.1}%)", lut_ms, lut_pct);
    println!(
        "  Quantization:     {:.2}ms ({:.1}%) - {:.3}ms/frame avg",
        quantize_ms, quantize_pct, quantize_per_frame
    );
    println!(
        "    - Fast path:    {} frames",
        stats.quantize_fast_path_count
    );
    println!(
        "    - imagequant:   {} frames",
        stats.quantize_imagequant_count
    );
    println!(
        "  Frame writes:     {:.2}ms ({:.1}%) - {:.3}ms/frame avg",
        write_ms, write_pct, write_per_frame
    );
    println!("  Total encode:     {:.2}ms", total_ms);
    println!("  Output size:      {:.1}KB", output_size_kb);
    println!();
}
