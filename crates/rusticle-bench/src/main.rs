//! Regression benchmark suite
//!
//! Tracks speed, quality, and file size across commits.
//! Results are appended to outputs/bench_results.jsonl by default.
//!
//! Run with: cargo run --release --bin bench

use rusticle::{Filter, Gif, OptLevel, QualityMetrics};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchResult {
    #[serde(default = "default_tool")]
    tool: String,
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

fn default_tool() -> String {
    "rusticle".to_string()
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

fn compute_quality_metrics(
    original_data: &[u8],
    output: &[u8],
    width: u32,
    height: u32,
) -> Option<(f64, f64, f64, f64)> {
    let reference = Gif::from_bytes(original_data)
        .ok()?
        .resize(width, height, Filter::Lanczos3)
        .ok()?;
    let processed_decoded = Gif::from_bytes(output).ok()?;

    let mut total_psnr = 0.0;
    let mut total_ssim = 0.0;
    let mut worst_psnr = f64::INFINITY;
    let mut worst_ssim = 1.0f64;
    let frame_count = reference.frames.len().min(processed_decoded.frames.len());
    if frame_count == 0 {
        return None;
    }

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

    Some((
        total_psnr / frame_count as f64,
        total_ssim / frame_count as f64,
        worst_psnr.min(100.0),
        worst_ssim,
    ))
}

fn bench_rusticle_file(path: &Path, operation: &str) -> Option<BenchResult> {
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
            .resize(320, 240, Filter::Lanczos3)
            .ok()?
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

    let (avg_psnr, avg_ssim, worst_psnr, worst_ssim) = compute_quality_metrics(
        &data,
        &output,
        processed.width as u32,
        processed.height as u32,
    )?;

    Some(BenchResult {
        tool: "rusticle".to_string(),
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
        avg_psnr,
        avg_ssim,
        worst_psnr,
        worst_ssim,
        frames,
    })
}

fn bench_gifsicle_file(path: &Path, operation: &str) -> Option<BenchResult> {
    let (commit_hash, commit_date) = get_git_info();
    let timestamp = chrono::Utc::now().to_rfc3339();

    let data = fs::read(path).ok()?;
    let input_bytes = data.len();
    let decoded = Gif::from_bytes(&data).ok()?;
    let frames = decoded.frames.len();

    let out_name = format!(
        "rusticle_bench_{}_{}_{}.gif",
        std::process::id(),
        operation,
        path.file_stem()?.to_string_lossy()
    );
    let out_path = std::env::temp_dir().join(out_name);

    let mut cmd = Command::new("gifsicle");
    match operation {
        "resize" => {
            cmd.args(["--resize", "320x240"]);
        }
        "all" => {
            cmd.args(["--resize", "320x240", "-O3", "--lossy=80"]);
        }
        _ => return None,
    }

    let start = Instant::now();
    let status = cmd.arg(path).arg("-o").arg(&out_path).status().ok()?;
    let total_ms = start.elapsed().as_secs_f64() * 1000.0;
    if !status.success() {
        return None;
    }

    let output = fs::read(&out_path).ok()?;
    let _ = fs::remove_file(&out_path);
    let output_bytes = output.len();

    let (avg_psnr, avg_ssim, worst_psnr, worst_ssim) =
        compute_quality_metrics(&data, &output, 320, 240)?;

    Some(BenchResult {
        tool: "gifsicle".to_string(),
        commit_hash,
        commit_date,
        timestamp,
        test_file: path.file_name()?.to_string_lossy().to_string(),
        operation: operation.to_string(),
        decode_ms: 0.0,
        process_ms: total_ms,
        encode_ms: 0.0,
        total_ms,
        input_bytes,
        output_bytes,
        compression_ratio: output_bytes as f64 / input_bytes as f64,
        avg_psnr,
        avg_ssim,
        worst_psnr,
        worst_ssim,
        frames,
    })
}

fn gifsicle_available() -> bool {
    Command::new("gifsicle")
        .arg("--version")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn main() {
    let (commit_hash, commit_date) = get_git_info();
    let timestamp = chrono::Utc::now().to_rfc3339();

    println!("Rusticle Regression Benchmark");
    println!("Commit: {} ({})", commit_hash, commit_date);
    println!("========================================\n");

    let has_gifsicle = gifsicle_available();
    if has_gifsicle {
        println!("gifsicle: available");
    } else {
        println!("gifsicle: NOT found (baseline comparison disabled)");
    }

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
            let file_name = path.file_name().unwrap().to_string_lossy();
            print!("Benchmarking {} ({})... ", file_name, op);
            std::io::stdout().flush().unwrap();

            let mut rust_runs: Vec<BenchResult> = (0..3)
                .filter_map(|_| bench_rusticle_file(path, op))
                .collect();
            if rust_runs.is_empty() {
                println!("FAILED (rusticle)");
                continue;
            }
            rust_runs.sort_by(|a, b| a.total_ms.partial_cmp(&b.total_ms).unwrap());
            let rust_median = rust_runs.swap_remove(rust_runs.len() / 2);

            print!(
                "rusticle {:.1}ms {:.1}% ",
                rust_median.total_ms,
                rust_median.compression_ratio * 100.0
            );
            results.push(rust_median.clone());

            if has_gifsicle {
                let mut gif_runs: Vec<BenchResult> = (0..3)
                    .filter_map(|_| bench_gifsicle_file(path, op))
                    .collect();
                if gif_runs.is_empty() {
                    println!("| gifsicle FAILED");
                    continue;
                }
                gif_runs.sort_by(|a, b| a.total_ms.partial_cmp(&b.total_ms).unwrap());
                let gif_median = gif_runs.swap_remove(gif_runs.len() / 2);

                let speedup = gif_median.total_ms / rust_median.total_ms;
                println!(
                    "| gifsicle {:.1}ms {:.1}% | speedup {:.2}x",
                    gif_median.total_ms,
                    gif_median.compression_ratio * 100.0,
                    speedup
                );
                results.push(gif_median);
            } else {
                println!();
            }
        }
    }

    let rust_results: Vec<&BenchResult> = results.iter().filter(|r| r.tool == "rusticle").collect();
    let gif_results: Vec<&BenchResult> = results.iter().filter(|r| r.tool == "gifsicle").collect();

    if rust_results.is_empty() {
        eprintln!("No benchmark results produced");
        std::process::exit(1);
    }

    // Summary
    let avg_psnr: f64 =
        rust_results.iter().map(|r| r.avg_psnr).sum::<f64>() / rust_results.len() as f64;
    let avg_ssim: f64 =
        rust_results.iter().map(|r| r.avg_ssim).sum::<f64>() / rust_results.len() as f64;

    let mut speedups = Vec::new();
    if !gif_results.is_empty() {
        let mut rust_map: HashMap<(String, String), f64> = HashMap::new();
        for r in &rust_results {
            rust_map.insert((r.test_file.clone(), r.operation.clone()), r.total_ms);
        }
        for g in &gif_results {
            if let Some(r_ms) = rust_map.get(&(g.test_file.clone(), g.operation.clone())) {
                if *r_ms > 0.0 {
                    speedups.push(g.total_ms / r_ms);
                }
            }
        }
    }
    let avg_speedup_vs_baseline = if speedups.is_empty() {
        None
    } else {
        Some(speedups.iter().sum::<f64>() / speedups.len() as f64)
    };

    println!("\n========================================");
    println!("SUMMARY");
    println!("  Rusticle tests: {}", rust_results.len());
    if !gif_results.is_empty() {
        println!("  Gifsicle tests: {}", gif_results.len());
    }
    println!("  Avg PSNR: {:.2} dB", avg_psnr);
    println!("  Avg SSIM: {:.4}", avg_ssim);
    if let Some(speedup) = avg_speedup_vs_baseline {
        println!("  Avg speedup vs gifsicle: {:.2}x", speedup);
    }

    // Save results
    let summary = BenchSummary {
        commit_hash: commit_hash.clone(),
        commit_date,
        timestamp,
        results,
        avg_speedup_vs_baseline,
        avg_psnr,
        avg_ssim,
    };

    let results_file_path = std::env::var("RUSTICLE_BENCH_RESULTS")
        .unwrap_or_else(|_| "outputs/bench_results.jsonl".to_string());
    let results_file = Path::new(&results_file_path);
    if let Some(parent) = results_file.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).expect("Failed to create benchmark output directory");
        }
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(results_file)
        .expect("Failed to open results file");

    writeln!(file, "{}", serde_json::to_string(&summary).unwrap()).unwrap();
    println!("\nResults appended to {}", results_file.display());

    // Check for regression vs previous run
    if let Ok(f) = fs::File::open(results_file) {
        let reader = BufReader::new(f);
        let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();

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
