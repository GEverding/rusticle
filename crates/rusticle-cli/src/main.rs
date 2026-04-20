#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use clap::{Args, Parser, Subcommand, ValueEnum};
use rusticle::{AdaptiveConfig, Filter, Gif, OptLevel, QualityMetrics};
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
}

#[derive(Debug, Clone, Args)]
struct QualityArgs {
    /// Original/reference GIF.
    original: PathBuf,
    /// Processed GIF to evaluate.
    processed: PathBuf,
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

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Resize(args) => run_resize(args),
        Command::Quality(args) => compare_quality(&args.original, &args.processed),
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
        processed.optimize(level.into())
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
