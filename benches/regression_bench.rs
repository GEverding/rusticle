//! Regression benchmark suite
//! 
//! Tracks speed, quality, and file size across commits.
//! Results are appended to bench_results.jsonl
//!
//! Run with: cargo run --release --bin bench

use rusticle::{Filter, Gif, OptLevel, QualityMetrics};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Serialize, Deserialize)]
struct BenchResult {
    commit_hash: String,
    commit_date: String,
    timestamp: String,
    test_file: String,
    operation: String,
    // Timing
    decode_ms: f64,
    process_ms: f64,
    encode_ms: f64,
    total_ms: f64,
    // Size
    input_bytes: usize,
    output_bytes: usize,
    compression_ratio: f64,
    // Quality (vs reference resize)
    avg_psnr: f64,
    avg_ssim: f64,
    worst_psnr: f64,
    worst_ssim: f64,
    frames: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct BenchSummary {
    commit_hash: String,
    commit_date: String,
    timestamp: String,
    results: Vec<BenchResult>,
    // Aggregates
    avg_speedup_vs_baseline: Option<f64>,
    avg_psnr: f64,
    avg_ssim: f64,
}

fn get_git_info() -> (String, String) {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    
    let date = Command::new("git")
        .args(["log", "-1", "--format=%ci"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    
    (hash, date)
}

fn bench_file(path: &Path, operation: &str) -> Option<BenchResult> {
    let (commit_hash, commit_date) = get_git_info();
    let timestamp = chrono::Utc::now().to_rfc3339();
    
    // Read and decode
    let data = fs::read(path).ok()?;
    let input_bytes = data.len();
    
    let decode_start = Instant::now();
    let gif = Gif::from_bytes(&data).ok()?;
    let decode_ms = decode_start.elapsed().as_secs_f64() * 1000.0;
    
    let frames = gif.frames.len();
    let _original_width = gif.width;
    let _original_height = gif.height;
    
    // Process
    let process_start = Instant::now();
    let processed = match operation {
        "resize" => gif.resize(320, 240, Filter::Lanczos3).ok()?,
        "optimize" => gif.optimize(OptLevel::O3),
        "lossy" => gif.lossy(80),
        "all" => gif
            .resize(320, 240, Filter::Lanczos3).ok()?
            .optimize(OptLevel::O3)
            .lossy(80),
        _ => return None,
    };
    let process_ms = process_start.elapsed().as_secs_f64() * 1000.0;
    
    // Encode
    let encode_start = Instant::now();
    let output = processed.to_bytes().ok()?;
    let encode_ms = encode_start.elapsed().as_secs_f64() * 1000.0;
    
    let output_bytes = output.len();
    let total_ms = decode_ms + process_ms + encode_ms;
    
    // Quality comparison - resize original to same dimensions for fair comparison
    let reference = Gif::from_bytes(&data).ok()?
        .resize(processed.width as u32, processed.height as u32, Filter::Lanczos3).ok()?;
    let processed_decoded = Gif::from_bytes(&output).ok()?;
    
    let mut total_psnr = 0.0;
    let mut total_ssim = 0.0;
    let mut worst_psnr = f64::INFINITY;
    let mut worst_ssim = 1.0f64;
    let frame_count = reference.frames.len().min(processed_decoded.frames.len());
    
    for i in 0..frame_count {
        let metrics = QualityMetrics::compare(
            &reference.frames[i].pixels,
            &processed_decoded.frames[i].pixels,
        );
        total_psnr += metrics.psnr.min(100.0);
        total_ssim += metrics.ssim;
        worst_psnr = worst_psnr.min(metrics.psnr);
        worst_ssim = worst_ssim.min(metrics.ssim);
    }
    
    Some(BenchResult {
        commit_hash,
        commit_date,
        timestamp,
        test_file: path.file_name()?.to_string_lossy().to_string(),
        operation: operation.to_string(),
        decode_ms,
        process_ms,
        encode_ms,
        total_ms,
        input_bytes,
        output_bytes,
        compression_ratio: output_bytes as f64 / input_bytes as f64,
        avg_psnr: total_psnr / frame_count as f64,
        avg_ssim: total_ssim / frame_count as f64,
        worst_psnr: worst_psnr.min(100.0),
        worst_ssim,
        frames,
    })
}

fn main() {
    let (commit_hash, commit_date) = get_git_info();
    let timestamp = chrono::Utc::now().to_rfc3339();
    
    println!("Rusticle Regression Benchmark");
    println!("Commit: {} ({})", commit_hash, commit_date);
    println!("========================================\n");
    
    let test_files = [
        "test_gifs/benchmark_suite/cartoon_01.gif",
        "test_gifs/benchmark_suite/photo_01.gif",
        "test_gifs/benchmark_suite/cartoon_02.gif",
    ];
    
    let operations = ["resize", "all"];
    
    let mut results = Vec::new();
    
    for file in &test_files {
        let path = Path::new(file);
        if !path.exists() {
            eprintln!("Skipping {}: not found", file);
            continue;
        }
        
        for op in &operations {
            print!("Benchmarking {} ({})... ", path.file_name().unwrap().to_string_lossy(), op);
            std::io::stdout().flush().unwrap();
            
            // Run 3 times, take median
            let mut runs: Vec<BenchResult> = (0..3)
                .filter_map(|_| bench_file(path, op))
                .collect();
            
            if runs.is_empty() {
                println!("FAILED");
                continue;
            }
            
            runs.sort_by(|a, b| a.total_ms.partial_cmp(&b.total_ms).unwrap());
            let median = runs.swap_remove(runs.len() / 2);
            
            println!("{:.1}ms, PSNR={:.1}dB, SSIM={:.4}, {:.1}%", 
                median.total_ms, 
                median.avg_psnr, 
                median.avg_ssim,
                median.compression_ratio * 100.0);
            
            results.push(median);
        }
    }
    
    // Summary
    let avg_psnr: f64 = results.iter().map(|r| r.avg_psnr).sum::<f64>() / results.len() as f64;
    let avg_ssim: f64 = results.iter().map(|r| r.avg_ssim).sum::<f64>() / results.len() as f64;
    
    println!("\n========================================");
    println!("SUMMARY");
    println!("  Tests: {}", results.len());
    println!("  Avg PSNR: {:.2} dB", avg_psnr);
    println!("  Avg SSIM: {:.4}", avg_ssim);
    
    // Save results
    let summary = BenchSummary {
        commit_hash: commit_hash.clone(),
        commit_date,
        timestamp,
        results,
        avg_speedup_vs_baseline: None,
        avg_psnr,
        avg_ssim,
    };
    
    let results_file = Path::new("bench_results.jsonl");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(results_file)
        .expect("Failed to open results file");
    
    writeln!(file, "{}", serde_json::to_string(&summary).unwrap()).unwrap();
    println!("\nResults appended to bench_results.jsonl");
    
    // Check for regression vs previous run
    if let Ok(f) = fs::File::open(results_file) {
        let reader = BufReader::new(f);
        let lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();
        
        if lines.len() >= 2 {
            let prev: BenchSummary = serde_json::from_str(&lines[lines.len() - 2]).unwrap();
            let psnr_diff = avg_psnr - prev.avg_psnr;
            let ssim_diff = avg_ssim - prev.avg_ssim;
            
            println!("\nVs previous ({}):", prev.commit_hash);
            println!("  PSNR: {:+.2} dB", psnr_diff);
            println!("  SSIM: {:+.4}", ssim_diff);
            
            if psnr_diff < -2.0 || ssim_diff < -0.01 {
                println!("\n⚠️  WARNING: Quality regression detected!");
                std::process::exit(1);
            }
        }
    }
}
