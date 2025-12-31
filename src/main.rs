#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use rusticle::{Filter, Gif, OptLevel, QualityMetrics};
use std::env;
use std::fs;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: rusticle <operation> <input.gif> [output.gif]");
        eprintln!("Operations: resize, optimize, lossy, all, quality");
        eprintln!("");
        eprintln!("quality: rusticle quality <original.gif> <processed.gif>");
        std::process::exit(1);
    }

    let op = &args[1];
    
    // Quality comparison mode
    if op == "quality" {
        if args.len() != 4 {
            eprintln!("Usage: rusticle quality <original.gif> <processed.gif>");
            std::process::exit(1);
        }
        return compare_quality(&args[2], &args[3]);
    }

    let input = &args[2];
    let output = args.get(3).map(|s| s.as_str());

    // Read file
    let start = Instant::now();
    let data = fs::read(input)?;
    let read_time = start.elapsed();

    // Decode
    let start = Instant::now();
    let gif = Gif::from_bytes(&data)?;
    let decode_time = start.elapsed();

    println!(
        "Input: {}x{}, {} frames, {:.2} MB",
        gif.width,
        gif.height,
        gif.frames.len(),
        data.len() as f64 / 1_000_000.0
    );
    println!("Read: {:?}", read_time);
    println!("Decode: {:?}", decode_time);

    // Process based on operation
    let start = Instant::now();
    let processed = match op.as_str() {
        "resize" => gif.resize(320, 240, Filter::Lanczos3)?,
        "optimize" => gif.optimize(OptLevel::O3),
        "lossy" => gif.lossy(80),
        "all" => gif
            .resize(320, 240, Filter::Lanczos3)?
            .optimize(OptLevel::O3)
            .lossy(80),
        _ => {
            eprintln!("Unknown operation: {}", op);
            std::process::exit(1);
        }
    };
    let process_time = start.elapsed();
    println!("Process ({}): {:?}", op, process_time);

    // Encode
    let start = Instant::now();
    let encoded = processed.to_bytes()?;
    let encode_time = start.elapsed();
    println!("Encode: {:?}", encode_time);
    println!(
        "Output: {:.2} MB ({:.1}% of original)",
        encoded.len() as f64 / 1_000_000.0,
        encoded.len() as f64 / data.len() as f64 * 100.0
    );

    // Write if output specified
    if let Some(out_path) = output {
        let start = Instant::now();
        fs::write(out_path, &encoded)?;
        println!("Write: {:?}", start.elapsed());
    }

    println!(
        "Total: {:?}",
        read_time + decode_time + process_time + encode_time
    );

    Ok(())
}

fn compare_quality(original_path: &str, processed_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let original_data = fs::read(original_path)?;
    let processed_data = fs::read(processed_path)?;
    
    let original = Gif::from_bytes(&original_data)?;
    let processed = Gif::from_bytes(&processed_data)?;
    
    println!("Original:  {}x{}, {} frames, {:.2} MB", 
        original.width, original.height, original.frames.len(),
        original_data.len() as f64 / 1_000_000.0);
    println!("Processed: {}x{}, {} frames, {:.2} MB", 
        processed.width, processed.height, processed.frames.len(),
        processed_data.len() as f64 / 1_000_000.0);
    println!();
    
    // Need same frame count
    if original.frames.len() != processed.frames.len() {
        eprintln!("Warning: frame count differs ({} vs {})", 
            original.frames.len(), processed.frames.len());
    }
    
    let frame_count = original.frames.len().min(processed.frames.len());
    
    // If dimensions differ, resize original to match for fair comparison
    let original = if original.width != processed.width || original.height != processed.height {
        println!("Resizing original to {}x{} for comparison...", processed.width, processed.height);
        original.resize(processed.width as u32, processed.height as u32, Filter::Lanczos3)?
    } else {
        original
    };
    
    let mut total_psnr = 0.0;
    let mut total_ssim = 0.0;
    let mut total_dist = 0.0;
    let mut worst_psnr: f64 = f64::INFINITY;
    let mut worst_ssim: f64 = 1.0;
    let mut good_frames = 0;
    let mut excellent_frames = 0;
    
    for i in 0..frame_count {
        let orig_frame = &original.frames[i];
        let proc_frame = &processed.frames[i];
        
        let metrics = QualityMetrics::compare(&orig_frame.pixels, &proc_frame.pixels);
        
        total_psnr += metrics.psnr.min(100.0); // Cap infinite PSNR
        total_ssim += metrics.ssim;
        total_dist += metrics.mean_color_distance;
        worst_psnr = worst_psnr.min(metrics.psnr);
        worst_ssim = worst_ssim.min(metrics.ssim);
        
        if metrics.is_good() { good_frames += 1; }
        if metrics.is_excellent() { excellent_frames += 1; }
        
        // Show first 3 and last frame
        if i < 3 || i == frame_count - 1 {
            println!("Frame {:3}: PSNR={:5.1}dB SSIM={:.4} Dist={:.1}", 
                i, metrics.psnr.min(99.9), metrics.ssim, metrics.mean_color_distance);
        } else if i == 3 {
            println!("...");
        }
    }
    
    println!();
    println!("=== QUALITY SUMMARY ({} frames) ===", frame_count);
    println!("Avg PSNR:  {:5.2} dB", total_psnr / frame_count as f64);
    println!("Avg SSIM:  {:.4}", total_ssim / frame_count as f64);
    println!("Avg Dist:  {:.2}", total_dist / frame_count as f64);
    println!("Worst PSNR: {:5.2} dB", worst_psnr.min(99.9));
    println!("Worst SSIM: {:.4}", worst_ssim);
    println!();
    println!("Quality ratings:");
    println!("  Excellent (PSNR≥40, SSIM≥0.95): {} frames ({:.0}%)", 
        excellent_frames, excellent_frames as f64 / frame_count as f64 * 100.0);
    println!("  Good (PSNR≥30, SSIM≥0.90):      {} frames ({:.0}%)", 
        good_frames, good_frames as f64 / frame_count as f64 * 100.0);
    
    // Overall verdict
    let avg_ssim = total_ssim / frame_count as f64;
    let avg_psnr = total_psnr / frame_count as f64;
    println!();
    if avg_psnr >= 40.0 && avg_ssim >= 0.95 {
        println!("VERDICT: EXCELLENT quality");
    } else if avg_psnr >= 30.0 && avg_ssim >= 0.90 {
        println!("VERDICT: GOOD quality");
    } else if avg_psnr >= 25.0 && avg_ssim >= 0.80 {
        println!("VERDICT: ACCEPTABLE quality");
    } else {
        println!("VERDICT: POOR quality - consider different settings");
    }
    
    Ok(())
}
