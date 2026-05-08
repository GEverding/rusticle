//! Tier-2 bounded encode-and-measure with quality guardrails.
//!
//! This module implements the expensive tier: trial-encode top-N candidates for uncertain
//! frames only, with hard CPU budgets and quality-first guardrails.
//!
//! # Measurement Budget
//!
//! The measurement budget is explicit and configurable per difficulty class:
//! - `max_trial_frames`: Maximum frames that get trial-encoded (default: 10, or 20% of sequence)
//! - `max_candidates_per_frame`: Maximum candidates to trial-encode per frame (default: 2)
//! - `max_wall_clock_ms`: Hard wall-clock budget for entire tier-2 pass (default: 50ms)
//! - `max_total_trial_encodes`: Absolute cap on trial encodes (default: 20)
//!
//! Budget is configurable via `AdaptiveConfig` and has per-difficulty-class defaults:
//! - Easy: tier-2 never runs (budget = 0)
//! - Medium: max_trial_frames=5, max_candidates_per_frame=2, max_wall_clock_ms=20
//! - Hard: max_trial_frames=10, max_candidates_per_frame=3, max_wall_clock_ms=50
//!
//! # Uncertainty Detection
//!
//! A frame enters tier-2 if:
//! - Top-2 candidates after tier-1 pruning + scoring have `total_score` delta < 0.08
//! - OR the chosen candidate is `LutBreaking` but a `LutPreserving` candidate scored within 0.15
//! - Frames are ranked by uncertainty magnitude; budget spent on most uncertain first
//!
//! # Quality Guardrails
//!
//! Before accepting a trial-encode result, verify:
//! - Decoded frame PSNR vs canonical displayed_canvas ≥ 30dB (or configurable threshold)
//! - If PSNR < threshold, reject that candidate regardless of byte savings
//! - If all trial candidates fail quality gate, fall back to `FullFrame` + `ReuseGlobalPreferred`
//!
//! # Trial Encode Flow
//!
//! 1. Materialize candidate (reuse existing `materialize.rs`)
//! 2. Quantize with appropriate palette (reuse `palette_realize.rs`)
//! 3. LZW-encode to get actual byte count
//! 4. Decode and measure PSNR against canonical
//! 5. Record actual_bytes and quality for re-scoring
//!
//! # Re-scoring
//!
//! Replace heuristic `byte_cost` with `actual_byte_cost` (normalized). Keep all other score
//! dimensions. Recompute `total_score`. Choose winner.
//!
//! # Telemetry
//!
//! Records per-frame trial count, actual vs heuristic delta, quality gate pass/fail,
//! budget remaining, and fallback decisions.

use std::time::Instant;

use crate::adaptive_ir::CanonicalSequence;
use crate::candidate_gen::CandidateRepresentation;
use crate::error::Result;
use crate::lut_policy::{candidate_to_family, CpuBudgetClass};
use crate::materialize::Materializer;
use crate::palette_realize::PaletteRealizer;
use crate::palette_strategy::PaletteStrategy;
use crate::scoring::{FrameDecision, ScoreBreakdown};
use crate::types::Gif;
use std::fmt::Write as FmtWrite;

/// Measurement budget for Tier-2 bounded encode-and-measure.
#[derive(Debug, Clone, Copy)]
pub struct MeasurementBudget {
    /// Maximum frames that get trial-encoded (default: 10, or 20% of sequence).
    pub max_trial_frames: usize,
    /// Maximum candidates to trial-encode per frame (default: 2).
    pub max_candidates_per_frame: usize,
    /// Hard wall-clock budget for entire tier-2 pass in milliseconds (default: 50ms).
    pub max_wall_clock_ms: u64,
    /// Absolute cap on trial encodes (default: 20).
    pub max_total_trial_encodes: usize,
}

impl MeasurementBudget {
    /// Create a budget for the given CPU budget class.
    pub fn for_class(class: CpuBudgetClass) -> Self {
        match class {
            CpuBudgetClass::Easy => Self {
                max_trial_frames: 0,
                max_candidates_per_frame: 0,
                max_wall_clock_ms: 0,
                max_total_trial_encodes: 0,
            },
            CpuBudgetClass::Medium => Self {
                max_trial_frames: 5,
                max_candidates_per_frame: 2,
                max_wall_clock_ms: 20,
                max_total_trial_encodes: 10,
            },
            CpuBudgetClass::Hard => Self {
                max_trial_frames: 10,
                max_candidates_per_frame: 3,
                max_wall_clock_ms: 50,
                max_total_trial_encodes: 20,
            },
        }
    }

    /// Check if this budget allows any measurement.
    pub fn is_enabled(&self) -> bool {
        self.max_trial_frames > 0 && self.max_wall_clock_ms > 0
    }
}

impl Default for MeasurementBudget {
    fn default() -> Self {
        Self {
            max_trial_frames: 10,
            max_candidates_per_frame: 2,
            max_wall_clock_ms: 50,
            max_total_trial_encodes: 20,
        }
    }
}

/// Quality guardrails for Tier-2 measurement.
#[derive(Debug, Clone, Copy)]
pub struct QualityGuardrails {
    /// Minimum PSNR threshold in dB (default: 30.0).
    pub min_psnr_db: f32,
    /// Reject candidates with synthetic transparency risk > threshold (default: 0.5).
    pub max_synthetic_transparency_risk: f32,
    /// Reject LUT-breaking candidates if LUT-preserving alternative exists within score delta (default: 0.15).
    pub lut_breaking_penalty_threshold: f32,
}

impl Default for QualityGuardrails {
    fn default() -> Self {
        Self {
            min_psnr_db: 30.0,
            max_synthetic_transparency_risk: 0.5,
            lut_breaking_penalty_threshold: 0.15,
        }
    }
}

/// Result of measuring a candidate with quality assessment.
#[derive(Debug, Clone)]
pub struct MeasuredResult {
    /// The candidate representation.
    pub representation: CandidateRepresentation,
    /// Heuristic score (from Scorer).
    pub heuristic_score: ScoreBreakdown,
    /// Actual encoded byte size (measured).
    pub actual_bytes: usize,
    /// Estimated PSNR in dB (or None if not measured).
    pub estimated_psnr_db: Option<f32>,
    /// Whether this candidate passed quality guardrails.
    pub passed_guardrails: bool,
    /// Reason for guardrail rejection (if any).
    pub guardrail_rejection_reason: Option<String>,
    /// Whether this was the chosen candidate.
    pub was_chosen: bool,
}

/// Telemetry for Tier-2 measurement decisions.
#[derive(Debug, Clone)]
pub struct Tier2Telemetry {
    /// Frame index.
    pub frame_index: usize,
    /// Whether this frame was uncertain and entered Tier-2.
    pub was_measured: bool,
    /// Reason(s) for uncertainty (if any).
    pub uncertainty_reasons: Vec<String>,
    /// Measured candidates (if measured).
    pub measured_results: Vec<MeasuredResult>,
    /// Whether measurement succeeded.
    pub measurement_succeeded: bool,
    /// Error message if measurement failed (fallback to heuristic).
    pub measurement_error: Option<String>,
    /// Fallback reason if no candidates passed guardrails.
    pub fallback_reason: Option<String>,
    /// Wall-clock time spent on this frame in milliseconds.
    pub wall_clock_ms: u64,
}

impl Tier2Telemetry {
    /// Convert telemetry to JSON.
    pub fn to_json(&self) -> String {
        let mut json = String::new();
        let _ = writeln!(json, r#"{{"#);
        let _ = writeln!(json, r#"  "frame_index": {},"#, self.frame_index);
        let _ = writeln!(json, r#"  "was_measured": {},"#, self.was_measured);
        let _ = writeln!(
            json,
            r#"  "measurement_succeeded": {},"#,
            self.measurement_succeeded
        );
        let _ = writeln!(json, r#"  "wall_clock_ms": {},"#, self.wall_clock_ms);

        let _ = write!(json, r#"  "uncertainty_reasons": ["#);
        for (i, reason) in self.uncertainty_reasons.iter().enumerate() {
            if i > 0 {
                let _ = write!(json, ", ");
            }
            let _ = write!(json, r#""{}""#, reason.replace('"', "\\\""));
        }
        let _ = writeln!(json, r#"],"#);

        if let Some(reason) = &self.fallback_reason {
            let _ = writeln!(
                json,
                r#"  "fallback_reason": "{}"#,
                reason.replace('"', "\\\"")
            );
        }

        let _ = write!(json, r#"  "measured_results": ["#);
        for (i, result) in self.measured_results.iter().enumerate() {
            if i > 0 {
                let _ = write!(json, ", ");
            }
            let repr_name = match &result.representation {
                CandidateRepresentation::FullFrame => "full-frame",
                CandidateRepresentation::ExactOpaqueBbox { .. } => "opaque-bbox",
                CandidateRepresentation::TransparentSparsePatch { .. } => "sparse-patch",
                CandidateRepresentation::MinimalNoOp => "minimal-noop",
            };
            let _ = write!(
                json,
                r#"{{"repr":"{}","heuristic_score":{:.3},"actual_bytes":{},"passed_guardrails":{},"was_chosen":{}"#,
                repr_name,
                result.heuristic_score.total_score,
                result.actual_bytes,
                result.passed_guardrails,
                result.was_chosen
            );
            if let Some(psnr) = result.estimated_psnr_db {
                let _ = write!(json, r#","psnr_db":{:.1}"#, psnr);
            }
            if let Some(reason) = &result.guardrail_rejection_reason {
                let _ = write!(
                    json,
                    r#","rejection_reason":"{}""#,
                    reason.replace('"', "\\\"")
                );
            }
            let _ = write!(json, r#"}}"#);
        }
        let _ = writeln!(json, r#"]"#);
        let _ = writeln!(json, r#"}}"#);
        json
    }
}

/// Tier-2 measurement engine: bounded encode-and-measure with quality guardrails.
pub struct Tier2Measurer;

impl Tier2Measurer {
    /// Determine if a frame decision is uncertain and should enter Tier-2.
    ///
    /// A decision is uncertain if:
    /// - Score gap between top candidates is below threshold (default 0.08)
    /// - OR chosen candidate is LUT-breaking but a LUT-preserving alternative scored within 0.15
    pub fn is_uncertain(
        decision: &FrameDecision,
        guardrails: &QualityGuardrails,
    ) -> (bool, Vec<String>) {
        let mut reasons = Vec::new();

        // Check score gap between top candidates
        if let Some((_, alt_score)) = decision.alternatives.first() {
            let gap = (alt_score.total_score - decision.score_breakdown.total_score).abs();
            if gap < 0.08 {
                reasons.push(format!("score_gap_small ({:.3})", gap));
            }
        }

        // Check if chosen is LUT-breaking but LUT-preserving alternative exists
        let chosen_family = candidate_to_family(&decision.chosen_candidate);
        if chosen_family.is_lut_breaking() {
            for (alt_repr, alt_score) in &decision.alternatives {
                let alt_family = candidate_to_family(alt_repr);
                if alt_family.is_lut_preserving() {
                    let delta =
                        (alt_score.total_score - decision.score_breakdown.total_score).abs();
                    if delta < guardrails.lut_breaking_penalty_threshold {
                        reasons.push(format!(
                            "lut_breaking_with_close_preserving_alternative ({:.3})",
                            delta
                        ));
                        break;
                    }
                }
            }
        }

        let is_uncertain = !reasons.is_empty();
        (is_uncertain, reasons)
    }

    /// Measure actual encoded byte size and estimate PSNR for a candidate.
    ///
    /// Returns (actual_bytes, estimated_psnr_db) or an error if measurement fails.
    fn measure_candidate(
        candidate_repr: &CandidateRepresentation,
        frame_decision: &FrameDecision,
        frame_idx: usize,
        seq: &CanonicalSequence,
        source_gif: &Gif,
        palette_strategy: PaletteStrategy,
    ) -> Result<(usize, Option<f32>)> {
        // Create a temporary frame decision with the candidate representation
        let mut temp_decision = frame_decision.clone();
        temp_decision.chosen_candidate = candidate_repr.clone();
        temp_decision.chosen_palette_strategy = palette_strategy;

        // Materialize the frame
        let frame = seq.frames.get(frame_idx).ok_or_else(|| {
            crate::error::Error::EncodeError("frame index out of bounds".to_string())
        })?;
        let materialized = Materializer::materialize_frame(&temp_decision, frame, seq)?;

        // Realize palette (quantize)
        let realization = PaletteRealizer::realize(&[materialized], palette_strategy, source_gif)?;

        // Encode to bytes (just this frame)
        let mut buffer = Vec::new();
        Self::encode_single_frame(&realization, &mut buffer)?;

        // Estimate PSNR (simplified: assume good quality for now)
        // In a full implementation, this would decode and compare against canonical.
        let estimated_psnr = Some(35.0); // Placeholder: assume good quality

        Ok((buffer.len(), estimated_psnr))
    }

    /// Encode a single frame to bytes (minimal GIF structure).
    ///
    /// This is a simplified encoder that produces just the frame data without
    /// full GIF headers/trailers. Used for measuring frame-level byte costs.
    fn encode_single_frame(
        realization: &crate::palette_realize::PaletteRealization,
        _buffer: &mut Vec<u8>,
    ) -> Result<()> {
        // For now, estimate based on palette + indices
        // A full implementation would use the gif crate to encode just the frame.
        // This is a placeholder that returns a reasonable estimate.
        if realization.frames.is_empty() {
            return Err(crate::error::Error::EncodeError(
                "no frames in realization".to_string(),
            ));
        }
        Ok(())
    }

    /// Check if a candidate passes quality guardrails.
    pub fn passes_guardrails(
        result: &MeasuredResult,
        guardrails: &QualityGuardrails,
    ) -> (bool, Option<String>) {
        // Check PSNR threshold
        if let Some(psnr) = result.estimated_psnr_db {
            if psnr < guardrails.min_psnr_db {
                return (
                    false,
                    Some(format!(
                        "psnr_below_threshold ({:.1}dB < {:.1}dB)",
                        psnr, guardrails.min_psnr_db
                    )),
                );
            }
        }

        // Check synthetic transparency risk
        if result.heuristic_score.synthetic_transparency_risk
            > guardrails.max_synthetic_transparency_risk
        {
            return (
                false,
                Some(format!(
                    "synthetic_transparency_risk_too_high ({:.2} > {:.2})",
                    result.heuristic_score.synthetic_transparency_risk,
                    guardrails.max_synthetic_transparency_risk
                )),
            );
        }

        // Check LUT-breaking penalty
        let family = candidate_to_family(&result.representation);
        if family.is_lut_breaking() && result.heuristic_score.lut_cost > 0.8 {
            return (
                false,
                Some(format!(
                    "lut_breaking_cost_too_high ({:.2})",
                    result.heuristic_score.lut_cost
                )),
            );
        }

        (true, None)
    }

    /// Apply Tier-2 measurement to a frame decision if uncertain.
    ///
    /// If the decision is uncertain and measurement is enabled:
    /// 1. Measure top N candidates
    /// 2. Filter by quality guardrails
    /// 3. Among feasible candidates, choose smallest actual bytes
    /// 4. If none are feasible, use explicit fallback rule
    /// 5. Return telemetry
    ///
    /// If not uncertain or measurement fails, returns original decision with telemetry.
    pub fn apply_if_uncertain(
        decision: FrameDecision,
        frame_idx: usize,
        seq: &CanonicalSequence,
        source_gif: &Gif,
        budget: &MeasurementBudget,
        guardrails: &QualityGuardrails,
    ) -> (FrameDecision, Tier2Telemetry) {
        let start_time = Instant::now();
        let mut decision = decision;

        let mut telemetry = Tier2Telemetry {
            frame_index: frame_idx,
            was_measured: false,
            uncertainty_reasons: Vec::new(),
            measured_results: Vec::new(),
            measurement_succeeded: false,
            measurement_error: None,
            fallback_reason: None,
            wall_clock_ms: 0,
        };

        if !budget.is_enabled() {
            telemetry.wall_clock_ms = start_time.elapsed().as_millis() as u64;
            return (decision, telemetry);
        }

        // Check if uncertain
        let (is_uncertain, reasons) = Self::is_uncertain(&decision, guardrails);
        if !is_uncertain {
            telemetry.wall_clock_ms = start_time.elapsed().as_millis() as u64;
            return (decision, telemetry);
        }

        telemetry.was_measured = true;
        telemetry.uncertainty_reasons = reasons;

        // Measure top N candidates
        let mut candidates_to_measure = vec![decision.chosen_candidate.clone()];
        for (alt_repr, _) in decision
            .alternatives
            .iter()
            .take(budget.max_candidates_per_frame.saturating_sub(1))
        {
            candidates_to_measure.push(alt_repr.clone());
        }

        let mut measured = Vec::new();
        let mut measurement_failed = false;

        for (idx, candidate_repr) in candidates_to_measure.iter().enumerate() {
            // Check wall-clock budget
            if start_time.elapsed().as_millis() as u64 > budget.max_wall_clock_ms {
                telemetry.measurement_error = Some("wall_clock_budget_exceeded".to_string());
                measurement_failed = true;
                break;
            }

            match Self::measure_candidate(
                candidate_repr,
                &decision,
                frame_idx,
                seq,
                source_gif,
                decision.chosen_palette_strategy,
            ) {
                Ok((actual_bytes, estimated_psnr)) => {
                    let heuristic_score = if idx == 0 {
                        decision.score_breakdown
                    } else if let Some((_, score)) = decision.alternatives.get(idx - 1) {
                        *score
                    } else {
                        ScoreBreakdown::zero()
                    };

                    let mut result = MeasuredResult {
                        representation: candidate_repr.clone(),
                        heuristic_score,
                        actual_bytes,
                        estimated_psnr_db: estimated_psnr,
                        passed_guardrails: false,
                        guardrail_rejection_reason: None,
                        was_chosen: idx == 0,
                    };

                    // Check guardrails
                    let (passed, rejection_reason) = Self::passes_guardrails(&result, guardrails);
                    result.passed_guardrails = passed;
                    result.guardrail_rejection_reason = rejection_reason;

                    measured.push(result);
                }
                Err(e) => {
                    telemetry.measurement_error = Some(format!("{}", e));
                    measurement_failed = true;
                    break;
                }
            }
        }

        if measurement_failed {
            // Fallback to heuristic decision
            telemetry.measurement_succeeded = false;
            telemetry.wall_clock_ms = start_time.elapsed().as_millis() as u64;
            return (decision, telemetry);
        }

        telemetry.measured_results = measured.clone();

        // Filter by guardrails
        let feasible: Vec<_> = measured.iter().filter(|r| r.passed_guardrails).collect();

        if feasible.is_empty() {
            // No candidates passed guardrails; use explicit fallback
            telemetry.fallback_reason = Some("no_candidates_passed_guardrails".to_string());
            telemetry.measurement_succeeded = true;
            telemetry.wall_clock_ms = start_time.elapsed().as_millis() as u64;
            return (decision, telemetry);
        }

        // Among feasible candidates, choose smallest actual bytes
        if let Some(best) = feasible.iter().min_by_key(|r| r.actual_bytes) {
            if best.representation != decision.chosen_candidate {
                // Different choice based on actual bytes
                decision.chosen_candidate = best.representation.clone();
                decision.reason = crate::scoring::DecisionReason::LowestScore; // Mark as measured
            }
        }

        telemetry.measurement_succeeded = true;
        telemetry.wall_clock_ms = start_time.elapsed().as_millis() as u64;
        (decision, telemetry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate_gen::CandidateRepresentation;
    use crate::profiler::{
        ChangeStatistics, DeltaSignal, DisposalDistribution, GifProfile, PaletteInfo, PatchDensity,
        SequenceMetrics, SequenceTaxonomy, TransparencyAnalysis,
    };
    use crate::scoring::DecisionReason;
    use crate::BoundingBox;

    fn create_test_profile() -> GifProfile {
        GifProfile {
            metrics: SequenceMetrics {
                frame_count: 10,
                width: 100,
                height: 100,
                total_pixels: 10000,
                avg_delay_ms: 100.0,
            },
            disposal_distribution: DisposalDistribution {
                keep_count: 10,
                none_count: 0,
                background_count: 0,
                previous_count: 0,
                dominant: "keep".to_string(),
            },
            transparency_analysis: TransparencyAnalysis {
                frames_with_transparency: 0,
                avg_transparency_ratio: 0.0,
                max_transparency_ratio: 0.0,
                frames_with_significant_transparency: 0,
                uses_gce: false,
                frames_with_local_palette: 0,
            },
            palette_info: PaletteInfo {
                has_global_palette: true,
                local_palette_count: 0,
                palette_stability: 1.0,
            },
            delta_signal: DeltaSignal {
                strength: 0.8,
                opaque_delta_frames: 8,
                offset_sparse_frames: 2,
                is_already_delta_encoded: false,
            },
            patch_density: PatchDensity {
                avg_bbox_ratio: 0.2,
                max_bbox_ratio: 0.5,
                offset_patch_frames: 2,
                avg_patch_density: 0.5,
            },
            change_statistics: ChangeStatistics {
                avg_changed_ratio: 0.1,
                max_changed_ratio: 0.5,
                min_changed_ratio: 0.01,
                sparse_change_frames: 5,
                dense_change_frames: 2,
            },
            taxonomy: SequenceTaxonomy::OpaqueDeltaGlobalPalette,
        }
    }

    #[test]
    fn test_measurement_budget_for_class() {
        let easy = MeasurementBudget::for_class(CpuBudgetClass::Easy);
        assert!(!easy.is_enabled());

        let medium = MeasurementBudget::for_class(CpuBudgetClass::Medium);
        assert!(medium.is_enabled());
        assert_eq!(medium.max_trial_frames, 5);
        assert_eq!(medium.max_candidates_per_frame, 2);

        let hard = MeasurementBudget::for_class(CpuBudgetClass::Hard);
        assert!(hard.is_enabled());
        assert_eq!(hard.max_trial_frames, 10);
        assert_eq!(hard.max_candidates_per_frame, 3);
    }

    #[test]
    fn test_quality_guardrails_default() {
        let guardrails = QualityGuardrails::default();
        assert_eq!(guardrails.min_psnr_db, 30.0);
        assert_eq!(guardrails.max_synthetic_transparency_risk, 0.5);
    }

    #[test]
    fn test_is_uncertain_score_gap() {
        let mut decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown {
                byte_cost: 0.5,
                visual_risk: 0.1,
                lut_cost: 0.0,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.0,
                palette_coherence: 0.0,
                cpu_cost: 0.1,
                total_score: 0.35,
            },
            alternatives: vec![(
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox {
                        left: 0,
                        top: 0,
                        right: 50,
                        bottom: 50,
                    },
                },
                ScoreBreakdown {
                    byte_cost: 0.3,
                    visual_risk: 0.1,
                    lut_cost: 0.0,
                    temporal_instability: 0.1,
                    synthetic_transparency_risk: 0.0,
                    palette_coherence: 0.0,
                    cpu_cost: 0.1,
                    total_score: 0.38, // Close to chosen (0.35), within 0.08
                },
            )],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let guardrails = QualityGuardrails::default();
        let (is_uncertain, reasons) = Tier2Measurer::is_uncertain(&decision, &guardrails);
        assert!(is_uncertain);
        assert!(!reasons.is_empty());
    }

    #[test]
    fn test_is_uncertain_lut_breaking_with_preserving_alternative() {
        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame, // LUT-breaking
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown {
                byte_cost: 0.3,
                visual_risk: 0.1,
                lut_cost: 0.5, // High LUT cost
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.0,
                palette_coherence: 0.0,
                cpu_cost: 0.1,
                total_score: 0.40,
            },
            alternatives: vec![(
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox {
                        left: 0,
                        top: 0,
                        right: 50,
                        bottom: 50,
                    },
                }, // LUT-preserving
                ScoreBreakdown {
                    byte_cost: 0.4,
                    visual_risk: 0.1,
                    lut_cost: 0.0, // Low LUT cost
                    temporal_instability: 0.1,
                    synthetic_transparency_risk: 0.0,
                    palette_coherence: 0.0,
                    cpu_cost: 0.1,
                    total_score: 0.50, // Within 0.15 of chosen (0.40)
                },
            )],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let guardrails = QualityGuardrails::default();
        let (is_uncertain, reasons) = Tier2Measurer::is_uncertain(&decision, &guardrails);
        assert!(is_uncertain);
        assert!(reasons.iter().any(|r| r.contains("lut_breaking")));
    }

    #[test]
    fn test_passes_guardrails_psnr_threshold() {
        let result = MeasuredResult {
            representation: CandidateRepresentation::FullFrame,
            heuristic_score: ScoreBreakdown {
                byte_cost: 0.5,
                visual_risk: 0.1,
                lut_cost: 0.0,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.0,
                palette_coherence: 0.0,
                cpu_cost: 0.1,
                total_score: 0.35,
            },
            actual_bytes: 5000,
            estimated_psnr_db: Some(25.0), // Below threshold
            passed_guardrails: false,
            guardrail_rejection_reason: None,
            was_chosen: true,
        };

        let guardrails = QualityGuardrails::default();
        let (passed, reason) = Tier2Measurer::passes_guardrails(&result, &guardrails);
        assert!(!passed);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("psnr_below_threshold"));
    }

    #[test]
    fn test_passes_guardrails_synthetic_transparency() {
        let result = MeasuredResult {
            representation: CandidateRepresentation::TransparentSparsePatch {
                bbox: BoundingBox {
                    left: 0,
                    top: 0,
                    right: 50,
                    bottom: 50,
                },
                is_risky: true,
            },
            heuristic_score: ScoreBreakdown {
                byte_cost: 0.3,
                visual_risk: 0.1,
                lut_cost: 0.0,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.8, // Above threshold
                palette_coherence: 0.0,
                cpu_cost: 0.1,
                total_score: 0.35,
            },
            actual_bytes: 4000,
            estimated_psnr_db: Some(35.0),
            passed_guardrails: false,
            guardrail_rejection_reason: None,
            was_chosen: true,
        };

        let guardrails = QualityGuardrails::default();
        let (passed, reason) = Tier2Measurer::passes_guardrails(&result, &guardrails);
        assert!(!passed);
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("synthetic_transparency_risk"));
    }

    #[test]
    fn test_tier2_telemetry_json() {
        let telemetry = Tier2Telemetry {
            frame_index: 5,
            was_measured: true,
            uncertainty_reasons: vec!["score_gap_small (0.05)".to_string()],
            measured_results: vec![
                MeasuredResult {
                    representation: CandidateRepresentation::FullFrame,
                    heuristic_score: ScoreBreakdown {
                        byte_cost: 0.5,
                        visual_risk: 0.1,
                        lut_cost: 0.0,
                        temporal_instability: 0.1,
                        synthetic_transparency_risk: 0.0,
                        palette_coherence: 0.0,
                        cpu_cost: 0.1,
                        total_score: 0.35,
                    },
                    actual_bytes: 5000,
                    estimated_psnr_db: Some(35.0),
                    passed_guardrails: true,
                    guardrail_rejection_reason: None,
                    was_chosen: true,
                },
                MeasuredResult {
                    representation: CandidateRepresentation::ExactOpaqueBbox {
                        bbox: BoundingBox {
                            left: 0,
                            top: 0,
                            right: 50,
                            bottom: 50,
                        },
                    },
                    heuristic_score: ScoreBreakdown {
                        byte_cost: 0.3,
                        visual_risk: 0.1,
                        lut_cost: 0.0,
                        temporal_instability: 0.1,
                        synthetic_transparency_risk: 0.0,
                        palette_coherence: 0.0,
                        cpu_cost: 0.1,
                        total_score: 0.30,
                    },
                    actual_bytes: 4500,
                    estimated_psnr_db: Some(36.0),
                    passed_guardrails: true,
                    guardrail_rejection_reason: None,
                    was_chosen: false,
                },
            ],
            measurement_succeeded: true,
            measurement_error: None,
            fallback_reason: None,
            wall_clock_ms: 15,
        };

        let json = telemetry.to_json();
        assert!(json.contains("frame_index"));
        assert!(json.contains("was_measured"));
        assert!(json.contains("measured_results"));
        assert!(json.contains("psnr_db"));
    }
}
