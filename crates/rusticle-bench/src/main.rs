//! Regression benchmark suite
//!
//! Tracks speed, quality, and file size across commits.
//! Results are appended to outputs/bench_results.jsonl by default.
//!
//! Run with: cargo run --release --bin bench

mod adaptive_comparison;
mod tiered_eval;

use rusticle::{Filter, Gif, OptLevel, QualityMetrics};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

type QualityResult = (f64, f64, f64, f64, Option<f64>, Option<f64>);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchResult {
    #[serde(default = "default_tool")]
    tool: String,
    commit_hash: String,
    commit_date: String,
    timestamp: String,
    test_file: String,
    operation: String,
    #[serde(default)]
    category: String,
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
    #[serde(default)]
    avg_butteraugli: Option<f64>,
    #[serde(default)]
    worst_butteraugli: Option<f64>,
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
    #[serde(default)]
    avg_butteraugli: Option<f64>,
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

fn get_bench_dimensions() -> (u32, u32) {
    let width = std::env::var("RUSTICLE_BENCH_WIDTH")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(320);
    let height = std::env::var("RUSTICLE_BENCH_HEIGHT")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(240);
    (width, height)
}

fn compute_quality_metrics(
    original_data: &[u8],
    output: &[u8],
    width: u32,
    height: u32,
) -> Option<QualityResult> {
    let reference = Gif::from_bytes(original_data)
        .ok()?
        .resize(width, height, Filter::Lanczos3)
        .ok()?;
    let processed_decoded = Gif::from_bytes(output).ok()?;

    let mut total_psnr = 0.0;
    let mut total_ssim = 0.0;
    let mut total_butteraugli = 0.0;
    let mut worst_psnr = f64::INFINITY;
    let mut worst_ssim = 1.0f64;
    let mut worst_butteraugli = 0.0f64;
    let mut butteraugli_count = 0;
    let frame_count = reference.frames.len().min(processed_decoded.frames.len());
    if frame_count == 0 {
        return None;
    }

    // Log frame count mismatch if present
    if reference.frames.len() != processed_decoded.frames.len() {
        eprintln!(
            "Warning: frame count mismatch (reference: {}, processed: {})",
            reference.frames.len(),
            processed_decoded.frames.len()
        );
    }

    for i in 0..frame_count {
        let metrics = QualityMetrics::compare_with_dimensions(
            &reference.frames[i].pixels,
            &processed_decoded.frames[i].pixels,
            width,
            height,
        );
        
        // Only accumulate metrics if comparison is valid
        if metrics.valid {
            total_psnr += metrics.psnr.min(100.0);
            total_ssim += metrics.ssim;
            worst_psnr = worst_psnr.min(metrics.psnr);
            worst_ssim = worst_ssim.min(metrics.ssim);

            if let Some(ba) = metrics.butteraugli {
                total_butteraugli += ba;
                worst_butteraugli = worst_butteraugli.max(ba);
                butteraugli_count += 1;
            }
        }
    }

    let avg_butteraugli = if butteraugli_count > 0 {
        Some(total_butteraugli / butteraugli_count as f64)
    } else {
        None
    };

    Some((
        total_psnr / frame_count as f64,
        total_ssim / frame_count as f64,
        worst_psnr.min(100.0),
        worst_ssim,
        avg_butteraugli,
        if butteraugli_count > 0 {
            Some(worst_butteraugli)
        } else {
            None
        },
    ))
}

fn bench_rusticle_file(path: &Path, operation: &str, category: &str) -> Option<BenchResult> {
    let (commit_hash, commit_date) = get_git_info();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let (bench_width, bench_height) = get_bench_dimensions();

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
        "resize" => gif
            .resize(bench_width, bench_height, Filter::Lanczos3)
            .ok()?,
        "optimize" => gif.optimize(OptLevel::O3),
        "lossy" => gif.lossy(80),
        "all" => gif
            .resize(bench_width, bench_height, Filter::Lanczos3)
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

    let (avg_psnr, avg_ssim, worst_psnr, worst_ssim, avg_butteraugli, worst_butteraugli) =
        compute_quality_metrics(
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
        category: category.to_string(),
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
        avg_butteraugli,
        worst_butteraugli,
        frames,
    })
}

fn bench_gifsicle_file(path: &Path, operation: &str, category: &str) -> Option<BenchResult> {
    let (commit_hash, commit_date) = get_git_info();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let (bench_width, bench_height) = get_bench_dimensions();

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

    let resize_arg = format!("{}x{}", bench_width, bench_height);
    let mut cmd = Command::new("gifsicle");
    match operation {
        "resize" => {
            cmd.args(["--resize", &resize_arg]);
        }
        "all" => {
            cmd.args(["--resize", &resize_arg, "-O3", "--lossy=80"]);
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

    let (avg_psnr, avg_ssim, worst_psnr, worst_ssim, avg_butteraugli, worst_butteraugli) =
        compute_quality_metrics(&data, &output, bench_width, bench_height)?;

    Some(BenchResult {
        tool: "gifsicle".to_string(),
        commit_hash,
        commit_date,
        timestamp,
        test_file: path.file_name()?.to_string_lossy().to_string(),
        operation: operation.to_string(),
        category: category.to_string(),
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
        avg_butteraugli,
        worst_butteraugli,
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

fn load_test_files() -> Vec<(String, String)> {
    let manifest_path = "test_gifs/benchmark_suite/manifest.json";

    // Try to load from manifest
    if let Ok(manifest_data) = fs::read_to_string(manifest_path) {
        if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&manifest_data) {
            if let Some(gifs) = manifest.get("gifs").and_then(|g| g.as_array()) {
                let mut files: Vec<(String, String)> = gifs
                    .iter()
                    .filter_map(|gif| {
                        // Only include successful entries
                        if gif.get("success").and_then(|s| s.as_bool()) == Some(true) {
                            let path = gif
                                .get("path")
                                .and_then(|p| p.as_str())
                                .map(|s| s.to_string())?;
                            let category = gif
                                .get("category")
                                .and_then(|c| c.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "uncategorized".to_string());
                            Some((path, category))
                        } else {
                            None
                        }
                    })
                    .collect();

                if !files.is_empty() {
                    // Sort for deterministic ordering
                    files.sort_by(|a, b| a.0.cmp(&b.0));
                    println!("Loaded {} test files from manifest", files.len());
                    return files;
                }
            }
        }
    }

    // Fallback to hardcoded list with inferred categories
    let fallback = vec![
        (
            "test_gifs/benchmark_suite/cartoon_01.gif".to_string(),
            "cartoon".to_string(),
        ),
        (
            "test_gifs/benchmark_suite/photo_01.gif".to_string(),
            "photographic".to_string(),
        ),
        (
            "test_gifs/benchmark_suite/cartoon_02.gif".to_string(),
            "cartoon".to_string(),
        ),
    ];
    println!(
        "Using fallback hardcoded test files ({} files)",
        fallback.len()
    );
    fallback
}

fn main() {
    // Check for comparison mode
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "adaptive-comparison" {
        run_adaptive_comparison();
        return;
    }
    if args.len() > 1 && args[1] == "tiered-eval" {
        run_tiered_eval();
        return;
    }

    let (commit_hash, commit_date) = get_git_info();
    let timestamp = chrono::Utc::now().to_rfc3339();
    let (bench_width, bench_height) = get_bench_dimensions();

    println!("Rusticle Regression Benchmark");
    println!("Commit: {} ({})", commit_hash, commit_date);
    println!("Target resize dimensions: {}x{}", bench_width, bench_height);
    println!("========================================\n");

    let has_gifsicle = gifsicle_available();
    if has_gifsicle {
        println!("gifsicle: available");
    } else {
        println!("gifsicle: NOT found (baseline comparison disabled)");
    }

    let test_files = load_test_files();
    let operations = ["resize", "all"];

    let mut results = Vec::new();

    for (file, category) in &test_files {
        let path = Path::new(file);
        if !path.exists() {
            eprintln!("Skipping {}: not found", file);
            continue;
        }

        for op in &operations {
            let file_name = path.file_name().unwrap().to_string_lossy();
            print!("Benchmarking {} [{}] ({})... ", file_name, category, op);
            std::io::stdout().flush().unwrap();

            let mut rust_runs: Vec<BenchResult> = (0..3)
                .filter_map(|_| bench_rusticle_file(path, op, category))
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
                    .filter_map(|_| bench_gifsicle_file(path, op, category))
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

    let avg_butteraugli: Option<f64> = {
        let ba_values: Vec<f64> = rust_results
            .iter()
            .filter_map(|r| r.avg_butteraugli)
            .collect();
        if ba_values.is_empty() {
            None
        } else {
            Some(ba_values.iter().sum::<f64>() / ba_values.len() as f64)
        }
    };

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
    if let Some(ba) = avg_butteraugli {
        println!("  Avg Butteraugli: {:.4} (lower is better)", ba);
    }
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
        avg_butteraugli,
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

            if let (Some(curr_ba), Some(prev_ba)) = (avg_butteraugli, prev.avg_butteraugli) {
                let ba_diff = curr_ba - prev_ba;
                println!("  Butteraugli: {:+.4}", ba_diff);
                if ba_diff > 0.5 {
                    println!(
                        "\n⚠️  WARNING: Butteraugli regression detected (worsened by {:.4})!",
                        ba_diff
                    );
                }
            }

            if psnr_diff < -2.0 || ssim_diff < -0.01 {
                println!("\n⚠️  WARNING: Quality regression detected!");
                std::process::exit(1);
            }
        }
    }
}

fn run_adaptive_comparison() {
    println!("=== Adaptive Bytes Emission Comparison ===\n");

    // Run offender comparison
    println!("Running comparison on known offenders...");
    let offender_files = adaptive_comparison::load_offender_files();
    let mut offender_results = Vec::new();

    for (path, category) in &offender_files {
        if let Some(result) = adaptive_comparison::compare_file(Path::new(path), category) {
            println!("  ✓ {}", result.file_name);
            offender_results.push(result);
        } else {
            println!("  ✗ {} (failed)", path);
        }
    }

    let offender_summary = adaptive_comparison::ComparisonSummary {
        timestamp: chrono::Utc::now().to_rfc3339(),
        corpus: "offenders".to_string(),
        total_files: offender_results.len(),
        aggregate_metrics: adaptive_comparison::compute_aggregate_metrics(&offender_results),
        results: offender_results.clone(),
    };

    // Save offender results
    if let Ok(json) = serde_json::to_string_pretty(&offender_summary) {
        let _ = fs::write("outputs/adaptive_bytes_offender_results.json", json);
        println!("\n✓ Saved offender results to outputs/adaptive_bytes_offender_results.json");
    }

    // Run holdout comparison
    println!("\nRunning comparison on 39-image holdout corpus...");
    let holdout_files = adaptive_comparison::load_holdout_files();
    let mut holdout_results = Vec::new();

    for (path, category) in &holdout_files {
        if let Some(result) = adaptive_comparison::compare_file(Path::new(path), category) {
            print!(".");
            std::io::stdout().flush().ok();
            holdout_results.push(result);
        }
    }
    println!();

    let holdout_summary = adaptive_comparison::ComparisonSummary {
        timestamp: chrono::Utc::now().to_rfc3339(),
        corpus: "holdout".to_string(),
        total_files: holdout_results.len(),
        aggregate_metrics: adaptive_comparison::compute_aggregate_metrics(&holdout_results),
        results: holdout_results.clone(),
    };

    // Save holdout results
    if let Ok(json) = serde_json::to_string_pretty(&holdout_summary) {
        let _ = fs::write("outputs/adaptive_bytes_holdout_summary.json", json);
        println!("✓ Saved holdout results to outputs/adaptive_bytes_holdout_summary.json");
    }

    // Generate markdown report
    generate_comparison_report(&offender_summary, &holdout_summary);
}

fn generate_comparison_report(
    offender_summary: &adaptive_comparison::ComparisonSummary,
    holdout_summary: &adaptive_comparison::ComparisonSummary,
) {
    let mut report = String::new();
    report.push_str("# Adaptive Bytes Emission Comparison Report\n\n");
    report.push_str(&format!(
        "**Generated:** {}\n\n",
        chrono::Utc::now().to_rfc3339()
    ));

    // Executive summary
    report.push_str("## Executive Summary\n\n");
    report.push_str(&format!(
        "- **Offender Corpus:** {} files\n",
        offender_summary.total_files
    ));
    report.push_str(&format!(
        "- **Holdout Corpus:** {} files\n",
        holdout_summary.total_files
    ));
    report.push_str(&format!(
        "- **Adaptive Fallback Rate (Offenders):** {:.1}%\n",
        offender_summary.aggregate_metrics.adaptive_fallback_rate * 100.0
    ));
    report.push_str(&format!(
        "- **Adaptive Fallback Rate (Holdout):** {:.1}%\n\n",
        holdout_summary.aggregate_metrics.adaptive_fallback_rate * 100.0
    ));

    // Offender results
    report.push_str("## Known Offenders Comparison\n\n");
    report_corpus(&mut report, offender_summary);

    // Holdout results
    report.push_str("\n## Holdout Corpus Comparison\n\n");
    report_corpus(&mut report, holdout_summary);

    // Key findings
    report.push_str("\n## Key Findings\n\n");
    report_findings(&mut report, offender_summary, holdout_summary);

    // Fallback analysis
    report.push_str("\n## Fallback Analysis\n\n");
    report_fallback_analysis(&mut report, offender_summary, holdout_summary);

    if fs::write("outputs/adaptive_bytes_comparison.md", report).is_ok() {
        println!("✓ Saved comparison report to outputs/adaptive_bytes_comparison.md");
    }
}

fn report_corpus(report: &mut String, summary: &adaptive_comparison::ComparisonSummary) {
    let agg = &summary.aggregate_metrics;

    report.push_str("### Metrics Summary\n\n");
    report.push_str("| Metric | Gifsicle | Rusticle Default | Rusticle Adaptive |\n");
    report.push_str("|--------|----------|------------------|-------------------|\n");

    report.push_str(&format!(
        "| Avg Output Bytes | {:.0} | {:.0} | {:.0} |\n",
        agg.gifsicle_baseline.avg_output_bytes,
        agg.rusticle_default.avg_output_bytes,
        agg.rusticle_adaptive_bytes.avg_output_bytes
    ));

    report.push_str(&format!(
        "| Avg Compression Ratio | {:.3} | {:.3} | {:.3} |\n",
        agg.gifsicle_baseline.avg_compression_ratio,
        agg.rusticle_default.avg_compression_ratio,
        agg.rusticle_adaptive_bytes.avg_compression_ratio
    ));

    report.push_str(&format!(
        "| Avg PSNR (dB) | {:.2} | {:.2} | {:.2} |\n",
        agg.gifsicle_baseline.avg_psnr,
        agg.rusticle_default.avg_psnr,
        agg.rusticle_adaptive_bytes.avg_psnr
    ));

    report.push_str(&format!(
        "| Avg SSIM | {:.4} | {:.4} | {:.4} |\n",
        agg.gifsicle_baseline.avg_ssim,
        agg.rusticle_default.avg_ssim,
        agg.rusticle_adaptive_bytes.avg_ssim
    ));

    if let (Some(gf), Some(rd), Some(ra)) = (
        agg.gifsicle_baseline.avg_butteraugli,
        agg.rusticle_default.avg_butteraugli,
        agg.rusticle_adaptive_bytes.avg_butteraugli,
    ) {
        report.push_str(&format!(
            "| Avg Butteraugli | {:.4} | {:.4} | {:.4} |\n",
            gf, rd, ra
        ));
    }

    report.push_str(&format!(
        "| Avg Runtime (ms) | {:.1} | {:.1} | {:.1} |\n",
        agg.gifsicle_baseline.avg_runtime_ms,
        agg.rusticle_default.avg_runtime_ms,
        agg.rusticle_adaptive_bytes.avg_runtime_ms
    ));

    report.push_str(&format!(
        "| Worst PSNR (dB) | {:.2} | {:.2} | {:.2} |\n",
        agg.gifsicle_baseline.worst_psnr,
        agg.rusticle_default.worst_psnr,
        agg.rusticle_adaptive_bytes.worst_psnr
    ));

    report.push_str(&format!(
        "| Worst SSIM | {:.4} | {:.4} | {:.4} |\n",
        agg.gifsicle_baseline.worst_ssim,
        agg.rusticle_default.worst_ssim,
        agg.rusticle_adaptive_bytes.worst_ssim
    ));

    report.push_str("\n### Per-File Results\n\n");
    report.push_str("| File | Input (KB) | Gifsicle | Default | Adaptive | Adaptive Fallback |\n");
    report.push_str("|------|-----------|----------|---------|----------|-------------------|\n");

    for result in &summary.results {
        let input_kb = result.input_bytes as f64 / 1024.0;
        let gf = result
            .profiles
            .get("gifsicle_baseline")
            .map(|p| format!("{} ({:.2}x)", p.output_bytes, p.compression_ratio))
            .unwrap_or_else(|| "N/A".to_string());
        let rd = result
            .profiles
            .get("rusticle_default")
            .map(|p| format!("{} ({:.2}x)", p.output_bytes, p.compression_ratio))
            .unwrap_or_else(|| "N/A".to_string());
        let ra = result
            .profiles
            .get("rusticle_adaptive_bytes")
            .map(|p| {
                format!(
                    "{} ({:.2}x)",
                    p.output_bytes, p.compression_ratio
                )
            })
            .unwrap_or_else(|| "N/A".to_string());
        let fallback = result
            .profiles
            .get("rusticle_adaptive_bytes")
            .map(|p| {
                if p.fallback {
                    format!("YES ({})", p.fallback_reason.as_deref().unwrap_or("unknown"))
                } else {
                    "NO".to_string()
                }
            })
            .unwrap_or_else(|| "N/A".to_string());

        report.push_str(&format!(
            "| {} | {:.1} | {} | {} | {} | {} |\n",
            result.file_name, input_kb, gf, rd, ra, fallback
        ));
    }
}

fn report_findings(
    report: &mut String,
    offender_summary: &adaptive_comparison::ComparisonSummary,
    holdout_summary: &adaptive_comparison::ComparisonSummary,
) {
    let off_agg = &offender_summary.aggregate_metrics;
    let hold_agg = &holdout_summary.aggregate_metrics;

    // Byte savings (positive = smaller files)
    let off_adaptive_vs_default = (off_agg.rusticle_default.avg_output_bytes
        - off_agg.rusticle_adaptive_bytes.avg_output_bytes)
        / off_agg.rusticle_default.avg_output_bytes
        * 100.0;
    let hold_adaptive_vs_default = (hold_agg.rusticle_default.avg_output_bytes
        - hold_agg.rusticle_adaptive_bytes.avg_output_bytes)
        / hold_agg.rusticle_default.avg_output_bytes
        * 100.0;

    report.push_str("**Byte Savings (Adaptive vs Default):**\n\n");
    report.push_str(&format!(
        "- Offenders: {:.1}% (avg {} → {} bytes)\n",
        off_adaptive_vs_default,
        off_agg.rusticle_default.avg_output_bytes as i64,
        off_agg.rusticle_adaptive_bytes.avg_output_bytes as i64
    ));
    report.push_str(&format!(
        "- Holdout: {:.1}% (avg {} → {} bytes)\n\n",
        hold_adaptive_vs_default,
        hold_agg.rusticle_default.avg_output_bytes as i64,
        hold_agg.rusticle_adaptive_bytes.avg_output_bytes as i64
    ));

    // Quality comparison
    report.push_str("**Quality Metrics (Adaptive vs Default):**\n\n");
    let off_psnr_diff = off_agg.rusticle_adaptive_bytes.avg_psnr - off_agg.rusticle_default.avg_psnr;
    let hold_psnr_diff = hold_agg.rusticle_adaptive_bytes.avg_psnr - hold_agg.rusticle_default.avg_psnr;
    report.push_str(&format!(
        "- Offenders PSNR: {:+.2} dB\n",
        off_psnr_diff
    ));
    report.push_str(&format!(
        "- Holdout PSNR: {:+.2} dB\n",
        hold_psnr_diff
    ));

    // Fallback rate
    report.push_str("\n**Fallback Rate:**\n\n");
    report.push_str(&format!(
        "- Offenders: {:.1}% ({} of {})\n",
        off_agg.adaptive_fallback_rate * 100.0,
        off_agg.adaptive_fallback_count,
        offender_summary.total_files
    ));
    report.push_str(&format!(
        "- Holdout: {:.1}% ({} of {})\n",
        hold_agg.adaptive_fallback_rate * 100.0,
        hold_agg.adaptive_fallback_count,
        holdout_summary.total_files
    ));
}

fn report_fallback_analysis(
    report: &mut String,
    offender_summary: &adaptive_comparison::ComparisonSummary,
    holdout_summary: &adaptive_comparison::ComparisonSummary,
) {
    report.push_str("### Offender Fallbacks\n\n");
    let mut off_fallbacks = Vec::new();
    for result in &offender_summary.results {
        if let Some(profile) = result.profiles.get("rusticle_adaptive_bytes") {
            if profile.fallback {
                off_fallbacks.push((
                    result.file_name.clone(),
                    profile.fallback_reason.clone(),
                ));
            }
        }
    }

    if off_fallbacks.is_empty() {
        report.push_str("No fallbacks detected on offenders.\n\n");
    } else {
        report.push_str("| File | Reason |\n");
        report.push_str("|------|--------|\n");
        for (file, reason) in off_fallbacks {
            report.push_str(&format!(
                "| {} | {} |\n",
                file,
                reason.unwrap_or_else(|| "unknown".to_string())
            ));
        }
        report.push('\n');
    }

    report.push_str("### Holdout Fallbacks\n\n");
    let mut hold_fallbacks = Vec::new();
    for result in &holdout_summary.results {
        if let Some(profile) = result.profiles.get("rusticle_adaptive_bytes") {
            if profile.fallback {
                hold_fallbacks.push((
                    result.file_name.clone(),
                    profile.fallback_reason.clone(),
                ));
            }
        }
    }

    if hold_fallbacks.is_empty() {
        report.push_str("No fallbacks detected on holdout corpus.\n\n");
    } else {
        report.push_str(&format!(
            "Detected {} fallbacks:\n\n",
            hold_fallbacks.len()
        ));
        report.push_str("| File | Reason |\n");
        report.push_str("|------|--------|\n");
        for (file, reason) in hold_fallbacks {
            report.push_str(&format!(
                "| {} | {} |\n",
                file,
                reason.unwrap_or_else(|| "unknown".to_string())
            ));
        }
        report.push('\n');
    }

    report.push_str("## Recommendation\n\n");
    let off_agg = &offender_summary.aggregate_metrics;
    let hold_agg = &holdout_summary.aggregate_metrics;

    let off_fallback_rate = off_agg.adaptive_fallback_rate;
    let hold_fallback_rate = hold_agg.adaptive_fallback_rate;

    if off_fallback_rate > 0.1 || hold_fallback_rate > 0.1 {
        report.push_str("⚠️ **Fallback rate is elevated.** Investigate fallback causes before rollout.\n\n");
    } else {
        report.push_str("✓ **Fallback rate is acceptable** (<10%).\n\n");
    }

    let off_bytes_improvement = (off_agg.rusticle_default.avg_output_bytes
        - off_agg.rusticle_adaptive_bytes.avg_output_bytes)
        / off_agg.rusticle_default.avg_output_bytes
        * 100.0;
    let hold_bytes_improvement = (hold_agg.rusticle_default.avg_output_bytes
        - hold_agg.rusticle_adaptive_bytes.avg_output_bytes)
        / hold_agg.rusticle_default.avg_output_bytes
        * 100.0;
    
    let off_quality_improvement = off_agg.rusticle_adaptive_bytes.avg_psnr - off_agg.rusticle_default.avg_psnr;
    let hold_quality_improvement = hold_agg.rusticle_adaptive_bytes.avg_psnr - hold_agg.rusticle_default.avg_psnr;

    if off_bytes_improvement > 5.0 || hold_bytes_improvement > 5.0 {
        report.push_str("✓ **Byte savings are significant** (>5%).\n\n");
    } else if off_bytes_improvement > 0.0 || hold_bytes_improvement > 0.0 {
        report.push_str("⚠️ **Byte savings are modest** (<5%).\n\n");
    } else {
        report.push_str("✗ **No byte savings detected.** Adaptive path may not be beneficial.\n\n");
    }

    report.push_str("**Overall Assessment:**\n\n");
    
    // Check for quality regressions
    let has_quality_regression = off_quality_improvement < -5.0 || hold_quality_improvement < -5.0;
    
    if off_fallback_rate < 0.05 && hold_fallback_rate < 0.05 && (off_bytes_improvement > 2.0 || hold_bytes_improvement > 2.0) && !has_quality_regression {
        report.push_str("🚀 **Adaptive bytes emission is ready for rollout.** Low fallback rate, measurable improvements, acceptable quality.\n");
    } else if off_fallback_rate < 0.1 && hold_fallback_rate < 0.1 && off_bytes_improvement > 0.0 && hold_bytes_improvement > 0.0 {
        if has_quality_regression {
            report.push_str("⚠️ **Adaptive bytes emission shows promise but has quality concerns.** Byte savings are good, but PSNR regressions are significant.\n");
        } else {
            report.push_str("📊 **Adaptive bytes emission is promising.** Continue optimization and monitoring.\n");
        }
    } else {
        report.push_str("🔧 **Adaptive bytes emission needs more work.** Address fallback causes and quality regressions.\n");
    }
}

fn run_tiered_eval() {
    println!("=== Tiered Optimizer Evaluation ===\n");

    // Run offender evaluation
    println!("Evaluating known offenders...");
    let offender_files = tiered_eval::load_offender_files();
    let mut offender_results = Vec::new();

    for (path, category) in &offender_files {
        if let Some(result) = tiered_eval::evaluate_file(std::path::Path::new(path), category) {
            println!("  ✓ {}", result.file_name);
            offender_results.push(result);
        } else {
            println!("  ✗ {} (failed)", path);
        }
    }

    let offender_summary = tiered_eval::TieredEvalSummary {
        timestamp: chrono::Utc::now().to_rfc3339(),
        corpus: "offenders".to_string(),
        total_files: offender_results.len(),
        aggregate_metrics: tiered_eval::compute_aggregate_metrics(&offender_results),
        tiered_analysis: tiered_eval::compute_tiered_analysis(&offender_results),
        results: offender_results.clone(),
    };

    // Save offender results
    if let Ok(json) = serde_json::to_string_pretty(&offender_summary) {
        let _ = fs::write("outputs/tiered_eval_offender_results.json", json);
        println!("\n✓ Saved offender results to outputs/tiered_eval_offender_results.json");
    }

    // Run holdout evaluation
    println!("\nEvaluating 39-image holdout corpus...");
    let holdout_files = tiered_eval::load_holdout_files();
    let mut holdout_results = Vec::new();

    for (path, category) in &holdout_files {
        if let Some(result) = tiered_eval::evaluate_file(std::path::Path::new(path), category) {
            print!(".");
            std::io::stdout().flush().ok();
            holdout_results.push(result);
        }
    }
    println!();

    let holdout_summary = tiered_eval::TieredEvalSummary {
        timestamp: chrono::Utc::now().to_rfc3339(),
        corpus: "holdout".to_string(),
        total_files: holdout_results.len(),
        aggregate_metrics: tiered_eval::compute_aggregate_metrics(&holdout_results),
        tiered_analysis: tiered_eval::compute_tiered_analysis(&holdout_results),
        results: holdout_results.clone(),
    };

    // Save holdout results
    if let Ok(json) = serde_json::to_string_pretty(&holdout_summary) {
        let _ = fs::write("outputs/tiered_eval_holdout_results.json", json);
        println!("✓ Saved holdout results to outputs/tiered_eval_holdout_results.json");
    }

    // Generate markdown report
    generate_tiered_eval_report(&offender_summary, &holdout_summary);
}

fn generate_tiered_eval_report(
    offender_summary: &tiered_eval::TieredEvalSummary,
    holdout_summary: &tiered_eval::TieredEvalSummary,
) {
    let mut report = String::new();
    report.push_str("# Tiered Optimizer Evaluation Report\n\n");
    report.push_str(&format!(
        "**Generated:** {}\n\n",
        chrono::Utc::now().to_rfc3339()
    ));

    // Executive summary
    report.push_str("## Executive Summary\n\n");
    report.push_str(&format!(
        "- **Offender Corpus:** {} files\n",
        offender_summary.total_files
    ));
    report.push_str(&format!(
        "- **Holdout Corpus:** {} files\n",
        holdout_summary.total_files
    ));
    report.push_str(&format!(
        "- **Tiered Fallback Rate (Offenders):** {:.1}%\n",
        offender_summary.aggregate_metrics.tiered_fallback_rate * 100.0
    ));
    report.push_str(&format!(
        "- **Tiered Fallback Rate (Holdout):** {:.1}%\n\n",
        holdout_summary.aggregate_metrics.tiered_fallback_rate * 100.0
    ));

    // Offender results
    report.push_str("## Known Offenders Evaluation\n\n");
    report_tiered_corpus(&mut report, offender_summary);

    // Holdout results
    report.push_str("\n## Holdout Corpus Evaluation\n\n");
    report_tiered_corpus(&mut report, holdout_summary);

    // Tiered analysis
    report.push_str("\n## Tiered Optimizer Analysis\n\n");
    report_tiered_analysis(&mut report, offender_summary, holdout_summary);

    // Key findings
    report.push_str("\n## Key Findings\n\n");
    report_tiered_findings(&mut report, offender_summary, holdout_summary);

    // Fallback analysis
    report.push_str("\n## Fallback Analysis\n\n");
    report_tiered_fallback_analysis(&mut report, offender_summary, holdout_summary);

    // Recommendations
    report.push_str("\n## Recommendations\n\n");
    report_tiered_recommendations(&mut report, offender_summary, holdout_summary);

    if fs::write("outputs/tiered_eval_report.md", report).is_ok() {
        println!("✓ Saved evaluation report to outputs/tiered_eval_report.md");
    }
}

fn report_tiered_corpus(report: &mut String, summary: &tiered_eval::TieredEvalSummary) {
    let agg = &summary.aggregate_metrics;

    report.push_str("### Metrics Summary\n\n");
    report.push_str("| Metric | Gifsicle | Rusticle Default | Rusticle Tiered |\n");
    report.push_str("|--------|----------|------------------|------------------|\n");

    report.push_str(&format!(
        "| Avg Output Bytes | {:.0} | {:.0} | {:.0} |\n",
        agg.gifsicle_baseline.avg_output_bytes,
        agg.rusticle_default.avg_output_bytes,
        agg.rusticle_tiered_adaptive.avg_output_bytes
    ));

    report.push_str(&format!(
        "| Avg Compression Ratio | {:.3} | {:.3} | {:.3} |\n",
        agg.gifsicle_baseline.avg_compression_ratio,
        agg.rusticle_default.avg_compression_ratio,
        agg.rusticle_tiered_adaptive.avg_compression_ratio
    ));

    report.push_str(&format!(
        "| Avg PSNR (dB) | {:.2} | {:.2} | {:.2} |\n",
        agg.gifsicle_baseline.avg_psnr,
        agg.rusticle_default.avg_psnr,
        agg.rusticle_tiered_adaptive.avg_psnr
    ));

    report.push_str(&format!(
        "| Avg SSIM | {:.4} | {:.4} | {:.4} |\n",
        agg.gifsicle_baseline.avg_ssim,
        agg.rusticle_default.avg_ssim,
        agg.rusticle_tiered_adaptive.avg_ssim
    ));

    if let (Some(gf), Some(rd), Some(ra)) = (
        agg.gifsicle_baseline.avg_butteraugli,
        agg.rusticle_default.avg_butteraugli,
        agg.rusticle_tiered_adaptive.avg_butteraugli,
    ) {
        report.push_str(&format!(
            "| Avg Butteraugli | {:.4} | {:.4} | {:.4} |\n",
            gf, rd, ra
        ));
    }

    report.push_str(&format!(
        "| Avg Runtime (ms) | {:.1} | {:.1} | {:.1} |\n",
        agg.gifsicle_baseline.avg_runtime_ms,
        agg.rusticle_default.avg_runtime_ms,
        agg.rusticle_tiered_adaptive.avg_runtime_ms
    ));

    report.push_str(&format!(
        "| Worst PSNR (dB) | {:.2} | {:.2} | {:.2} |\n",
        agg.gifsicle_baseline.worst_psnr,
        agg.rusticle_default.worst_psnr,
        agg.rusticle_tiered_adaptive.worst_psnr
    ));

    report.push_str(&format!(
        "| Worst SSIM | {:.4} | {:.4} | {:.4} |\n",
        agg.gifsicle_baseline.worst_ssim,
        agg.rusticle_default.worst_ssim,
        agg.rusticle_tiered_adaptive.worst_ssim
    ));

    report.push_str("\n### Per-File Results\n\n");
    report.push_str("| File | Input (KB) | Gifsicle | Default | Tiered | Fallback |\n");
    report.push_str("|------|-----------|----------|---------|--------|----------|\n");

    for result in &summary.results {
        let input_kb = result.input_bytes as f64 / 1024.0;
        let gf = result
            .profiles
            .get("gifsicle_baseline")
            .map(|p| format!("{} ({:.2}x)", p.output_bytes, p.compression_ratio))
            .unwrap_or_else(|| "N/A".to_string());
        let rd = result
            .profiles
            .get("rusticle_default")
            .map(|p| format!("{} ({:.2}x)", p.output_bytes, p.compression_ratio))
            .unwrap_or_else(|| "N/A".to_string());
        let ra = result
            .profiles
            .get("rusticle_tiered_adaptive")
            .map(|p| {
                format!(
                    "{} ({:.2}x)",
                    p.output_bytes, p.compression_ratio
                )
            })
            .unwrap_or_else(|| "N/A".to_string());
        let fallback = result
            .profiles
            .get("rusticle_tiered_adaptive")
            .map(|p| {
                if p.fallback {
                    format!("YES ({})", p.fallback_reason.as_deref().unwrap_or("unknown"))
                } else {
                    "NO".to_string()
                }
            })
            .unwrap_or_else(|| "N/A".to_string());

        report.push_str(&format!(
            "| {} | {:.1} | {} | {} | {} | {} |\n",
            result.file_name, input_kb, gf, rd, ra, fallback
        ));
    }
}

fn report_tiered_analysis(
    report: &mut String,
    offender_summary: &tiered_eval::TieredEvalSummary,
    holdout_summary: &tiered_eval::TieredEvalSummary,
) {
    report.push_str("### Tier-0 Decision Distribution\n\n");
    
    report.push_str("**Offenders:**\n\n");
    for (decision, count) in &offender_summary.tiered_analysis.tier0_decision_distribution {
        let pct = *count as f64 / offender_summary.total_files as f64 * 100.0;
        report.push_str(&format!("- {}: {} ({:.1}%)\n", decision, count, pct));
    }

    report.push_str("\n**Holdout:**\n\n");
    for (decision, count) in &holdout_summary.tiered_analysis.tier0_decision_distribution {
        let pct = *count as f64 / holdout_summary.total_files as f64 * 100.0;
        report.push_str(&format!("- {}: {} ({:.1}%)\n", decision, count, pct));
    }

    report.push_str("\n### Candidate Pruning Effectiveness\n\n");
    report.push_str("| Metric | Offenders | Holdout |\n");
    report.push_str("|--------|-----------|----------|\n");
    report.push_str(&format!(
        "| Avg Candidates Before Pruning | {:.1} | {:.1} |\n",
        offender_summary.tiered_analysis.avg_candidates_before_pruning,
        holdout_summary.tiered_analysis.avg_candidates_before_pruning
    ));
    report.push_str(&format!(
        "| Avg Candidates After Pruning | {:.1} | {:.1} |\n",
        offender_summary.tiered_analysis.avg_candidates_after_pruning,
        holdout_summary.tiered_analysis.avg_candidates_after_pruning
    ));
    report.push_str(&format!(
        "| Avg Pruning Rate | {:.1}% | {:.1}% |\n",
        offender_summary.tiered_analysis.avg_pruning_rate * 100.0,
        holdout_summary.tiered_analysis.avg_pruning_rate * 100.0
    ));

    report.push_str("\n### Tier-2 Measurement Usage\n\n");
    report.push_str("| Metric | Offenders | Holdout |\n");
    report.push_str("|--------|-----------|----------|\n");
    report.push_str(&format!(
        "| Tier-2 Measurement Count | {} | {} |\n",
        offender_summary.tiered_analysis.tier2_measurement_usage_count,
        holdout_summary.tiered_analysis.tier2_measurement_usage_count
    ));
    report.push_str(&format!(
        "| Tier-2 Measurement Rate | {:.1}% | {:.1}% |\n",
        offender_summary.tiered_analysis.tier2_measurement_rate * 100.0,
        holdout_summary.tiered_analysis.tier2_measurement_rate * 100.0
    ));

    report.push_str("\n### Sequence Optimizer Chunks\n\n");
    report.push_str(&format!(
        "- Offenders: {:.1} chunks/file (avg)\n",
        offender_summary.tiered_analysis.avg_sequence_optimizer_chunks
    ));
    report.push_str(&format!(
        "- Holdout: {:.1} chunks/file (avg)\n",
        holdout_summary.tiered_analysis.avg_sequence_optimizer_chunks
    ));
}

fn report_tiered_findings(
    report: &mut String,
    offender_summary: &tiered_eval::TieredEvalSummary,
    holdout_summary: &tiered_eval::TieredEvalSummary,
) {
    let off_agg = &offender_summary.aggregate_metrics;
    let hold_agg = &holdout_summary.aggregate_metrics;

    // Byte savings (positive = smaller files)
    let off_tiered_vs_default = (off_agg.rusticle_default.avg_output_bytes
        - off_agg.rusticle_tiered_adaptive.avg_output_bytes)
        / off_agg.rusticle_default.avg_output_bytes
        * 100.0;
    let hold_tiered_vs_default = (hold_agg.rusticle_default.avg_output_bytes
        - hold_agg.rusticle_tiered_adaptive.avg_output_bytes)
        / hold_agg.rusticle_default.avg_output_bytes
        * 100.0;

    report.push_str("**Byte Savings (Tiered vs Default):**\n\n");
    report.push_str(&format!(
        "- Offenders: {:.1}% (avg {} → {} bytes)\n",
        off_tiered_vs_default,
        off_agg.rusticle_default.avg_output_bytes as i64,
        off_agg.rusticle_tiered_adaptive.avg_output_bytes as i64
    ));
    report.push_str(&format!(
        "- Holdout: {:.1}% (avg {} → {} bytes)\n\n",
        hold_tiered_vs_default,
        hold_agg.rusticle_default.avg_output_bytes as i64,
        hold_agg.rusticle_tiered_adaptive.avg_output_bytes as i64
    ));

    // Quality comparison
    report.push_str("**Quality Metrics (Tiered vs Default):**\n\n");
    let off_psnr_diff = off_agg.rusticle_tiered_adaptive.avg_psnr - off_agg.rusticle_default.avg_psnr;
    let hold_psnr_diff = hold_agg.rusticle_tiered_adaptive.avg_psnr - hold_agg.rusticle_default.avg_psnr;
    report.push_str(&format!(
        "- Offenders PSNR: {:+.2} dB\n",
        off_psnr_diff
    ));
    report.push_str(&format!(
        "- Holdout PSNR: {:+.2} dB\n",
        hold_psnr_diff
    ));

    // Runtime overhead
    let off_runtime_ratio = off_agg.rusticle_tiered_adaptive.avg_runtime_ms / off_agg.rusticle_default.avg_runtime_ms;
    let hold_runtime_ratio = hold_agg.rusticle_tiered_adaptive.avg_runtime_ms / hold_agg.rusticle_default.avg_runtime_ms;
    report.push_str("\n**Runtime Overhead (Tiered vs Default):**\n\n");
    report.push_str(&format!(
        "- Offenders: {:.2}x\n",
        off_runtime_ratio
    ));
    report.push_str(&format!(
        "- Holdout: {:.2}x\n",
        hold_runtime_ratio
    ));
}

fn report_tiered_fallback_analysis(
    report: &mut String,
    offender_summary: &tiered_eval::TieredEvalSummary,
    holdout_summary: &tiered_eval::TieredEvalSummary,
) {
    report.push_str("### Offender Fallbacks\n\n");
    let mut off_fallbacks = Vec::new();
    for result in &offender_summary.results {
        if let Some(profile) = result.profiles.get("rusticle_tiered_adaptive") {
            if profile.fallback {
                off_fallbacks.push((
                    result.file_name.clone(),
                    profile.fallback_reason.clone(),
                ));
            }
        }
    }

    if off_fallbacks.is_empty() {
        report.push_str("No fallbacks detected on offenders.\n\n");
    } else {
        report.push_str("| File | Reason |\n");
        report.push_str("|------|--------|\n");
        for (file, reason) in off_fallbacks {
            report.push_str(&format!(
                "| {} | {} |\n",
                file,
                reason.unwrap_or_else(|| "unknown".to_string())
            ));
        }
        report.push('\n');
    }

    report.push_str("### Holdout Fallbacks\n\n");
    let mut hold_fallbacks = Vec::new();
    for result in &holdout_summary.results {
        if let Some(profile) = result.profiles.get("rusticle_tiered_adaptive") {
            if profile.fallback {
                hold_fallbacks.push((
                    result.file_name.clone(),
                    profile.fallback_reason.clone(),
                ));
            }
        }
    }

    if hold_fallbacks.is_empty() {
        report.push_str("No fallbacks detected on holdout corpus.\n\n");
    } else {
        report.push_str(&format!(
            "Detected {} fallbacks:\n\n",
            hold_fallbacks.len()
        ));
        report.push_str("| File | Reason |\n");
        report.push_str("|------|--------|\n");
        for (file, reason) in hold_fallbacks {
            report.push_str(&format!(
                "| {} | {} |\n",
                file,
                reason.unwrap_or_else(|| "unknown".to_string())
            ));
        }
        report.push('\n');
    }
}

fn report_tiered_recommendations(
    report: &mut String,
    offender_summary: &tiered_eval::TieredEvalSummary,
    holdout_summary: &tiered_eval::TieredEvalSummary,
) {
    let off_agg = &offender_summary.aggregate_metrics;
    let hold_agg = &holdout_summary.aggregate_metrics;

    let off_fallback_rate = off_agg.tiered_fallback_rate;
    let hold_fallback_rate = hold_agg.tiered_fallback_rate;

    if off_fallback_rate > 0.1 || hold_fallback_rate > 0.1 {
        report.push_str("⚠️ **Fallback rate is elevated.** Investigate fallback causes before rollout.\n\n");
    } else {
        report.push_str("✓ **Fallback rate is acceptable** (<10%).\n\n");
    }

    let off_bytes_improvement = (off_agg.rusticle_default.avg_output_bytes
        - off_agg.rusticle_tiered_adaptive.avg_output_bytes)
        / off_agg.rusticle_default.avg_output_bytes
        * 100.0;
    let hold_bytes_improvement = (hold_agg.rusticle_default.avg_output_bytes
        - hold_agg.rusticle_tiered_adaptive.avg_output_bytes)
        / hold_agg.rusticle_default.avg_output_bytes
        * 100.0;
    
    let off_quality_improvement = off_agg.rusticle_tiered_adaptive.avg_psnr - off_agg.rusticle_default.avg_psnr;
    let hold_quality_improvement = hold_agg.rusticle_tiered_adaptive.avg_psnr - hold_agg.rusticle_default.avg_psnr;

    if off_bytes_improvement > 5.0 || hold_bytes_improvement > 5.0 {
        report.push_str("✓ **Byte savings are significant** (>5%).\n\n");
    } else if off_bytes_improvement > 0.0 || hold_bytes_improvement > 0.0 {
        report.push_str("⚠️ **Byte savings are modest** (<5%).\n\n");
    } else {
        report.push_str("✗ **No byte savings detected.** Tiered path may not be beneficial.\n\n");
    }

    report.push_str("**Overall Assessment:**\n\n");
    
    // Check for quality regressions
    let has_quality_regression = off_quality_improvement < -5.0 || hold_quality_improvement < -5.0;
    
    if off_fallback_rate < 0.05 && hold_fallback_rate < 0.05 && (off_bytes_improvement > 2.0 || hold_bytes_improvement > 2.0) && !has_quality_regression {
        report.push_str("🚀 **Tiered optimizer is ready for rollout.** Low fallback rate, measurable improvements, acceptable quality.\n");
    } else if off_fallback_rate < 0.1 && hold_fallback_rate < 0.1 && off_bytes_improvement > 0.0 && hold_bytes_improvement > 0.0 {
        if has_quality_regression {
            report.push_str("⚠️ **Tiered optimizer shows promise but has quality concerns.** Byte savings are good, but PSNR regressions are significant.\n");
        } else {
            report.push_str("📊 **Tiered optimizer is promising.** Continue optimization and monitoring.\n");
        }
    } else {
        report.push_str("🔧 **Tiered optimizer needs more work.** Address fallback causes and quality regressions.\n");
    }
}
