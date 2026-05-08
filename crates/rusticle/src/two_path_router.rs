//! Two-path optimizer routing: wire classifier → Path A / Path B behind experimental flag.
//!
//! **STATUS**: Research / Future Opt-In (not current mainline product path)
//!
//! This module implements the integration point for the two-path optimizer strategy.
//! It provides:
//!
//! 1. **Routing modes**: `Legacy` (current default), `Auto` (classifier decides),
//!    `PathA` (forced), `PathB` (forced).
//! 2. **Deterministic classification**: Routes based on structural features of the
//!    decoded GIF sequence.
//! 3. **Safe fallback**: If Path A fails, falls back to Path B or legacy path.
//! 4. **Telemetry**: Logs path selection, classifier features, and fallback reasons.
//!
//! # Design
//!
//! This is an **experimental routing candidate**, separate from the corrected default path.
//! It is bounded to two explicit strategies:
//!
//! - **Path A**: Conservative opaque-delta reconstruction for already-optimized sequences.
//! - **Path B**: General sparse/transparent optimization for mixed/transparency-heavy sequences.
//!
//! The legacy path is always available as a rollback during transition.
//!
//! # What It Was Trying to Solve
//!
//! The two-path system explored whether routing different GIF classes to different optimization
//! strategies could improve overall compression without expensive measurement/classification overhead.
//! Hypothesis: opaque-delta GIFs and transparency-heavy GIFs have fundamentally different optimal
//! strategies. A simple, deterministic classifier could route them appropriately.
//!
//! # Structural Assumptions
//!
//! - Deterministic classification based on structural features (no randomness or measurement)
//! - Two bounded paths: Path A (conservative opaque-delta) and Path B (general sparse/transparent)
//! - Fast classification (no encode-and-measure overhead)
//! - Safe fallback (if Path A fails, fall back to Path B or legacy)
//!
//! # Latest Evidence
//!
//! Two-path routing shows promise for certain GIF classes, but generality is not established.
//! Path A works well for already-optimized opaque-delta sequences. Path B is a reasonable fallback
//! for mixed/transparency-heavy GIFs. The classifier is simple and deterministic, but may be too
//! conservative (routes too many GIFs to Path B). Validation on a larger, more diverse GIF corpus
//! is needed before promoting to mainline.
//!
//! # Current Status
//!
//! - Not integrated into mainline product path (corrected default path is the explicit mainline)
//! - Available via `OptimizerStrategy::Auto` flag if explicitly enabled
//! - Safe fallback to legacy path always available
//! - No runtime integration by default
//!
//! See `docs/RESEARCH_VOYAGER_AND_TWO_PATH.md` for full context.

use crate::adaptive_ir::CanonicalSequenceBuilder;
use crate::classifier::{classify_sequence, ClassificationResult, OptimizerPath};
use crate::error::Result;
use crate::path_a::{optimize_path_a, PathAConfig};
use crate::path_b::{optimize_path_b, PathBConfig};
use crate::types::{Frame, Gif, OptLevel};
use std::fmt;

/// Optimizer routing strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizerStrategy {
    /// Use current default behavior (no routing, no classifier).
    /// This is the safe rollback during transition.
    Legacy,
    /// Classifier decides: Path A for opaque-delta, Path B for mixed/transparent.
    Auto,
    /// Force Path A regardless of input characteristics.
    PathA,
    /// Force Path B regardless of input characteristics.
    PathB,
}

impl OptimizerStrategy {
    /// Human-readable name for the strategy.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Legacy => "Legacy (current default)",
            Self::Auto => "Auto (classifier decides)",
            Self::PathA => "Path A (forced)",
            Self::PathB => "Path B (forced)",
        }
    }
}

impl fmt::Display for OptimizerStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Telemetry from two-path routing.
#[derive(Debug, Clone)]
pub struct TwoPathTelemetry {
    /// Which strategy was used.
    pub strategy: OptimizerStrategy,
    /// Which path was selected (if applicable).
    pub selected_path: Option<OptimizerPath>,
    /// Classification result (if Auto mode).
    pub classification: Option<ClassificationResult>,
    /// Whether fallback was used.
    pub fallback_used: bool,
    /// Reason for fallback (if any).
    pub fallback_reason: Option<String>,
}

impl TwoPathTelemetry {
    /// Emit telemetry to stderr in human-readable format.
    pub fn emit_to_stderr(&self) {
        eprintln!("[two-path-router] strategy={}", self.strategy);
        if let Some(path) = self.selected_path {
            eprintln!("[two-path-router] selected_path={}", path.name());
        }
        if let Some(ref classification) = self.classification {
            eprintln!(
                "[two-path-router] classification_features: \
                 has_transparent_gce={}, \
                 keep_none_disposal_ratio={:.2}, \
                 palette_stability={:.2}, \
                 offset_patch_ratio={:.2}, \
                 median_changed_area_ratio={:.2}",
                classification.features.has_transparent_gce,
                classification.features.keep_none_disposal_ratio,
                classification.features.palette_stability,
                classification.features.offset_patch_ratio,
                classification.features.median_changed_area_ratio
            );
            for reason in &classification.reasons {
                eprintln!("[two-path-router] reason: {}", reason);
            }
        }
        if self.fallback_used {
            eprintln!(
                "[two-path-router] fallback_used=true, reason={:?}",
                self.fallback_reason
            );
        }
    }
}

/// Configuration for two-path routing.
#[derive(Debug, Clone, Copy)]
pub struct TwoPathConfig {
    /// Which routing strategy to use.
    pub strategy: OptimizerStrategy,
    /// Configuration for Path A (if used).
    pub path_a_config: PathAConfig,
    /// Configuration for Path B (if used).
    pub path_b_config: PathBConfig,
    /// Emit telemetry to stderr.
    pub emit_telemetry: bool,
}

impl Default for TwoPathConfig {
    fn default() -> Self {
        Self {
            strategy: OptimizerStrategy::Legacy,
            path_a_config: PathAConfig::default(),
            path_b_config: PathBConfig::default(),
            emit_telemetry: false,
        }
    }
}

/// Result of two-path routing.
#[derive(Debug)]
pub struct TwoPathResult {
    /// Optimized frames.
    pub frames: Vec<Frame>,
    /// Telemetry from routing.
    pub telemetry: TwoPathTelemetry,
}

/// Route optimization through Path A or Path B based on strategy.
///
/// # Arguments
/// * `gif` - The GIF to optimize (already resized)
/// * `level` - Optimization level
/// * `config` - Routing configuration
///
/// # Returns
/// Optimized frames and telemetry
///
/// # Fallback Behavior
/// If Path A is selected but fails (e.g., palette realization error), falls back to Path B.
/// If both fail, falls back to legacy path.
pub fn route_optimize(
    gif: &Gif,
    level: OptLevel,
    config: TwoPathConfig,
) -> Result<TwoPathResult> {
    let mut telemetry = TwoPathTelemetry {
        strategy: config.strategy,
        selected_path: None,
        classification: None,
        fallback_used: false,
        fallback_reason: None,
    };

    let frames = match config.strategy {
        OptimizerStrategy::Legacy => {
            // Legacy path: use current default behavior (Path B)
            optimize_path_b(&gif.frames, config.path_b_config)
        }
        OptimizerStrategy::Auto => {
            // Classify the sequence and route accordingly
            match classify_and_route(gif, level, config, &mut telemetry) {
                Ok(frames) => frames,
                Err(e) => {
                    // Fallback to Path B on classification error
                    eprintln!("[two-path-router] classification failed: {}, falling back to Path B", e);
                    telemetry.fallback_used = true;
                    telemetry.fallback_reason = Some(format!("classification error: {}", e));
                    optimize_path_b(&gif.frames, config.path_b_config)
                }
            }
        }
        OptimizerStrategy::PathA => {
            // Force Path A
            telemetry.selected_path = Some(OptimizerPath::PathA);
            match try_path_a(gif, config, &mut telemetry) {
                Ok(frames) => frames,
                Err(e) => {
                    // Fallback to Path B if Path A fails
                    eprintln!("[two-path-router] Path A failed: {}, falling back to Path B", e);
                    telemetry.fallback_used = true;
                    telemetry.fallback_reason = Some(format!("Path A error: {}", e));
                    optimize_path_b(&gif.frames, config.path_b_config)
                }
            }
        }
        OptimizerStrategy::PathB => {
            // Force Path B
            telemetry.selected_path = Some(OptimizerPath::PathB);
            optimize_path_b(&gif.frames, config.path_b_config)
        }
    };

    if config.emit_telemetry {
        telemetry.emit_to_stderr();
    }

    Ok(TwoPathResult { frames, telemetry })
}

/// Classify the sequence and route to Path A or Path B.
fn classify_and_route(
    gif: &Gif,
    _level: OptLevel,
    config: TwoPathConfig,
    telemetry: &mut TwoPathTelemetry,
) -> Result<Vec<Frame>> {
    // Build canonical sequence from decoded frames
    let canonical = CanonicalSequenceBuilder::build(gif)?;

    // Classify
    let classification = classify_sequence(&canonical)?;
    telemetry.classification = Some(classification.clone());
    telemetry.selected_path = Some(classification.path);

    match classification.path {
        OptimizerPath::PathA => try_path_a(gif, config, telemetry),
        OptimizerPath::PathB => Ok(optimize_path_b(&gif.frames, config.path_b_config)),
    }
}

/// Try to optimize using Path A, with fallback to Path B on error.
fn try_path_a(
    gif: &Gif,
    config: TwoPathConfig,
    _telemetry: &mut TwoPathTelemetry,
) -> Result<Vec<Frame>> {
    // Build canonical sequence for Path A
    let canonical = CanonicalSequenceBuilder::build(gif)?;

    // Extract displayed canvases and delays from canonical frames
    let canvases: Vec<_> = canonical.frames.iter().map(|f| f.displayed_canvas.clone()).collect();
    let delays: Vec<_> = canonical.frames.iter().map(|f| f.delay).collect();

    // Optimize using Path A
    let path_a_frames = optimize_path_a(&canvases, &delays, config.path_a_config)?;

    // Convert back to Frame format
    // Path A returns PathAFrame which we need to convert to Frame
    let frames = path_a_frames
        .into_iter()
        .map(|paf| {
            Frame {
                pixels: paf.pixels,
                delay: paf.delay,
                dispose: paf.dispose,
                local_palette: None,
                left: paf.left,
                top: paf.top,
                width: paf.width,
                height: paf.height,
            }
        })
        .collect();

    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DisposalMethod;
    use std::time::Duration;

    fn make_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
        let frames = (0..frame_count)
            .map(|i| Frame {
                pixels: vec![0u8; (width as usize * height as usize * 4)],
                delay: Duration::from_millis(100),
                dispose: if i == 0 {
                    DisposalMethod::None
                } else {
                    DisposalMethod::Keep
                },
                local_palette: None,
                left: 0,
                top: 0,
                width,
                height,
            })
            .collect();

        Gif {
            width,
            height,
            global_palette: None,
            frames,
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        }
    }

    #[test]
    fn test_legacy_strategy_unchanged() {
        let gif = make_test_gif(100, 100, 3);
        let config = TwoPathConfig {
            strategy: OptimizerStrategy::Legacy,
            ..Default::default()
        };

        let result = route_optimize(&gif, OptLevel::O3, config);
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.telemetry.strategy, OptimizerStrategy::Legacy);
        assert_eq!(result.telemetry.selected_path, None);
        assert!(!result.telemetry.fallback_used);
    }

    #[test]
    fn test_forced_path_a() {
        let gif = make_test_gif(100, 100, 3);
        let config = TwoPathConfig {
            strategy: OptimizerStrategy::PathA,
            ..Default::default()
        };

        let result = route_optimize(&gif, OptLevel::O3, config);
        // Path A may fail on simple test data, but should attempt
        let result = result.unwrap_or_else(|_| {
            // If Path A fails, it should have fallen back
            panic!("Path A should not fail on simple test data");
        });
        assert_eq!(result.telemetry.selected_path, Some(OptimizerPath::PathA));
    }

    #[test]
    fn test_forced_path_b() {
        let gif = make_test_gif(100, 100, 3);
        let config = TwoPathConfig {
            strategy: OptimizerStrategy::PathB,
            ..Default::default()
        };

        let result = route_optimize(&gif, OptLevel::O3, config);
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.telemetry.selected_path, Some(OptimizerPath::PathB));
    }

    #[test]
    fn test_telemetry_emission() {
        let gif = make_test_gif(100, 100, 3);
        let config = TwoPathConfig {
            strategy: OptimizerStrategy::PathB,
            emit_telemetry: true,
            ..Default::default()
        };

        let result = route_optimize(&gif, OptLevel::O3, config);
        assert!(result.is_ok());
        // Telemetry should be populated
        let result = result.unwrap();
        assert_eq!(result.telemetry.strategy, OptimizerStrategy::PathB);
    }
}
