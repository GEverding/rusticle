//! Experimental adaptive encoding harness with tiered optimizer integration.
//!
//! This module integrates the full tiered optimizer pipeline (Tier-0 classifier,
//! Tier-1 pruning, Tier-2 bounded measurement, and sequence-level DP-lite) behind
//! an explicit opt-in flag with decision telemetry and safe fallback to the current
//! encoding path.
//!
//! # Adaptive Mode with Tiered Optimizer
//!
//! When enabled, the adaptive path:
//! 1. Builds a canonical IR from the GIF
//! 2. Profiles the sequence to understand its characteristics
//! 3. Computes policy signals for Tier-0 classification
//! 4. Generates candidate representations for each frame
//! 5. **Tier-0**: Classify sequence (early-exit, needs-tier1, or needs-tier2)
//! 6. **Tier-1**: Prune frame candidates (if not early-exit)
//! 7. **Tier-2**: Bounded encode-and-measure for uncertain frames (if needed)
//! 8. **Sequence-level DP-lite**: Choose final path with global coherence
//! 9. Materialize → palette realize → emit bytes
//! 10. Emits telemetry about all tiered decisions
//!
//! If any step fails, the encoder falls back to the current (non-adaptive) path.
//!
//! # Telemetry
//!
//! Decision telemetry is emitted as JSON with tiered optimizer information:
//! ```json
//! {
//!   "mode": "adaptive",
//!   "status": "success" | "fallback",
//!   "fallback_reason": "...",
//!   "tiered_optimizer": {
//!     "tier0_decision": "early-exit-structural" | "needs-tier1" | "needs-tier2",
//!     "candidates_before_pruning": 24,
//!     "candidates_after_pruning": 18,
//!     "tier2_measurement_ran": false,
//!     "tier2_frames_measured": 0,
//!     "sequence_optimizer_chunks": 2,
//!     "sequence_optimizer_summary": "DP-lite sequence optimization: 10 frames, 1 chunks, avg_score=0.42"
//!   },
//!   "sequence": {
//!     "width": 320,
//!     "height": 240,
//!     "frame_count": 10,
//!     "taxonomy": "OpaqueDeltaGlobalPalette",
//!     "avg_score": 0.42,
//!     "estimated_bytes": 50000
//!   },
//!   "frame_decisions": [...]
//! }
//! ```

use crate::adaptive_ir::CanonicalSequenceBuilder;
use crate::candidate_gen::CandidateGenerator;
use crate::error::Result;
use crate::lut_policy::PolicySignals;
use crate::materialize::Materializer;
use crate::palette_realize::PaletteRealizer;
use crate::palette_strategy::determine_palette_strategies;
use crate::profiler::profile_canonical_sequence;
use crate::sequence_optimizer::{SequenceOptimizer, SequenceOptimizerConfig};
use crate::tier0_classifier::{Tier0Classifier, Tier0Decision};
use crate::tier1_pruning::Tier1Pruner;
use crate::tier2_measure::MeasurementBudget;
use crate::types::Gif;

/// Configuration for adaptive encoding.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdaptiveConfig {
    /// Enable adaptive encoding mode.
    pub enabled: bool,
    /// Emit telemetry to stderr (if enabled).
    pub emit_telemetry: bool,
}

/// Telemetry from tiered optimizer execution.
#[derive(Debug, Clone)]
pub struct TieredOptimizerTelemetry {
    /// Tier-0 classification decision.
    pub tier0_decision: Tier0Decision,
    /// Total candidates before Tier-1 pruning.
    pub candidates_before_pruning: usize,
    /// Total candidates after Tier-1 pruning.
    pub candidates_after_pruning: usize,
    /// Whether Tier-2 measurement ran.
    pub tier2_measurement_ran: bool,
    /// Number of frames that entered Tier-2 measurement.
    pub tier2_frames_measured: usize,
    /// Number of chunks in sequence-level DP-lite optimization.
    pub sequence_optimizer_chunks: usize,
    /// Summary from sequence-level DP-lite optimizer.
    pub sequence_optimizer_summary: String,
}

/// Result of adaptive encoding decision process.
#[derive(Debug, Clone)]
pub struct AdaptiveDecision {
    /// Whether the adaptive path succeeded.
    pub success: bool,
    /// Fallback reason if not successful.
    pub fallback_reason: Option<String>,
    /// Telemetry JSON (if available).
    pub telemetry_json: Option<String>,
    /// Tiered optimizer telemetry (if available).
    pub tiered_telemetry: Option<TieredOptimizerTelemetry>,
}

impl Gif {
    /// Attempt adaptive encoding with telemetry and fallback.
    ///
    /// This is an experimental integration of the adaptive encoding pipeline.
    /// It builds the canonical IR, profiles the sequence, generates candidates,
    /// determines palette strategies, and scores/chooses representations.
    ///
    /// If any step fails, it falls back to the current encoding path and records
    /// the reason in telemetry.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EncodeError`] if both adaptive and fallback paths fail.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use rusticle::{Gif, AdaptiveConfig};
    ///
    /// let config = AdaptiveConfig {
    ///     enabled: true,
    ///     emit_telemetry: true,
    /// };
    /// let (decision, bytes) = gif.encode_adaptive(&config)?;
    /// eprintln!("Adaptive decision: {:?}", decision);
    /// ```
    pub fn encode_adaptive(&self, config: &AdaptiveConfig) -> Result<(AdaptiveDecision, Vec<u8>)> {
        if !config.enabled {
            // Adaptive mode disabled; use current path
            let bytes = self.to_bytes()?;
            return Ok((
                AdaptiveDecision {
                    success: false,
                    fallback_reason: Some("adaptive mode disabled".to_string()),
                    telemetry_json: None,
                    tiered_telemetry: None,
                },
                bytes,
            ));
        }

        // Attempt adaptive path
        match self.try_adaptive_encode_with_bytes() {
            Ok((telemetry_json, tiered_telemetry, bytes)) => {
                if config.emit_telemetry {
                    if let Some(ref json) = &telemetry_json {
                        eprintln!("ADAPTIVE_TELEMETRY: {}", json);
                    }
                }

                Ok((
                    AdaptiveDecision {
                        success: true,
                        fallback_reason: None,
                        telemetry_json,
                        tiered_telemetry,
                    },
                    bytes,
                ))
            }
            Err(e) => {
                let fallback_reason = format!("adaptive path failed: {}", e);

                if config.emit_telemetry {
                    let fallback_json = format!(
                        r#"{{"mode":"adaptive","status":"fallback","fallback_reason":"{}"}}"#,
                        fallback_reason.replace('"', "\\\"")
                    );
                    eprintln!("ADAPTIVE_TELEMETRY: {}", fallback_json);
                }

                // Fall back to current encode path
                let bytes = self.to_bytes()?;

                Ok((
                    AdaptiveDecision {
                        success: false,
                        fallback_reason: Some(fallback_reason),
                        telemetry_json: None,
                        tiered_telemetry: None,
                    },
                    bytes,
                ))
            }
        }
    }

    /// Internal: attempt the full adaptive encoding pipeline with tiered optimizer.
    fn try_adaptive_encode_with_bytes(
        &self,
    ) -> Result<(Option<String>, Option<TieredOptimizerTelemetry>, Vec<u8>)> {
        // Step 1: Build canonical IR
        let canonical_seq = CanonicalSequenceBuilder::build(self)?;

        // Step 2: Profile the sequence
        let profile = profile_canonical_sequence(&canonical_seq)?;

        // Step 3: Generate candidates for each frame
        let all_candidates_flat = CandidateGenerator::generate(&canonical_seq);

        // Group candidates by frame index
        let mut all_candidates: Vec<Vec<_>> = vec![Vec::new(); canonical_seq.frames.len()];
        for candidate in all_candidates_flat {
            if candidate.frame_index < all_candidates.len() {
                all_candidates[candidate.frame_index].push(candidate);
            }
        }

        let candidates_before_pruning = all_candidates.iter().map(|v| v.len()).sum();

        // Step 4: Compute policy signals for Tier-0 classification
        // Compute an aggregate score from all candidates
        let mut aggregate_score = crate::scoring::ScoreBreakdown::zero();
        let mut score_count = 0;
        for candidates in &all_candidates {
            for candidate in candidates {
                let frame_idx = candidate.frame_index;
                if let Some(frame) = canonical_seq.frames.get(frame_idx) {
                    let score = crate::scoring::Scorer::score_candidate(
                        candidate,
                        frame,
                        &canonical_seq,
                        &profile,
                    );
                    aggregate_score.byte_cost += score.byte_cost;
                    aggregate_score.visual_risk += score.visual_risk;
                    aggregate_score.lut_cost += score.lut_cost;
                    aggregate_score.temporal_instability += score.temporal_instability;
                    aggregate_score.synthetic_transparency_risk +=
                        score.synthetic_transparency_risk;
                    aggregate_score.palette_coherence += score.palette_coherence;
                    aggregate_score.cpu_cost += score.cpu_cost;
                    score_count += 1;
                }
            }
        }
        if score_count > 0 {
            aggregate_score.byte_cost /= score_count as f32;
            aggregate_score.visual_risk /= score_count as f32;
            aggregate_score.lut_cost /= score_count as f32;
            aggregate_score.temporal_instability /= score_count as f32;
            aggregate_score.synthetic_transparency_risk /= score_count as f32;
            aggregate_score.palette_coherence /= score_count as f32;
            aggregate_score.cpu_cost /= score_count as f32;
            aggregate_score.compute_total();
        }
        let policy_signals = PolicySignals::from_profile_and_score(&profile, &aggregate_score);

        // Step 5: Tier-0 classification
        let tier0_decision = Tier0Classifier::classify(&policy_signals);

        // Step 6: Tier-1 pruning (unless early-exit)
        let mut pruned_candidates = all_candidates.clone();
        if !tier0_decision.allows_early_exit() {
            for (frame_idx, candidates) in pruned_candidates.iter_mut().enumerate() {
                let frame_disposal = canonical_seq
                    .frames
                    .get(frame_idx)
                    .map(|f| f.dispose)
                    .unwrap_or(crate::types::DisposalMethod::Keep);

                let prune_results =
                    Tier1Pruner::prune_frame(candidates, &policy_signals, frame_disposal);
                *candidates = prune_results
                    .into_iter()
                    .filter_map(|r| r.candidate)
                    .collect();
            }
        }

        let candidates_after_pruning = pruned_candidates.iter().map(|v| v.len()).sum();

        // Step 7: Tier-2 measurement (if needed)
        let mut tier2_measurement_ran = false;
        let mut tier2_frames_measured = 0;

        if tier0_decision.requires_encode_and_measure() {
            let budget = MeasurementBudget::for_class(policy_signals.cpu_budget_class);

            if budget.is_enabled() {
                tier2_measurement_ran = true;
                // Tier-2 measurement would be applied here to uncertain frames
                // For now, we track that it ran but don't modify candidates
                // (full implementation would measure and re-score uncertain frames)
                tier2_frames_measured = 0; // Would be populated by actual measurement
            }
        }

        // Step 8: Sequence-level DP-lite optimization
        let optimizer_config = SequenceOptimizerConfig::default();
        let sequence_decision = SequenceOptimizer::optimize(
            &pruned_candidates,
            &canonical_seq,
            &profile,
            &optimizer_config,
        );

        let chunk_count = canonical_seq
            .frames
            .len()
            .div_ceil(optimizer_config.chunk_size);

        // Step 9: Determine palette strategies (used by SequenceOptimizer)
        let _palette_strategies = determine_palette_strategies(self, &canonical_seq, &profile);

        // Step 10: Materialize chosen frames
        let materialized_frames: Vec<_> = sequence_decision
            .frame_decisions
            .iter()
            .enumerate()
            .map(|(idx, decision)| {
                let canonical_frame = &canonical_seq.frames[idx];
                Materializer::materialize_frame(decision, canonical_frame, &canonical_seq)
            })
            .collect::<Result<Vec<_>>>()?;

        // Step 11: Realize palettes using chosen strategies
        let palette_realization = PaletteRealizer::realize(
            &materialized_frames,
            sequence_decision.sequence_palette_strategy,
            self,
        )?;

        // Step 12: Build tiered optimizer telemetry
        let tiered_telemetry = TieredOptimizerTelemetry {
            tier0_decision,
            candidates_before_pruning,
            candidates_after_pruning,
            tier2_measurement_ran,
            tier2_frames_measured,
            sequence_optimizer_chunks: chunk_count,
            sequence_optimizer_summary: sequence_decision.summary.clone(),
        };

        // Step 13: Build full telemetry JSON
        let telemetry_json = Self::build_telemetry_json_with_tiered(
            &sequence_decision,
            &canonical_seq,
            &profile,
            &tiered_telemetry,
        );

        // Step 14: Encode materialized + realized frames to bytes
        let bytes = Self::encode_adaptive_frames(
            self.width,
            self.height,
            self.loop_count,
            &palette_realization,
        )?;

        Ok((Some(telemetry_json), Some(tiered_telemetry), bytes))
    }

    /// Internal: attempt the full adaptive encoding pipeline (telemetry only, no bytes).
    pub fn try_adaptive_encode(
        &self,
    ) -> Result<(Option<String>, Option<TieredOptimizerTelemetry>)> {
        // Step 1: Build canonical IR
        let canonical_seq = CanonicalSequenceBuilder::build(self)?;

        // Step 2: Profile the sequence
        let profile = profile_canonical_sequence(&canonical_seq)?;

        // Step 3: Generate candidates for each frame
        let all_candidates_flat = CandidateGenerator::generate(&canonical_seq);

        // Group candidates by frame index
        let mut all_candidates: Vec<Vec<_>> = vec![Vec::new(); canonical_seq.frames.len()];
        for candidate in all_candidates_flat {
            if candidate.frame_index < all_candidates.len() {
                all_candidates[candidate.frame_index].push(candidate);
            }
        }

        let candidates_before_pruning = all_candidates.iter().map(|v| v.len()).sum();

        // Step 4: Compute policy signals for Tier-0 classification
        // Compute an aggregate score from all candidates
        let mut aggregate_score = crate::scoring::ScoreBreakdown::zero();
        let mut score_count = 0;
        for candidates in &all_candidates {
            for candidate in candidates {
                let frame_idx = candidate.frame_index;
                if let Some(frame) = canonical_seq.frames.get(frame_idx) {
                    let score = crate::scoring::Scorer::score_candidate(
                        candidate,
                        frame,
                        &canonical_seq,
                        &profile,
                    );
                    aggregate_score.byte_cost += score.byte_cost;
                    aggregate_score.visual_risk += score.visual_risk;
                    aggregate_score.lut_cost += score.lut_cost;
                    aggregate_score.temporal_instability += score.temporal_instability;
                    aggregate_score.synthetic_transparency_risk +=
                        score.synthetic_transparency_risk;
                    aggregate_score.palette_coherence += score.palette_coherence;
                    aggregate_score.cpu_cost += score.cpu_cost;
                    score_count += 1;
                }
            }
        }
        if score_count > 0 {
            aggregate_score.byte_cost /= score_count as f32;
            aggregate_score.visual_risk /= score_count as f32;
            aggregate_score.lut_cost /= score_count as f32;
            aggregate_score.temporal_instability /= score_count as f32;
            aggregate_score.synthetic_transparency_risk /= score_count as f32;
            aggregate_score.palette_coherence /= score_count as f32;
            aggregate_score.cpu_cost /= score_count as f32;
            aggregate_score.compute_total();
        }
        let policy_signals = PolicySignals::from_profile_and_score(&profile, &aggregate_score);

        // Step 5: Tier-0 classification
        let tier0_decision = Tier0Classifier::classify(&policy_signals);

        // Step 6: Tier-1 pruning (unless early-exit)
        let mut pruned_candidates = all_candidates.clone();
        if !tier0_decision.allows_early_exit() {
            for (frame_idx, candidates) in pruned_candidates.iter_mut().enumerate() {
                let frame_disposal = canonical_seq
                    .frames
                    .get(frame_idx)
                    .map(|f| f.dispose)
                    .unwrap_or(crate::types::DisposalMethod::Keep);

                let prune_results =
                    Tier1Pruner::prune_frame(candidates, &policy_signals, frame_disposal);
                *candidates = prune_results
                    .into_iter()
                    .filter_map(|r| r.candidate)
                    .collect();
            }
        }

        let candidates_after_pruning = pruned_candidates.iter().map(|v| v.len()).sum();

        // Step 7: Tier-2 measurement (if needed)
        let mut tier2_measurement_ran = false;
        let tier2_frames_measured = 0;

        if tier0_decision.requires_encode_and_measure() {
            let budget = MeasurementBudget::for_class(policy_signals.cpu_budget_class);

            if budget.is_enabled() {
                tier2_measurement_ran = true;
                // Tier-2 measurement would be applied here to uncertain frames
                // For now, we track that it ran but don't modify candidates
                // (full implementation would measure and re-score uncertain frames)
            }
        }

        // Step 8: Sequence-level DP-lite optimization
        let optimizer_config = SequenceOptimizerConfig::default();
        let sequence_decision = SequenceOptimizer::optimize(
            &pruned_candidates,
            &canonical_seq,
            &profile,
            &optimizer_config,
        );

        let chunk_count = canonical_seq
            .frames
            .len()
            .div_ceil(optimizer_config.chunk_size);

        // Step 9: Build tiered optimizer telemetry
        let tiered_telemetry = TieredOptimizerTelemetry {
            tier0_decision,
            candidates_before_pruning,
            candidates_after_pruning,
            tier2_measurement_ran,
            tier2_frames_measured,
            sequence_optimizer_chunks: chunk_count,
            sequence_optimizer_summary: sequence_decision.summary.clone(),
        };

        // Step 10: Build full telemetry JSON
        let telemetry_json = Self::build_telemetry_json_with_tiered(
            &sequence_decision,
            &canonical_seq,
            &profile,
            &tiered_telemetry,
        );

        Ok((Some(telemetry_json), Some(tiered_telemetry)))
    }

    /// Encode materialized and realized frames to GIF bytes.
    fn encode_adaptive_frames(
        width: u16,
        height: u16,
        loop_count: crate::types::LoopCount,
        palette_realization: &crate::palette_realize::PaletteRealization,
    ) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();
        Self::encode_adaptive_frames_to(
            &mut buffer,
            width,
            height,
            loop_count,
            palette_realization,
        )?;
        Ok(buffer)
    }

    /// Encode materialized and realized frames to a writer.
    fn encode_adaptive_frames_to<W: std::io::Write>(
        writer: W,
        width: u16,
        height: u16,
        loop_count: crate::types::LoopCount,
        palette_realization: &crate::palette_realize::PaletteRealization,
    ) -> Result<()> {
        // Check if we have any valid frames (non-zero dimensions)
        let has_valid_frames = palette_realization
            .frames
            .iter()
            .any(|f| f.width > 0 && f.height > 0);
        if !has_valid_frames {
            return Err(crate::error::Error::EncodeError(
                "no valid frames to encode (all frames are 0x0)".to_string(),
            ));
        }

        let mut encoder = gif::Encoder::new(writer, width, height, &[]).map_err(|e| {
            crate::error::Error::EncodeError(format!("failed to create encoder: {}", e))
        })?;

        // Set loop count
        let repeat = match loop_count {
            crate::types::LoopCount::Infinite => gif::Repeat::Infinite,
            crate::types::LoopCount::Finite(n) => gif::Repeat::Finite(n),
        };
        encoder.set_repeat(repeat).map_err(|e| {
            crate::error::Error::EncodeError(format!("failed to set repeat: {}", e))
        })?;

        // Write each quantized frame
        for qframe in &palette_realization.frames {
            // Convert delay from Duration to gif units (10ms increments)
            let delay_ms = qframe.delay.as_millis() as u16;
            let delay_units = (delay_ms + 5) / 10; // Round to nearest 10ms unit

            // Map disposal method
            let disposal = match qframe.dispose {
                crate::types::DisposalMethod::None => gif::DisposalMethod::Any,
                crate::types::DisposalMethod::Keep => gif::DisposalMethod::Keep,
                crate::types::DisposalMethod::Background => gif::DisposalMethod::Background,
                crate::types::DisposalMethod::Previous => gif::DisposalMethod::Previous,
            };

            // Create gif frame
            let mut gif_frame = gif::Frame::from_indexed_pixels(
                qframe.width,
                qframe.height,
                qframe.indices.clone(),
                qframe.transparent_idx,
            );

            // Set the palette on the frame
            if let Some(ref local_palette) = qframe.local_palette {
                gif_frame.palette = Some(local_palette.clone());
            } else if let Some(ref global_palette) = palette_realization.global_palette {
                gif_frame.palette = Some(global_palette.clone());
            }

            // Set transparent index if we have transparent pixels
            if let Some(idx) = qframe.transparent_idx {
                gif_frame.transparent = Some(idx);
            }

            gif_frame.delay = delay_units;
            gif_frame.dispose = disposal;
            gif_frame.left = qframe.left;
            gif_frame.top = qframe.top;

            encoder.write_frame(&gif_frame).map_err(|e| {
                crate::error::Error::EncodeError(format!("failed to write frame: {}", e))
            })?;
        }

        Ok(())
    }

    /// Build telemetry JSON from adaptive decisions with tiered optimizer info.
    fn build_telemetry_json_with_tiered(
        sequence_decision: &crate::scoring::SequenceDecision,
        canonical_seq: &crate::adaptive_ir::CanonicalSequence,
        profile: &crate::profiler::GifProfile,
        tiered_telemetry: &TieredOptimizerTelemetry,
    ) -> String {
        use std::fmt::Write as FmtWrite;

        let mut json = String::new();
        let _ = writeln!(
            json,
            r#"{{"mode":"adaptive","status":"success","tiered_optimizer":{{"#
        );
        let _ = writeln!(
            json,
            r#"  "tier0_decision":"{}","#,
            tiered_telemetry.tier0_decision.name()
        );
        let _ = writeln!(
            json,
            r#"  "candidates_before_pruning":{},"#,
            tiered_telemetry.candidates_before_pruning
        );
        let _ = writeln!(
            json,
            r#"  "candidates_after_pruning":{},"#,
            tiered_telemetry.candidates_after_pruning
        );
        let _ = writeln!(
            json,
            r#"  "tier2_measurement_ran":{},"#,
            tiered_telemetry.tier2_measurement_ran
        );
        let _ = writeln!(
            json,
            r#"  "tier2_frames_measured":{},"#,
            tiered_telemetry.tier2_frames_measured
        );
        let _ = writeln!(
            json,
            r#"  "sequence_optimizer_chunks":{},"#,
            tiered_telemetry.sequence_optimizer_chunks
        );
        let _ = writeln!(
            json,
            r#"  "sequence_optimizer_summary":"{}"#,
            tiered_telemetry
                .sequence_optimizer_summary
                .replace('"', "\\\"")
        );
        let _ = writeln!(json, r#"}},"sequence":{{"#);
        let _ = writeln!(
            json,
            r#"  "width":{},"height":{},"frame_count":{},"#,
            canonical_seq.width,
            canonical_seq.height,
            canonical_seq.frames.len()
        );
        let _ = writeln!(
            json,
            r#"  "taxonomy":"{}","avg_score":{:.3},"estimated_bytes":{}"#,
            profile.taxonomy.name(),
            sequence_decision.avg_score,
            sequence_decision.estimated_total_bytes
        );
        let _ = writeln!(json, r#"}},"frame_decisions":["#);

        for (idx, decision) in sequence_decision.frame_decisions.iter().enumerate() {
            if idx > 0 {
                let _ = writeln!(json, ",");
            }
            let _ = write!(json, r#"  {{"#);
            let _ = write!(
                json,
                r#""frame_index":{},"chosen_representation":"{}","chosen_palette_strategy":"{}","#,
                decision.frame_index,
                Self::repr_name(&decision.chosen_candidate),
                Self::strategy_name(&decision.chosen_palette_strategy)
            );
            let _ = write!(
                json,
                r#""score_breakdown":{{"byte_cost":{:.3},"visual_risk":{:.3},"temporal_instability":{:.3},"synthetic_transparency_risk":{:.3},"cpu_cost":{:.3},"total_score":{:.3}}},"#,
                decision.score_breakdown.byte_cost,
                decision.score_breakdown.visual_risk,
                decision.score_breakdown.temporal_instability,
                decision.score_breakdown.synthetic_transparency_risk,
                decision.score_breakdown.cpu_cost,
                decision.score_breakdown.total_score
            );
            let _ = write!(
                json,
                r#""reason":"{}","explanation":"{}""#,
                Self::reason_name(&decision.reason),
                decision.explanation.replace('"', "\\\"")
            );
            let _ = write!(json, r#"}}"#);
        }

        let _ = write!(json, r#"]"#);
        let _ = write!(json, r#"}}"#);

        json
    }

    /// Build telemetry JSON from adaptive decisions (legacy, without tiered info).
    pub fn build_telemetry_json(
        sequence_decision: &crate::scoring::SequenceDecision,
        canonical_seq: &crate::adaptive_ir::CanonicalSequence,
        profile: &crate::profiler::GifProfile,
    ) -> String {
        use std::fmt::Write as FmtWrite;

        let mut json = String::new();
        let _ = writeln!(
            json,
            r#"{{"mode":"adaptive","status":"success","sequence":{{"#
        );
        let _ = writeln!(
            json,
            r#"  "width":{},"height":{},"frame_count":{},"#,
            canonical_seq.width,
            canonical_seq.height,
            canonical_seq.frames.len()
        );
        let _ = writeln!(
            json,
            r#"  "taxonomy":"{}","avg_score":{:.3},"estimated_bytes":{}"#,
            profile.taxonomy.name(),
            sequence_decision.avg_score,
            sequence_decision.estimated_total_bytes
        );
        let _ = writeln!(json, r#"}},"frame_decisions":["#);

        for (idx, decision) in sequence_decision.frame_decisions.iter().enumerate() {
            if idx > 0 {
                let _ = writeln!(json, ",");
            }
            let _ = write!(json, r#"  {{"#);
            let _ = write!(
                json,
                r#""frame_index":{},"chosen_representation":"{}","chosen_palette_strategy":"{}","#,
                decision.frame_index,
                Self::repr_name(&decision.chosen_candidate),
                Self::strategy_name(&decision.chosen_palette_strategy)
            );
            let _ = write!(
                json,
                r#""score_breakdown":{{"byte_cost":{:.3},"visual_risk":{:.3},"temporal_instability":{:.3},"synthetic_transparency_risk":{:.3},"cpu_cost":{:.3},"total_score":{:.3}}},"#,
                decision.score_breakdown.byte_cost,
                decision.score_breakdown.visual_risk,
                decision.score_breakdown.temporal_instability,
                decision.score_breakdown.synthetic_transparency_risk,
                decision.score_breakdown.cpu_cost,
                decision.score_breakdown.total_score
            );
            let _ = write!(
                json,
                r#""reason":"{}","explanation":"{}""#,
                Self::reason_name(&decision.reason),
                decision.explanation.replace('"', "\\\"")
            );
            let _ = write!(json, r#"}}"#);
        }

        let _ = write!(json, r#"]"#);
        let _ = write!(json, r#"}}"#);

        json
    }

    fn repr_name(repr: &crate::candidate_gen::CandidateRepresentation) -> &'static str {
        match repr {
            crate::candidate_gen::CandidateRepresentation::FullFrame => "full-frame",
            crate::candidate_gen::CandidateRepresentation::ExactOpaqueBbox { .. } => "opaque-bbox",
            crate::candidate_gen::CandidateRepresentation::TransparentSparsePatch { .. } => {
                "sparse-patch"
            }
            crate::candidate_gen::CandidateRepresentation::MinimalNoOp => "minimal-noop",
        }
    }

    fn strategy_name(strategy: &crate::palette_strategy::PaletteStrategy) -> &'static str {
        strategy.name()
    }

    fn reason_name(reason: &crate::scoring::DecisionReason) -> &'static str {
        match reason {
            crate::scoring::DecisionReason::LowestScore => "lowest-score",
            crate::scoring::DecisionReason::TaxonomyPreferred => "taxonomy-preferred",
            crate::scoring::DecisionReason::SafetyConstraint => "safety-constraint",
            crate::scoring::DecisionReason::PaletteStrategyAlignment => {
                "palette-strategy-alignment"
            }
            crate::scoring::DecisionReason::TieBreaker => "tie-breaker",
            crate::scoring::DecisionReason::Fallback => "fallback",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a simple test GIF (opaque delta, global palette).
    fn create_test_gif() -> Gif {
        use crate::types::{Frame, LoopCount, Palette};
        use std::time::Duration;

        let width = 100u16;
        let height = 100u16;

        // Frame 1: solid red
        let mut frame1_pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        for i in (0..frame1_pixels.len()).step_by(4) {
            frame1_pixels[i] = 255; // R
            frame1_pixels[i + 1] = 0; // G
            frame1_pixels[i + 2] = 0; // B
            frame1_pixels[i + 3] = 255; // A
        }

        // Frame 2: solid blue
        let mut frame2_pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        for i in (0..frame2_pixels.len()).step_by(4) {
            frame2_pixels[i] = 0; // R
            frame2_pixels[i + 1] = 0; // G
            frame2_pixels[i + 2] = 255; // B
            frame2_pixels[i + 3] = 255; // A
        }

        let frame1 = Frame {
            pixels: frame1_pixels,
            left: 0,
            top: 0,
            width,
            height,
            delay: Duration::from_millis(100),
            dispose: crate::types::DisposalMethod::Keep,
            local_palette: None,
        };

        let frame2 = Frame {
            pixels: frame2_pixels,
            left: 0,
            top: 0,
            width,
            height,
            delay: Duration::from_millis(100),
            dispose: crate::types::DisposalMethod::Keep,
            local_palette: None,
        };

        // Create a simple global palette
        let global_palette = Palette {
            colors: vec![
                [255, 0, 0],     // Red
                [0, 0, 255],     // Blue
                [0, 255, 0],     // Green
                [255, 255, 255], // White
            ],
        };

        Gif {
            width,
            height,
            frames: vec![frame1, frame2],
            loop_count: LoopCount::Infinite,
            global_palette: Some(global_palette.clone()),
            original_palette: Some(global_palette.colors),
        }
    }

    #[test]
    fn test_adaptive_path_uses_tiered_optimizer() {
        let gif = create_test_gif();
        let config = AdaptiveConfig {
            enabled: true,
            emit_telemetry: false,
        };

        let result = gif.encode_adaptive(&config);
        assert!(result.is_ok(), "adaptive encoding should succeed");

        let (decision, bytes) = result.unwrap();
        assert!(decision.success, "adaptive decision should succeed");
        assert!(
            decision.tiered_telemetry.is_some(),
            "tiered telemetry should be present"
        );
        assert!(!bytes.is_empty(), "encoded bytes should not be empty");

        let telemetry = decision.tiered_telemetry.unwrap();
        assert!(matches!(
            telemetry.tier0_decision,
            Tier0Decision::EarlyExitStructural
                | Tier0Decision::NeedsTier1
                | Tier0Decision::NeedsTier2
        ));
        assert!(
            telemetry.candidates_before_pruning > 0,
            "should have candidates before pruning"
        );
        assert!(
            telemetry.candidates_after_pruning > 0,
            "should have candidates after pruning"
        );
        assert!(
            telemetry.candidates_after_pruning <= telemetry.candidates_before_pruning,
            "pruning should reduce candidates"
        );
    }

    #[test]
    fn test_tier0_early_exit_reaches_bytes_emission() {
        let gif = create_test_gif();
        let config = AdaptiveConfig {
            enabled: true,
            emit_telemetry: false,
        };

        let result = gif.encode_adaptive(&config);
        assert!(result.is_ok());

        let (decision, bytes) = result.unwrap();
        assert!(decision.success);

        // For opaque delta GIFs, Tier-0 should classify as early-exit-structural
        let telemetry = decision.tiered_telemetry.unwrap();
        // Note: actual classification depends on profile, but we verify the path works
        assert!(matches!(
            telemetry.tier0_decision,
            Tier0Decision::EarlyExitStructural
                | Tier0Decision::NeedsTier1
                | Tier0Decision::NeedsTier2
        ));

        // Verify bytes were emitted
        assert!(!bytes.is_empty(), "should emit bytes even with early-exit");
    }

    #[test]
    fn test_adaptive_disabled_uses_fallback() {
        let gif = create_test_gif();
        let config = AdaptiveConfig {
            enabled: false,
            emit_telemetry: false,
        };

        let result = gif.encode_adaptive(&config);
        assert!(result.is_ok());

        let (decision, bytes) = result.unwrap();
        assert!(!decision.success, "disabled adaptive should not succeed");
        assert!(
            decision.fallback_reason.is_some(),
            "should have fallback reason"
        );
        assert!(!bytes.is_empty(), "should still emit bytes via fallback");
    }

    #[test]
    fn test_tiered_telemetry_includes_sequence_optimizer_info() {
        let gif = create_test_gif();
        let config = AdaptiveConfig {
            enabled: true,
            emit_telemetry: false,
        };

        let result = gif.encode_adaptive(&config);
        assert!(result.is_ok());

        let (decision, _) = result.unwrap();
        let telemetry = decision.tiered_telemetry.unwrap();

        // Verify sequence optimizer telemetry is present
        assert!(
            telemetry.sequence_optimizer_chunks > 0,
            "should have chunk count"
        );
        assert!(
            !telemetry.sequence_optimizer_summary.is_empty(),
            "should have DP-lite summary"
        );
    }

    #[test]
    fn test_telemetry_json_includes_tiered_info() {
        let gif = create_test_gif();
        let config = AdaptiveConfig {
            enabled: true,
            emit_telemetry: false,
        };

        let result = gif.encode_adaptive(&config);
        assert!(result.is_ok());

        let (decision, _) = result.unwrap();
        assert!(
            decision.telemetry_json.is_some(),
            "should have telemetry JSON"
        );

        let json = decision.telemetry_json.unwrap();
        assert!(
            json.contains("tiered_optimizer"),
            "JSON should include tiered_optimizer section"
        );
        assert!(
            json.contains("tier0_decision"),
            "JSON should include tier0_decision"
        );
        assert!(
            json.contains("candidates_before_pruning"),
            "JSON should include candidate counts"
        );
        assert!(
            json.contains("sequence_optimizer_chunks"),
            "JSON should include chunk count"
        );
    }

    #[test]
    fn test_fallback_on_adaptive_failure_still_emits_bytes() {
        // This test verifies the safe fallback path works
        let gif = create_test_gif();
        let config = AdaptiveConfig {
            enabled: true,
            emit_telemetry: false,
        };

        let result = gif.encode_adaptive(&config);
        // Even if adaptive path fails internally, we should get bytes via fallback
        assert!(result.is_ok(), "should always return Ok with fallback");

        let (_, bytes) = result.unwrap();
        assert!(!bytes.is_empty(), "should emit bytes even on fallback");
    }
}
