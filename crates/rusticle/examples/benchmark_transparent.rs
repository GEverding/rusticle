use rusticle::Gif;
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Transparent GIF Compression Benchmark ===\n");

    let samples = vec![
        "samples/transparent_1_resize_50pct.gif",
        "samples/transparent_2_resize_opt_o2.gif",
        "samples/transparent_3_resize_opt_o3.gif",
        "samples/transparent_4_resize_lossy80.gif",
        "samples/transparent_5_resize_lossy60.gif",
        "samples/transparent_6_full_pipeline.gif",
        "samples/transparent_7_aggressive.gif",
    ];

    let mut total_original = 0u64;
    let mut total_reencoded = 0u64;

    for sample_path in samples {
        if !Path::new(sample_path).exists() {
            println!("⚠ {} not found, skipping", sample_path);
            continue;
        }

        let data = fs::read(sample_path)?;
        let original_size = data.len() as u64;

        // Decode and re-encode
        let gif = Gif::from_bytes(&data)?;
        let reencoded = gif.to_bytes()?;
        let reencoded_size = reencoded.len() as u64;

        let diff = (reencoded_size as i64) - (original_size as i64);
        let pct = if original_size > 0 {
            (diff as f64 / original_size as f64) * 100.0
        } else {
            0.0
        };

        println!(
            "{}",
            Path::new(sample_path)
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        println!("  Original:   {} bytes", original_size);
        println!("  Re-encoded: {} bytes", reencoded_size);
        println!("  Difference: {} bytes ({:+.2}%)", diff, pct);
        println!("  Frames:     {}", gif.frames.len());
        println!();

        total_original += original_size;
        total_reencoded += reencoded_size;
    }

    if total_original > 0 {
        let total_diff = (total_reencoded as i64) - (total_original as i64);
        let total_pct = (total_diff as f64 / total_original as f64) * 100.0;
        println!("=== TOTALS ===");
        println!("Original:   {} bytes", total_original);
        println!("Re-encoded: {} bytes", total_reencoded);
        println!("Difference: {} bytes ({:+.2}%)", total_diff, total_pct);
    }

    Ok(())
}
