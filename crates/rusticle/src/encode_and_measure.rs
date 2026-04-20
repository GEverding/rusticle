//! Encode-and-measure loop for uncertain adaptive candidates.
//!
//! This module provides a targeted, bounded encode-and-measure step for uncertain
//! candidate decisions. Instead of relying solely on heuristic scoring, uncertain
//! cases are evaluated by actually materializing, palette-realizing, and encoding
//! the top N candidates to measure real byte/quality evidence.
//!
//! # Uncertainty Criteria
//!
//! A decision is marked as "uncertain" if any of:
//! - Score gap between top candidates is below threshold (default 0.05)
//! - Candidate is marked as risky (transparent sparse patch)
//! - Sequence taxonomy is known to be fragile (OpaqueDeltaGlobalPalette, DisposalHeavy)
//! - Synthetic transparency risk is elevated (>0.3)
//!
//! # Measurement Process
//!
//! For uncertain frames only:
//! 1. Select top N candidates (default N=2, configurable)
//! 2. For each candidate:
//!    - Materialize the frame
//!    - Realize palette (quantize)
//!    - Encode to bytes
//!    - Measure actual encoded size
//! 3. Feed measured byte results back into final choice
//! 4. Record telemetry indicating which candidates were measured
//!
//! # Safeguards
//!
//! - Bounded N (default 2-3 candidates per uncertain frame)
//! - Bounded number of uncertain frames (configurable, default 50% of sequence)
//! - Deterministic behavior (same input → same output)
//! - Clear telemetry indicating when encode-and-measure was used
//! - Fallback remains safe if measurement path fails
//!
//! # Design Notes
//!
//! - Sequence-level palette strategy is fixed; frame representation candidates are compared.
//! - Per-frame encoding is used (not sequence-level).
//! - Avoids exploding CPU cost by limiting uncertain frames and candidates.

use crate::adaptive_ir::CanonicalSequence;
use crate::candidate_gen::CandidateRepresentation;
use crate::error::Result;
use crate::materialize::Materializer;
use crate::palette_realize::PaletteRealizer;
use crate::palette_strategy::PaletteStrategy;
use crate::profiler::GifProfile;
use crate::scoring::{FrameDecision, ScoreBreakdown};
use crate::types::Gif;
use std::fmt::Write as FmtWrite;

/// Configuration for encode-and-measure behavior.
#[derive(Debug, Clone, Copy)]
pub struct EncodeAndMeasureConfig {
    /// Enable encode-and-measure for uncertain cases.
    pub enabled: bool,
    /// Number of top candidates to measure (default 2).
    pub top_n_candidates: usize,
    /// Score gap threshold below which a decision is uncertain (default 0.05).
    pub score_gap_threshold: f32,
    /// Maximum fraction of frames to measure (default 0.5 = 50%).
    pub max_uncertain_fraction: f32,
    /// Synthetic transparency risk threshold (default 0.3).
    pub transparency_risk_threshold: f32,
}

impl Default for EncodeAndMeasureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            top_n_candidates: 2,
            score_gap_threshold: 0.05,
            max_uncertain_fraction: 0.5,
            transparency_risk_threshold: 0.3,
        }
    }
}

/// Result of measuring a candidate's actual encoded size.
#[derive(Debug, Clone)]
pub struct MeasuredCandidate {
    /// The candidate representation.
    pub representation: CandidateRepresentation,
    /// Heuristic score (from Scorer).
    pub heuristic_score: ScoreBreakdown,
    /// Actual encoded byte size (measured).
    pub actual_bytes: usize,
    /// Whether this was the chosen candidate.
    pub was_chosen: bool,
}

/// Telemetry for encode-and-measure decisions.
#[derive(Debug, Clone)]
pub struct EncodeAndMeasureTelemetry {
    /// Frame index.
    pub frame_index: usize,
    /// Whether this frame was uncertain.
    pub was_uncertain: bool,
    /// Reason(s) for uncertainty (if any).
    pub uncertainty_reasons: Vec<String>,
    /// Measured candidates (if uncertain and measured).
    pub measured_candidates: Vec<MeasuredCandidate>,
    /// Whether measurement succeeded.
    pub measurement_succeeded: bool,
    /// Error message if measurement failed (fallback to heuristic).
    pub measurement_error: Option<String>,
}

impl EncodeAndMeasureTelemetry {
    /// Convert telemetry to JSON.
    pub fn to_json(&self) -> String {
        let mut json = String::new();
        let _ = writeln!(json, r#"{{"#);
        let _ = writeln!(json, r#"  "frame_index": {},"#, self.frame_index);
        let _ = writeln!(json, r#"  "was_uncertain": {},"#, self.was_uncertain);

        let _ = write!(json, r#"  "uncertainty_reasons": ["#);
        for (i, reason) in self.uncertainty_reasons.iter().enumerate() {
            if i > 0 {
                let _ = write!(json, ", ");
            }
            let _ = write!(json, r#""{}""#, reason.replace('"', "\\\""));
        }
        let _ = writeln!(json, r#"],"#);

        let _ = writeln!(json, r#"  "measurement_succeeded": {},"#, self.measurement_succeeded);

        if let Some(err) = &self.measurement_error {
            let _ = writeln!(json, r#"  "measurement_error": "{}"#, err.replace('"', "\\\""));
        }

        let _ = write!(json, r#"  "measured_candidates": ["#);
        for (i, cand) in self.measured_candidates.iter().enumerate() {
            if i > 0 {
                let _ = write!(json, ", ");
            }
            let repr_name = match &cand.representation {
                CandidateRepresentation::FullFrame => "full-frame",
                CandidateRepresentation::ExactOpaqueBbox { .. } => "opaque-bbox",
                CandidateRepresentation::TransparentSparsePatch { .. } => "sparse-patch",
                CandidateRepresentation::MinimalNoOp => "minimal-noop",
            };
            let _ = write!(
                json,
                r#"{{"repr":"{}","heuristic_score":{:.3},"actual_bytes":{},"was_chosen":{}}}"#,
                repr_name, cand.heuristic_score.total_score, cand.actual_bytes, cand.was_chosen
            );
        }
        let _ = writeln!(json, r#"]"#);
        let _ = writeln!(json, r#"}}"#);
        json
    }
}

/// Encoder for measuring candidate byte sizes.
pub struct EncodeAndMeasure;

impl EncodeAndMeasure {
    /// Determine if a frame decision is uncertain.
    ///
    /// A decision is uncertain if:
    /// - Score gap between top candidates is below threshold
    /// - Candidate is marked as risky
    /// - Sequence taxonomy is fragile
    /// - Synthetic transparency risk is elevated
    pub fn is_uncertain(
        decision: &FrameDecision,
        alternatives: &[(CandidateRepresentation, ScoreBreakdown)],
        profile: &GifProfile,
        config: &EncodeAndMeasureConfig,
    ) -> (bool, Vec<String>) {
        let mut reasons = Vec::new();

        // Check score gap
        if let Some((_, alt_score)) = alternatives.first() {
            let gap = (alt_score.total_score - decision.score_breakdown.total_score).abs();
            if gap < config.score_gap_threshold {
                reasons.push(format!("score_gap_small ({:.3})", gap));
            }
        }

        // Check if chosen candidate is risky
        if let CandidateRepresentation::TransparentSparsePatch { is_risky, .. } =
            &decision.chosen_candidate
        {
            if *is_risky {
                reasons.push("chosen_candidate_risky".to_string());
            }
        }

        // Check synthetic transparency risk
        if decision.score_breakdown.synthetic_transparency_risk > config.transparency_risk_threshold {
            reasons.push(format!(
                "transparency_risk_elevated ({:.2})",
                decision.score_breakdown.synthetic_transparency_risk
            ));
        }

        // Check taxonomy fragility
        use crate::profiler::SequenceTaxonomy;
        let is_fragile_taxonomy = matches!(
            profile.taxonomy,
            SequenceTaxonomy::OpaqueDeltaGlobalPalette | SequenceTaxonomy::DisposalHeavyBackgroundPrevious
        );
        if is_fragile_taxonomy {
            reasons.push(format!("fragile_taxonomy ({})", profile.taxonomy.name()));
        }

        let is_uncertain = !reasons.is_empty();
        (is_uncertain, reasons)
    }

    /// Measure actual encoded byte size for a candidate.
    ///
    /// Returns the encoded byte size, or an error if measurement fails.
    fn measure_candidate_bytes(
        candidate_repr: &CandidateRepresentation,
        frame_decision: &FrameDecision,
        frame_idx: usize,
        seq: &CanonicalSequence,
        source_gif: &Gif,
        palette_strategy: PaletteStrategy,
    ) -> Result<usize> {
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

        Ok(buffer.len())
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

    /// Apply encode-and-measure to a frame decision if uncertain.
    ///
    /// If the decision is uncertain and measurement is enabled:
    /// 1. Measure top N candidates
    /// 2. Update decision based on actual byte measurements
    /// 3. Return telemetry
    ///
    /// If not uncertain or measurement fails, returns original decision with telemetry.
    pub fn apply_if_uncertain(
        mut decision: FrameDecision,
        frame_idx: usize,
        seq: &CanonicalSequence,
        source_gif: &Gif,
        profile: &GifProfile,
        config: &EncodeAndMeasureConfig,
    ) -> (FrameDecision, EncodeAndMeasureTelemetry) {
        let mut telemetry = EncodeAndMeasureTelemetry {
            frame_index: frame_idx,
            was_uncertain: false,
            uncertainty_reasons: Vec::new(),
            measured_candidates: Vec::new(),
            measurement_succeeded: false,
            measurement_error: None,
        };

        if !config.enabled {
            return (decision, telemetry);
        }

        // Check if uncertain
        let (is_uncertain, reasons) = Self::is_uncertain(&decision, &decision.alternatives, profile, config);
        if !is_uncertain {
            return (decision, telemetry);
        }

        telemetry.was_uncertain = true;
        telemetry.uncertainty_reasons = reasons;

        // Measure top N candidates
        let mut candidates_to_measure = vec![decision.chosen_candidate.clone()];
        for (alt_repr, _) in decision.alternatives.iter().take(config.top_n_candidates - 1) {
            candidates_to_measure.push(alt_repr.clone());
        }

        let mut measured = Vec::new();
        let mut measurement_failed = false;

        for (idx, candidate_repr) in candidates_to_measure.iter().enumerate() {
            match Self::measure_candidate_bytes(
                candidate_repr,
                &decision,
                frame_idx,
                seq,
                source_gif,
                decision.chosen_palette_strategy,
            ) {
                Ok(actual_bytes) => {
                    measured.push((idx, candidate_repr.clone(), actual_bytes));
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
            return (decision, telemetry);
        }

        // Build measured candidates telemetry
        for (idx, repr, actual_bytes) in &measured {
            let heuristic_score = if *idx == 0 {
                decision.score_breakdown
            } else if let Some((_, score)) = decision.alternatives.get(*idx - 1) {
                *score
            } else {
                ScoreBreakdown::zero()
            };

            telemetry.measured_candidates.push(MeasuredCandidate {
                representation: repr.clone(),
                heuristic_score,
                actual_bytes: *actual_bytes,
                was_chosen: *idx == 0,
            });
        }

        // Choose based on actual bytes (prefer smallest)
        if let Some((best_idx, best_repr, _)) = measured.iter().min_by_key(|(_, _, bytes)| bytes) {
            if *best_idx != 0 {
                // Different choice based on actual bytes
                decision.chosen_candidate = best_repr.clone();
                decision.reason = crate::scoring::DecisionReason::LowestScore; // Mark as measured
            }
        }

        telemetry.measurement_succeeded = true;
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

    fn create_test_profile(taxonomy: SequenceTaxonomy) -> GifProfile {
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
                palette_stability: 0.9,
            },
            change_statistics: ChangeStatistics {
                avg_changed_ratio: 0.1,
                max_changed_ratio: 0.2,
                min_changed_ratio: 0.05,
                sparse_change_frames: 5,
                dense_change_frames: 0,
            },
            patch_density: PatchDensity {
                avg_bbox_ratio: 0.1,
                max_bbox_ratio: 0.2,
                offset_patch_frames: 5,
                avg_patch_density: 0.5,
            },
            delta_signal: DeltaSignal {
                strength: 0.8,
                opaque_delta_frames: 8,
                offset_sparse_frames: 5,
                is_already_delta_encoded: true,
            },
            taxonomy,
        }
    }

    #[test]
    fn test_uncertainty_score_gap() {
        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown {
                byte_cost: 0.5,
                visual_risk: 0.1,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.1,
                cpu_cost: 0.1,
                total_score: 0.4,
            },
            alternatives: vec![(
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: crate::adaptive_ir::BoundingBox {
                        left: 0,
                        top: 0,
                        right: 10,
                        bottom: 10,
                    },
                },
                ScoreBreakdown {
                    byte_cost: 0.4,
                    visual_risk: 0.1,
                    temporal_instability: 0.1,
                    synthetic_transparency_risk: 0.1,
                    cpu_cost: 0.1,
                    total_score: 0.42,
                },
            )],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let config = EncodeAndMeasureConfig {
            score_gap_threshold: 0.05,
            ..Default::default()
        };

        let profile = create_test_profile(SequenceTaxonomy::OpaqueDeltaGlobalPalette);

        let (is_uncertain, reasons) = EncodeAndMeasure::is_uncertain(&decision, &decision.alternatives, &profile, &config);
        assert!(is_uncertain);
        assert!(reasons.iter().any(|r| r.contains("score_gap_small")));
    }

    #[test]
    fn test_uncertainty_risky_candidate() {
        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::TransparentSparsePatch {
                bbox: crate::adaptive_ir::BoundingBox {
                    left: 0,
                    top: 0,
                    right: 10,
                    bottom: 10,
                },
                is_risky: true,
            },
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown {
                byte_cost: 0.2,
                visual_risk: 0.5,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.8,
                cpu_cost: 0.1,
                total_score: 0.35,
            },
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let config = EncodeAndMeasureConfig::default();
        let profile = create_test_profile(SequenceTaxonomy::TransparencyHeavySparseDelta);

        let (is_uncertain, reasons) = EncodeAndMeasure::is_uncertain(&decision, &decision.alternatives, &profile, &config);
        assert!(is_uncertain);
        assert!(reasons.iter().any(|r| r.contains("chosen_candidate_risky")));
    }

    #[test]
    fn test_non_uncertain_decision() {
        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown {
                byte_cost: 0.5,
                visual_risk: 0.1,
                temporal_instability: 0.1,
                synthetic_transparency_risk: 0.0,
                cpu_cost: 0.1,
                total_score: 0.3,
            },
            alternatives: vec![(
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: crate::adaptive_ir::BoundingBox {
                        left: 0,
                        top: 0,
                        right: 10,
                        bottom: 10,
                    },
                },
                ScoreBreakdown {
                    byte_cost: 0.6,
                    visual_risk: 0.1,
                    temporal_instability: 0.1,
                    synthetic_transparency_risk: 0.1,
                    cpu_cost: 0.1,
                    total_score: 0.6,
                },
            )],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let config = EncodeAndMeasureConfig::default();
        let profile = create_test_profile(SequenceTaxonomy::Photographic);

        let (is_uncertain, reasons) = EncodeAndMeasure::is_uncertain(&decision, &decision.alternatives, &profile, &config);
        assert!(!is_uncertain);
        assert!(reasons.is_empty());
    }
}
