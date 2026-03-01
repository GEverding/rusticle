//! Benchmark different imagequant speed/quality/dither combinations across multiple GIF files.
//!
//! Tests the impact of imagequant's speed parameter (1-10) combined with
//! different dithering levels and quality ranges on encoding time, file size,
//! and output quality across various content types.
//!
//! Run with: cargo run --example bench_imagequant_speed --release

use std::fs;
use std::path::Path;
use std::time::Instant;

use rusticle::{Gif, QualityMetrics};

#[derive(Clone)]
struct BenchConfig {
    speed: i32,
    dither: f32,
    quality_min: u8,
    quality_max: u8,
}

struct BenchResult {
    config: BenchConfig,
    time_ms: f64,
    size_kb: f64,
    metrics: Option<QualityMetrics>,
}

struct FileResults {
    filename: String,
    width: u32,
    height: u32,
    frame_count: usize,
    results: Vec<BenchResult>,
}

#[derive(Clone)]
struct BestConfig {
    config: BenchConfig,
    avg_time_ms: f64,
    avg_size_kb: f64,
    avg_psnr: f64,
    avg_ssim: f64,
}

fn main() {
    println!("=== imagequant Speed/Quality/Dither Benchmark ===\n");

    // Test files from benchmark suite
    let test_files = vec![
        "test_gifs/benchmark_suite/cartoon_01.gif",
        "test_gifs/benchmark_suite/cartoon_02.gif",
        "test_gifs/benchmark_suite/photo_01.gif",
        "test_gifs/benchmark_suite/photo_02.gif",
        "test_gifs/benchmark_suite/pixel_art_01.gif",
        "test_gifs/benchmark_suite/transparent_01.gif",
        "test_gifs/benchmark_suite/large_dims_01.gif",
        "test_gifs/benchmark_suite/many_frames_01.gif",
        "test_gifs/benchmark_suite/small_simple_01.gif",
        "test_gifs/benchmark_suite/small_simple_02.gif",
    ];

    // Streamlined test matrix - focus on key comparisons
    let speeds = vec![4, 8, 10];
    let dithers = vec![0.5, 1.0];
    let qualities = vec![(0, 100), (30, 80)];

    let mut all_file_results = Vec::new();

    // Process each test file
    for test_path in &test_files {
        if !Path::new(test_path).exists() {
            eprintln!("Skipping missing file: {}", test_path);
            continue;
        }

        let data = match fs::read(test_path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Failed to read {}: {}", test_path, e);
                continue;
            }
        };

        let gif = match Gif::from_bytes(&data) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("Failed to decode {}: {}", test_path, e);
                continue;
            }
        };

        let filename = Path::new(test_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        // Get original RGBA data for quality comparison
        let original_rgba = extract_rgba_data(&gif);

        let mut results = Vec::new();

        // Run benchmarks for this file
        for &speed in &speeds {
            for &dither in &dithers {
                for &(qmin, qmax) in &qualities {
                    let config = BenchConfig {
                        speed,
                        dither,
                        quality_min: qmin,
                        quality_max: qmax,
                    };

                    let result = bench_config(&gif, &config, &original_rgba);
                    results.push(result);
                }
            }
        }

        all_file_results.push(FileResults {
            filename,
            width: gif.width as u32,
            height: gif.height as u32,
            frame_count: gif.frames.len(),
            results,
        });
    }

    // Print results grouped by file
    for file_result in &all_file_results {
        print_file_results(file_result);
    }

    // Print summary
    print_summary(&all_file_results);
}

fn bench_config(gif: &Gif, config: &BenchConfig, original_rgba: &[u8]) -> BenchResult {
    // Clone gif for encoding
    let test_gif = gif.clone();

    // Measure encoding time with custom quantization
    let start = Instant::now();
    let encoded = encode_with_config(test_gif, config);
    let elapsed = start.elapsed();

    let (size_kb, metrics) = match encoded {
        Ok(bytes) => {
            let size = bytes.len() as f64 / 1024.0;

            // Decode to measure quality
            let metrics = match Gif::from_bytes(&bytes) {
                Ok(decoded) => {
                    let decoded_rgba = extract_rgba_data(&decoded);
                    if decoded_rgba.len() == original_rgba.len() {
                        Some(QualityMetrics::compare(original_rgba, &decoded_rgba))
                    } else {
                        None
                    }
                }
                Err(_) => None,
            };

            (size, metrics)
        }
        Err(e) => {
            eprintln!(
                "Encode failed (speed={}, dither={}, quality={}-{}): {}",
                config.speed, config.dither, config.quality_min, config.quality_max, e
            );
            (0.0, None)
        }
    };

    BenchResult {
        config: BenchConfig {
            speed: config.speed,
            dither: config.dither,
            quality_min: config.quality_min,
            quality_max: config.quality_max,
        },
        time_ms: elapsed.as_secs_f64() * 1000.0,
        size_kb,
        metrics,
    }
}

fn encode_with_config(gif: Gif, config: &BenchConfig) -> Result<Vec<u8>, rusticle::Error> {
    // We need to manually quantize each frame with custom settings
    // Since the public API doesn't expose these parameters, we'll use
    // imagequant directly on the RGBA data

    let mut quantized_frames = Vec::new();

    for frame in &gif.frames {
        let width = frame.width as usize;
        let height = frame.height as usize;

        // Convert to imagequant RGBA format
        let rgba_data: Vec<imagequant::RGBA> = frame
            .pixels
            .chunks_exact(4)
            .map(|chunk| imagequant::RGBA {
                r: chunk[0],
                g: chunk[1],
                b: chunk[2],
                a: chunk[3],
            })
            .collect();

        // Create imagequant attributes with custom settings
        let mut attr = imagequant::Attributes::new();
        attr.set_max_colors(256)
            .map_err(|e| rusticle::Error::EncodeError(format!("set_max_colors: {}", e)))?;
        attr.set_quality(config.quality_min, config.quality_max)
            .map_err(|e| rusticle::Error::EncodeError(format!("set_quality: {}", e)))?;
        attr.set_speed(config.speed)
            .map_err(|e| rusticle::Error::EncodeError(format!("set_speed: {}", e)))?;

        // Create image
        let mut img = attr
            .new_image_borrowed(&rgba_data, width, height, 0.0)
            .map_err(|e| rusticle::Error::EncodeError(format!("new_image: {}", e)))?;

        // Quantize
        let mut result = attr
            .quantize(&mut img)
            .map_err(|e| rusticle::Error::EncodeError(format!("quantize: {}", e)))?;

        // Set dithering
        result
            .set_dithering_level(config.dither)
            .map_err(|e| rusticle::Error::EncodeError(format!("set_dithering: {}", e)))?;

        // Remap pixels
        let (palette, indices) = result
            .remapped(&mut img)
            .map_err(|e| rusticle::Error::EncodeError(format!("remap: {}", e)))?;

        // Convert palette to RGB
        let mut palette_rgb = Vec::with_capacity(palette.len() * 3);
        for color in palette {
            palette_rgb.push(color.r);
            palette_rgb.push(color.g);
            palette_rgb.push(color.b);
        }

        quantized_frames.push((palette_rgb, indices));
    }

    // Now encode using gif crate directly
    encode_gif_direct(&gif, quantized_frames)
}

fn encode_gif_direct(
    gif: &Gif,
    quantized_frames: Vec<(Vec<u8>, Vec<u8>)>,
) -> Result<Vec<u8>, rusticle::Error> {
    use std::io::Cursor;

    let mut output = Vec::new();
    let mut cursor = Cursor::new(&mut output);

    {
        let mut encoder = gif::Encoder::new(&mut cursor, gif.width, gif.height, &[])
            .map_err(|e| rusticle::Error::EncodeError(format!("encoder init: {}", e)))?;

        encoder
            .set_repeat(gif::Repeat::Infinite)
            .map_err(|e| rusticle::Error::EncodeError(format!("set_repeat: {}", e)))?;

        for (i, (palette, indices)) in quantized_frames.into_iter().enumerate() {
            let frame_data = &gif.frames[i];

            // Convert delay from Duration to gif units (10ms increments)
            let delay_ms = frame_data.delay.as_millis() as u16;
            let delay_units = (delay_ms + 5) / 10;

            // Convert disposal method
            let disposal = match frame_data.dispose {
                rusticle::DisposalMethod::None => gif::DisposalMethod::Any,
                rusticle::DisposalMethod::Keep => gif::DisposalMethod::Keep,
                rusticle::DisposalMethod::Background => gif::DisposalMethod::Background,
                rusticle::DisposalMethod::Previous => gif::DisposalMethod::Previous,
            };

            let frame = gif::Frame {
                width: frame_data.width,
                height: frame_data.height,
                delay: delay_units,
                dispose: disposal,
                transparent: None,
                palette: Some(palette),
                buffer: indices.into(),
                ..Default::default()
            };

            encoder
                .write_frame(&frame)
                .map_err(|e| rusticle::Error::EncodeError(format!("write_frame: {}", e)))?;
        }
    }

    Ok(output)
}

fn extract_rgba_data(gif: &Gif) -> Vec<u8> {
    // Concatenate all frame RGBA data
    let mut rgba = Vec::new();
    for frame in &gif.frames {
        rgba.extend_from_slice(&frame.pixels);
    }
    rgba
}

fn print_file_results(file_result: &FileResults) {
    println!(
        "\n=== {} ({}x{}, {} frames) ===",
        file_result.filename, file_result.width, file_result.height, file_result.frame_count
    );
    println!("Speed | Dither | Quality   | Time (ms) | Size (KB) | PSNR (dB) | SSIM");
    println!("------|--------|-----------|-----------|-----------|-----------|-------");

    for result in &file_result.results {
        let quality_str = format!(
            "({:2},{:3})",
            result.config.quality_min, result.config.quality_max
        );

        let (psnr_str, ssim_str) = if let Some(ref m) = result.metrics {
            (format!("{:9.2}", m.psnr), format!("{:6.4}", m.ssim))
        } else {
            ("    -    ".to_string(), "  -   ".to_string())
        };

        println!(
            "{:5} | {:6.1} | {:9} | {:9.2} | {:9.2} | {} | {}",
            result.config.speed,
            result.config.dither,
            quality_str,
            result.time_ms,
            result.size_kb,
            psnr_str,
            ssim_str
        );
    }
}

fn print_summary(all_results: &[FileResults]) {
    println!("\n=== Summary: Best Configurations Across All Files ===\n");

    // Collect all unique configs
    let mut config_stats: std::collections::HashMap<String, Vec<(f64, f64, f64, f64)>> =
        std::collections::HashMap::new();

    for file_result in all_results {
        for result in &file_result.results {
            let key = format!(
                "speed={} dither={:.1} quality=({},{})",
                result.config.speed,
                result.config.dither,
                result.config.quality_min,
                result.config.quality_max
            );

            if let Some(ref m) = result.metrics {
                config_stats.entry(key).or_default().push((
                    result.time_ms,
                    result.size_kb,
                    m.psnr,
                    m.ssim,
                ));
            }
        }
    }

    // Calculate averages and find best configs
    let mut best_configs = Vec::new();

    for (key, stats) in config_stats {
        let count = stats.len() as f64;
        let avg_time = stats.iter().map(|(t, _, _, _)| t).sum::<f64>() / count;
        let avg_size = stats.iter().map(|(_, s, _, _)| s).sum::<f64>() / count;
        let avg_psnr = stats.iter().map(|(_, _, p, _)| p).sum::<f64>() / count;
        let avg_ssim = stats.iter().map(|(_, _, _, s)| s).sum::<f64>() / count;

        // Parse config from key
        let parts: Vec<&str> = key.split_whitespace().collect();
        if parts.len() >= 4 {
            if let (Some(speed), Some(dither)) = (
                parts[0].strip_prefix("speed=").and_then(|s| s.parse().ok()),
                parts[1]
                    .strip_prefix("dither=")
                    .and_then(|s| s.parse().ok()),
            ) {
                let quality_part = parts[2].strip_prefix("quality=(").unwrap_or("0,100");
                let quality_nums: Vec<&str> =
                    quality_part.trim_end_matches(')').split(',').collect();
                if quality_nums.len() == 2 {
                    if let (Some(qmin), Some(qmax)) =
                        (quality_nums[0].parse().ok(), quality_nums[1].parse().ok())
                    {
                        best_configs.push(BestConfig {
                            config: BenchConfig {
                                speed,
                                dither,
                                quality_min: qmin,
                                quality_max: qmax,
                            },
                            avg_time_ms: avg_time,
                            avg_size_kb: avg_size,
                            avg_psnr,
                            avg_ssim,
                        });
                    }
                }
            }
        }
    }

    // Sort by SSIM (quality) descending
    best_configs.sort_by(|a, b| b.avg_ssim.partial_cmp(&a.avg_ssim).unwrap());

    println!("Ranked by Quality (SSIM):");
    println!("Speed | Dither | Quality   | Avg Time (ms) | Avg Size (KB) | Avg PSNR | Avg SSIM");
    println!("------|--------|-----------|---------------|---------------|----------|----------");

    for best in &best_configs {
        println!(
            "{:5} | {:6.1} | ({:2},{:3})   | {:13.2} | {:13.2} | {:8.2} | {:8.4}",
            best.config.speed,
            best.config.dither,
            best.config.quality_min,
            best.config.quality_max,
            best.avg_time_ms,
            best.avg_size_kb,
            best.avg_psnr,
            best.avg_ssim
        );
    }

    // Also show fastest config
    let mut by_speed = best_configs.clone();
    by_speed.sort_by(|a, b| a.avg_time_ms.partial_cmp(&b.avg_time_ms).unwrap());

    if let Some(fastest) = by_speed.first() {
        println!(
            "\nFastest: speed={} dither={:.1} quality=({},{}) - {:.2}ms avg",
            fastest.config.speed,
            fastest.config.dither,
            fastest.config.quality_min,
            fastest.config.quality_max,
            fastest.avg_time_ms
        );
    }

    // Show smallest file size
    let mut by_size = best_configs.clone();
    by_size.sort_by(|a, b| a.avg_size_kb.partial_cmp(&b.avg_size_kb).unwrap());

    if let Some(smallest) = by_size.first() {
        println!(
            "Smallest: speed={} dither={:.1} quality=({},{}) - {:.2}KB avg",
            smallest.config.speed,
            smallest.config.dither,
            smallest.config.quality_min,
            smallest.config.quality_max,
            smallest.avg_size_kb
        );
    }
}
