use rusticle::Gif;
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let holdout_dir = "test_gifs/holdout_suite";
    let benchmark_dir = "test_gifs/benchmark_suite";

    let mut candidates = Vec::new();

    // Analyze holdout suite
    if let Ok(entries) = fs::read_dir(holdout_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "gif") {
                    if let Some(name) = path.file_stem() {
                        if let Some(name_str) = name.to_str() {
                            if let Ok(data) = fs::read(&path) {
                                if let Ok(gif) = Gif::from_bytes(&data) {
                                    let analysis = analyze_gif(&gif, &path);
                                    candidates.push((name_str.to_string(), analysis));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Analyze benchmark suite
    if let Ok(entries) = fs::read_dir(benchmark_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "gif") {
                    if let Some(name) = path.file_stem() {
                        if let Some(name_str) = name.to_str() {
                            if let Ok(data) = fs::read(&path) {
                                if let Ok(gif) = Gif::from_bytes(&data) {
                                    let analysis = analyze_gif(&gif, &path);
                                    candidates.push((name_str.to_string(), analysis));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Sort all candidates by score
    candidates.sort_by(|a, b| {
        let score_a = a.1.voyager_score();
        let score_b = b.1.voyager_score();
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Filter for voyager-class
    let mut voyager_class = Vec::new();
    for (name, analysis) in &candidates {
        if analysis.matches_voyager_class() {
            voyager_class.push((name.clone(), analysis.clone()));
        }
    }

    println!("=== TOP 20 CANDIDATES (by voyager score) ===\n");
    for (i, (name, analysis)) in candidates.iter().take(20).enumerate() {
        let matches = analysis.matches_voyager_class();
        let marker = if matches { "✓" } else { " " };
        println!("{} {}. {}", marker, i + 1, name);
        println!("    Frames: {}", analysis.frame_count);
        println!("    Dimensions: {}x{}", analysis.width, analysis.height);
        println!("    Has transparent: {}", analysis.has_transparent_gces);
        println!(
            "    None/Keep disposal: {:.1}%",
            analysis.none_keep_disposal_pct
        );
        println!(
            "    Offset subframes: {:.1}%",
            analysis.offset_subframes_pct
        );
        println!("    Global palette: {}", analysis.has_global_palette);
        println!("    Voyager score: {:.2}", analysis.voyager_score());
        println!();
    }

    println!("\n=== STRICT VOYAGER-CLASS MATCHES ===\n");
    if voyager_class.is_empty() {
        println!(
            "No files match all strict criteria (no transparent + 90% None/Keep + 50% offset)."
        );
        println!("\nAnalyzing offset subframe distribution:\n");

        let mut by_offset = Vec::new();
        for (name, analysis) in &candidates {
            if !analysis.has_transparent_gces && analysis.none_keep_disposal_pct >= 90.0 {
                by_offset.push((name.clone(), analysis.clone()));
            }
        }

        by_offset.sort_by(|a, b| {
            b.1.offset_subframes_pct
                .partial_cmp(&a.1.offset_subframes_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        println!("Files with no transparent + 90% None/Keep disposal (sorted by offset %):");
        for (name, analysis) in by_offset.iter().take(15) {
            println!("{}", name);
            println!("    Frames: {}", analysis.frame_count);
            println!("    Dimensions: {}x{}", analysis.width, analysis.height);
            println!(
                "    None/Keep disposal: {:.1}%",
                analysis.none_keep_disposal_pct
            );
            println!(
                "    Offset subframes: {:.1}%",
                analysis.offset_subframes_pct
            );
            println!("    Global palette: {}", analysis.has_global_palette);
            println!();
        }
    } else {
        for (name, analysis) in &voyager_class {
            println!("{}", name);
            println!("  Frames: {}", analysis.frame_count);
            println!("  Dimensions: {}x{}", analysis.width, analysis.height);
            println!("  Has transparent: {}", analysis.has_transparent_gces);
            println!(
                "  None/Keep disposal: {:.1}%",
                analysis.none_keep_disposal_pct
            );
            println!("  Offset subframes: {:.1}%", analysis.offset_subframes_pct);
            println!("  Global palette: {}", analysis.has_global_palette);
            println!("  Voyager score: {:.2}", analysis.voyager_score());
            println!();
        }
    }

    println!(
        "Total strict voyager-class candidates: {}",
        voyager_class.len()
    );

    Ok(())
}

#[derive(Debug, Clone)]
struct GifAnalysis {
    frame_count: usize,
    width: u32,
    height: u32,
    has_transparent_gces: bool,
    none_keep_disposal_pct: f64,
    offset_subframes_pct: f64,
    has_global_palette: bool,
}

impl GifAnalysis {
    fn matches_voyager_class(&self) -> bool {
        !self.has_transparent_gces
            && self.none_keep_disposal_pct >= 90.0
            && self.offset_subframes_pct >= 50.0
    }

    fn voyager_score(&self) -> f64 {
        let mut score = 0.0;
        if !self.has_transparent_gces {
            score += 25.0;
        }
        score += (self.none_keep_disposal_pct / 100.0) * 25.0;
        score += (self.offset_subframes_pct / 100.0) * 25.0;
        if self.has_global_palette {
            score += 25.0;
        }
        score
    }
}

fn analyze_gif(gif: &Gif, _path: &Path) -> GifAnalysis {
    let frames = &gif.frames;
    let frame_count = frames.len();

    let width = gif.width as u32;
    let height = gif.height as u32;

    // Check if any frame has transparent pixels (alpha < 255)
    let has_transparent_gces = frames.iter().any(|f| {
        f.pixels.chunks(4).any(|chunk| {
            if chunk.len() == 4 {
                chunk[3] < 255
            } else {
                false
            }
        })
    });

    let none_keep_count = frames
        .iter()
        .filter(|f| {
            matches!(
                f.dispose,
                rusticle::types::DisposalMethod::None | rusticle::types::DisposalMethod::Keep
            )
        })
        .count();
    let none_keep_disposal_pct = if frame_count > 0 {
        (none_keep_count as f64 / frame_count as f64) * 100.0
    } else {
        0.0
    };

    let offset_count = frames.iter().filter(|f| f.left != 0 || f.top != 0).count();
    let offset_subframes_pct = if frame_count > 0 {
        (offset_count as f64 / frame_count as f64) * 100.0
    } else {
        0.0
    };

    let has_global_palette = gif.global_palette.is_some();

    GifAnalysis {
        frame_count,
        width,
        height,
        has_transparent_gces,
        none_keep_disposal_pct,
        offset_subframes_pct,
        has_global_palette,
    }
}
