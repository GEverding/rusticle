#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use clap::{Args, Parser, Subcommand, ValueEnum};
use rusticle::{Filter, Gif, OptLevel, QualityMetrics};
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
    let encoded = processed.to_bytes()?;
    let encode_time = encode_start.elapsed();

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
            "Warning: frame count differs ({} vs {})",
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
    let mut worst_psnr: f64 = f64::INFINITY;
    let mut worst_ssim: f64 = 1.0;
    let mut good_frames = 0;
    let mut excellent_frames = 0;

    for i in 0..frame_count {
        let metrics =
            QualityMetrics::compare(&original.frames[i].pixels, &processed.frames[i].pixels);

        total_psnr += metrics.psnr.min(100.0);
        total_ssim += metrics.ssim;
        total_dist += metrics.mean_color_distance;
        worst_psnr = worst_psnr.min(metrics.psnr);
        worst_ssim = worst_ssim.min(metrics.ssim);

        if metrics.is_good() {
            good_frames += 1;
        }
        if metrics.is_excellent() {
            excellent_frames += 1;
        }

        if i < 3 || i == frame_count - 1 {
            eprintln!(
                "Frame {:3}: PSNR={:5.1}dB SSIM={:.4} Dist={:.1}",
                i,
                metrics.psnr.min(99.9),
                metrics.ssim,
                metrics.mean_color_distance
            );
        } else if i == 3 {
            eprintln!("...");
        }
    }

    let avg_psnr = total_psnr / frame_count as f64;
    let avg_ssim = total_ssim / frame_count as f64;

    eprintln!();
    eprintln!("=== QUALITY SUMMARY ({frame_count} frames) ===");
    eprintln!("Avg PSNR:   {:5.2} dB", avg_psnr);
    eprintln!("Avg SSIM:   {:.4}", avg_ssim);
    eprintln!("Avg Dist:   {:.2}", total_dist / frame_count as f64);
    eprintln!("Worst PSNR: {:5.2} dB", worst_psnr.min(99.9));
    eprintln!("Worst SSIM: {:.4}", worst_ssim);
    eprintln!();
    eprintln!("Excellent (PSNR≥40, SSIM≥0.95): {excellent_frames}/{frame_count} frames");
    eprintln!("Good      (PSNR≥30, SSIM≥0.90): {good_frames}/{frame_count} frames");

    eprintln!();
    if avg_psnr >= 40.0 && avg_ssim >= 0.95 {
        eprintln!("VERDICT: EXCELLENT quality");
    } else if avg_psnr >= 30.0 && avg_ssim >= 0.90 {
        eprintln!("VERDICT: GOOD quality");
    } else if avg_psnr >= 25.0 && avg_ssim >= 0.80 {
        eprintln!("VERDICT: ACCEPTABLE quality");
    } else {
        eprintln!("VERDICT: POOR quality — consider different settings");
    }

    Ok(())
}
