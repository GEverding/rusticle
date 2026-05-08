//! Tier-0 classifier: cheap front-door decision for LUT-friendly sequences.
//!
//! This module implements the first bounded-CPU control layer for optimizer-v2.
//! It classifies sequences into decision states based on policy signals, strongly
//! biasing toward LUT-friendly structural paths and early exit when possible.
//!
//! # Decision States
//!
//! - **EarlyExitStructural**: Sequence is easy enough to skip expensive search.
//!   Use LUT-friendly structural path; strongly bias toward palette-preserving
//!   and structural/lossless behavior.
//!
//! - **NeedsTier1**: Sequence requires cheap proxy pruning to narrow candidate space.
//!   Run bounded candidate evaluation with conservative heuristics.
//!
//! - **NeedsTier2**: Sequence is fragile/uncertain enough to permit bounded
//!   encode-and-measure. Full candidate evaluation with quality metrics.
//!
//! # Classification Criteria
//!
//! The classifier uses explicit entry criteria from the typed policy model:
//! - LUT eligibility (preserving vs breaking)
//! - CPU budget class (easy/medium/hard)
//! - Taxonomy fragility (robust vs fragile)
//! - Transparency risk (safe vs risky)
//! - Palette churn risk (stable vs unstable)
//! - Changed-area characteristics (sparse vs dense)
//!
//! # Rationale
//!
//! Tier-0 is the "cheap front door" that avoids expensive search for sequences
//! that are structurally simple and well-understood. It strongly favors:
//! - LUT-preserving candidate families
//! - Palette-preserving/global-preferred strategies
//! - Structural/lossless behavior
//!
//! This keeps the optimizer fast for common cases (Voyager-like opaque deltas)
//! while routing complex sequences to more expensive tiers.

use crate::lut_policy::{CpuBudgetClass, LutEligibility, PolicySignals};
use crate::profiler::GifProfile;
use crate::scoring::ScoreBreakdown;

/// Tier-0 classification decision state.
///
/// Determines whether a sequence should early-exit with structural path,
/// or proceed to Tier-1 or Tier-2 for more expensive evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Tier0Decision {
    /// Early exit: use LUT-friendly structural path, skip expensive search.
    ///
    /// Entry criteria:
    /// - LUT eligibility is Preserving or Mixed (not Breaking)
    /// - CPU budget class is Easy or Medium
    /// - Taxonomy fragility is low (< 0.4)
    /// - Synthetic transparency risk is low (< 0.2)
    /// - Palette churn risk is low (< 0.3)
    /// - Changed-area ratio is sparse (< 0.4)
    ///
    /// Strongly bias toward:
    /// - LUT-preserving candidate families (OpaqueBbox, MinimalNoOp, PalettePreserving)
    /// - Palette-preserving strategies
    /// - Structural/lossless behavior
    EarlyExitStructural,

    /// Needs Tier-1: run cheap proxy pruning to narrow candidate space.
    ///
    /// Entry criteria:
    /// - LUT eligibility is Mixed or Breaking
    /// - CPU budget class is Medium
    /// - Taxonomy fragility is moderate (0.4-0.7)
    /// - Synthetic transparency risk is moderate (0.2-0.5)
    /// - Palette churn risk is moderate (0.3-0.6)
    /// - Changed-area ratio is moderate (0.4-0.6)
    ///
    /// Use conservative heuristics:
    /// - Prefer opaque candidates over transparent
    /// - Penalize palette-breaking strategies
    /// - Avoid risky sparse patches
    NeedsTier1,

    /// Needs Tier-2: fragile/uncertain, permit bounded encode-and-measure.
    ///
    /// Entry criteria:
    /// - LUT eligibility is Breaking
    /// - CPU budget class is Hard
    /// - Taxonomy fragility is high (> 0.7)
    /// - Synthetic transparency risk is high (> 0.5)
    /// - Palette churn risk is high (> 0.6)
    /// - Changed-area ratio is dense (> 0.6)
    ///
    /// Full candidate evaluation with quality metrics:
    /// - Encode-and-measure for top candidates
    /// - Perceptual quality assessment
    /// - Temporal stability verification
    NeedsTier2,
}

impl Tier0Decision {
    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::EarlyExitStructural => "early-exit-structural",
            Self::NeedsTier1 => "needs-tier1",
            Self::NeedsTier2 => "needs-tier2",
        }
    }

    /// Whether this decision allows early exit (skip expensive search).
    pub fn allows_early_exit(&self) -> bool {
        matches!(self, Self::EarlyExitStructural)
    }

    /// Whether this decision requires encode-and-measure.
    pub fn requires_encode_and_measure(&self) -> bool {
        matches!(self, Self::NeedsTier2)
    }
}

/// Tier-0 classifier: maps policy signals to decision states.
pub struct Tier0Classifier;

impl Tier0Classifier {
    /// Classify a sequence based on policy signals.
    ///
    /// This is the main entry point for Tier-0 decision-making.
    /// It uses explicit criteria from the typed policy model to classify
    /// sequences into decision states.
    pub fn classify(signals: &PolicySignals) -> Tier0Decision {
        // Early exit criteria: LUT-preserving, easy/medium, low fragility, low risk
        if Self::is_early_exit_candidate(signals) {
            return Tier0Decision::EarlyExitStructural;
        }

        // Tier-2 criteria: breaking, hard, high fragility, high risk
        if Self::is_tier2_candidate(signals) {
            return Tier0Decision::NeedsTier2;
        }

        // Default to Tier-1 for everything else
        Tier0Decision::NeedsTier1
    }

    /// Check if a sequence qualifies for early exit.
    ///
    /// Entry criteria:
    /// - LUT eligibility is Preserving or Mixed (not Breaking)
    /// - CPU budget class is Easy or Medium
    /// - Taxonomy fragility is low (< 0.4)
    /// - Synthetic transparency risk is low (< 0.2)
    /// - Palette churn risk is low (< 0.3)
    /// - Changed-area ratio is sparse (< 0.4)
    fn is_early_exit_candidate(signals: &PolicySignals) -> bool {
        // LUT eligibility: must be preserving or mixed (not breaking)
        let lut_ok = matches!(
            signals.lut_eligibility,
            LutEligibility::Preserving | LutEligibility::Mixed
        );
        if !lut_ok {
            return false;
        }

        // CPU budget: must be easy or medium (not hard)
        let cpu_ok = matches!(
            signals.cpu_budget_class,
            CpuBudgetClass::Easy | CpuBudgetClass::Medium
        );
        if !cpu_ok {
            return false;
        }

        // Taxonomy fragility: must be low (< 0.4)
        if signals.taxonomy_fragility >= 0.4 {
            return false;
        }

        // Synthetic transparency risk: must be low (< 0.2)
        if signals.synthetic_transparency_risk >= 0.2 {
            return false;
        }

        // Palette churn risk: must be low (< 0.3)
        if signals.palette_switch_cost >= 0.3 {
            return false;
        }

        // Changed-area ratio: must be sparse (< 0.4)
        if signals.byte_cost >= 0.4 {
            return false;
        }

        true
    }

    /// Check if a sequence requires Tier-2 (encode-and-measure).
    ///
    /// Entry criteria:
    /// - LUT eligibility is Breaking
    /// - CPU budget class is Hard
    /// - Taxonomy fragility is high (> 0.7)
    /// - Synthetic transparency risk is high (> 0.5)
    /// - Palette churn risk is high (> 0.6)
    /// - Changed-area ratio is dense (> 0.6)
    fn is_tier2_candidate(signals: &PolicySignals) -> bool {
        // LUT eligibility: must be breaking
        let lut_breaking = matches!(signals.lut_eligibility, LutEligibility::Breaking);

        // CPU budget: must be hard
        let cpu_hard = matches!(signals.cpu_budget_class, CpuBudgetClass::Hard);

        // Taxonomy fragility: must be high (> 0.7)
        let fragile = signals.taxonomy_fragility > 0.7;

        // Synthetic transparency risk: must be high (> 0.5)
        let high_transparency_risk = signals.synthetic_transparency_risk > 0.5;

        // Palette churn risk: must be high (> 0.6)
        let high_palette_churn = signals.palette_switch_cost > 0.6;

        // Changed-area ratio: must be dense (> 0.6)
        let dense_changes = signals.byte_cost > 0.6;

        // Require at least 4 out of 6 criteria to be Tier-2
        let criteria_met = [
            lut_breaking,
            cpu_hard,
            fragile,
            high_transparency_risk,
            high_palette_churn,
            dense_changes,
        ]
        .iter()
        .filter(|&&c| c)
        .count();

        criteria_met >= 4
    }

    /// Classify from profile and score directly (convenience method).
    pub fn classify_from_profile_and_score(
        profile: &GifProfile,
        score: &ScoreBreakdown,
    ) -> Tier0Decision {
        let signals = PolicySignals::from_profile_and_score(profile, score);
        Self::classify(&signals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiler::{
        ChangeStatistics, DeltaSignal, DisposalDistribution, GifProfile, PaletteInfo, PatchDensity,
        SequenceMetrics, SequenceTaxonomy, TransparencyAnalysis,
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

    /// Create a test profile for disposal-heavy sequences.
    fn make_disposal_heavy_profile() -> GifProfile {
        GifProfile {
            metrics: SequenceMetrics {
                frame_count: 15,
                width: 1024,
                height: 768,
                total_pixels: 1024 * 768,
                avg_delay_ms: 80.0,
            },
            disposal_distribution: DisposalDistribution {
                keep_count: 2,
                none_count: 3,
                background_count: 5,
                previous_count: 5,
                dominant: "Previous".to_string(),
            },
            transparency_analysis: TransparencyAnalysis {
                frames_with_transparency: 10,
                avg_transparency_ratio: 0.2,
                max_transparency_ratio: 0.5,
                frames_with_significant_transparency: 8,
                uses_gce: true,
                frames_with_local_palette: 3,
            },
            palette_info: PaletteInfo {
                has_global_palette: true,
                local_palette_count: 3,
                palette_stability: 0.55,
            },
            change_statistics: ChangeStatistics {
                avg_changed_ratio: 0.5,
                max_changed_ratio: 0.8,
                min_changed_ratio: 0.2,
                sparse_change_frames: 5,
                dense_change_frames: 8,
            },
            patch_density: PatchDensity {
                avg_bbox_ratio: 0.6,
                max_bbox_ratio: 0.9,
                offset_patch_frames: 10,
                avg_patch_density: 0.4,
            },
            delta_signal: DeltaSignal {
                strength: 0.4,
                opaque_delta_frames: 3,
                offset_sparse_frames: 8,
                is_already_delta_encoded: false,
            },
            taxonomy: SequenceTaxonomy::DisposalHeavyBackgroundPrevious,
        }
    }

    #[test]
    fn test_opaque_delta_early_exit_structural() {
        let profile = make_opaque_delta_profile();
        let score = ScoreBreakdown {
            byte_cost: 0.15,
            visual_risk: 0.05,
            lut_cost: 0.0,
            temporal_instability: 0.02,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.08,
        };

        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        assert_eq!(decision, Tier0Decision::EarlyExitStructural);
        assert!(decision.allows_early_exit());
        assert!(!decision.requires_encode_and_measure());
    }

    #[test]
    fn test_transparency_heavy_needs_tier1_or_tier2() {
        let profile = make_transparency_heavy_profile();
        let score = ScoreBreakdown {
            byte_cost: 0.7,
            visual_risk: 0.6,
            lut_cost: 0.5,
            temporal_instability: 0.5,
            synthetic_transparency_risk: 0.6,
            palette_coherence: 0.3,
            cpu_cost: 0.3,
            total_score: 0.6,
        };

        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        // Should be Tier-2 because of high transparency risk, breaking LUT, and high byte cost
        // Criteria met: breaking LUT, high transparency risk, high palette churn, dense changes
        assert_eq!(decision, Tier0Decision::NeedsTier2);
        assert!(!decision.allows_early_exit());
        assert!(decision.requires_encode_and_measure());
    }

    #[test]
    fn test_disposal_heavy_needs_tier1() {
        let profile = make_disposal_heavy_profile();
        let score = ScoreBreakdown {
            byte_cost: 0.45,
            visual_risk: 0.3,
            lut_cost: 0.2,
            temporal_instability: 0.25,
            synthetic_transparency_risk: 0.3,
            palette_coherence: 0.1,
            cpu_cost: 0.2,
            total_score: 0.35,
        };

        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        // Should be Tier-1: mixed LUT, medium CPU, moderate fragility
        assert_eq!(decision, Tier0Decision::NeedsTier1);
        assert!(!decision.allows_early_exit());
        assert!(!decision.requires_encode_and_measure());
    }

    #[test]
    fn test_early_exit_requires_all_criteria() {
        // Start with opaque delta profile (good baseline)
        let mut profile = make_opaque_delta_profile();
        let mut score = ScoreBreakdown {
            byte_cost: 0.15,
            visual_risk: 0.05,
            lut_cost: 0.0,
            temporal_instability: 0.02,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.08,
        };

        // Baseline: should be early exit
        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        assert_eq!(decision, Tier0Decision::EarlyExitStructural);

        // Violate transparency risk criterion
        score.synthetic_transparency_risk = 0.3;
        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        assert_ne!(decision, Tier0Decision::EarlyExitStructural);

        // Reset and violate palette churn criterion
        score.synthetic_transparency_risk = 0.0;
        profile.palette_info.palette_stability = 0.6; // churn = 0.4
        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        assert_ne!(decision, Tier0Decision::EarlyExitStructural);

        // Reset and violate changed-area criterion
        profile.palette_info.palette_stability = 0.95;
        score.byte_cost = 0.5;
        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        assert_ne!(decision, Tier0Decision::EarlyExitStructural);
    }

    #[test]
    fn test_tier2_requires_multiple_criteria() {
        let profile = make_transparency_heavy_profile();
        let score = ScoreBreakdown {
            byte_cost: 0.7,
            visual_risk: 0.6,
            lut_cost: 0.5,
            temporal_instability: 0.5,
            synthetic_transparency_risk: 0.7,
            palette_coherence: 0.3,
            cpu_cost: 0.4,
            total_score: 0.6,
        };

        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        assert_eq!(decision, Tier0Decision::NeedsTier2);
    }

    #[test]
    fn test_deterministic_classification() {
        let profile = make_opaque_delta_profile();
        let score = ScoreBreakdown {
            byte_cost: 0.15,
            visual_risk: 0.05,
            lut_cost: 0.0,
            temporal_instability: 0.02,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.08,
        };

        // Same inputs should always produce same decision
        let decision1 = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        let decision2 = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        let decision3 = Tier0Classifier::classify_from_profile_and_score(&profile, &score);

        assert_eq!(decision1, decision2);
        assert_eq!(decision2, decision3);
    }

    #[test]
    fn test_decision_state_names() {
        assert_eq!(
            Tier0Decision::EarlyExitStructural.name(),
            "early-exit-structural"
        );
        assert_eq!(Tier0Decision::NeedsTier1.name(), "needs-tier1");
        assert_eq!(Tier0Decision::NeedsTier2.name(), "needs-tier2");
    }

    #[test]
    fn test_decision_state_properties() {
        assert!(Tier0Decision::EarlyExitStructural.allows_early_exit());
        assert!(!Tier0Decision::EarlyExitStructural.requires_encode_and_measure());

        assert!(!Tier0Decision::NeedsTier1.allows_early_exit());
        assert!(!Tier0Decision::NeedsTier1.requires_encode_and_measure());

        assert!(!Tier0Decision::NeedsTier2.allows_early_exit());
        assert!(Tier0Decision::NeedsTier2.requires_encode_and_measure());
    }

    #[test]
    fn test_mixed_lut_eligibility_allows_early_exit() {
        let mut profile = make_opaque_delta_profile();
        // Keep opaque delta taxonomy (fragility 0.1) but change to Mixed LUT eligibility
        // by adjusting palette stability to be lower but still good
        profile.palette_info.palette_stability = 0.75; // churn = 0.25 (< 0.3)

        let score = ScoreBreakdown {
            byte_cost: 0.15,
            visual_risk: 0.05,
            lut_cost: 0.0,
            temporal_instability: 0.02,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.08,
        };

        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        // Opaque delta with good stability should allow early exit
        // (LUT eligibility will be Preserving due to high stability)
        assert_eq!(decision, Tier0Decision::EarlyExitStructural);
    }

    #[test]
    fn test_breaking_lut_prevents_early_exit() {
        let mut profile = make_opaque_delta_profile();
        profile.taxonomy = SequenceTaxonomy::TransparencyHeavySparseDelta;

        let score = ScoreBreakdown {
            byte_cost: 0.15,
            visual_risk: 0.05,
            lut_cost: 0.5,
            temporal_instability: 0.02,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.2,
            cpu_cost: 0.1,
            total_score: 0.08,
        };

        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        // Breaking LUT should prevent early exit
        assert_ne!(decision, Tier0Decision::EarlyExitStructural);
    }

    #[test]
    fn test_mixed_lut_with_moderate_fragility_needs_tier1() {
        let mut profile = make_opaque_delta_profile();
        // Use Mixed taxonomy which has fragility 0.5 (fails early exit criterion)
        profile.taxonomy = SequenceTaxonomy::Mixed;

        let score = ScoreBreakdown {
            byte_cost: 0.15,
            visual_risk: 0.05,
            lut_cost: 0.0,
            temporal_instability: 0.02,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.1,
            total_score: 0.08,
        };

        let decision = Tier0Classifier::classify_from_profile_and_score(&profile, &score);
        // Mixed LUT with moderate fragility should go to Tier-1, not early exit
        assert_eq!(decision, Tier0Decision::NeedsTier1);
    }
}
