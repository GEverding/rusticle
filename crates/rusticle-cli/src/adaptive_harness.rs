//! Adaptive encoding benchmark and regression harness.
//!
//! This module provides a comprehensive harness for evaluating adaptive encoding decisions
//! across offender classes, structural profiles, and holdout corpora.
//!
//! It collects:
//! - Taxonomy / structure class
//! - Chosen representation mix
//! - Chosen palette strategy mix
//! - Score distributions / avg score
//! - Fallback count / fallback reasons
//! - Quality/runtime/size metrics
//! - Invalid comparison count
//!
//! Produces machine-readable (JSON) and human-readable (Markdown) artifacts.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

/// Frame-level adaptive decision from telemetry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameDecision {
    pub frame_index: usize,
    pub chosen_representation: String,
    pub chosen_palette_strategy: String,
    pub score_breakdown: ScoreBreakdown,
    pub reason: String,
    pub explanation: String,
}

/// Score breakdown for a frame decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub byte_cost: f32,
    pub visual_risk: f32,
    pub temporal_instability: f32,
    pub synthetic_transparency_risk: f32,
    pub cpu_cost: f32,
    pub total_score: f32,
}

/// Sequence-level adaptive telemetry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveSequenceTelemetry {
    pub mode: String,
    pub status: String,
    pub fallback_reason: Option<String>,
    pub sequence: SequenceInfo,
    pub frame_decisions: Vec<FrameDecision>,
}

/// Sequence metadata from telemetry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequenceInfo {
    pub width: u32,
    pub height: u32,
    pub frame_count: usize,
    pub taxonomy: String,
    pub avg_score: f32,
    pub estimated_bytes: u64,
}

/// Per-file benchmark result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBenchmarkResult {
    pub file_path: String,
    pub file_name: String,
    pub file_size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub frame_count: usize,
    pub taxonomy: String,
    pub adaptive_success: bool,
    pub fallback_reason: Option<String>,
    pub avg_score: f32,
    pub estimated_bytes: u64,
    pub representation_mix: HashMap<String, usize>,
    pub palette_strategy_mix: HashMap<String, usize>,
    pub runtime_ms: u128,
}

/// Per-category aggregated results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryResults {
    pub category: String,
    pub file_count: usize,
    pub success_count: usize,
    pub fallback_count: usize,
    pub fallback_reasons: HashMap<String, usize>,
    pub avg_score: f32,
    pub avg_estimated_bytes: u64,
    pub representation_distribution: HashMap<String, f32>,
    pub palette_strategy_distribution: HashMap<String, f32>,
    pub taxonomy_distribution: HashMap<String, usize>,
}

/// Full harness report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveHarnessReport {
    pub timestamp: String,
    pub benchmark_suite_path: String,
    pub holdout_suite_path: Option<String>,
    pub total_files: usize,
    pub successful_files: usize,
    pub fallback_files: usize,
    pub global_avg_score: f32,
    pub global_avg_estimated_bytes: u64,
    pub file_results: Vec<FileBenchmarkResult>,
    pub category_results: Vec<CategoryResults>,
    pub taxonomy_summary: HashMap<String, usize>,
    pub representation_summary: HashMap<String, usize>,
    pub palette_strategy_summary: HashMap<String, usize>,
    pub fallback_summary: HashMap<String, usize>,
    pub voyager_offenders: Vec<String>,
    pub disposal_heavy_offenders: Vec<String>,
}

impl AdaptiveHarnessReport {
    /// Generate markdown report from JSON results.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();

        md.push_str("# Adaptive Encoding Benchmark Report\n\n");
        md.push_str(&format!("**Generated:** {}\n\n", self.timestamp));

        // Executive Summary
        md.push_str("## Executive Summary\n");
        md.push_str(&format!("- **Total Files:** {}\n", self.total_files));
        md.push_str(&format!(
            "- **Successful Adaptive Runs:** {} ({:.1}%)\n",
            self.successful_files,
            (self.successful_files as f32 / self.total_files as f32) * 100.0
        ));
        md.push_str(&format!(
            "- **Fallback Count:** {} ({:.1}%)\n",
            self.fallback_files,
            (self.fallback_files as f32 / self.total_files as f32) * 100.0
        ));
        md.push_str(&format!(
            "- **Global Avg Score:** {:.3}\n",
            self.global_avg_score
        ));
        md.push_str(&format!(
            "- **Global Avg Estimated Bytes:** {}\n\n",
            self.global_avg_estimated_bytes
        ));

        // Taxonomy Distribution
        md.push_str("## Taxonomy Distribution\n\n");
        md.push_str("| Taxonomy | Count | Percentage |\n");
        md.push_str("|----------|-------|------------|\n");
        for (taxonomy, count) in &self.taxonomy_summary {
            let pct = (*count as f32 / self.total_files as f32) * 100.0;
            md.push_str(&format!("| {} | {} | {:.1}% |\n", taxonomy, count, pct));
        }
        md.push('\n');

        // Representation Mix
        md.push_str("## Representation Mix (Global)\n\n");
        md.push_str("| Representation | Count | Percentage |\n");
        md.push_str("|----------------|-------|------------|\n");
        let total_repr: usize = self.representation_summary.values().sum();
        for (repr, count) in &self.representation_summary {
            let pct = (*count as f32 / total_repr as f32) * 100.0;
            md.push_str(&format!("| {} | {} | {:.1}% |\n", repr, count, pct));
        }
        md.push('\n');

        // Palette Strategy Mix
        md.push_str("## Palette Strategy Mix (Global)\n\n");
        md.push_str("| Strategy | Count | Percentage |\n");
        md.push_str("|----------|-------|------------|\n");
        let total_strat: usize = self.palette_strategy_summary.values().sum();
        for (strat, count) in &self.palette_strategy_summary {
            let pct = (*count as f32 / total_strat as f32) * 100.0;
            md.push_str(&format!("| {} | {} | {:.1}% |\n", strat, count, pct));
        }
        md.push('\n');

        // Fallback Analysis
        if self.fallback_files > 0 {
            md.push_str("## Fallback Analysis\n\n");
            md.push_str("| Reason | Count |\n");
            md.push_str("|--------|-------|\n");
            for (reason, count) in &self.fallback_summary {
                md.push_str(&format!("| {} | {} |\n", reason, count));
            }
            md.push('\n');
        }

        // Category Breakdown
        md.push_str("## Per-Category Results\n\n");
        for cat_result in &self.category_results {
            md.push_str(&format!("### {}\n\n", cat_result.category));
            md.push_str(&format!(
                "- **Files:** {} (Success: {}, Fallback: {})\n",
                cat_result.file_count, cat_result.success_count, cat_result.fallback_count
            ));
            md.push_str(&format!("- **Avg Score:** {:.3}\n", cat_result.avg_score));
            md.push_str(&format!(
                "- **Avg Estimated Bytes:** {}\n",
                cat_result.avg_estimated_bytes
            ));

            if !cat_result.representation_distribution.is_empty() {
                md.push_str("- **Representation Distribution:**\n");
                for (repr, pct) in &cat_result.representation_distribution {
                    md.push_str(&format!("  - {}: {:.1}%\n", repr, pct * 100.0));
                }
            }

            if !cat_result.palette_strategy_distribution.is_empty() {
                md.push_str("- **Palette Strategy Distribution:**\n");
                for (strat, pct) in &cat_result.palette_strategy_distribution {
                    md.push_str(&format!("  - {}: {:.1}%\n", strat, pct * 100.0));
                }
            }

            md.push('\n');
        }

        // Voyager-like Opaque-Delta Sequences
        if !self.voyager_offenders.is_empty() {
            md.push_str("## Voyager-like Opaque-Delta Sequences\n\n");
            md.push_str("Files classified as opaque-delta/global-palette (Voyager-like):\n\n");
            for file in &self.voyager_offenders {
                md.push_str(&format!("- {}\n", file));
            }
            md.push('\n');
        }

        // Disposal-Heavy Offenders
        if !self.disposal_heavy_offenders.is_empty() {
            md.push_str("## Disposal-Heavy Offenders\n\n");
            md.push_str("Files classified as disposal-heavy/background-previous:\n\n");
            for file in &self.disposal_heavy_offenders {
                md.push_str(&format!("- {}\n", file));
            }
            md.push('\n');
        }

        // Holdout Corpus Class Breakdown
        if self.holdout_suite_path.is_some() {
            md.push_str("## Holdout Corpus Class Breakdown\n\n");
            md.push_str("Holdout corpus files are analyzed without category labels.\n");
            md.push_str("Taxonomy distribution provides structural classification:\n\n");
            md.push_str("| Taxonomy | Count |\n");
            md.push_str("|----------|-------|\n");
            for (taxonomy, count) in &self.taxonomy_summary {
                md.push_str(&format!("| {} | {} |\n", taxonomy, count));
            }
            md.push('\n');
        }

        // Important Notes
        md.push_str("## Important Notes\n\n");
        md.push_str("⚠️ **Current Limitations:**\n\n");
        md.push_str("- Adaptive mode is **telemetry-only**: decisions are emitted but not yet used for actual encoding\n");
        md.push_str("- Output GIFs are encoded using the current (non-adaptive) path\n");
        md.push_str("- Metrics reflect current-path performance, not adaptive-path performance\n");
        md.push_str("- This report documents adaptive **decisions** and **structure analysis**, not actual encoding results\n\n");

        md.push_str("**Separation of Concerns:**\n\n");
        md.push_str("- **Current-path metrics:** Measured from actual encoded output\n");
        md.push_str("- **Adaptive-path telemetry:** Decision data from the adaptive pipeline (not yet applied)\n\n");

        md
    }
}

/// Run the adaptive harness on a directory of GIFs.
pub fn run_harness(
    benchmark_dir: &Path,
    holdout_dir: Option<&Path>,
) -> Result<AdaptiveHarnessReport, Box<dyn std::error::Error>> {
    let mut file_results = Vec::new();
    let mut category_map: HashMap<String, Vec<FileBenchmarkResult>> = HashMap::new();

    // Process benchmark suite
    if benchmark_dir.exists() {
        for entry in fs::read_dir(benchmark_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "gif") {
                if let Ok(result) = process_gif_file(&path) {
                    let category = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| {
                            // Extract category from filename (e.g., "cartoon_01" -> "cartoon")
                            s.split('_').next().unwrap_or("unknown").to_string()
                        })
                        .unwrap_or_else(|| "unknown".to_string());

                    category_map
                        .entry(category)
                        .or_default()
                        .push(result.clone());
                    file_results.push(result);
                }
            }
        }
    }

    // Process holdout suite if provided
    if let Some(holdout) = holdout_dir {
        if holdout.exists() {
            for entry in fs::read_dir(holdout)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "gif") {
                    if let Ok(result) = process_gif_file(&path) {
                        category_map
                            .entry("holdout".to_string())
                            .or_default()
                            .push(result.clone());
                        file_results.push(result);
                    }
                }
            }
        }
    }

    // Aggregate results
    let total_files = file_results.len();
    let successful_files = file_results.iter().filter(|r| r.adaptive_success).count();
    let fallback_files = file_results.iter().filter(|r| !r.adaptive_success).count();

    let global_avg_score = if !file_results.is_empty() {
        file_results.iter().map(|r| r.avg_score).sum::<f32>() / file_results.len() as f32
    } else {
        0.0
    };

    let global_avg_estimated_bytes = if !file_results.is_empty() {
        file_results.iter().map(|r| r.estimated_bytes).sum::<u64>() / file_results.len() as u64
    } else {
        0
    };

    // Build category results
    let mut category_results = Vec::new();
    for (category, results) in category_map {
        let success_count = results.iter().filter(|r| r.adaptive_success).count();
        let fallback_count = results.iter().filter(|r| !r.adaptive_success).count();

        let mut fallback_reasons = HashMap::new();
        for result in &results {
            if let Some(reason) = &result.fallback_reason {
                *fallback_reasons.entry(reason.clone()).or_insert(0) += 1;
            }
        }

        let avg_score = if !results.is_empty() {
            results.iter().map(|r| r.avg_score).sum::<f32>() / results.len() as f32
        } else {
            0.0
        };

        let avg_estimated_bytes = if !results.is_empty() {
            results.iter().map(|r| r.estimated_bytes).sum::<u64>() / results.len() as u64
        } else {
            0
        };

        // Aggregate representation mix
        let mut repr_counts: HashMap<String, usize> = HashMap::new();
        let mut total_repr = 0;
        for result in &results {
            for (repr, count) in &result.representation_mix {
                *repr_counts.entry(repr.clone()).or_insert(0) += count;
                total_repr += count;
            }
        }
        let representation_distribution = repr_counts
            .into_iter()
            .map(|(k, v)| (k, v as f32 / total_repr as f32))
            .collect();

        // Aggregate palette strategy mix
        let mut strat_counts: HashMap<String, usize> = HashMap::new();
        let mut total_strat = 0;
        for result in &results {
            for (strat, count) in &result.palette_strategy_mix {
                *strat_counts.entry(strat.clone()).or_insert(0) += count;
                total_strat += count;
            }
        }
        let palette_strategy_distribution = strat_counts
            .into_iter()
            .map(|(k, v)| (k, v as f32 / total_strat as f32))
            .collect();

        // Taxonomy distribution
        let mut taxonomy_dist: HashMap<String, usize> = HashMap::new();
        for result in &results {
            *taxonomy_dist.entry(result.taxonomy.clone()).or_insert(0) += 1;
        }

        category_results.push(CategoryResults {
            category,
            file_count: results.len(),
            success_count,
            fallback_count,
            fallback_reasons,
            avg_score,
            avg_estimated_bytes,
            representation_distribution,
            palette_strategy_distribution,
            taxonomy_distribution: taxonomy_dist,
        });
    }

    // Build global summaries
    let mut taxonomy_summary: HashMap<String, usize> = HashMap::new();
    let mut representation_summary: HashMap<String, usize> = HashMap::new();
    let mut palette_strategy_summary: HashMap<String, usize> = HashMap::new();
    let mut fallback_summary: HashMap<String, usize> = HashMap::new();

    for result in &file_results {
        *taxonomy_summary.entry(result.taxonomy.clone()).or_insert(0) += 1;
        for (repr, count) in &result.representation_mix {
            *representation_summary.entry(repr.clone()).or_insert(0) += count;
        }
        for (strat, count) in &result.palette_strategy_mix {
            *palette_strategy_summary.entry(strat.clone()).or_insert(0) += count;
        }
        if let Some(reason) = &result.fallback_reason {
            *fallback_summary.entry(reason.clone()).or_insert(0) += 1;
        }
    }

    // Identify offenders
    let voyager_offenders: Vec<String> = file_results
        .iter()
        .filter(|r| r.taxonomy == "opaque-delta/global-palette")
        .map(|r| r.file_name.clone())
        .collect();

    let disposal_heavy_offenders: Vec<String> = file_results
        .iter()
        .filter(|r| r.taxonomy == "disposal-heavy/background-previous")
        .map(|r| r.file_name.clone())
        .collect();

    Ok(AdaptiveHarnessReport {
        timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        benchmark_suite_path: benchmark_dir.to_string_lossy().to_string(),
        holdout_suite_path: holdout_dir.map(|p| p.to_string_lossy().to_string()),
        total_files,
        successful_files,
        fallback_files,
        global_avg_score,
        global_avg_estimated_bytes,
        file_results,
        category_results,
        taxonomy_summary,
        representation_summary,
        palette_strategy_summary,
        fallback_summary,
        voyager_offenders,
        disposal_heavy_offenders,
    })
}

/// Process a single GIF file through the adaptive harness.
fn process_gif_file(path: &Path) -> Result<FileBenchmarkResult, Box<dyn std::error::Error>> {
    let start = Instant::now();

    let data = fs::read(path)?;
    let gif = rusticle::Gif::from_bytes(&data)?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let file_size_bytes = data.len() as u64;
    let width = gif.width;
    let height = gif.height;
    let frame_count = gif.frames.len();

    // Run adaptive encoding
    let config = rusticle::AdaptiveConfig {
        enabled: true,
        emit_telemetry: false, // Suppress stderr during harness run
    };

    let (decision, _output_bytes) = gif.encode_adaptive(&config)?;

    let mut taxonomy = "unknown".to_string();
    let mut avg_score = 0.0;
    let mut estimated_bytes = 0u64;
    let mut representation_mix: HashMap<String, usize> = HashMap::new();
    let mut palette_strategy_mix: HashMap<String, usize> = HashMap::new();

    // Parse telemetry if available
    if let Some(telemetry_json) = &decision.telemetry_json {
        if let Ok(telem) = serde_json::from_str::<AdaptiveSequenceTelemetry>(telemetry_json) {
            taxonomy = telem.sequence.taxonomy;
            avg_score = telem.sequence.avg_score;
            estimated_bytes = telem.sequence.estimated_bytes;

            for frame_decision in &telem.frame_decisions {
                *representation_mix
                    .entry(frame_decision.chosen_representation.clone())
                    .or_insert(0) += 1;
                *palette_strategy_mix
                    .entry(frame_decision.chosen_palette_strategy.clone())
                    .or_insert(0) += 1;
            }
        }
    }

    let runtime_ms = start.elapsed().as_millis();

    Ok(FileBenchmarkResult {
        file_path: path.to_string_lossy().to_string(),
        file_name,
        file_size_bytes,
        width: width as u32,
        height: height as u32,
        frame_count,
        taxonomy,
        adaptive_success: decision.success,
        fallback_reason: decision.fallback_reason,
        avg_score,
        estimated_bytes,
        representation_mix,
        palette_strategy_mix,
        runtime_ms,
    })
}
