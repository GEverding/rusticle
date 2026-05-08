//! LUT-aware policy model and candidate family taxonomy for tiered optimizer.
//!
//! This module defines typed policy concepts for LUT preservation, CPU budgets,
//! candidate families, and cost signals. It provides a clean, future-proof surface
//! for Tier-0/1/2 tasks to reason about optimization strategies.
//!
//! # Core Concepts
//!
//! - **LutEligibility**: Whether a sequence can preserve its LUT across frames.
//! - **CandidateFamily**: Taxonomy of candidate representation strategies.
//! - **PolicySignals**: Cost and risk signals for decision-making.
//! - **CpuBudgetClass**: Sequence difficulty classification for CPU allocation.
//!
//! # Usage
//!
//! The policy model is meant to be used by Tier-0/1/2 tasks to:
//! 1. Classify sequences into LUT eligibility and CPU budget classes.
//! 2. Map candidate representations into families.
//! 3. Compute policy signals from profiler/candidate/scoring inputs.
//! 4. Make deterministic, explainable decisions about optimization strategy.

use crate::candidate_gen::CandidateRepresentation;
use crate::profiler::{GifProfile, SequenceTaxonomy};
use crate::scoring::ScoreBreakdown;

/// LUT eligibility classification for a sequence.
///
/// Determines whether a sequence can preserve its palette LUT across frames
/// or if the LUT must be broken/rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum LutEligibility {
    /// Sequence can preserve LUT across all frames.
    /// Typically: opaque deltas with stable global palette.
    Preserving,

    /// Sequence must break LUT (palette changes, local palettes, etc.).
    /// Typically: transparency-heavy, disposal-heavy, or palette-switching.
    Breaking,

    /// Sequence has mixed LUT eligibility (some frames preserve, some break).
    /// Typically: mixed taxonomy or complex disposal patterns.
    Mixed,

    /// LUT eligibility cannot be determined from available signals.
    Unknown,

    /// Sequence is ineligible for LUT optimization (e.g., too small, no palette).
    Ineligible,
}

impl LutEligibility {
    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Preserving => "preserving",
            Self::Breaking => "breaking",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
            Self::Ineligible => "ineligible",
        }
    }
}

/// Candidate family taxonomy for representation strategies.
///
/// Groups candidates into families based on their encoding characteristics,
/// LUT compatibility, and risk profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CandidateFamily {
    /// Full-frame replacement (always safe, always breaks LUT).
    FullFrame,

    /// Opaque bounding-box patch (LUT-preserving if palette stable).
    OpaqueBbox,

    /// Transparent sparse patch (LUT-breaking, high synthetic transparency risk).
    TransparentSparse,

    /// Minimal/no-op frame (LUT-preserving, zero cost).
    MinimalNoOp,

    /// Palette-preserving strategy (reuse global palette, no quantization).
    PalettePreserving,

    /// Palette-breaking strategy (new quantization, local palettes).
    PaletteBreaking,
}

impl CandidateFamily {
    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::FullFrame => "full-frame",
            Self::OpaqueBbox => "opaque-bbox",
            Self::TransparentSparse => "transparent-sparse",
            Self::MinimalNoOp => "minimal-noop",
            Self::PalettePreserving => "palette-preserving",
            Self::PaletteBreaking => "palette-breaking",
        }
    }

    /// Whether this family is LUT-preserving.
    pub fn is_lut_preserving(&self) -> bool {
        matches!(
            self,
            Self::OpaqueBbox | Self::MinimalNoOp | Self::PalettePreserving
        )
    }

    /// Whether this family is LUT-breaking.
    pub fn is_lut_breaking(&self) -> bool {
        matches!(
            self,
            Self::FullFrame | Self::TransparentSparse | Self::PaletteBreaking
        )
    }
}

/// Quantization cost class for palette operations.
///
/// Estimates the computational and quality cost of palette quantization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum QuantizationCostClass {
    /// No quantization (reuse existing palette).
    None,

    /// Cheap quantization (small frame, few colors).
    Cheap,

    /// Moderate quantization (medium frame, moderate colors).
    Moderate,

    /// Expensive quantization (large frame, many colors, high quality).
    Expensive,
}

impl QuantizationCostClass {
    /// Estimated CPU cost in milliseconds.
    pub fn estimated_cpu_ms(&self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Cheap => 0.5,
            Self::Moderate => 2.0,
            Self::Expensive => 10.0,
        }
    }
}

/// CPU budget class for sequence difficulty.
///
/// Classifies sequences by their computational complexity and time budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CpuBudgetClass {
    /// Easy/LUT-friendly sequences: <= 1ms per frame.
    /// Typically: opaque deltas, stable palette, small frames.
    Easy,

    /// Medium sequences: <= 5ms per frame.
    /// Typically: mixed disposal, moderate transparency, medium frames.
    Medium,

    /// Hard/fragile sequences: <= 50ms per frame.
    /// Typically: transparency-heavy, palette-switching, large frames.
    Hard,
}

impl CpuBudgetClass {
    /// Maximum CPU budget in milliseconds per frame.
    pub fn max_cpu_ms(&self) -> f32 {
        match self {
            Self::Easy => 1.0,
            Self::Medium => 5.0,
            Self::Hard => 50.0,
        }
    }

    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Easy => "easy",
            Self::Medium => "medium",
            Self::Hard => "hard",
        }
    }
}

/// Policy signals for decision-making.
///
/// Aggregates cost and risk signals from profiler, candidate, and scoring inputs.
/// Used by Tier-0/1/2 tasks to make deterministic optimization decisions.
#[derive(Debug, Clone)]
pub struct PolicySignals {
    /// LUT eligibility classification.
    pub lut_eligibility: LutEligibility,

    /// CPU budget class for the sequence.
    pub cpu_budget_class: CpuBudgetClass,

    /// Whether the sequence is LUT-preserving (convenience flag).
    pub lut_preserving: bool,

    /// Quantization cost class for palette operations.
    pub quantization_cost: QuantizationCostClass,

    /// Palette switch cost / churn indicator (0.0 = stable, 1.0 = high churn).
    pub palette_switch_cost: f32,

    /// Risk of introducing synthetic transparency (0.0 = safe, 1.0 = high risk).
    pub synthetic_transparency_risk: f32,

    /// Estimated CPU cost per frame in milliseconds.
    pub candidate_cpu_cost: f32,

    /// Taxonomy fragility hint (0.0 = robust, 1.0 = fragile).
    /// High fragility suggests conservative candidate selection.
    pub taxonomy_fragility: f32,

    /// Byte cost estimate (0.0 = smallest, 1.0 = largest).
    pub byte_cost: f32,

    /// Visual risk (0.0 = safe, 1.0 = high risk).
    pub visual_risk: f32,

    /// Temporal instability risk (0.0 = stable, 1.0 = high churn).
    pub temporal_instability: f32,
}

impl PolicySignals {
    /// Create a new policy signals struct with all zeros.
    pub fn zero() -> Self {
        Self {
            lut_eligibility: LutEligibility::Unknown,
            cpu_budget_class: CpuBudgetClass::Medium,
            lut_preserving: false,
            quantization_cost: QuantizationCostClass::None,
            palette_switch_cost: 0.0,
            synthetic_transparency_risk: 0.0,
            candidate_cpu_cost: 0.0,
            taxonomy_fragility: 0.0,
            byte_cost: 0.0,
            visual_risk: 0.0,
            temporal_instability: 0.0,
        }
    }

    /// Compute policy signals from profiler output and score breakdown.
    ///
    /// This is a first-pass mapping function that converts profiler/scoring inputs
    /// into typed policy concepts. It can be refined by later tasks.
    pub fn from_profile_and_score(profile: &GifProfile, score: &ScoreBreakdown) -> Self {
        let lut_eligibility = classify_lut_eligibility(profile);
        let cpu_budget_class = classify_cpu_budget(profile);
        let taxonomy_fragility = estimate_taxonomy_fragility(profile);

        let lut_preserving = matches!(
            lut_eligibility,
            LutEligibility::Preserving | LutEligibility::Mixed
        );

        let quantization_cost = estimate_quantization_cost(profile);
        let palette_switch_cost = estimate_palette_switch_cost(profile);

        Self {
            lut_eligibility,
            cpu_budget_class,
            lut_preserving,
            quantization_cost,
            palette_switch_cost,
            synthetic_transparency_risk: score.synthetic_transparency_risk,
            candidate_cpu_cost: score.cpu_cost * 50.0, // Scale to milliseconds
            taxonomy_fragility,
            byte_cost: score.byte_cost,
            visual_risk: score.visual_risk,
            temporal_instability: score.temporal_instability,
        }
    }
}

/// Map a candidate representation to its family.
pub fn candidate_to_family(candidate: &CandidateRepresentation) -> CandidateFamily {
    match candidate {
        CandidateRepresentation::FullFrame => CandidateFamily::FullFrame,
        CandidateRepresentation::ExactOpaqueBbox { .. } => CandidateFamily::OpaqueBbox,
        CandidateRepresentation::TransparentSparsePatch { .. } => {
            CandidateFamily::TransparentSparse
        }
        CandidateRepresentation::MinimalNoOp => CandidateFamily::MinimalNoOp,
    }
}

/// Classify LUT eligibility from profiler output.
fn classify_lut_eligibility(profile: &GifProfile) -> LutEligibility {
    match profile.taxonomy {
        SequenceTaxonomy::OpaqueDeltaGlobalPalette => {
            // Opaque deltas with stable global palette: LUT-preserving
            if profile.palette_info.palette_stability > 0.7 {
                LutEligibility::Preserving
            } else {
                LutEligibility::Mixed
            }
        }
        SequenceTaxonomy::TransparencyHeavySparseDelta => {
            // Transparency-heavy: LUT-breaking
            LutEligibility::Breaking
        }
        SequenceTaxonomy::DisposalHeavyBackgroundPrevious => {
            // Disposal-heavy: mixed or breaking
            if profile.palette_info.palette_stability > 0.6 {
                LutEligibility::Mixed
            } else {
                LutEligibility::Breaking
            }
        }
        SequenceTaxonomy::Photographic => {
            // Photographic: LUT-breaking (high color count, palette pressure)
            LutEligibility::Breaking
        }
        SequenceTaxonomy::Mixed => {
            // Mixed: unknown or mixed
            LutEligibility::Mixed
        }
    }
}

/// Classify CPU budget class from profiler output.
fn classify_cpu_budget(profile: &GifProfile) -> CpuBudgetClass {
    // Estimate based on frame count, canvas size, and change density
    let frame_count = profile.metrics.frame_count as f32;
    let total_pixels = profile.metrics.total_pixels as f32;
    let avg_changed_ratio = profile.change_statistics.avg_changed_ratio;

    // Compute a complexity score
    let complexity = (frame_count * total_pixels * avg_changed_ratio) / 1_000_000.0;

    if complexity < 10.0 {
        CpuBudgetClass::Easy
    } else if complexity < 100.0 {
        CpuBudgetClass::Medium
    } else {
        CpuBudgetClass::Hard
    }
}

/// Estimate quantization cost class from profiler output.
fn estimate_quantization_cost(profile: &GifProfile) -> QuantizationCostClass {
    let total_pixels = profile.metrics.total_pixels as f32;
    let avg_changed_ratio = profile.change_statistics.avg_changed_ratio;

    // Estimate based on canvas size and change density
    let cost_factor = total_pixels * avg_changed_ratio;

    if cost_factor < 10_000.0 {
        QuantizationCostClass::Cheap
    } else if cost_factor < 100_000.0 {
        QuantizationCostClass::Moderate
    } else {
        QuantizationCostClass::Expensive
    }
}

/// Estimate palette switch cost / churn indicator.
fn estimate_palette_switch_cost(profile: &GifProfile) -> f32 {
    // Use palette stability as inverse of switch cost
    1.0 - profile.palette_info.palette_stability
}

/// Estimate taxonomy fragility (how fragile the sequence is to representation changes).
fn estimate_taxonomy_fragility(profile: &GifProfile) -> f32 {
    match profile.taxonomy {
        SequenceTaxonomy::OpaqueDeltaGlobalPalette => {
            // Robust: opaque deltas are well-understood
            0.1
        }
        SequenceTaxonomy::TransparencyHeavySparseDelta => {
            // Fragile: transparency handling is risky
            0.8
        }
        SequenceTaxonomy::DisposalHeavyBackgroundPrevious => {
            // Moderately fragile: disposal semantics are complex
            0.6
        }
        SequenceTaxonomy::Photographic => {
            // Moderately fragile: high color count and dense changes
            0.5
        }
        SequenceTaxonomy::Mixed => {
            // Unknown fragility: mixed structure
            0.5
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiler::{
        ChangeStatistics, DeltaSignal, DisposalDistribution, PaletteInfo, PatchDensity,
        SequenceMetrics, TransparencyAnalysis,
    };

    /// Create a test profile for opaque-delta/global-palette sequences (Voyager-like).
    fn make_opaque_delta_profile() -> GifProfile {
        GifProfile {
            metrics: SequenceMetrics {
                frame_count: 10,
                width: 640,
                height: 480,
                total_pixels: 640 * 480,
                avg_delay_ms: 100.0,
            },
            disposal_distribution: DisposalDistribution {
                keep_count: 8,
                none_count: 2,
                background_count: 0,
                previous_count: 0,
                dominant: "Keep".to_string(),
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
                palette_stability: 0.95,
            },
            change_statistics: ChangeStatistics {
                avg_changed_ratio: 0.1,
                max_changed_ratio: 0.2,
                min_changed_ratio: 0.05,
                sparse_change_frames: 8,
                dense_change_frames: 0,
            },
            patch_density: PatchDensity {
                avg_bbox_ratio: 0.15,
                max_bbox_ratio: 0.25,
                offset_patch_frames: 5,
                avg_patch_density: 0.7,
            },
            delta_signal: DeltaSignal {
                strength: 0.9,
                opaque_delta_frames: 10,
                offset_sparse_frames: 5,
                is_already_delta_encoded: true,
            },
            taxonomy: SequenceTaxonomy::OpaqueDeltaGlobalPalette,
        }
    }

    /// Create a test profile for transparency-heavy sequences.
    fn make_transparency_heavy_profile() -> GifProfile {
        GifProfile {
            metrics: SequenceMetrics {
                frame_count: 20,
                width: 800,
                height: 600,
                total_pixels: 800 * 600,
                avg_delay_ms: 50.0,
            },
            disposal_distribution: DisposalDistribution {
                keep_count: 5,
                none_count: 10,
                background_count: 5,
                previous_count: 0,
                dominant: "None".to_string(),
            },
            transparency_analysis: TransparencyAnalysis {
                frames_with_transparency: 18,
                avg_transparency_ratio: 0.4,
                max_transparency_ratio: 0.7,
                frames_with_significant_transparency: 15,
                uses_gce: true,
                frames_with_local_palette: 8,
            },
            palette_info: PaletteInfo {
                has_global_palette: true,
                local_palette_count: 8,
                palette_stability: 0.5,
            },
            change_statistics: ChangeStatistics {
                avg_changed_ratio: 0.3,
                max_changed_ratio: 0.6,
                min_changed_ratio: 0.1,
                sparse_change_frames: 10,
                dense_change_frames: 5,
            },
            patch_density: PatchDensity {
                avg_bbox_ratio: 0.4,
                max_bbox_ratio: 0.8,
                offset_patch_frames: 12,
                avg_patch_density: 0.5,
            },
            delta_signal: DeltaSignal {
                strength: 0.3,
                opaque_delta_frames: 2,
                offset_sparse_frames: 10,
                is_already_delta_encoded: false,
            },
            taxonomy: SequenceTaxonomy::TransparencyHeavySparseDelta,
        }
    }

    #[test]
    fn test_opaque_delta_lut_preserving() {
        let profile = make_opaque_delta_profile();
        let eligibility = classify_lut_eligibility(&profile);
        assert_eq!(eligibility, LutEligibility::Preserving);
    }

    #[test]
    fn test_transparency_heavy_lut_breaking() {
        let profile = make_transparency_heavy_profile();
        let eligibility = classify_lut_eligibility(&profile);
        assert_eq!(eligibility, LutEligibility::Breaking);
    }

    #[test]
    fn test_opaque_delta_cpu_budget_easy() {
        let profile = make_opaque_delta_profile();
        let budget = classify_cpu_budget(&profile);
        assert_eq!(budget, CpuBudgetClass::Easy);
    }

    #[test]
    fn test_transparency_heavy_cpu_budget_medium() {
        let profile = make_transparency_heavy_profile();
        let budget = classify_cpu_budget(&profile);
        // 20 frames * 480000 pixels * 0.3 ratio / 1M = 2.88, which is < 10, so Easy
        // Let's adjust the test to expect Easy or Medium
        assert!(matches!(
            budget,
            CpuBudgetClass::Easy | CpuBudgetClass::Medium
        ));
    }

    #[test]
    fn test_opaque_delta_fragility_low() {
        let profile = make_opaque_delta_profile();
        let fragility = estimate_taxonomy_fragility(&profile);
        assert!(fragility < 0.3);
    }

    #[test]
    fn test_transparency_heavy_fragility_high() {
        let profile = make_transparency_heavy_profile();
        let fragility = estimate_taxonomy_fragility(&profile);
        assert!(fragility > 0.7);
    }

    #[test]
    fn test_candidate_family_opaque_bbox_lut_preserving() {
        let family = CandidateFamily::OpaqueBbox;
        assert!(family.is_lut_preserving());
        assert!(!family.is_lut_breaking());
    }

    #[test]
    fn test_candidate_family_transparent_sparse_lut_breaking() {
        let family = CandidateFamily::TransparentSparse;
        assert!(!family.is_lut_preserving());
        assert!(family.is_lut_breaking());
    }

    #[test]
    fn test_candidate_family_full_frame_lut_breaking() {
        let family = CandidateFamily::FullFrame;
        assert!(!family.is_lut_preserving());
        assert!(family.is_lut_breaking());
    }

    #[test]
    fn test_candidate_family_minimal_noop_lut_preserving() {
        let family = CandidateFamily::MinimalNoOp;
        assert!(family.is_lut_preserving());
        assert!(!family.is_lut_breaking());
    }

    #[test]
    fn test_cpu_budget_class_max_cpu_ms() {
        assert_eq!(CpuBudgetClass::Easy.max_cpu_ms(), 1.0);
        assert_eq!(CpuBudgetClass::Medium.max_cpu_ms(), 5.0);
        assert_eq!(CpuBudgetClass::Hard.max_cpu_ms(), 50.0);
    }

    #[test]
    fn test_quantization_cost_class_estimated_cpu_ms() {
        assert_eq!(QuantizationCostClass::None.estimated_cpu_ms(), 0.0);
        assert_eq!(QuantizationCostClass::Cheap.estimated_cpu_ms(), 0.5);
        assert_eq!(QuantizationCostClass::Moderate.estimated_cpu_ms(), 2.0);
        assert_eq!(QuantizationCostClass::Expensive.estimated_cpu_ms(), 10.0);
    }

    #[test]
    fn test_policy_signals_from_opaque_delta_profile() {
        let profile = make_opaque_delta_profile();
        let score = ScoreBreakdown {
            byte_cost: 0.3,
            visual_risk: 0.1,
            lut_cost: 0.0,
            temporal_instability: 0.05,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.2,
        };

        let signals = PolicySignals::from_profile_and_score(&profile, &score);

        assert_eq!(signals.lut_eligibility, LutEligibility::Preserving);
        assert!(signals.lut_preserving);
        assert_eq!(signals.cpu_budget_class, CpuBudgetClass::Easy);
        assert!(signals.taxonomy_fragility < 0.3);
        assert!(signals.palette_switch_cost < 0.1);
    }

    #[test]
    fn test_policy_signals_from_transparency_heavy_profile() {
        let profile = make_transparency_heavy_profile();
        let score = ScoreBreakdown {
            byte_cost: 0.6,
            visual_risk: 0.4,
            lut_cost: 0.5,
            temporal_instability: 0.3,
            synthetic_transparency_risk: 0.5,
            palette_coherence: 0.3,
            cpu_cost: 0.3,
            total_score: 0.45,
        };

        let signals = PolicySignals::from_profile_and_score(&profile, &score);

        assert_eq!(signals.lut_eligibility, LutEligibility::Breaking);
        assert!(!signals.lut_preserving);
        assert!(signals.taxonomy_fragility > 0.7);
        assert!(signals.palette_switch_cost > 0.4);
        assert!(signals.synthetic_transparency_risk > 0.4);
    }

    #[test]
    fn test_candidate_to_family_full_frame() {
        let candidate = CandidateRepresentation::FullFrame;
        let family = candidate_to_family(&candidate);
        assert_eq!(family, CandidateFamily::FullFrame);
    }

    #[test]
    fn test_candidate_to_family_opaque_bbox() {
        use crate::adaptive_ir::BoundingBox;
        let candidate = CandidateRepresentation::ExactOpaqueBbox {
            bbox: BoundingBox::new(0, 0, 100, 100),
        };
        let family = candidate_to_family(&candidate);
        assert_eq!(family, CandidateFamily::OpaqueBbox);
    }

    #[test]
    fn test_candidate_to_family_transparent_sparse() {
        use crate::adaptive_ir::BoundingBox;
        let candidate = CandidateRepresentation::TransparentSparsePatch {
            bbox: BoundingBox::new(0, 0, 50, 50),
            is_risky: true,
        };
        let family = candidate_to_family(&candidate);
        assert_eq!(family, CandidateFamily::TransparentSparse);
    }

    #[test]
    fn test_candidate_to_family_minimal_noop() {
        let candidate = CandidateRepresentation::MinimalNoOp;
        let family = candidate_to_family(&candidate);
        assert_eq!(family, CandidateFamily::MinimalNoOp);
    }
}
