//! Tiered optimizer evaluation harness.
//!
//! Comprehensive evaluation of the tiered optimizer against:
//! 1. gifsicle baseline
//! 2. rusticle default (non-adaptive)
//! 3. rusticle adaptive with tiered optimizer (experimental)
//!
//! Collects detailed metrics on:
//! - Output bytes, compression ratio
//! - Visual quality (PSNR, SSIM, Butteraugli)
//! - Runtime overhead
//! - Fallback rate and reasons
//! - Tier-0 decision distribution
//! - Candidate pruning effectiveness
//! - Tier-2 measurement usage
//! - Sequence optimizer chunk distribution
//! - LUT-preserving selection rate (if telemetry available)

use rusticle::{AdaptiveConfig, Filter, Gif, OptLevel, QualityMetrics};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

type QualityResult = (f64, f64, f64, f64, Option<f64>, Option<f64>);

fn tier0_decision_label(decision: rusticle::Tier0Decision) -> String {
    decision.name().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredEvalResult {
    pub file_name: String,
    pub file_path: String,
    pub category: String,
    pub input_bytes: usize,
    pub width: u32,
    pub height: u32,
    pub frame_count: usize,
    pub profiles: HashMap<String, ProfileResult>,
    pub tiered_telemetry: Option<TieredTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileResult {
    pub tool: String,
    pub profile: String,
    pub output_bytes: usize,
    pub compression_ratio: f64,
    pub avg_psnr: f64,
    pub avg_ssim: f64,
    pub worst_psnr: f64,
    pub worst_ssim: f64,
    pub avg_butteraugli: Option<f64>,
    pub worst_butteraugli: Option<f64>,
    pub runtime_ms: f64,
    pub fallback: bool,
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredTelemetry {
    pub tier0_decision: String,
    pub candidates_before_pruning: usize,
    pub candidates_after_pruning: usize,
    pub tier2_measurement_ran: bool,
    pub tier2_frames_measured: usize,
    pub sequence_optimizer_chunks: usize,
    pub sequence_optimizer_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredEvalSummary {
    pub timestamp: String,
    pub corpus: String,
    pub total_files: usize,
    pub results: Vec<TieredEvalResult>,
    pub aggregate_metrics: AggregateMetrics,
    pub tiered_analysis: TieredAnalysis,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateMetrics {
    pub gifsicle_baseline: ProfileAggregate,
    pub rusticle_default: ProfileAggregate,
    pub rusticle_tiered_adaptive: ProfileAggregate,
    pub tiered_fallback_count: usize,
    pub tiered_fallback_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileAggregate {
    pub avg_output_bytes: f64,
    pub avg_compression_ratio: f64,
    pub avg_psnr: f64,
    pub avg_ssim: f64,
    pub worst_psnr: f64,
    pub worst_ssim: f64,
    pub avg_butteraugli: Option<f64>,
    pub worst_butteraugli: Option<f64>,
    pub avg_runtime_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredAnalysis {
    pub tier0_decision_distribution: HashMap<String, usize>,
    pub avg_candidates_before_pruning: f64,
    pub avg_candidates_after_pruning: f64,
    pub avg_pruning_rate: f64,
    pub tier2_measurement_usage_count: usize,
    pub tier2_measurement_rate: f64,
    pub avg_sequence_optimizer_chunks: f64,
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

    for i in 0..frame_count {
        let metrics = QualityMetrics::compare_with_dimensions(
            &reference.frames[i].pixels,
            &processed_decoded.frames[i].pixels,
            width,
            height,
        );

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

pub fn encode_rusticle_default(data: &[u8]) -> Option<(Vec<u8>, f64)> {
    let start = Instant::now();
    let gif = Gif::from_bytes(data).ok()?;
    let output = gif.optimize(OptLevel::O3).lossy(80).to_bytes().ok()?;
    let runtime_ms = start.elapsed().as_secs_f64() * 1000.0;
    Some((output, runtime_ms))
}

type TieredEncodeResult = (Vec<u8>, f64, bool, Option<String>, Option<TieredTelemetry>);

pub fn encode_rusticle_tiered(data: &[u8]) -> Option<TieredEncodeResult> {
    let start = Instant::now();
    let gif = Gif::from_bytes(data).ok()?;
    let config = AdaptiveConfig {
        enabled: true,
        emit_telemetry: false,
    };
    let (decision, output) = gif.encode_adaptive(&config).ok()?;

    // Decode the adaptive output and apply the same optimizations as default path
    let adaptive_gif = Gif::from_bytes(&output).ok()?;
    let optimized = adaptive_gif.optimize(OptLevel::O3).lossy(80);
    let final_output = optimized.to_bytes().ok()?;

    let runtime_ms = start.elapsed().as_secs_f64() * 1000.0;

    // Extract tiered telemetry if available
    let tiered_telemetry = decision.tiered_telemetry.map(|t| TieredTelemetry {
        tier0_decision: tier0_decision_label(t.tier0_decision),
        candidates_before_pruning: t.candidates_before_pruning,
        candidates_after_pruning: t.candidates_after_pruning,
        tier2_measurement_ran: t.tier2_measurement_ran,
        tier2_frames_measured: t.tier2_frames_measured,
        sequence_optimizer_chunks: t.sequence_optimizer_chunks,
        sequence_optimizer_summary: t.sequence_optimizer_summary,
    });

    Some((
        final_output,
        runtime_ms,
        decision.success,
        decision.fallback_reason,
        tiered_telemetry,
    ))
}

pub fn encode_gifsicle(path: &Path) -> Option<(Vec<u8>, f64)> {
    let out_name = format!("tiered_eval_{}_gifsicle.gif", std::process::id());
    let out_path = std::env::temp_dir().join(out_name);

    let start = Instant::now();
    let status = Command::new("gifsicle")
        .args(["-O3", "--lossy=80"])
        .arg(path)
        .arg("-o")
        .arg(&out_path)
        .status()
        .ok()?;
    let runtime_ms = start.elapsed().as_secs_f64() * 1000.0;

    if !status.success() {
        return None;
    }

    let output = fs::read(&out_path).ok()?;
    let _ = fs::remove_file(&out_path);
    Some((output, runtime_ms))
}

pub fn evaluate_file(path: &Path, category: &str) -> Option<TieredEvalResult> {
    let file_name = path.file_name()?.to_string_lossy().to_string();
    let file_path = path.to_string_lossy().to_string();
    let data = fs::read(path).ok()?;
    let input_bytes = data.len();

    let gif = Gif::from_bytes(&data).ok()?;
    let width = gif.width as u32;
    let height = gif.height as u32;
    let frame_count = gif.frames.len();

    let mut profiles = HashMap::new();
    let mut tiered_telemetry = None;

    // Gifsicle baseline
    if let Some((output, runtime_ms)) = encode_gifsicle(path) {
        if let Some((avg_psnr, avg_ssim, worst_psnr, worst_ssim, avg_ba, worst_ba)) =
            compute_quality_metrics(&data, &output, width, height)
        {
            let output_bytes = output.len();
            profiles.insert(
                "gifsicle_baseline".to_string(),
                ProfileResult {
                    tool: "gifsicle".to_string(),
                    profile: "baseline".to_string(),
                    output_bytes,
                    compression_ratio: output_bytes as f64 / input_bytes as f64,
                    avg_psnr,
                    avg_ssim,
                    worst_psnr,
                    worst_ssim,
                    avg_butteraugli: avg_ba,
                    worst_butteraugli: worst_ba,
                    runtime_ms,
                    fallback: false,
                    fallback_reason: None,
                },
            );
        }
    }

    // Rusticle default
    if let Some((output, runtime_ms)) = encode_rusticle_default(&data) {
        if let Some((avg_psnr, avg_ssim, worst_psnr, worst_ssim, avg_ba, worst_ba)) =
            compute_quality_metrics(&data, &output, width, height)
        {
            let output_bytes = output.len();
            profiles.insert(
                "rusticle_default".to_string(),
                ProfileResult {
                    tool: "rusticle".to_string(),
                    profile: "default".to_string(),
                    output_bytes,
                    compression_ratio: output_bytes as f64 / input_bytes as f64,
                    avg_psnr,
                    avg_ssim,
                    worst_psnr,
                    worst_ssim,
                    avg_butteraugli: avg_ba,
                    worst_butteraugli: worst_ba,
                    runtime_ms,
                    fallback: false,
                    fallback_reason: None,
                },
            );
        }
    }

    // Rusticle tiered adaptive
    if let Some((output, runtime_ms, success, fallback_reason, telem)) =
        encode_rusticle_tiered(&data)
    {
        if let Some((avg_psnr, avg_ssim, worst_psnr, worst_ssim, avg_ba, worst_ba)) =
            compute_quality_metrics(&data, &output, width, height)
        {
            let output_bytes = output.len();
            profiles.insert(
                "rusticle_tiered_adaptive".to_string(),
                ProfileResult {
                    tool: "rusticle".to_string(),
                    profile: "tiered_adaptive".to_string(),
                    output_bytes,
                    compression_ratio: output_bytes as f64 / input_bytes as f64,
                    avg_psnr,
                    avg_ssim,
                    worst_psnr,
                    worst_ssim,
                    avg_butteraugli: avg_ba,
                    worst_butteraugli: worst_ba,
                    runtime_ms,
                    fallback: !success,
                    fallback_reason,
                },
            );
            tiered_telemetry = telem;
        }
    }

    Some(TieredEvalResult {
        file_name,
        file_path,
        category: category.to_string(),
        input_bytes,
        width,
        height,
        frame_count,
        profiles,
        tiered_telemetry,
    })
}

pub fn load_offender_files() -> Vec<(String, String)> {
    vec![
        (
            "test_gifs/holdout_suite/trapezius_animation_small2.gif".to_string(),
            "offender".to_string(),
        ),
        (
            "test_gifs/holdout_suite/galilean_moon_laplace_resonance_animation_2.gif".to_string(),
            "offender".to_string(),
        ),
        (
            "test_gifs/holdout_suite/790106_0203_voyager_58m_to_31m_reduced.gif".to_string(),
            "offender".to_string(),
        ),
    ]
}

pub fn load_holdout_files() -> Vec<(String, String)> {
    let holdout_dir = "test_gifs/holdout_suite";
    let mut files = Vec::new();

    if let Ok(entries) = fs::read_dir(holdout_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "gif") {
                files.push((path.to_string_lossy().to_string(), "holdout".to_string()));
            }
        }
    }

    files.sort();
    files
}

pub fn compute_aggregate_metrics(results: &[TieredEvalResult]) -> AggregateMetrics {
    let mut gifsicle_bytes = Vec::new();
    let mut gifsicle_psnr = Vec::new();
    let mut gifsicle_ssim = Vec::new();
    let mut gifsicle_worst_psnr = Vec::new();
    let mut gifsicle_worst_ssim = Vec::new();
    let mut gifsicle_ba = Vec::new();
    let mut gifsicle_worst_ba = Vec::new();
    let mut gifsicle_runtime = Vec::new();

    let mut rusticle_default_bytes = Vec::new();
    let mut rusticle_default_psnr = Vec::new();
    let mut rusticle_default_ssim = Vec::new();
    let mut rusticle_default_worst_psnr = Vec::new();
    let mut rusticle_default_worst_ssim = Vec::new();
    let mut rusticle_default_ba = Vec::new();
    let mut rusticle_default_worst_ba = Vec::new();
    let mut rusticle_default_runtime = Vec::new();

    let mut tiered_bytes = Vec::new();
    let mut tiered_psnr = Vec::new();
    let mut tiered_ssim = Vec::new();
    let mut tiered_worst_psnr = Vec::new();
    let mut tiered_worst_ssim = Vec::new();
    let mut tiered_ba = Vec::new();
    let mut tiered_worst_ba = Vec::new();
    let mut tiered_runtime = Vec::new();
    let mut tiered_fallback_count = 0;

    for result in results {
        if let Some(profile) = result.profiles.get("gifsicle_baseline") {
            gifsicle_bytes.push(profile.output_bytes as f64);
            gifsicle_psnr.push(profile.avg_psnr);
            gifsicle_ssim.push(profile.avg_ssim);
            gifsicle_worst_psnr.push(profile.worst_psnr);
            gifsicle_worst_ssim.push(profile.worst_ssim);
            if let Some(ba) = profile.avg_butteraugli {
                gifsicle_ba.push(ba);
            }
            if let Some(ba) = profile.worst_butteraugli {
                gifsicle_worst_ba.push(ba);
            }
            gifsicle_runtime.push(profile.runtime_ms);
        }

        if let Some(profile) = result.profiles.get("rusticle_default") {
            rusticle_default_bytes.push(profile.output_bytes as f64);
            rusticle_default_psnr.push(profile.avg_psnr);
            rusticle_default_ssim.push(profile.avg_ssim);
            rusticle_default_worst_psnr.push(profile.worst_psnr);
            rusticle_default_worst_ssim.push(profile.worst_ssim);
            if let Some(ba) = profile.avg_butteraugli {
                rusticle_default_ba.push(ba);
            }
            if let Some(ba) = profile.worst_butteraugli {
                rusticle_default_worst_ba.push(ba);
            }
            rusticle_default_runtime.push(profile.runtime_ms);
        }

        if let Some(profile) = result.profiles.get("rusticle_tiered_adaptive") {
            tiered_bytes.push(profile.output_bytes as f64);
            tiered_psnr.push(profile.avg_psnr);
            tiered_ssim.push(profile.avg_ssim);
            tiered_worst_psnr.push(profile.worst_psnr);
            tiered_worst_ssim.push(profile.worst_ssim);
            if let Some(ba) = profile.avg_butteraugli {
                tiered_ba.push(ba);
            }
            if let Some(ba) = profile.worst_butteraugli {
                tiered_worst_ba.push(ba);
            }
            tiered_runtime.push(profile.runtime_ms);
            if profile.fallback {
                tiered_fallback_count += 1;
            }
        }
    }

    let avg = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let min = |v: &[f64]| v.iter().cloned().fold(f64::INFINITY, f64::min);

    AggregateMetrics {
        gifsicle_baseline: ProfileAggregate {
            avg_output_bytes: avg(&gifsicle_bytes),
            avg_compression_ratio: avg(&gifsicle_bytes)
                / avg(&[results.iter().map(|r| r.input_bytes as f64).sum::<f64>()
                    / results.len() as f64]),
            avg_psnr: avg(&gifsicle_psnr),
            avg_ssim: avg(&gifsicle_ssim),
            worst_psnr: min(&gifsicle_worst_psnr),
            worst_ssim: min(&gifsicle_worst_ssim),
            avg_butteraugli: if gifsicle_ba.is_empty() {
                None
            } else {
                Some(avg(&gifsicle_ba))
            },
            worst_butteraugli: if gifsicle_worst_ba.is_empty() {
                None
            } else {
                Some(gifsicle_worst_ba.iter().cloned().fold(0.0, f64::max))
            },
            avg_runtime_ms: avg(&gifsicle_runtime),
        },
        rusticle_default: ProfileAggregate {
            avg_output_bytes: avg(&rusticle_default_bytes),
            avg_compression_ratio: avg(&rusticle_default_bytes)
                / avg(&[results.iter().map(|r| r.input_bytes as f64).sum::<f64>()
                    / results.len() as f64]),
            avg_psnr: avg(&rusticle_default_psnr),
            avg_ssim: avg(&rusticle_default_ssim),
            worst_psnr: min(&rusticle_default_worst_psnr),
            worst_ssim: min(&rusticle_default_worst_ssim),
            avg_butteraugli: if rusticle_default_ba.is_empty() {
                None
            } else {
                Some(avg(&rusticle_default_ba))
            },
            worst_butteraugli: if rusticle_default_worst_ba.is_empty() {
                None
            } else {
                Some(
                    rusticle_default_worst_ba
                        .iter()
                        .cloned()
                        .fold(0.0, f64::max),
                )
            },
            avg_runtime_ms: avg(&rusticle_default_runtime),
        },
        rusticle_tiered_adaptive: ProfileAggregate {
            avg_output_bytes: avg(&tiered_bytes),
            avg_compression_ratio: avg(&tiered_bytes)
                / avg(&[results.iter().map(|r| r.input_bytes as f64).sum::<f64>()
                    / results.len() as f64]),
            avg_psnr: avg(&tiered_psnr),
            avg_ssim: avg(&tiered_ssim),
            worst_psnr: min(&tiered_worst_psnr),
            worst_ssim: min(&tiered_worst_ssim),
            avg_butteraugli: if tiered_ba.is_empty() {
                None
            } else {
                Some(avg(&tiered_ba))
            },
            worst_butteraugli: if tiered_worst_ba.is_empty() {
                None
            } else {
                Some(tiered_worst_ba.iter().cloned().fold(0.0, f64::max))
            },
            avg_runtime_ms: avg(&tiered_runtime),
        },
        tiered_fallback_count,
        tiered_fallback_rate: if results.is_empty() {
            0.0
        } else {
            tiered_fallback_count as f64 / results.len() as f64
        },
    }
}

pub fn compute_tiered_analysis(results: &[TieredEvalResult]) -> TieredAnalysis {
    let mut tier0_distribution: HashMap<String, usize> = HashMap::new();
    let mut candidates_before = Vec::new();
    let mut candidates_after = Vec::new();
    let mut tier2_count = 0;
    let mut chunks = Vec::new();

    for result in results {
        if let Some(telem) = &result.tiered_telemetry {
            *tier0_distribution
                .entry(telem.tier0_decision.clone())
                .or_insert(0) += 1;
            candidates_before.push(telem.candidates_before_pruning as f64);
            candidates_after.push(telem.candidates_after_pruning as f64);
            if telem.tier2_measurement_ran {
                tier2_count += 1;
            }
            chunks.push(telem.sequence_optimizer_chunks as f64);
        }
    }

    let avg = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };

    let avg_before = avg(&candidates_before);
    let avg_after = avg(&candidates_after);
    let avg_pruning_rate = if avg_before > 0.0 {
        (avg_before - avg_after) / avg_before
    } else {
        0.0
    };

    TieredAnalysis {
        tier0_decision_distribution: tier0_distribution,
        avg_candidates_before_pruning: avg_before,
        avg_candidates_after_pruning: avg_after,
        avg_pruning_rate,
        tier2_measurement_usage_count: tier2_count,
        tier2_measurement_rate: if results.is_empty() {
            0.0
        } else {
            tier2_count as f64 / results.len() as f64
        },
        avg_sequence_optimizer_chunks: avg(&chunks),
    }
}
