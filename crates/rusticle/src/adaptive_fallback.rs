//! Fallback and telemetry handling for adaptive materialization/palette realization.
//!
//! This module provides explicit failure handling and telemetry for the adaptive encoding
//! pipeline stages (materialization, palette realization, and pre-encode preparation).
//!
//! When any stage fails, the system degrades cleanly to the current (non-adaptive) path
//! with explicit telemetry recording the failure stage, error summary, and fallback reason.
//!
//! # Design
//!
//! - **Explicit fallback**: No silent failures. Every fallback is recorded with stage and reason.
//! - **Deterministic**: Fallback path is the current (proven) encoding path.
//! - **Safe**: No partial adaptive state leaks into current-path output.
//! - **Debuggable**: Telemetry captures stage, error, and reason code for analysis.
//!
//! # Usage
//!
//! ```ignore
//! use rusticle::adaptive_fallback::{AdaptiveBytesPreparer, AdaptiveStage};
//!
//! let preparer = AdaptiveBytesPreparer::new(&materialized_frames, &decisions, &canonical_seq);
//! match preparer.prepare_with_fallback(&source_gif) {
//!     Ok((realization, telemetry)) => {
//!         // Success: use realization for adaptive encoding
//!         eprintln!("Adaptive bytes prepared: {:?}", telemetry);
//!     }
//!     Err((fallback_reason, telemetry)) => {
//!         // Fallback: use current path, but record telemetry
//!         eprintln!("Fallback triggered: {} (telemetry: {:?})", fallback_reason, telemetry);
//!     }
//! }
//! ```

use crate::adaptive_ir::CanonicalSequence;
use crate::error::{Error, Result};
use crate::materialize::Materializer;
use crate::palette_realize::{PaletteRealization, PaletteRealizer};
use crate::palette_strategy::PaletteStrategy;
use crate::scoring::FrameDecision;
use crate::types::Gif;

/// Stage in the adaptive bytes preparation pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveStage {
    /// Materialization stage (converting decisions to frames).
    Materialization,
    /// Palette realization stage (quantizing frames to palette indices).
    PaletteRealization,
    /// Pre-encode preparation (validation, metadata setup).
    PreEncodePrep,
}

impl AdaptiveStage {
    /// Human-readable name for this stage.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Materialization => "materialization",
            Self::PaletteRealization => "palette_realization",
            Self::PreEncodePrep => "pre_encode_prep",
        }
    }
}

/// Reason code for fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// Materialization failed (invalid bbox, memory, etc.).
    MaterializationFailed,
    /// Palette realization failed (quantization, color mapping, etc.).
    PaletteRealizationFailed,
    /// Pre-encode validation failed.
    PreEncodePrepFailed,
    /// Unknown/unclassified failure.
    Unknown,
}

impl FallbackReason {
    /// Human-readable code for this reason.
    pub fn code(&self) -> &'static str {
        match self {
            Self::MaterializationFailed => "materialization_failed",
            Self::PaletteRealizationFailed => "palette_realization_failed",
            Self::PreEncodePrepFailed => "pre_encode_prep_failed",
            Self::Unknown => "unknown",
        }
    }
}

/// Telemetry recorded for adaptive bytes preparation.
#[derive(Debug, Clone)]
pub struct FallbackTelemetry {
    /// Stage that failed (if any).
    pub failed_stage: Option<AdaptiveStage>,
    /// Error summary (first 256 chars).
    pub error_summary: Option<String>,
    /// Whether fallback was used.
    pub fallback_used: bool,
    /// Fallback reason code.
    pub fallback_reason: FallbackReason,
    /// Number of frames processed before failure (if applicable).
    pub frames_processed: usize,
}

impl FallbackTelemetry {
    /// Create a success telemetry record.
    pub fn success(frames_processed: usize) -> Self {
        Self {
            failed_stage: None,
            error_summary: None,
            fallback_used: false,
            fallback_reason: FallbackReason::Unknown,
            frames_processed,
        }
    }

    /// Create a fallback telemetry record.
    pub fn fallback(stage: AdaptiveStage, error: &Error, frames_processed: usize) -> Self {
        let error_summary = format!("{}", error);
        let error_summary = if error_summary.len() > 256 {
            error_summary[..256].to_string()
        } else {
            error_summary
        };

        let fallback_reason = match stage {
            AdaptiveStage::Materialization => FallbackReason::MaterializationFailed,
            AdaptiveStage::PaletteRealization => FallbackReason::PaletteRealizationFailed,
            AdaptiveStage::PreEncodePrep => FallbackReason::PreEncodePrepFailed,
        };

        Self {
            failed_stage: Some(stage),
            error_summary: Some(error_summary),
            fallback_used: true,
            fallback_reason,
            frames_processed,
        }
    }

    /// Convert telemetry to JSON for logging.
    pub fn to_json(&self) -> String {
        use std::fmt::Write as FmtWrite;

        let mut json = String::new();
        let _ = writeln!(json, r#"{{"adaptive_fallback":{{"#);
        let _ = writeln!(json, r#"  "fallback_used":{},"#, self.fallback_used);

        if let Some(stage) = self.failed_stage {
            let _ = writeln!(json, r#"  "failed_stage":"{}","#, stage.name());
        }

        let _ = writeln!(
            json,
            r#"  "fallback_reason":"{}","#,
            self.fallback_reason.code()
        );

        if let Some(ref error) = self.error_summary {
            let escaped = error.replace('"', "\\\"");
            let _ = writeln!(json, r#"  "error_summary":"{}","#, escaped);
        }

        let _ = writeln!(json, r#"  "frames_processed":{}"#, self.frames_processed);
        let _ = writeln!(json, r#"}}}}"#);

        json
    }
}

/// Prepares adaptive bytes with explicit fallback handling.
///
/// This struct wraps the materialization and palette realization stages,
/// catching failures and falling back to the current path with telemetry.
pub struct AdaptiveBytesPreparer {
    decisions: Vec<FrameDecision>,
    canonical_seq: CanonicalSequence,
}

impl AdaptiveBytesPreparer {
    /// Create a new preparer for the given decisions and canonical sequence.
    pub fn new(decisions: Vec<FrameDecision>, canonical_seq: CanonicalSequence) -> Self {
        Self {
            decisions,
            canonical_seq,
        }
    }

    /// Prepare adaptive bytes with fallback handling.
    ///
    /// Attempts to:
    /// 1. Materialize frames from decisions
    /// 2. Realize palettes for materialized frames
    /// 3. Validate pre-encode state
    ///
    /// If any step fails, returns a fallback error with telemetry.
    /// The caller should then use the current (non-adaptive) encoding path.
    ///
    /// # Returns
    ///
    /// - `Ok((realization, telemetry))`: Adaptive path succeeded.
    /// - `Err((fallback_reason, telemetry))`: Fallback triggered; use current path.
    pub fn prepare_with_fallback(
        &self,
        source_gif: &Gif,
    ) -> std::result::Result<(PaletteRealization, FallbackTelemetry), (String, FallbackTelemetry)>
    {
        // Step 1: Materialize frames
        let materialized_frames =
            match Materializer::materialize_sequence(&self.decisions, &self.canonical_seq) {
                Ok(frames) => frames,
                Err(e) => {
                    let telemetry =
                        FallbackTelemetry::fallback(AdaptiveStage::Materialization, &e, 0);
                    let reason = format!("materialization failed: {}", e);
                    return Err((reason, telemetry));
                }
            };

        let frames_materialized = materialized_frames.len();

        // Step 2: Determine palette strategy (use first decision's strategy for now)
        // In a full implementation, this would be per-frame or sequence-wide.
        let palette_strategy = if !self.decisions.is_empty() {
            self.decisions[0].chosen_palette_strategy
        } else {
            PaletteStrategy::DeriveSequenceGlobalPreferred
        };

        // Step 3: Realize palettes
        let realization =
            match PaletteRealizer::realize(&materialized_frames, palette_strategy, source_gif) {
                Ok(r) => r,
                Err(e) => {
                    let telemetry = FallbackTelemetry::fallback(
                        AdaptiveStage::PaletteRealization,
                        &e,
                        frames_materialized,
                    );
                    let reason = format!("palette realization failed: {}", e);
                    return Err((reason, telemetry));
                }
            };

        // Step 4: Pre-encode validation
        if let Err(e) = Self::validate_pre_encode(&realization) {
            let telemetry =
                FallbackTelemetry::fallback(AdaptiveStage::PreEncodePrep, &e, frames_materialized);
            let reason = format!("pre-encode validation failed: {}", e);
            return Err((reason, telemetry));
        }

        let telemetry = FallbackTelemetry::success(frames_materialized);
        Ok((realization, telemetry))
    }

    /// Validate pre-encode state.
    fn validate_pre_encode(realization: &PaletteRealization) -> Result<()> {
        // Check that all frames have indices
        for (i, frame) in realization.frames.iter().enumerate() {
            if frame.indices.is_empty() && frame.width > 0 && frame.height > 0 {
                return Err(Error::EncodeError(format!(
                    "frame {} has no indices but non-zero dimensions",
                    i
                )));
            }

            // Check that indices are valid (0-255)
            for (j, &idx) in frame.indices.iter().enumerate() {
                if idx as usize >= 256 {
                    return Err(Error::EncodeError(format!(
                        "frame {} pixel {} has invalid index {}",
                        i, j, idx
                    )));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_ir::CanonicalSequenceBuilder;
    use crate::candidate_gen::CandidateRepresentation;
    use crate::palette_strategy::PaletteStrategy;
    use crate::scoring::{DecisionReason, ScoreBreakdown};
    use crate::types::{DisposalMethod, Gif, LoopCount};
    use std::time::Duration;

    /// Create a simple test GIF.
    fn create_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
        let mut frames = Vec::new();
        for i in 0..frame_count {
            let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
            let color = [
                (255, 0, 0, 255), // Red
                (0, 255, 0, 255), // Green
                (0, 0, 255, 255), // Blue
            ][i % 3];

            for chunk in pixels.chunks_exact_mut(4) {
                chunk[0] = color.0;
                chunk[1] = color.1;
                chunk[2] = color.2;
                chunk[3] = color.3;
            }

            frames.push(crate::types::Frame {
                pixels,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
                left: 0,
                top: 0,
                width,
                height,
            });
        }

        Gif {
            width,
            height,
            global_palette: None,
            frames,
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    #[test]
    fn test_fallback_telemetry_success() {
        let telemetry = FallbackTelemetry::success(3);
        assert!(!telemetry.fallback_used);
        assert_eq!(telemetry.failed_stage, None);
        assert_eq!(telemetry.frames_processed, 3);
    }

    #[test]
    fn test_fallback_telemetry_materialization_failure() {
        let error = Error::EncodeError("test error".to_string());
        let telemetry = FallbackTelemetry::fallback(AdaptiveStage::Materialization, &error, 0);

        assert!(telemetry.fallback_used);
        assert_eq!(telemetry.failed_stage, Some(AdaptiveStage::Materialization));
        assert_eq!(
            telemetry.fallback_reason,
            FallbackReason::MaterializationFailed
        );
        assert!(telemetry.error_summary.is_some());
    }

    #[test]
    fn test_fallback_telemetry_palette_realization_failure() {
        let error = Error::EncodeError("quantization failed".to_string());
        let telemetry = FallbackTelemetry::fallback(AdaptiveStage::PaletteRealization, &error, 2);

        assert!(telemetry.fallback_used);
        assert_eq!(
            telemetry.failed_stage,
            Some(AdaptiveStage::PaletteRealization)
        );
        assert_eq!(
            telemetry.fallback_reason,
            FallbackReason::PaletteRealizationFailed
        );
        assert_eq!(telemetry.frames_processed, 2);
    }

    #[test]
    fn test_fallback_telemetry_to_json() {
        let error = Error::EncodeError("test error".to_string());
        let telemetry = FallbackTelemetry::fallback(AdaptiveStage::Materialization, &error, 1);

        let json = telemetry.to_json();
        assert!(json.contains("adaptive_fallback"));
        assert!(json.contains("fallback_used"));
        assert!(json.contains("materialization"));
        assert!(json.contains("materialization_failed"));
    }

    #[test]
    fn test_adaptive_stage_names() {
        assert_eq!(AdaptiveStage::Materialization.name(), "materialization");
        assert_eq!(
            AdaptiveStage::PaletteRealization.name(),
            "palette_realization"
        );
        assert_eq!(AdaptiveStage::PreEncodePrep.name(), "pre_encode_prep");
    }

    #[test]
    fn test_fallback_reason_codes() {
        assert_eq!(
            FallbackReason::MaterializationFailed.code(),
            "materialization_failed"
        );
        assert_eq!(
            FallbackReason::PaletteRealizationFailed.code(),
            "palette_realization_failed"
        );
        assert_eq!(
            FallbackReason::PreEncodePrepFailed.code(),
            "pre_encode_prep_failed"
        );
    }

    #[test]
    fn test_adaptive_bytes_preparer_success() {
        let gif = create_test_gif(50, 50, 2);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        let decisions = vec![
            FrameDecision {
                frame_index: 0,
                chosen_candidate: CandidateRepresentation::FullFrame,
                chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
                score_breakdown: ScoreBreakdown::zero(),
                alternatives: vec![],
                reason: DecisionReason::LowestScore,
                explanation: "test".to_string(),
            },
            FrameDecision {
                frame_index: 1,
                chosen_candidate: CandidateRepresentation::FullFrame,
                chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
                score_breakdown: ScoreBreakdown::zero(),
                alternatives: vec![],
                reason: DecisionReason::LowestScore,
                explanation: "test".to_string(),
            },
        ];

        let preparer = AdaptiveBytesPreparer::new(decisions, seq);
        let result = preparer.prepare_with_fallback(&gif);

        assert!(result.is_ok(), "Adaptive bytes preparation should succeed");
        let (realization, telemetry) = result.unwrap();
        assert!(!telemetry.fallback_used);
        assert_eq!(realization.frames.len(), 2);
    }

    #[test]
    fn test_adaptive_bytes_preparer_materialization_failure() {
        let gif = create_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        // Create a decision with an out-of-bounds frame index
        let decisions = vec![FrameDecision {
            frame_index: 999, // Out of bounds
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        }];

        let preparer = AdaptiveBytesPreparer::new(decisions, seq);
        let result = preparer.prepare_with_fallback(&gif);

        assert!(
            result.is_err(),
            "Should trigger fallback on materialization failure"
        );
        let (reason, telemetry) = result.unwrap_err();
        assert!(telemetry.fallback_used);
        assert_eq!(telemetry.failed_stage, Some(AdaptiveStage::Materialization));
        assert!(reason.contains("materialization failed"));
    }

    #[test]
    fn test_adaptive_bytes_preparer_fallback_telemetry_captured() {
        let gif = create_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        let decisions = vec![FrameDecision {
            frame_index: 999, // Out of bounds
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        }];

        let preparer = AdaptiveBytesPreparer::new(decisions, seq);
        let result = preparer.prepare_with_fallback(&gif);

        assert!(result.is_err());
        let (_reason, telemetry) = result.unwrap_err();
        let json = telemetry.to_json();
        assert!(json.contains("adaptive_fallback"));
        assert!(json.contains("true")); // fallback_used: true
    }

    #[test]
    fn test_adaptive_bytes_preparer_preserves_frame_metadata() {
        let gif = create_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        let decisions = vec![FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        }];

        let preparer = AdaptiveBytesPreparer::new(decisions, seq);
        let result = preparer.prepare_with_fallback(&gif);

        assert!(result.is_ok());
        let (realization, _telemetry) = result.unwrap();
        let frame = &realization.frames[0];

        // Metadata should be preserved from materialization
        assert_eq!(frame.width, 50);
        assert_eq!(frame.height, 50);
        assert_eq!(frame.delay, Duration::from_millis(100));
    }

    #[test]
    fn test_adaptive_bytes_preparer_empty_sequence() {
        let gif = create_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        let decisions = vec![]; // Empty decisions

        let preparer = AdaptiveBytesPreparer::new(decisions, seq);
        let result = preparer.prepare_with_fallback(&gif);

        // Should succeed with empty realization
        assert!(result.is_ok());
        let (realization, telemetry) = result.unwrap();
        assert!(!telemetry.fallback_used);
        assert_eq!(realization.frames.len(), 0);
    }
}
