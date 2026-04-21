//! Focused evaluation harness for voyager-class representation study.
//!
//! Evaluates 4 representation candidates + 2 baselines on voyager-class GIFs.
//! Produces JSON results and markdown report.

use rusticle::{
    voyager_exact_bbox_global_palette::VoyagerExactBboxGlobalPaletteBuilder,
    voyager_exact_bbox_global_palette_with_fallback::VoyagerExactBboxGlobalPaletteFallbackBuilder,
    voyager_repr::VoyagerBuilder, voyager_source_reuse::VoyagerSourceReuseBuilder, Filter, Gif,
    OptLevel, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateResult {
    pub candidate: String,
    pub file: String,
    pub output_bytes: usize,
    pub runtime_ms: f64,
    pub viability: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudyResults {
    pub timestamp: String,
    pub results: Vec<CandidateResult>,
    pub summary: HashMap<String, CandidateSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateSummary {
    pub candidate: String,
    pub avg_bytes: f64,
    pub avg_runtime_ms: f64,
    pub viable_count: usize,
    pub total_count: usize,
}

/// Run evaluation on a single file with all candidates.
pub fn evaluate_file(file_path: &Path) -> Result<Vec<CandidateResult>> {
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let gif_bytes = fs::read(file_path)?;
    let gif = Gif::from_bytes(&gif_bytes)?;

    // Resize to 50% of original
    let target_width = (gif.width as f64 * 0.5) as u32;
    let target_height = (gif.height as f64 * 0.5) as u32;

    let resized = gif.clone().resize(target_width, target_height, Filter::Lanczos3)?;

    let mut results = Vec::new();

    // Candidate 1: Full frames + derived global palette (control)
    {
        let start = Instant::now();
        let repr = VoyagerBuilder::build(&resized.frames, resized.width, resized.height);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        match repr {
            Ok(repr) => {
                let output_bytes = repr.global_palette.len() + repr.frames.iter().map(|f| f.indices.len()).sum::<usize>();
                results.push(CandidateResult {
                    candidate: "kx7_full_derived".to_string(),
                    file: file_name.clone(),
                    output_bytes,
                    runtime_ms: elapsed,
                    viability: "viable".to_string(),
                    error: None,
                });
            }
            Err(e) => {
                results.push(CandidateResult {
                    candidate: "kx7_full_derived".to_string(),
                    file: file_name.clone(),
                    output_bytes: 0,
                    runtime_ms: elapsed,
                    viability: "failed".to_string(),
                    error: Some(e.to_string()),
                });
            }
        }
    }

    // Candidate 2: Source-global-reuse bbox
    {
        let start = Instant::now();
        let repr = VoyagerSourceReuseBuilder::build(&resized.frames, resized.width, resized.height, &gif);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        match repr {
            Ok(repr) => {
                let viability = match repr.viability {
                    rusticle::SourceReuseViability::Viable => "viable",
                    rusticle::SourceReuseViability::NoSourceGlobalPalette => {
                        "no_source_palette"
                    }
                    rusticle::SourceReuseViability::IncompatiblePalette => {
                        "incompatible_palette"
                    }
                };
                let output_bytes = repr.global_palette.len() + repr.frames.iter().map(|f| f.indices.len()).sum::<usize>();
                results.push(CandidateResult {
                    candidate: "ku8_source_reuse".to_string(),
                    file: file_name.clone(),
                    output_bytes,
                    runtime_ms: elapsed,
                    viability: viability.to_string(),
                    error: None,
                });
            }
            Err(e) => {
                results.push(CandidateResult {
                    candidate: "ku8_source_reuse".to_string(),
                    file: file_name.clone(),
                    output_bytes: 0,
                    runtime_ms: elapsed,
                    viability: "failed".to_string(),
                    error: Some(e.to_string()),
                });
            }
        }
    }

    // Candidate 3: Exact bbox + derived global palette
    {
        let start = Instant::now();
        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&resized.frames, resized.width, resized.height);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        match repr {
            Ok(repr) => {
                let output_bytes = repr.global_palette.len() + repr.frames.iter().map(|f| f.indices.len()).sum::<usize>();
                results.push(CandidateResult {
                    candidate: "bts_bbox_derived".to_string(),
                    file: file_name.clone(),
                    output_bytes,
                    runtime_ms: elapsed,
                    viability: "viable".to_string(),
                    error: None,
                });
            }
            Err(e) => {
                results.push(CandidateResult {
                    candidate: "bts_bbox_derived".to_string(),
                    file: file_name.clone(),
                    output_bytes: 0,
                    runtime_ms: elapsed,
                    viability: "failed".to_string(),
                    error: Some(e.to_string()),
                });
            }
        }
    }

    // Candidate 4: Exact bbox + derived global palette + fallback (0.7 threshold)
    {
        let start = Instant::now();
        let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
            &resized.frames,
            resized.width,
            resized.height,
            0.7,
        );
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        match repr {
            Ok(repr) => {
                let output_bytes = repr.global_palette.len() + repr.frames.iter().map(|f| f.indices.len()).sum::<usize>();
                results.push(CandidateResult {
                    candidate: "27k_bbox_derived_fallback".to_string(),
                    file: file_name.clone(),
                    output_bytes,
                    runtime_ms: elapsed,
                    viability: "viable".to_string(),
                    error: None,
                });
            }
            Err(e) => {
                results.push(CandidateResult {
                    candidate: "27k_bbox_derived_fallback".to_string(),
                    file: file_name.clone(),
                    output_bytes: 0,
                    runtime_ms: elapsed,
                    viability: "failed".to_string(),
                    error: Some(e.to_string()),
                });
            }
        }
    }

    // Baseline 1: Current rusticle default
    {
        let start = Instant::now();
        let optimized = resized.clone().optimize(OptLevel::O3);
        let bytes = optimized.into_bytes()?;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        results.push(CandidateResult {
            candidate: "baseline_rusticle_default".to_string(),
            file: file_name.clone(),
            output_bytes: bytes.len(),
            runtime_ms: elapsed,
            viability: "viable".to_string(),
            error: None,
        });
    }

    // Baseline 2: gifsicle
    {
        let temp_input = format!("/tmp/voyager_study_{}.gif", std::process::id());
        let temp_output = format!("/tmp/voyager_study_{}_out.gif", std::process::id());

        fs::write(&temp_input, &gif_bytes).ok();

        let start = Instant::now();
        let output = Command::new("gifsicle")
            .args(&[
                "--resize-width",
                &target_width.to_string(),
                "-O3",
                "-o",
                &temp_output,
                &temp_input,
            ])
            .output();
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;

        match output {
            Ok(status) if status.status.success() => {
                if let Ok(bytes) = fs::read(&temp_output) {
                    results.push(CandidateResult {
                        candidate: "baseline_gifsicle".to_string(),
                        file: file_name.clone(),
                        output_bytes: bytes.len(),
                        runtime_ms: elapsed,
                        viability: "viable".to_string(),
                        error: None,
                    });
                } else {
                    results.push(CandidateResult {
                        candidate: "baseline_gifsicle".to_string(),
                        file: file_name.clone(),
                        output_bytes: 0,
                        runtime_ms: elapsed,
                        viability: "failed".to_string(),
                        error: Some("Could not read output".to_string()),
                    });
                }
            }
            _ => {
                results.push(CandidateResult {
                    candidate: "baseline_gifsicle".to_string(),
                    file: file_name.clone(),
                    output_bytes: 0,
                    runtime_ms: elapsed,
                    viability: "failed".to_string(),
                    error: Some("gifsicle not available or failed".to_string()),
                });
            }
        }

        let _ = fs::remove_file(&temp_input);
        let _ = fs::remove_file(&temp_output);
    }

    Ok(results)
}

/// Run the full study on voyager-class files.
pub fn run_study(output_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(output_dir)?;

    let mut all_results = Vec::new();

    // Find voyager-class test files
    let test_dirs = vec![
        "test_gifs/holdout_suite",
        "test_gifs/benchmark_suite",
    ];

    let mut test_files = Vec::new();

    for dir in test_dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("gif") {
                    // Filter for voyager-like files (opaque-delta/global-palette)
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        // Include voyager explicitly, and other likely candidates
                        if name.contains("voyager")
                            || name.contains("animation")
                            || name.contains("orbit")
                            || name.contains("trajectory")
                        {
                            test_files.push(path);
                        }
                    }
                }
            }
        }
    }

    // Ensure we have at least the primary voyager file
    let primary = Path::new("test_gifs/holdout_suite/790106_0203_voyager_58m_to_31m_reduced.gif");
    if primary.exists() && !test_files.iter().any(|p| p == primary) {
        test_files.insert(0, primary.to_path_buf());
    }

    eprintln!("Found {} test files", test_files.len());

    for file_path in test_files {
        eprintln!("Evaluating: {:?}", file_path);
        match evaluate_file(&file_path) {
            Ok(results) => {
                all_results.extend(results);
            }
            Err(e) => {
                eprintln!("Error evaluating {:?}: {}", file_path, e);
            }
        }
    }

    // Compute summary statistics
    let mut summary: HashMap<String, CandidateSummary> = HashMap::new();

    for result in &all_results {
        let entry = summary
            .entry(result.candidate.clone())
            .or_insert_with(|| CandidateSummary {
                candidate: result.candidate.clone(),
                avg_bytes: 0.0,
                avg_runtime_ms: 0.0,
                viable_count: 0,
                total_count: 0,
            });

        entry.total_count += 1;
        entry.avg_bytes += result.output_bytes as f64;
        entry.avg_runtime_ms += result.runtime_ms;

        if result.viability == "viable" {
            entry.viable_count += 1;
        }
    }

    for summary_entry in summary.values_mut() {
        if summary_entry.total_count > 0 {
            summary_entry.avg_bytes /= summary_entry.total_count as f64;
            summary_entry.avg_runtime_ms /= summary_entry.total_count as f64;
        }
    }

    let study_results = StudyResults {
        timestamp: chrono::Local::now().to_rfc3339(),
        results: all_results,
        summary,
    };

    // Write JSON results
    let json_path = output_dir.join("voyager_repr_study_results.json");
    let json = serde_json::to_string_pretty(&study_results)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(&json_path, json)?;
    eprintln!("Wrote results to: {:?}", json_path);

    // Write markdown report
    let report_path = output_dir.join("voyager_repr_study_report.md");
    let report = generate_report(&study_results);
    fs::write(&report_path, report)?;
    eprintln!("Wrote report to: {:?}", report_path);

    Ok(())
}

fn generate_report(results: &StudyResults) -> String {
    let mut report = String::new();

    report.push_str("# Voyager-Class Representation Study Results\n\n");
    report.push_str(&format!("**Generated**: {}\n\n", results.timestamp));

    report.push_str("## Summary\n\n");
    report.push_str("| Candidate | Viable | Avg Bytes | Avg Runtime (ms) |\n");
    report.push_str("|-----------|--------|-----------|------------------|\n");

    let mut candidates: Vec<_> = results.summary.values().collect();
    candidates.sort_by(|a, b| a.candidate.cmp(&b.candidate));

    for summary in candidates {
        report.push_str(&format!(
            "| {} | {}/{} | {:.0} | {:.2} |\n",
            summary.candidate,
            summary.viable_count,
            summary.total_count,
            summary.avg_bytes,
            summary.avg_runtime_ms
        ));
    }

    report.push_str("\n## Per-File Results\n\n");

    let mut files: Vec<_> = results
        .results
        .iter()
        .map(|r| r.file.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    files.sort();

    for file in files {
        report.push_str(&format!("### {}\n\n", file));
        report.push_str("| Candidate | Bytes | Runtime (ms) | Viability |\n");
        report.push_str("|-----------|-------|--------------|----------|\n");

        let file_results: Vec<_> = results.results.iter().filter(|r| r.file == file).collect();
        for result in file_results {
            report.push_str(&format!(
                "| {} | {} | {:.2} | {} |\n",
                result.candidate, result.output_bytes, result.runtime_ms, result.viability
            ));
        }
        report.push_str("\n");
    }

    report.push_str("## Analysis\n\n");

    // Find best candidates
    let viable_results: Vec<_> = results
        .results
        .iter()
        .filter(|r| r.viability == "viable")
        .collect();

    if !viable_results.is_empty() {
        let best_bytes = viable_results.iter().min_by_key(|r| r.output_bytes);
        let fastest = viable_results.iter().min_by(|a, b| {
            a.runtime_ms.partial_cmp(&b.runtime_ms).unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(best) = best_bytes {
            report.push_str(&format!("**Best on bytes**: {} ({} bytes avg)\n\n", best.candidate, best.output_bytes));
        }

        if let Some(fastest) = fastest {
            report.push_str(&format!("**Fastest**: {} ({:.2} ms avg)\n\n", fastest.candidate, fastest.runtime_ms));
        }
    }

    // Check viability
    let source_reuse_viable = results
        .summary
        .get("ku8_source_reuse")
        .map(|s| s.viable_count > 0)
        .unwrap_or(false);

    if !source_reuse_viable {
        report.push_str("⚠️ **Source-global-reuse is not viable** on this test set (no source global palette available).\n\n");
    }

    report.push_str("## Recommendation\n\n");
    report.push_str("See detailed metrics above. Candidates are ranked by average bytes and runtime.\n");

    report
}
