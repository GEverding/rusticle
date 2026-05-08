//! Palette strategy layer for stable global/local palette choices.
//!
//! This module defines palette strategy selection that can prefer stable global palettes,
//! reuse existing palette structure when safe, and fall back to local palettes only when
//! justified by quality or color-count pressure.
//!
//! # Strategy Types
//!
//! - **ReuseGlobalPreferred**: Reuse the source GIF's global palette if present and stable.
//! - **DeriveSequenceGlobalPreferred**: Derive a new global palette from the entire sequence.
//! - **LocalPaletteFallback**: Use local palettes per frame (highest flexibility, highest cost).
//! - **MixedAdaptive**: Wrapper strategy that allows multiple sub-strategies per frame.
//!
//! # Rules
//!
//! - Voyager-like sequences (opaque-delta/global-palette) bias strongly toward global-preferred.
//! - Transparency-heavy/mixed sequences may allow local fallback.
//! - Disposal-heavy sequences should be conservative about palette churn.

use crate::adaptive_ir::CanonicalSequence;
use crate::profiler::{GifProfile, SequenceTaxonomy};
use crate::types::Gif;

/// Palette strategy for a sequence or frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PaletteStrategy {
    /// Reuse the source GIF's global palette if present and stable.
    /// Preferred for voyager-like sequences with minimal palette churn.
    ReuseGlobalPreferred,
    /// Derive a new global palette from the entire sequence.
    /// Useful when source lacks global palette or has unstable palette.
    DeriveSequenceGlobalPreferred,
    /// Use local palettes per frame (highest flexibility, highest cost).
    /// Fallback for transparency-heavy or color-pressure cases.
    LocalPaletteFallback,
}

impl PaletteStrategy {
    /// Human-readable name for the strategy.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ReuseGlobalPreferred => "reuse-global-preferred",
            Self::DeriveSequenceGlobalPreferred => "derive-sequence-global-preferred",
            Self::LocalPaletteFallback => "local-palette-fallback",
        }
    }
}

/// Reason code for palette strategy selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StrategyReason {
    /// Source has stable global palette; reuse is safe and efficient.
    SourceGlobalStable,
    /// Source lacks global palette; derive from sequence.
    SourceNoGlobal,
    /// Source global palette is unstable; derive new one.
    SourceGlobalUnstable,
    /// Transparency pressure requires local palette flexibility.
    TransparencyPressure,
    /// Color pressure (high unique colors) requires local palettes.
    ColorPressure,
    /// Mixed/unknown structure; allow local fallback.
    MixedStructure,
    /// Disposal-heavy sequence; conservative about churn.
    DisposalHeavy,
}

/// Palette strategy summary / metadata.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PaletteStrategyMetadata {
    /// Whether source has global palette.
    pub source_has_global_palette: bool,
    /// Whether source uses local palettes.
    pub source_uses_local_palettes: bool,
    /// Palette stability from profiler (0.0 = unstable, 1.0 = stable).
    pub palette_stability: f32,
    /// Transparency pressure (0.0 = none, 1.0 = high).
    pub transparency_pressure: f32,
    /// Color pressure / complexity (0.0 = low, 1.0 = high).
    pub color_pressure: f32,
    /// Disposal churn indicator (0.0 = stable, 1.0 = high churn).
    pub disposal_churn: f32,
    /// Rationale / reason codes for chosen strategy set.
    pub reason_codes: Vec<StrategyReason>,
}

/// Palette strategy set for a sequence.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PaletteStrategySet {
    /// Preferred strategies in priority order.
    pub preferred: Vec<PaletteStrategy>,
    /// Allowed fallback strategies.
    pub allowed: Vec<PaletteStrategy>,
    /// Metadata about the strategy selection.
    pub metadata: PaletteStrategyMetadata,
}

impl PaletteStrategySet {
    /// Get the highest-priority preferred strategy.
    pub fn primary(&self) -> Option<PaletteStrategy> {
        self.preferred.first().copied()
    }

    /// Check if a strategy is allowed.
    pub fn is_allowed(&self, strategy: PaletteStrategy) -> bool {
        self.preferred.contains(&strategy) || self.allowed.contains(&strategy)
    }
}

/// Determine palette strategies for a sequence given canonical IR and profiler output.
///
/// This function analyzes the sequence structure and profiler summary to produce
/// a set of allowed/preferred palette strategies. It does not fully quantize yet,
/// but expresses preferences and constraints cleanly for the later chooser.
///
/// # Rules
///
/// - Voyager-like sequences bias strongly toward global-preferred strategies.
/// - Transparency-heavy/mixed sequences may allow local fallback.
/// - Disposal-heavy sequences should be conservative about palette churn.
pub fn determine_palette_strategies(
    gif: &Gif,
    _seq: &CanonicalSequence,
    profile: &GifProfile,
) -> PaletteStrategySet {
    let mut preferred = Vec::new();
    let mut allowed = Vec::new();
    let mut reason_codes = Vec::new();

    // Extract key metrics
    let has_global_palette = gif.global_palette.is_some();
    let uses_local_palettes = gif.frames.iter().any(|f| f.local_palette.is_some());
    let palette_stability = profile.palette_info.palette_stability;
    let transparency_pressure = compute_transparency_pressure(profile);
    let color_pressure = compute_color_pressure(profile);
    let disposal_churn = compute_disposal_churn(profile);

    // Rule 1: Voyager-like sequences bias strongly toward global-preferred
    if profile.taxonomy == SequenceTaxonomy::OpaqueDeltaGlobalPalette {
        if has_global_palette && palette_stability > 0.7 {
            preferred.push(PaletteStrategy::ReuseGlobalPreferred);
            reason_codes.push(StrategyReason::SourceGlobalStable);
        } else {
            preferred.push(PaletteStrategy::DeriveSequenceGlobalPreferred);
            reason_codes.push(StrategyReason::SourceNoGlobal);
        }
        // Allow local fallback only as last resort
        allowed.push(PaletteStrategy::LocalPaletteFallback);
    }
    // Rule 2: Transparency-heavy sequences may allow local fallback
    else if profile.taxonomy == SequenceTaxonomy::TransparencyHeavySparseDelta {
        if transparency_pressure > 0.6 {
            preferred.push(PaletteStrategy::LocalPaletteFallback);
            reason_codes.push(StrategyReason::TransparencyPressure);
        } else if has_global_palette && palette_stability > 0.6 {
            preferred.push(PaletteStrategy::ReuseGlobalPreferred);
            reason_codes.push(StrategyReason::SourceGlobalStable);
        } else {
            preferred.push(PaletteStrategy::DeriveSequenceGlobalPreferred);
            reason_codes.push(StrategyReason::SourceGlobalUnstable);
        }
        allowed.push(PaletteStrategy::LocalPaletteFallback);
    }
    // Rule 3: Disposal-heavy sequences should be conservative about palette churn
    else if profile.taxonomy == SequenceTaxonomy::DisposalHeavyBackgroundPrevious {
        if has_global_palette && palette_stability > 0.6 {
            preferred.push(PaletteStrategy::ReuseGlobalPreferred);
            reason_codes.push(StrategyReason::SourceGlobalStable);
        } else {
            preferred.push(PaletteStrategy::DeriveSequenceGlobalPreferred);
            reason_codes.push(StrategyReason::SourceGlobalUnstable);
        }
        reason_codes.push(StrategyReason::DisposalHeavy);
        allowed.push(PaletteStrategy::LocalPaletteFallback);
    }
    // Rule 4: Photographic sequences prefer local palettes for quality
    else if profile.taxonomy == SequenceTaxonomy::Photographic {
        if color_pressure > 0.7 {
            preferred.push(PaletteStrategy::LocalPaletteFallback);
            reason_codes.push(StrategyReason::ColorPressure);
        } else if has_global_palette && palette_stability > 0.5 {
            preferred.push(PaletteStrategy::ReuseGlobalPreferred);
            reason_codes.push(StrategyReason::SourceGlobalStable);
        } else {
            preferred.push(PaletteStrategy::DeriveSequenceGlobalPreferred);
            reason_codes.push(StrategyReason::SourceGlobalUnstable);
        }
        allowed.push(PaletteStrategy::LocalPaletteFallback);
    }
    // Rule 5: Mixed/unknown structure - use adaptive strategy
    else {
        reason_codes.push(StrategyReason::MixedStructure);
        if transparency_pressure > 0.5 || color_pressure > 0.6 {
            preferred.push(PaletteStrategy::LocalPaletteFallback);
        } else if has_global_palette && palette_stability > 0.5 {
            preferred.push(PaletteStrategy::ReuseGlobalPreferred);
        } else {
            preferred.push(PaletteStrategy::DeriveSequenceGlobalPreferred);
        }
        allowed.push(PaletteStrategy::LocalPaletteFallback);
    }

    // Ensure we always have at least one preferred strategy
    if preferred.is_empty() {
        preferred.push(PaletteStrategy::DeriveSequenceGlobalPreferred);
    }

    // Ensure allowed contains all preferred strategies
    for &strategy in &preferred {
        if !allowed.contains(&strategy) {
            allowed.push(strategy);
        }
    }

    let metadata = PaletteStrategyMetadata {
        source_has_global_palette: has_global_palette,
        source_uses_local_palettes: uses_local_palettes,
        palette_stability,
        transparency_pressure,
        color_pressure,
        disposal_churn,
        reason_codes,
    };

    PaletteStrategySet {
        preferred,
        allowed,
        metadata,
    }
}

/// Compute transparency pressure (0.0 = none, 1.0 = high).
fn compute_transparency_pressure(profile: &GifProfile) -> f32 {
    let frames_with_transparency = profile.transparency_analysis.frames_with_transparency as f32;
    let frame_count = profile.metrics.frame_count as f32;
    let transparency_ratio = frames_with_transparency / frame_count.max(1.0);

    let avg_transparency = profile.transparency_analysis.avg_transparency_ratio;

    // Combine: frames with transparency + average transparency level
    (transparency_ratio * 0.5 + avg_transparency * 0.5).min(1.0)
}

/// Compute color pressure / complexity (0.0 = low, 1.0 = high).
fn compute_color_pressure(profile: &GifProfile) -> f32 {
    // Heuristic: dense changes + high frame count = high color pressure
    let dense_ratio = profile.change_statistics.dense_change_frames as f32
        / profile.metrics.frame_count.max(1) as f32;
    let avg_changed = profile.change_statistics.avg_changed_ratio;

    // Combine: dense changes + average changed ratio
    (dense_ratio * 0.5 + avg_changed * 0.5).min(1.0)
}

/// Compute disposal churn indicator (0.0 = stable, 1.0 = high churn).
fn compute_disposal_churn(profile: &GifProfile) -> f32 {
    let dist = &profile.disposal_distribution;
    let total =
        (dist.keep_count + dist.none_count + dist.background_count + dist.previous_count) as f32;

    if total == 0.0 {
        return 0.0;
    }

    // Churn is high when disposal methods are mixed (not dominated by one)
    let keep_ratio = dist.keep_count as f32 / total;
    let none_ratio = dist.none_count as f32 / total;
    let background_ratio = dist.background_count as f32 / total;
    let previous_ratio = dist.previous_count as f32 / total;

    // Entropy-like measure: high when all methods are equally used
    let max_ratio = keep_ratio
        .max(none_ratio)
        .max(background_ratio)
        .max(previous_ratio);
    (1.0 - max_ratio).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_ir::CanonicalSequenceBuilder;

    fn create_test_gif_opaque_delta() -> Gif {
        // Voyager-like: opaque deltas with global palette
        use crate::types::{DisposalMethod, Frame, Palette};
        use std::time::Duration;

        let palette = Palette {
            colors: vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]],
        };

        let mut frames = Vec::new();
        for _i in 0..3 {
            let mut pixels = vec![0u8; 100 * 100 * 4];
            // Fill with opaque red
            for j in 0..100 * 100 {
                pixels[j * 4] = 255;
                pixels[j * 4 + 3] = 255;
            }
            frames.push(Frame {
                pixels,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
                left: 0,
                top: 0,
                width: 100,
                height: 100,
            });
        }

        Gif {
            width: 100,
            height: 100,
            global_palette: Some(palette),
            frames,
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        }
    }

    fn create_test_gif_transparency_heavy() -> Gif {
        // Transparency-heavy: sparse changes with transparency
        use crate::types::{DisposalMethod, Frame, Palette};
        use std::time::Duration;

        let palette = Palette {
            colors: vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]],
        };

        let mut frames = Vec::new();
        for _i in 0..3 {
            let mut pixels = vec![0u8; 100 * 100 * 4];
            // Fill with mostly transparent, some opaque
            for j in 0..100 * 100 {
                if j % 10 == 0 {
                    pixels[j * 4] = 255;
                    pixels[j * 4 + 3] = 255;
                } else {
                    pixels[j * 4 + 3] = 0; // transparent
                }
            }
            frames.push(Frame {
                pixels,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
                left: 0,
                top: 0,
                width: 100,
                height: 100,
            });
        }

        Gif {
            width: 100,
            height: 100,
            global_palette: Some(palette),
            frames,
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        }
    }

    #[test]
    fn test_voyager_like_opaque_delta_global_preferred() {
        let gif = create_test_gif_opaque_delta();
        let seq = CanonicalSequenceBuilder::build(&gif).expect("build sequence");
        let profile = crate::profiler::profile_canonical_sequence(&seq).expect("profile");

        let strategy_set = determine_palette_strategies(&gif, &seq, &profile);

        // Voyager-like should prefer global-preferred strategies
        assert!(!strategy_set.preferred.is_empty());
        let primary = strategy_set.primary().expect("has primary");
        assert!(
            primary == PaletteStrategy::ReuseGlobalPreferred
                || primary == PaletteStrategy::DeriveSequenceGlobalPreferred,
            "voyager-like should prefer global-preferred, got {:?}",
            primary
        );

        // Should allow local fallback
        assert!(strategy_set.is_allowed(PaletteStrategy::LocalPaletteFallback));
    }

    #[test]
    fn test_transparency_heavy_allows_local_fallback() {
        let gif = create_test_gif_transparency_heavy();
        let seq = CanonicalSequenceBuilder::build(&gif).expect("build sequence");
        let profile = crate::profiler::profile_canonical_sequence(&seq).expect("profile");

        let strategy_set = determine_palette_strategies(&gif, &seq, &profile);

        // Should allow local fallback
        assert!(strategy_set.is_allowed(PaletteStrategy::LocalPaletteFallback));

        // Should have at least one preferred strategy
        assert!(!strategy_set.preferred.is_empty());
    }

    #[test]
    fn test_strategy_metadata_populated() {
        let gif = create_test_gif_opaque_delta();
        let seq = CanonicalSequenceBuilder::build(&gif).expect("build sequence");
        let profile = crate::profiler::profile_canonical_sequence(&seq).expect("profile");

        let strategy_set = determine_palette_strategies(&gif, &seq, &profile);

        // Metadata should be populated
        assert!(strategy_set.metadata.source_has_global_palette);
        assert!(strategy_set.metadata.palette_stability >= 0.0);
        assert!(strategy_set.metadata.palette_stability <= 1.0);
        assert!(strategy_set.metadata.transparency_pressure >= 0.0);
        assert!(strategy_set.metadata.transparency_pressure <= 1.0);
        assert!(strategy_set.metadata.color_pressure >= 0.0);
        assert!(strategy_set.metadata.color_pressure <= 1.0);
        assert!(!strategy_set.metadata.reason_codes.is_empty());
    }

    #[test]
    fn test_strategy_always_has_preferred() {
        let gif = create_test_gif_opaque_delta();
        let seq = CanonicalSequenceBuilder::build(&gif).expect("build sequence");
        let profile = crate::profiler::profile_canonical_sequence(&seq).expect("profile");

        let strategy_set = determine_palette_strategies(&gif, &seq, &profile);

        // Must always have at least one preferred strategy
        assert!(!strategy_set.preferred.is_empty());
        assert!(strategy_set.primary().is_some());
    }

    #[test]
    fn test_allowed_contains_preferred() {
        let gif = create_test_gif_opaque_delta();
        let seq = CanonicalSequenceBuilder::build(&gif).expect("build sequence");
        let profile = crate::profiler::profile_canonical_sequence(&seq).expect("profile");

        let strategy_set = determine_palette_strategies(&gif, &seq, &profile);

        // All preferred strategies should be in allowed
        for &strategy in &strategy_set.preferred {
            assert!(
                strategy_set.allowed.contains(&strategy),
                "preferred strategy {:?} not in allowed",
                strategy
            );
        }
    }
}
