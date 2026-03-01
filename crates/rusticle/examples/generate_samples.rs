//! Generate sample outputs for visual comparison.
//!
//! Run with: cargo run --release --example generate_samples

use rusticle::{Filter, Gif, OptLevel};
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let samples_dir = Path::new("samples");
    fs::create_dir_all(samples_dir)?;

    // Test files to process
    let test_files = [
        ("test_gifs/benchmark_suite/cartoon_01.gif", "cartoon"),
        ("test_gifs/benchmark_suite/photo_01.gif", "photo"),
        ("test_gifs/benchmark_suite/pixel_art_01.gif", "pixel_art"),
        (
            "test_gifs/benchmark_suite/transparent_01.gif",
            "transparent",
        ),
    ];

    for (input_path, name) in test_files {
        if !Path::new(input_path).exists() {
            println!("Skipping {} (not found)", input_path);
            continue;
        }

        println!("\n=== Processing {} ===", name);
        let data = fs::read(input_path)?;
        let original_size = data.len();
        let gif = Gif::from_bytes(&data)?;

        println!(
            "Original: {}x{}, {} frames, {} bytes",
            gif.width,
            gif.height,
            gif.frames.len(),
            original_size
        );

        // 1. Resize only (50% size)
        let target_w = gif.width / 2;
        let target_h = gif.height / 2;

        let resized = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)?;
        let resized_bytes = resized.to_bytes()?;
        let resized_path = samples_dir.join(format!("{}_1_resize_50pct.gif", name));
        fs::write(&resized_path, &resized_bytes)?;
        println!(
            "  1. resize 50%: {} bytes ({:.1}%)",
            resized_bytes.len(),
            100.0 * resized_bytes.len() as f64 / original_size as f64
        );

        // 2. Resize + Optimize O2
        let opt = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)?
            .optimize(OptLevel::O2);
        let opt_bytes = opt.to_bytes()?;
        let opt_path = samples_dir.join(format!("{}_2_resize_opt_o2.gif", name));
        fs::write(&opt_path, &opt_bytes)?;
        println!(
            "  2. resize+O2: {} bytes ({:.1}%)",
            opt_bytes.len(),
            100.0 * opt_bytes.len() as f64 / original_size as f64
        );

        // 3. Resize + Optimize O3
        let opt3 = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)?
            .optimize(OptLevel::O3);
        let opt3_bytes = opt3.to_bytes()?;
        let opt3_path = samples_dir.join(format!("{}_3_resize_opt_o3.gif", name));
        fs::write(&opt3_path, &opt3_bytes)?;
        println!(
            "  3. resize+O3: {} bytes ({:.1}%)",
            opt3_bytes.len(),
            100.0 * opt3_bytes.len() as f64 / original_size as f64
        );

        // 4. Resize + Lossy 80
        let lossy80 = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)?
            .lossy(80);
        let lossy80_bytes = lossy80.to_bytes()?;
        let lossy80_path = samples_dir.join(format!("{}_4_resize_lossy80.gif", name));
        fs::write(&lossy80_path, &lossy80_bytes)?;
        println!(
            "  4. resize+lossy80: {} bytes ({:.1}%)",
            lossy80_bytes.len(),
            100.0 * lossy80_bytes.len() as f64 / original_size as f64
        );

        // 5. Resize + Lossy 60
        let lossy60 = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)?
            .lossy(60);
        let lossy60_bytes = lossy60.to_bytes()?;
        let lossy60_path = samples_dir.join(format!("{}_5_resize_lossy60.gif", name));
        fs::write(&lossy60_path, &lossy60_bytes)?;
        println!(
            "  5. resize+lossy60: {} bytes ({:.1}%)",
            lossy60_bytes.len(),
            100.0 * lossy60_bytes.len() as f64 / original_size as f64
        );

        // 6. Full pipeline: Resize + O2 + Lossy 80
        let full = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)?
            .optimize(OptLevel::O2)
            .lossy(80);
        let full_bytes = full.to_bytes()?;
        let full_path = samples_dir.join(format!("{}_6_full_pipeline.gif", name));
        fs::write(&full_path, &full_bytes)?;
        println!(
            "  6. full (resize+O2+lossy80): {} bytes ({:.1}%)",
            full_bytes.len(),
            100.0 * full_bytes.len() as f64 / original_size as f64
        );

        // 7. Aggressive: Resize + O3 + Lossy 50
        let aggressive = gif
            .clone()
            .resize(target_w as u32, target_h as u32, Filter::Lanczos3)?
            .optimize(OptLevel::O3)
            .lossy(50);
        let aggressive_bytes = aggressive.to_bytes()?;
        let aggressive_path = samples_dir.join(format!("{}_7_aggressive.gif", name));
        fs::write(&aggressive_path, &aggressive_bytes)?;
        println!(
            "  7. aggressive (resize+O3+lossy50): {} bytes ({:.1}%)",
            aggressive_bytes.len(),
            100.0 * aggressive_bytes.len() as f64 / original_size as f64
        );
    }

    println!("\n=== Samples written to samples/ directory ===");
    println!("Files are numbered for easy comparison:");
    println!("  1 = resize only (baseline)");
    println!("  2 = resize + optimize O2");
    println!("  3 = resize + optimize O3");
    println!("  4 = resize + lossy 80");
    println!("  5 = resize + lossy 60");
    println!("  6 = full pipeline (resize + O2 + lossy 80)");
    println!("  7 = aggressive (resize + O3 + lossy 50)");

    Ok(())
}
