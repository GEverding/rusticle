//! Profile the real-world pipeline: decode → resize → optimize → encode
//! This shows actual fast path usage vs imagequant.

use rusticle::{Filter, Gif, OptLevel};
use std::time::Instant;

fn main() {
    println!("=== Pipeline Profiling (Real-World Usage) ===\n");

    let test_files = [
        "test_gifs/benchmark_suite/cartoon_01.gif",
        "test_gifs/benchmark_suite/photo_01.gif",
        "test_gifs/benchmark_suite/pixel_art_01.gif",
        "test_gifs/benchmark_suite/transparent_01.gif",
    ];

    for path in &test_files {
        if !std::path::Path::new(path).exists() {
            println!("Skipping {} (not found)", path);
            continue;
        }

        let data = std::fs::read(path).unwrap();
        let gif = Gif::from_bytes(&data).unwrap();
        let name = path.split('/').last().unwrap();

        println!("=== {} ===", name);
        println!(
            "Original: {}x{}, {} frames",
            gif.width,
            gif.height,
            gif.frames.len()
        );

        // Pipeline 1: Just encode (no processing)
        let start = Instant::now();
        let (bytes, stats) = gif.clone().to_bytes_with_stats().unwrap();
        let time = start.elapsed();
        println!("\n1. Encode only:");
        println!(
            "   Fast path: {}, imagequant: {}",
            stats.quantize_fast_path_count, stats.quantize_imagequant_count
        );
        println!(
            "   Time: {:.1}ms, Size: {}KB",
            time.as_secs_f64() * 1000.0,
            bytes.len() / 1024
        );

        // Pipeline 2: Resize 50% → Encode
        let target_w = gif.width / 2;
        let target_h = gif.height / 2;
        let start = Instant::now();
        let resized = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)
            .unwrap();
        let resize_time = start.elapsed();

        let start = Instant::now();
        let (bytes, stats) = resized.to_bytes_with_stats().unwrap();
        let encode_time = start.elapsed();
        println!("\n2. Resize 50% → Encode:");
        println!(
            "   Fast path: {}, imagequant: {}",
            stats.quantize_fast_path_count, stats.quantize_imagequant_count
        );
        println!(
            "   Resize: {:.1}ms, Encode: {:.1}ms, Size: {}KB",
            resize_time.as_secs_f64() * 1000.0,
            encode_time.as_secs_f64() * 1000.0,
            bytes.len() / 1024
        );

        // Pipeline 3: Resize → Optimize O3 → Encode
        let start = Instant::now();
        let optimized = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)
            .unwrap()
            .optimize(OptLevel::O3);
        let opt_time = start.elapsed();

        let start = Instant::now();
        let (bytes, stats) = optimized.to_bytes_with_stats().unwrap();
        let encode_time = start.elapsed();
        println!("\n3. Resize → O3 → Encode:");
        println!(
            "   Fast path: {}, imagequant: {}",
            stats.quantize_fast_path_count, stats.quantize_imagequant_count
        );
        println!(
            "   Process: {:.1}ms, Encode: {:.1}ms, Size: {}KB",
            opt_time.as_secs_f64() * 1000.0,
            encode_time.as_secs_f64() * 1000.0,
            bytes.len() / 1024
        );

        // Pipeline 4: Resize → Lossy 80 → Encode
        let start = Instant::now();
        let lossy = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)
            .unwrap()
            .lossy(80);
        let lossy_time = start.elapsed();

        let start = Instant::now();
        let (bytes, stats) = lossy.to_bytes_with_stats().unwrap();
        let encode_time = start.elapsed();
        println!("\n4. Resize → Lossy 80 → Encode:");
        println!(
            "   Fast path: {}, imagequant: {}",
            stats.quantize_fast_path_count, stats.quantize_imagequant_count
        );
        println!(
            "   Process: {:.1}ms, Encode: {:.1}ms, Size: {}KB",
            lossy_time.as_secs_f64() * 1000.0,
            encode_time.as_secs_f64() * 1000.0,
            bytes.len() / 1024
        );

        println!("\n");
    }
}
