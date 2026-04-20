//! Experimental adaptive encoding harness.
//!
//! This module integrates the adaptive encoding pipeline (canonical IR, profiler,
//! candidate generation, palette strategy, and scorer/chooser) behind an explicit
//! opt-in flag with decision telemetry and safe fallback to the current encoding path.
//!
//! # Adaptive Mode
//!
//! When enabled, the adaptive path:
//! 1. Builds a canonical IR from the GIF
//! 2. Profiles the sequence to understand its characteristics
//! 3. Generates candidate representations for each frame
//! 4. Determines palette strategies
//! 5. Scores and chooses the best representation per frame
//! 6. Emits telemetry about the decisions made
//!
//! If any step fails, the encoder falls back to the current (non-adaptive) path.
//!
//! # Telemetry
//!
//! Decision telemetry is emitted as JSON with the following structure:
//! ```json
//! {
//!   "mode": "adaptive",
//!   "status": "success" | "fallback",
//!   "fallback_reason": "...",
//!   "sequence": {
//!     "width": 320,
//!     "height": 240,
//!     "frame_count": 10,
//!     "taxonomy": "OpaqueDeltaGlobalPalette",
//!     "avg_score": 0.42,
//!     "estimated_bytes": 50000
//!   },
//!   "frame_decisions": [
//!     {
//!       "frame_index": 0,
//!       "chosen_representation": "full-frame",
//!       "chosen_palette_strategy": "DeriveSequenceGlobalPreferred",
//!       "score_breakdown": {
//!         "byte_cost": 0.8,
//!         "visual_risk": 0.1,
//!         "temporal_instability": 0.2,
//!         "synthetic_transparency_risk": 0.0,
//!         "cpu_cost": 0.1,
//!         "total_score": 0.42
//!       },
//!       "reason": "LowestScore",
//!       "explanation": "..."
//!     }
//!   ]
//! }
//! ```

use crate::adaptive_ir::CanonicalSequenceBuilder;
use crate::candidate_gen::CandidateGenerator;
use crate::error::Result;
use crate::palette_strategy::determine_palette_strategies;
use crate::profiler::profile_canonical_sequence;
use crate::scoring::Chooser;
use crate::types::Gif;

/// Configuration for adaptive encoding.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdaptiveConfig {
    /// Enable adaptive encoding mode.
    pub enabled: bool,
    /// Emit telemetry to stderr (if enabled).
    pub emit_telemetry: bool,
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
                },
                bytes,
            ));
        }

        // Attempt adaptive path
        match self.try_adaptive_encode() {
            Ok(telemetry_json) => {
                if config.emit_telemetry {
                    if let Some(ref json) = telemetry_json {
                        eprintln!("ADAPTIVE_TELEMETRY: {}", json);
                    }
                }

                // For now, we emit telemetry but still use the current encode path
                // (actual adaptive encoding would use the decisions to guide quantization)
                let bytes = self.to_bytes()?;

                Ok((
                    AdaptiveDecision {
                        success: true,
                        fallback_reason: None,
                        telemetry_json,
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
                    },
                    bytes,
                ))
            }
        }
    }

    /// Internal: attempt the full adaptive encoding pipeline.
    fn try_adaptive_encode(&self) -> Result<Option<String>> {
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

        // Step 4: Determine palette strategies
        let palette_strategies = determine_palette_strategies(self, &canonical_seq, &profile);

        // Step 5: Score and choose
        let sequence_decision = Chooser::choose_sequence(&all_candidates, &canonical_seq, &profile, &palette_strategies);

        // Step 6: Emit telemetry
        let telemetry_json = Self::build_telemetry_json(&sequence_decision, &canonical_seq, &profile);

        Ok(Some(telemetry_json))
    }

    /// Build telemetry JSON from adaptive decisions.
    fn build_telemetry_json(
        sequence_decision: &crate::scoring::SequenceDecision,
        canonical_seq: &crate::adaptive_ir::CanonicalSequence,
        profile: &crate::profiler::GifProfile,
    ) -> String {
        use std::fmt::Write as FmtWrite;

        let mut json = String::new();
        let _ = writeln!(json, r#"{{"mode":"adaptive","status":"success","sequence":{{"#);
        let _ = writeln!(
            json,
            r#"  "width":{},"height":{},"frame_count":{},"#,
            canonical_seq.width, canonical_seq.height, canonical_seq.frames.len()
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
            crate::candidate_gen::CandidateRepresentation::TransparentSparsePatch { .. } => "sparse-patch",
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
            crate::scoring::DecisionReason::PaletteStrategyAlignment => "palette-strategy-alignment",
            crate::scoring::DecisionReason::TieBreaker => "tie-breaker",
            crate::scoring::DecisionReason::Fallback => "fallback",
        }
    }
}
