#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

mod adaptive_harness;
#[cfg(feature = "with-imagequant")]
mod voyager_study;

use clap::{Args, Parser, Subcommand, ValueEnum};
use rusticle::{AdaptiveConfig, Filter, Gif, OptLevel, QualityMetrics, TwoPathConfig};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(name = "rusticle")]
#[command(about = "High-performance GIF resize and optimization CLI")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Resize a GIF and optionally optimize/compress it.
    Resize(ResizeArgs),
    /// Compare quality metrics between two GIFs.
    Quality(QualityArgs),
    /// Run adaptive encoding benchmark harness.
    AdaptiveHarness(AdaptiveHarnessArgs),
    /// Run corrected voyager-class study.
    #[cfg(feature = "with-imagequant")]
    VoyagerStudy(VoyagerStudyArgs),
}

#[derive(Debug, Clone, Args)]
struct ResizeArgs {
    /// Input GIF path.
    input: PathBuf,

    /// Output GIF path (default: <input_stem>_rusticle.gif).
    output: Option<PathBuf>,

    /// Target width in pixels.
    #[arg(long, short = 'W', default_value_t = 320)]
    width: u32,

    /// Target height in pixels.
    #[arg(long, short = 'H', default_value_t = 240)]
    height: u32,

    /// Fit within width/height while preserving aspect ratio.
    #[arg(long)]
    fit: bool,

    /// Resize filter algorithm.
    #[arg(long, value_enum, default_value_t = CliFilter::Lanczos3)]
    filter: CliFilter,

    /// Frame optimization level (o1=basic, o2=standard, o3=aggressive).
    #[arg(long, value_enum)]
    optimize: Option<CliOptLevel>,

    /// Lossy quality 0-100 (lower = smaller file, more artifacts).
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
    lossy: Option<u8>,

    /// Enable experimental adaptive encoding mode.
    #[arg(long)]
    adaptive: bool,

    /// Emit adaptive encoding telemetry to stderr (requires --adaptive).
    #[arg(long)]
    adaptive_telemetry: bool,

    /// Two-path optimizer strategy: legacy (default), auto (classifier), path-a (forced), path-b (forced).
    #[arg(long, value_enum, default_value_t = CliOptimizerStrategy::Legacy)]
    optimizer_strategy: CliOptimizerStrategy,

    /// Emit two-path router telemetry to stderr (requires --optimizer-strategy != legacy).
    #[arg(long)]
    optimizer_telemetry: bool,
}

#[derive(Debug, Clone, Args)]
struct QualityArgs {
    /// Original/reference GIF.
    original: PathBuf,
    /// Processed GIF to evaluate.
    processed: PathBuf,
}

#[derive(Debug, Clone, Args)]
struct AdaptiveHarnessArgs {
    /// Benchmark suite directory (e.g., test_gifs/benchmark_suite).
    #[arg(long)]
    benchmark_dir: PathBuf,

    /// Holdout suite directory (optional).
    #[arg(long)]
    holdout_dir: Option<PathBuf>,

    /// Output directory for reports (default: outputs/).
    #[arg(long, default_value = "outputs")]
    output_dir: PathBuf,
}

#[cfg(feature = "with-imagequant")]
#[derive(Debug, Clone, Args)]
struct VoyagerStudyArgs {
    /// Input voyager GIF file.
    input: PathBuf,

    /// Output directory for study results (default: outputs/).
    #[arg(long, default_value = "outputs")]
    output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliFilter {
    Nearest,
    Bilinear,
    Mitchell,
    Lanczos3,
}

impl From<CliFilter> for Filter {
    fn from(value: CliFilter) -> Self {
        match value {
            CliFilter::Nearest => Filter::Nearest,
            CliFilter::Bilinear => Filter::Bilinear,
            CliFilter::Mitchell => Filter::Mitchell,
            CliFilter::Lanczos3 => Filter::Lanczos3,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliOptLevel {
    O1,
    O2,
    O3,
}

impl From<CliOptLevel> for OptLevel {
    fn from(value: CliOptLevel) -> Self {
        match value {
            CliOptLevel::O1 => OptLevel::O1,
            CliOptLevel::O2 => OptLevel::O2,
            CliOptLevel::O3 => OptLevel::O3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CliOptimizerStrategy {
    Legacy,
    Auto,
    #[value(name = "path-a")]
    PathA,
    #[value(name = "path-b")]
    PathB,
}

impl From<CliOptimizerStrategy> for rusticle::OptimizerStrategy {
    fn from(value: CliOptimizerStrategy) -> Self {
        match value {
            CliOptimizerStrategy::Legacy => rusticle::OptimizerStrategy::Legacy,
            CliOptimizerStrategy::Auto => rusticle::OptimizerStrategy::Auto,
            CliOptimizerStrategy::PathA => rusticle::OptimizerStrategy::PathA,
            CliOptimizerStrategy::PathB => rusticle::OptimizerStrategy::PathB,
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Resize(args) => run_resize(args),
        Command::Quality(args) => compare_quality(&args.original, &args.processed),
        Command::AdaptiveHarness(args) => run_adaptive_harness(args),
        #[cfg(feature = "with-imagequant")]
        Command::VoyagerStudy(args) => run_voyager_study(args),
    }
}

fn run_resize(args: ResizeArgs) -> Result<(), Box<dyn Error>> {
    let output_path = args
        .output
        .clone()
        .unwrap_or_else(|| default_output_path(&args.input));

    let read_start = Instant::now();
    let data = fs::read(&args.input)?;
    let input_bytes = data.len();
    let read_time = read_start.elapsed();

    let decode_start = Instant::now();
    let gif = Gif::from_bytes(&data)?;
    let decode_time = decode_start.elapsed();

    eprintln!(
        "Input:  {} ({}x{}, {} frames, {:.2} MB)",
        args.input.display(),
        gif.width,
        gif.height,
        gif.frames.len(),
        input_bytes as f64 / 1_000_000.0
    );

    let process_start = Instant::now();
    let processed = if args.fit {
        gif.resize_fit(args.width, args.height, args.filter.into())?
    } else {
        gif.resize(args.width, args.height, args.filter.into())?
    };

    let processed = if let Some(level) = args.optimize {
        let opt_level = level.into();

        // Use two-path router if strategy is not legacy
        let processed = if args.optimizer_strategy != CliOptimizerStrategy::Legacy {
            let config = TwoPathConfig {
                strategy: args.optimizer_strategy.into(),
                path_a_config: Default::default(),
                path_b_config: rusticle::PathBConfig { level: opt_level },
                emit_telemetry: args.optimizer_telemetry,
            };

            match rusticle::route_optimize(&processed, opt_level, config) {
                Ok(result) => {
                    // Create a new Gif with the routed frames
                    let mut routed = processed.clone();
                    routed.frames = result.frames;
                    routed
                }
                Err(e) => {
                    eprintln!(
                        "[two-path-router] routing failed: {}, falling back to legacy optimize",
                        e
                    );
                    processed.optimize(opt_level)
                }
            }
        } else {
            processed.optimize(opt_level)
        };
        processed
    } else {
        processed
    };

    let processed = if let Some(quality) = args.lossy {
        processed.lossy(quality)
    } else {
        processed
    };
    let process_time = process_start.elapsed();

    let encode_start = Instant::now();
    let (adaptive_decision, encoded) = if args.adaptive {
        let config = AdaptiveConfig {
            enabled: true,
            emit_telemetry: args.adaptive_telemetry,
        };
        processed.encode_adaptive(&config)?
    } else {
        let bytes = processed.to_bytes()?;
        (
            rusticle::AdaptiveDecision {
                success: false,
                fallback_reason: Some("adaptive mode disabled".to_string()),
                telemetry_json: None,
                tiered_telemetry: None,
            },
            bytes,
        )
    };
    let encode_time = encode_start.elapsed();

    if args.adaptive {
        eprintln!(
            "Adaptive: success={}, fallback_reason={:?}",
            adaptive_decision.success, adaptive_decision.fallback_reason
        );
    }

    let write_start = Instant::now();
    fs::write(&output_path, &encoded)?;
    let write_time = write_start.elapsed();

    let total = read_time + decode_time + process_time + encode_time + write_time;
    let ratio = encoded.len() as f64 / input_bytes as f64 * 100.0;

    eprintln!(
        "Output: {} ({}x{}, {:.2} MB, {:.1}% of input)",
        output_path.display(),
        processed.width,
        processed.height,
        encoded.len() as f64 / 1_000_000.0,
        ratio
    );
    eprintln!(
        "Timing: read={:.0?} decode={:.0?} process={:.0?} encode={:.0?} write={:.0?} total={:.0?}",
        read_time, decode_time, process_time, encode_time, write_time, total
    );

    Ok(())
}

fn default_output_path(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    input
        .parent()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(format!("{stem}_rusticle.gif"))
}

fn compare_quality(original_path: &Path, processed_path: &Path) -> Result<(), Box<dyn Error>> {
    let original_data = fs::read(original_path)?;
    let processed_data = fs::read(processed_path)?;

    let original = Gif::from_bytes(&original_data)?;
    let processed = Gif::from_bytes(&processed_data)?;

    eprintln!(
        "Original:  {}x{}, {} frames, {:.2} MB",
        original.width,
        original.height,
        original.frames.len(),
        original_data.len() as f64 / 1_000_000.0
    );
    eprintln!(
        "Processed: {}x{}, {} frames, {:.2} MB",
        processed.width,
        processed.height,
        processed.frames.len(),
        processed_data.len() as f64 / 1_000_000.0
    );
    eprintln!();

    if original.frames.len() != processed.frames.len() {
        eprintln!(
            "Warning: frame count differs (original: {} vs processed: {})",
            original.frames.len(),
            processed.frames.len()
        );
    }

    let frame_count = original.frames.len().min(processed.frames.len());

    let original = if original.width != processed.width || original.height != processed.height {
        eprintln!(
            "Resizing original to {}x{} for comparison...",
            processed.width, processed.height
        );
        original.resize(
            processed.width as u32,
            processed.height as u32,
            Filter::Lanczos3,
        )?
    } else {
        original
    };

    let mut total_psnr = 0.0;
    let mut total_ssim = 0.0;
    let mut total_dist = 0.0;
    let mut total_butteraugli = 0.0;
    let mut worst_psnr: f64 = f64::INFINITY;
    let mut worst_ssim: f64 = 1.0;
    let mut worst_butteraugli: f64 = 0.0;
    let mut butteraugli_count = 0;
    let mut good_frames = 0;
    let mut excellent_frames = 0;

    for i in 0..frame_count {
        let metrics = QualityMetrics::compare_with_dimensions(
            &original.frames[i].pixels,
            &processed.frames[i].pixels,
            original.width as u32,
            original.height as u32,
        );

        // Only accumulate metrics if comparison is valid
        if metrics.valid {
            total_psnr += metrics.psnr.min(100.0);
            total_ssim += metrics.ssim;
            total_dist += metrics.mean_color_distance;
            worst_psnr = worst_psnr.min(metrics.psnr);
            worst_ssim = worst_ssim.min(metrics.ssim);

            if let Some(ba) = metrics.butteraugli {
                total_butteraugli += ba;
                worst_butteraugli = worst_butteraugli.max(ba);
                butteraugli_count += 1;
            }

            if metrics.is_good() {
                good_frames += 1;
            }
            if metrics.is_excellent() {
                excellent_frames += 1;
            }
        }

        if i < 3 || i == frame_count - 1 {
            if !metrics.valid {
                eprintln!("Frame {:3}: INVALID (buffer mismatch)", i);
            } else {
                let ba_str = if let Some(ba) = metrics.butteraugli {
                    format!("{:.2}", ba)
                } else {
                    "N/A".to_string()
                };
                eprintln!(
                    "Frame {:3}: PSNR={:5.1}dB SSIM={:.4} Dist={:.1} BA={}",
                    i,
                    metrics.psnr.min(99.9),
                    metrics.ssim,
                    metrics.mean_color_distance,
                    ba_str
                );
            }
        } else if i == 3 {
            eprintln!("...");
        }
    }

    let avg_psnr = total_psnr / frame_count as f64;
    let avg_ssim = total_ssim / frame_count as f64;
    let avg_butteraugli = if butteraugli_count > 0 {
        Some(total_butteraugli / butteraugli_count as f64)
    } else {
        None
    };

    eprintln!();
    eprintln!("=== QUALITY SUMMARY ({frame_count} frames) ===");
    eprintln!("Avg PSNR:   {:5.2} dB", avg_psnr);
    eprintln!("Avg SSIM:   {:.4}", avg_ssim);
    eprintln!("Avg Dist:   {:.2}", total_dist / frame_count as f64);
    eprintln!("Worst PSNR: {:5.2} dB", worst_psnr.min(99.9));
    eprintln!("Worst SSIM: {:.4}", worst_ssim);
    if let Some(avg_ba) = avg_butteraugli {
        eprintln!("Avg BA:     {:.2} (lower is better)", avg_ba);
        eprintln!("Worst BA:   {:.2} (lower is better)", worst_butteraugli);
    }
    eprintln!();
    eprintln!("Excellent (PSNR≥40, SSIM≥0.95): {excellent_frames}/{frame_count} frames");
    eprintln!("Good      (PSNR≥30, SSIM≥0.90): {good_frames}/{frame_count} frames");

    eprintln!();
    if let Some(avg_ba) = avg_butteraugli {
        if avg_psnr >= 40.0 && avg_ssim >= 0.95 && avg_ba < 1.0 {
            eprintln!("VERDICT: EXCELLENT quality");
        } else if avg_psnr >= 30.0 && avg_ssim >= 0.90 && avg_ba < 2.0 {
            eprintln!("VERDICT: GOOD quality");
        } else if avg_psnr >= 25.0 && avg_ssim >= 0.80 {
            eprintln!("VERDICT: ACCEPTABLE quality");
        } else {
            eprintln!("VERDICT: POOR quality — consider different settings");
        }
    } else {
        if avg_psnr >= 40.0 && avg_ssim >= 0.95 {
            eprintln!("VERDICT: EXCELLENT quality");
        } else if avg_psnr >= 30.0 && avg_ssim >= 0.90 {
            eprintln!("VERDICT: GOOD quality");
        } else if avg_psnr >= 25.0 && avg_ssim >= 0.80 {
            eprintln!("VERDICT: ACCEPTABLE quality");
        } else {
            eprintln!("VERDICT: POOR quality — consider different settings");
        }
    }

    Ok(())
}

fn run_adaptive_harness(args: AdaptiveHarnessArgs) -> Result<(), Box<dyn Error>> {
    eprintln!("Running adaptive encoding benchmark harness...");
    eprintln!("Benchmark dir: {}", args.benchmark_dir.display());
    if let Some(ref holdout) = args.holdout_dir {
        eprintln!("Holdout dir: {}", holdout.display());
    }

    // Create output directory if it doesn't exist
    fs::create_dir_all(&args.output_dir)?;

    // Run the harness
    let report = adaptive_harness::run_harness(&args.benchmark_dir, args.holdout_dir.as_deref())?;

    // Write JSON report
    let json_path = args.output_dir.join("adaptive_harness_report.json");
    let json_content = serde_json::to_string_pretty(&report)?;
    fs::write(&json_path, json_content)?;
    eprintln!("✓ JSON report: {}", json_path.display());

    // Write Markdown report
    let md_path = args.output_dir.join("adaptive_harness_report.md");
    let md_content = report.to_markdown();
    fs::write(&md_path, md_content)?;
    eprintln!("✓ Markdown report: {}", md_path.display());

    // Print summary to stderr
    eprintln!();
    eprintln!("=== ADAPTIVE HARNESS SUMMARY ===");
    eprintln!("Total files: {}", report.total_files);
    eprintln!(
        "Successful: {} ({:.1}%)",
        report.successful_files,
        (report.successful_files as f32 / report.total_files as f32) * 100.0
    );
    eprintln!(
        "Fallback: {} ({:.1}%)",
        report.fallback_files,
        (report.fallback_files as f32 / report.total_files as f32) * 100.0
    );
    eprintln!("Global avg score: {:.3}", report.global_avg_score);
    eprintln!(
        "Global avg estimated bytes: {}",
        report.global_avg_estimated_bytes
    );
    eprintln!();
    eprintln!("Taxonomy distribution:");
    for (taxonomy, count) in &report.taxonomy_summary {
        eprintln!("  {}: {}", taxonomy, count);
    }
    eprintln!();
    eprintln!("Voyager-like offenders: {}", report.voyager_offenders.len());
    eprintln!(
        "Disposal-heavy offenders: {}",
        report.disposal_heavy_offenders.len()
    );

    Ok(())
}

#[cfg(feature = "with-imagequant")]
fn run_voyager_study(args: VoyagerStudyArgs) -> Result<(), Box<dyn Error>> {
    eprintln!("=== VOYAGER CORRECTED STUDY ===");
    eprintln!("Input: {}", args.input.display());
    eprintln!("Target: 160x120");
    eprintln!();

    fs::create_dir_all(&args.output_dir)?;

    // Run the study
    let results = voyager_study::run_voyager_study(&args.input, &args.output_dir)?;

    // Write JSON results
    let json_path = args.output_dir.join("voyager_corrected_study_results.json");
    let json = serde_json::to_string_pretty(&results)?;
    fs::write(&json_path, json)?;
    eprintln!("✓ JSON results: {}", json_path.display());

    // Write Markdown report
    let md_path = args.output_dir.join("voyager_corrected_study_report.md");
    let md_report = format_voyager_report(&results);
    fs::write(&md_path, md_report)?;
    eprintln!("✓ Markdown report: {}", md_path.display());

    // Print summary
    eprintln!();
    eprintln!("=== RESULTS SUMMARY ===");
    eprintln!("Input file: {}", results.input_file);
    eprintln!("Input bytes: {}", results.input_bytes);
    eprintln!(
        "Target dimensions: {}x{}",
        results.target_width, results.target_height
    );
    eprintln!();
    eprintln!("Candidates:");
    for candidate in &results.candidates {
        if let Some(err) = &candidate.error {
            eprintln!("  {}: ERROR - {}", candidate.name, err);
        } else {
            eprintln!(
                "  {}: {} bytes ({:.1}ms, avg patch area: {:.0}, transparent: {:.1}%)",
                candidate.name,
                candidate.output_bytes,
                candidate.runtime_ms as f64,
                candidate.avg_patch_area,
                candidate.transparent_usage * 100.0
            );
        }
    }
    eprintln!();
    eprintln!("Best by bytes: {}", results.best_bytes);
    eprintln!("Recommendation: {}", results.recommendation);

    Ok(())
}

#[cfg(feature = "with-imagequant")]
fn format_voyager_report(results: &voyager_study::StudyResults) -> String {
    let mut report = String::new();
    report.push_str("# Voyager Corrected Study Results\n\n");
    report.push_str(&format!("**Input:** {}\n", results.input_file));
    report.push_str(&format!("**Input bytes:** {}\n", results.input_bytes));
    report.push_str(&format!(
        "**Target dimensions:** {}x{}\n\n",
        results.target_width, results.target_height
    ));

    report.push_str("## Candidate Metrics\n\n");
    report.push_str(
        "| Candidate | Bytes | Runtime (ms) | Avg Patch Area | Transparent % | Status |\n",
    );
    report.push_str(
        "|-----------|-------|--------------|----------------|---------------|--------|\n",
    );

    for candidate in &results.candidates {
        let status = if let Some(err) = &candidate.error {
            format!("ERROR: {}", err)
        } else {
            "OK".to_string()
        };

        report.push_str(&format!(
            "| {} | {} | {} | {:.0} | {:.1}% | {} |\n",
            candidate.name,
            candidate.output_bytes,
            candidate.runtime_ms,
            candidate.avg_patch_area,
            candidate.transparent_usage * 100.0,
            status
        ));
    }

    report.push_str("\n## Analysis\n\n");
    report.push_str(&format!(
        "**Best candidate by bytes:** {}\n\n",
        results.best_bytes
    ));

    // Compute compression ratios
    let successful: Vec<_> = results
        .candidates
        .iter()
        .filter(|c| c.error.is_none())
        .collect();

    if !successful.is_empty() {
        report.push_str("### Compression Ratios\n\n");
        for candidate in &successful {
            let ratio = candidate.output_bytes as f64 / results.input_bytes as f64;
            report.push_str(&format!(
                "- {}: {:.2}% of original\n",
                candidate.name,
                ratio * 100.0
            ));
        }
        report.push('\n');
    }

    // Identify key dimensions
    report.push_str("### Key Dimensions\n\n");

    let transparent_usage: Vec<_> = successful
        .iter()
        .filter(|c| c.transparent_usage > 0.0)
        .collect();

    if !transparent_usage.is_empty() {
        report.push_str("**Transparent bbox usage detected:**\n");
        for candidate in transparent_usage {
            report.push_str(&format!(
                "- {}: {:.1}% transparent pixels in patches\n",
                candidate.name,
                candidate.transparent_usage * 100.0
            ));
        }
        report.push_str(
            "\nThis indicates that transparent unchanged pixels are present in the animation.\n\n",
        );
    }

    let patch_areas: Vec<_> = successful
        .iter()
        .filter(|c| c.avg_patch_area > 0.0)
        .collect();

    if !patch_areas.is_empty() {
        report.push_str("**Patch geometry:**\n");
        for candidate in patch_areas {
            let canvas_area = (results.target_width * results.target_height) as f64;
            let patch_ratio = candidate.avg_patch_area / canvas_area;
            report.push_str(&format!(
                "- {}: avg patch {:.0} pixels ({:.1}% of canvas)\n",
                candidate.name,
                candidate.avg_patch_area,
                patch_ratio * 100.0
            ));
        }
        report.push('\n');
    }

    report.push_str("## Recommendation\n\n");
    report.push_str(&format!("{}\n", results.recommendation));

    report
}
