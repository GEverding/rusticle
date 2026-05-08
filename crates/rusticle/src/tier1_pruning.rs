//! Tier-1 proxy pruning: cheap candidate filtering before scoring.
//!
//! This module implements the second tier of the tiered optimizer: cheap proxy-based
//! candidate pruning that eliminates clearly-inferior candidates before any scoring
//! or measurement.
//!
//! # Pruning Rules
//!
//! Tier-1 applies four categories of pruning rules, all O(candidates) with no encode calls:
//!
//! 1. **LUT eligibility filter**: Prune LUT-breaking candidates when LUT-preserving alternatives exist.
//! 2. **Structural dominance pruning**: Prune candidates dominated by smaller/safer alternatives.
//! 3. **Palette proxy filter**: Prune expensive palette-switching candidates when cheaper alternatives exist.
//! 4. **Transparency risk filter**: Prune risky transparent patches in unsafe disposal contexts.
//!
//! # Invariant
//!
//! Pruning must never eliminate all candidates for a frame. If all candidates would be pruned,
//! the LUT-preserving candidate (or FullFrame as fallback) is retained.
//!
//! # Integration
//!
//! Tier-1 is called by `TieredOptimizer::optimize()` for `NeedsTier1` sequences after Tier-0
//! rejects early-exit. Pruned candidates then proceed to `Scorer::score_candidate()`.
//!
//! # Telemetry
//!
//! Pruning produces reason codes for each pruned candidate:
//! - `pruned_by_lut_policy`: LUT-breaking candidate pruned due to LUT-preserving alternative
//! - `pruned_by_structural_dominance`: Dominated by smaller/safer candidate
//! - `pruned_by_palette_churn_risk`: Expensive palette-switching candidate pruned
//! - `pruned_by_transparency_risk`: Risky transparent patch in unsafe context
//! - `pruned_by_cpu_budget`: Candidate exceeds CPU budget (reserved for future use)
//! - `retained_for_measurement`: Candidate retained for later measurement

use crate::candidate_gen::{Candidate, CandidateRepresentation};
use crate::lut_policy::{candidate_to_family, LutEligibility, PolicySignals};
use crate::types::DisposalMethod;

/// Reason code for pruning a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PruneReason {
    /// LUT-breaking candidate pruned due to LUT-preserving alternative.
    PrunedByLutPolicy,
    /// Candidate dominated by smaller/safer alternative.
    PrunedByStructuralDominance,
    /// Expensive palette-switching candidate pruned.
    PrunedByPaletteChurnRisk,
    /// Risky transparent patch in unsafe disposal context.
    PrunedByTransparencyRisk,
    /// Candidate exceeds CPU budget.
    PrunedByCpuBudget,
    /// Candidate retained for later measurement.
    RetainedForMeasurement,
}

impl PruneReason {
    /// Human-readable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::PrunedByLutPolicy => "pruned_by_lut_policy",
            Self::PrunedByStructuralDominance => "pruned_by_structural_dominance",
            Self::PrunedByPaletteChurnRisk => "pruned_by_palette_churn_risk",
            Self::PrunedByTransparencyRisk => "pruned_by_transparency_risk",
            Self::PrunedByCpuBudget => "pruned_by_cpu_budget",
            Self::RetainedForMeasurement => "retained_for_measurement",
        }
    }

    /// Whether this reason indicates the candidate was pruned.
    pub fn is_pruned(&self) -> bool {
        !matches!(self, Self::RetainedForMeasurement)
    }
}

/// Result of pruning a single candidate.
#[derive(Debug, Clone)]
pub struct PruneResult {
    /// The candidate (if retained) or None if pruned.
    pub candidate: Option<Candidate>,
    /// Reason for the decision.
    pub reason: PruneReason,
    /// Original candidate (kept for restoration if needed).
    original: Option<Candidate>,
}

/// Tier-1 pruner: applies cheap proxy-based filtering to candidate sets.
pub struct Tier1Pruner;

impl Tier1Pruner {
    /// Prune candidates for a single frame.
    ///
    /// Applies all pruning rules and returns a pruned candidate list.
    /// Guarantees at least one candidate is retained per frame.
    ///
    /// # Arguments
    ///
    /// - `candidates`: Candidates for this frame (typically 1-4 per frame)
    /// - `signals`: Policy signals for the sequence
    /// - `frame_disposal`: Disposal method of this frame
    ///
    /// # Returns
    ///
    /// Pruned candidates with reason codes.
    pub fn prune_frame(
        candidates: &[Candidate],
        signals: &PolicySignals,
        frame_disposal: DisposalMethod,
    ) -> Vec<PruneResult> {
        if candidates.is_empty() {
            return Vec::new();
        }

        // Apply pruning rules in order
        let mut results = candidates
            .iter()
            .map(|c| PruneResult {
                candidate: Some(c.clone()),
                reason: PruneReason::RetainedForMeasurement,
                original: Some(c.clone()),
            })
            .collect::<Vec<_>>();

        // Rule 1: LUT eligibility filter
        Self::apply_lut_eligibility_filter(&mut results, signals);

        // Rule 2: Structural dominance pruning
        Self::apply_structural_dominance_filter(&mut results);

        // Rule 3: Palette proxy filter
        Self::apply_palette_proxy_filter(&mut results, signals);

        // Rule 4: Transparency risk filter
        Self::apply_transparency_risk_filter(&mut results, frame_disposal);

        // Ensure at least one candidate is retained
        Self::ensure_minimum_retention(&mut results);

        results
    }

    /// Apply LUT eligibility filter.
    ///
    /// If a LUT-preserving candidate exists with reasonable bbox, prune LUT-breaking candidates.
    fn apply_lut_eligibility_filter(results: &mut [PruneResult], signals: &PolicySignals) {
        // Only apply if sequence is LUT-preserving or mixed
        if !matches!(
            signals.lut_eligibility,
            LutEligibility::Preserving | LutEligibility::Mixed
        ) {
            return;
        }

        // Find if there's a LUT-preserving candidate with bbox_ratio < 0.6
        let has_lut_preserving_small = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                let family = candidate_to_family(&c.representation);
                if family.is_lut_preserving() {
                    let bbox_ratio = c.metadata.changed_ratio;
                    return bbox_ratio < 0.6;
                }
            }
            false
        });

        // If so, prune all LUT-breaking candidates
        if has_lut_preserving_small {
            for r in results.iter_mut() {
                if r.candidate.is_some() && r.reason == PruneReason::RetainedForMeasurement {
                    if let Some(c) = &r.candidate {
                        let family = candidate_to_family(&c.representation);
                        if family.is_lut_breaking() {
                            r.candidate = None;
                            r.reason = PruneReason::PrunedByLutPolicy;
                        }
                    }
                }
            }
        }
    }

    /// Apply structural dominance pruning.
    ///
    /// Prune candidates that are dominated by smaller/safer alternatives.
    fn apply_structural_dominance_filter(results: &mut [PruneResult]) {
        // Rule 1: If ExactOpaqueBbox with bbox_ratio < 0.3 exists, prune FullFrame
        let has_small_opaque_bbox = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                if matches!(
                    c.representation,
                    CandidateRepresentation::ExactOpaqueBbox { .. }
                ) {
                    return c.metadata.changed_ratio < 0.3;
                }
            }
            false
        });

        if has_small_opaque_bbox {
            for r in results.iter_mut() {
                if r.candidate.is_some() && r.reason == PruneReason::RetainedForMeasurement {
                    if let Some(c) = &r.candidate {
                        if matches!(c.representation, CandidateRepresentation::FullFrame) {
                            r.candidate = None;
                            r.reason = PruneReason::PrunedByStructuralDominance;
                        }
                    }
                }
            }
        }

        // Rule 2: If MinimalNoOp is safe (disposal=Keep, no transparency), prune all others
        let has_safe_minimal_noop = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                if matches!(c.representation, CandidateRepresentation::MinimalNoOp) {
                    // MinimalNoOp is safe if disposal is Keep and no transparency
                    return c.metadata.disposal_method == DisposalMethod::Keep
                        && !c.metadata.source_has_transparency;
                }
            }
            false
        });

        if has_safe_minimal_noop {
            for r in results.iter_mut() {
                if r.candidate.is_some() && r.reason == PruneReason::RetainedForMeasurement {
                    if let Some(c) = &r.candidate {
                        if !matches!(c.representation, CandidateRepresentation::MinimalNoOp) {
                            r.candidate = None;
                            r.reason = PruneReason::PrunedByStructuralDominance;
                        }
                    }
                }
            }
        }

        // Rule 3: If TransparentSparsePatch is risky and a non-risky alternative exists
        // with bbox_ratio < 2x, prune the risky one
        let risky_sparse_bbox_ratio = results.iter().find_map(|r| {
            if let Some(c) = &r.candidate {
                if let CandidateRepresentation::TransparentSparsePatch { is_risky: true, .. } =
                    c.representation
                {
                    return Some(c.metadata.changed_ratio);
                }
            }
            None
        });

        if let Some(risky_ratio) = risky_sparse_bbox_ratio {
            let has_safer_alternative = results.iter().any(|r| {
                if let Some(c) = &r.candidate {
                    if let CandidateRepresentation::TransparentSparsePatch {
                        is_risky: false, ..
                    } = c.representation
                    {
                        return c.metadata.changed_ratio < risky_ratio * 2.0;
                    }
                    // Also consider non-sparse alternatives
                    if !matches!(
                        c.representation,
                        CandidateRepresentation::TransparentSparsePatch { .. }
                    ) {
                        return c.metadata.changed_ratio < risky_ratio * 2.0;
                    }
                }
                false
            });

            if has_safer_alternative {
                for r in results.iter_mut() {
                    if r.candidate.is_some() && r.reason == PruneReason::RetainedForMeasurement {
                        if let Some(c) = &r.candidate {
                            if let CandidateRepresentation::TransparentSparsePatch {
                                is_risky: true,
                                ..
                            } = c.representation
                            {
                                r.candidate = None;
                                r.reason = PruneReason::PrunedByStructuralDominance;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Apply palette proxy filter.
    ///
    /// Prune expensive palette-switching candidates when cheaper alternatives exist.
    fn apply_palette_proxy_filter(results: &mut [PruneResult], signals: &PolicySignals) {
        // Only apply if palette switch cost is high
        if signals.palette_switch_cost <= 0.5 {
            return;
        }

        // Find if there's a candidate with palette_switch_cost == 0.0 (or very low)
        let has_cheap_palette = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                // Heuristic: LUT-preserving candidates have low palette cost
                let family = candidate_to_family(&c.representation);
                return family.is_lut_preserving();
            }
            false
        });

        if has_cheap_palette {
            // Find the bbox ratio of the cheapest candidate
            let cheap_bbox_ratio = results
                .iter()
                .filter_map(|r| {
                    if let Some(c) = &r.candidate {
                        let family = candidate_to_family(&c.representation);
                        if family.is_lut_preserving() {
                            return Some(c.metadata.changed_ratio);
                        }
                    }
                    None
                })
                .fold(f32::INFINITY, f32::min);

            // Prune expensive palette-switching candidates with bbox > 1.5x the cheap one
            for r in results.iter_mut() {
                if r.candidate.is_some() && r.reason == PruneReason::RetainedForMeasurement {
                    if let Some(c) = &r.candidate {
                        let family = candidate_to_family(&c.representation);
                        if family.is_lut_breaking()
                            && c.metadata.changed_ratio > cheap_bbox_ratio * 1.5
                        {
                            r.candidate = None;
                            r.reason = PruneReason::PrunedByPaletteChurnRisk;
                        }
                    }
                }
            }
        }
    }

    /// Apply transparency risk filter.
    ///
    /// Prune risky transparent patches in unsafe disposal contexts.
    fn apply_transparency_risk_filter(results: &mut [PruneResult], frame_disposal: DisposalMethod) {
        // Only apply if disposal is Background or Previous (unsafe for transparency)
        if !matches!(
            frame_disposal,
            DisposalMethod::Background | DisposalMethod::Previous
        ) {
            return;
        }

        // Prune all TransparentSparsePatch candidates
        for r in results.iter_mut() {
            if r.candidate.is_some() && r.reason == PruneReason::RetainedForMeasurement {
                if let Some(c) = &r.candidate {
                    if matches!(
                        c.representation,
                        CandidateRepresentation::TransparentSparsePatch { .. }
                    ) {
                        r.candidate = None;
                        r.reason = PruneReason::PrunedByTransparencyRisk;
                    }
                }
            }
        }
    }

    /// Ensure at least one candidate is retained per frame.
    ///
    /// If all candidates were pruned, restore the LUT-preserving candidate or FullFrame.
    fn ensure_minimum_retention(results: &mut [PruneResult]) {
        // Check if all candidates were pruned
        if results.iter().all(|r| r.candidate.is_none()) {
            // Find the best candidate to restore: prefer LUT-preserving, then FullFrame
            let best_idx = results
                .iter()
                .enumerate()
                .find_map(|(idx, r)| {
                    if let Some(orig) = &r.original {
                        let family = candidate_to_family(&orig.representation);
                        if family.is_lut_preserving() {
                            return Some(idx);
                        }
                    }
                    None
                })
                .or_else(|| {
                    // Fallback: find FullFrame
                    results.iter().enumerate().find_map(|(idx, r)| {
                        if let Some(orig) = &r.original {
                            if matches!(orig.representation, CandidateRepresentation::FullFrame) {
                                return Some(idx);
                            }
                        }
                        None
                    })
                })
                .unwrap_or(0);

            // Restore the best candidate
            if best_idx < results.len() {
                results[best_idx].candidate = results[best_idx].original.clone();
                results[best_idx].reason = PruneReason::RetainedForMeasurement;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_ir::BoundingBox;
    use crate::candidate_gen::{CandidateMetadata, SafetyReason};
    use crate::lut_policy::CpuBudgetClass;

    fn make_test_candidate(
        frame_index: usize,
        representation: CandidateRepresentation,
        changed_ratio: f32,
        disposal: DisposalMethod,
    ) -> Candidate {
        Candidate {
            frame_index,
            representation,
            metadata: CandidateMetadata {
                changed_bbox: BoundingBox::new(0, 0, 100, 100),
                changed_pixel_count: 1000,
                changed_ratio,
                source_is_full_canvas: false,
                source_has_transparency: false,
                source_transparent_count: 0,
                source_opaque_count: 1000,
                disposal_method: disposal,
                safety_reason: SafetyReason::AlwaysSafe,
            },
        }
    }

    fn make_test_signals() -> PolicySignals {
        PolicySignals {
            lut_eligibility: LutEligibility::Preserving,
            cpu_budget_class: CpuBudgetClass::Medium,
            lut_preserving: true,
            quantization_cost: crate::lut_policy::QuantizationCostClass::None,
            palette_switch_cost: 0.2,
            synthetic_transparency_risk: 0.1,
            candidate_cpu_cost: 1.0,
            taxonomy_fragility: 0.2,
            byte_cost: 0.2,
            visual_risk: 0.1,
            temporal_instability: 0.1,
        }
    }

    #[test]
    fn test_opaque_delta_prunes_to_small_set() {
        // Voyager-like: opaque deltas with 4 candidate types
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                },
                0.15,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::TransparentSparsePatch {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                    is_risky: false,
                },
                0.15,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::MinimalNoOp,
                0.0,
                DisposalMethod::Keep,
            ),
        ];

        let signals = make_test_signals();
        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        // Should prune FullFrame (dominated by small opaque bbox)
        let pruned_count = results.iter().filter(|r| r.candidate.is_none()).count();
        assert!(pruned_count >= 1, "Expected at least 1 pruned candidate");

        // Should retain at least 1 candidate
        let retained_count = results.iter().filter(|r| r.candidate.is_some()).count();
        assert!(
            retained_count >= 1,
            "Expected at least 1 retained candidate"
        );
    }

    #[test]
    fn test_transparency_heavy_retains_sparse() {
        // Transparency-heavy: should retain sparse patches when safe
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::TransparentSparsePatch {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                    is_risky: false,
                },
                0.15,
                DisposalMethod::Keep,
            ),
        ];

        let signals = PolicySignals {
            lut_eligibility: LutEligibility::Breaking,
            cpu_budget_class: CpuBudgetClass::Medium,
            lut_preserving: false,
            quantization_cost: crate::lut_policy::QuantizationCostClass::Moderate,
            palette_switch_cost: 0.6,
            synthetic_transparency_risk: 0.6,
            candidate_cpu_cost: 2.0,
            taxonomy_fragility: 0.7,
            byte_cost: 0.4,
            visual_risk: 0.3,
            temporal_instability: 0.3,
        };

        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        // Should retain sparse patch (not risky in Keep disposal)
        let sparse_retained = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                matches!(
                    c.representation,
                    CandidateRepresentation::TransparentSparsePatch { .. }
                )
            } else {
                false
            }
        });
        assert!(sparse_retained, "Expected sparse patch to be retained");
    }

    #[test]
    fn test_disposal_background_prunes_sparse() {
        // Background disposal: should prune transparent sparse patches
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Background,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::TransparentSparsePatch {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                    is_risky: false,
                },
                0.15,
                DisposalMethod::Background,
            ),
        ];

        let signals = make_test_signals();
        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Background);

        // Should prune sparse patch (risky in Background disposal)
        let sparse_pruned = results.iter().any(|r| {
            r.candidate.is_none() && matches!(r.reason, PruneReason::PrunedByTransparencyRisk)
        });
        assert!(
            sparse_pruned,
            "Expected sparse patch to be pruned in Background disposal"
        );

        // Should retain at least FullFrame
        let full_frame_retained = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                matches!(c.representation, CandidateRepresentation::FullFrame)
            } else {
                false
            }
        });
        assert!(full_frame_retained, "Expected FullFrame to be retained");
    }

    #[test]
    fn test_never_prunes_to_empty() {
        // Property test: never prune all candidates
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                },
                0.15,
                DisposalMethod::Keep,
            ),
        ];

        let signals = make_test_signals();
        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        let retained_count = results.iter().filter(|r| r.candidate.is_some()).count();
        assert!(
            retained_count >= 1,
            "Expected at least 1 candidate retained"
        );
    }

    #[test]
    fn test_pruning_is_deterministic() {
        // Pruning should be deterministic: same input → same output
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                },
                0.15,
                DisposalMethod::Keep,
            ),
        ];

        let signals = make_test_signals();

        let results1 = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);
        let results2 = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        assert_eq!(results1.len(), results2.len());
        for (r1, r2) in results1.iter().zip(results2.iter()) {
            assert_eq!(r1.reason, r2.reason);
            assert_eq!(
                r1.candidate.as_ref().map(|c| c.frame_index),
                r2.candidate.as_ref().map(|c| c.frame_index)
            );
            assert_eq!(r1.reason, r2.reason);
        }
    }

    #[test]
    fn test_restores_candidate_when_all_pruned() {
        // If all candidates would be pruned, restore the best one
        // This is a synthetic test case where all pruning rules would fire
        let candidates = vec![make_test_candidate(
            0,
            CandidateRepresentation::FullFrame,
            1.0,
            DisposalMethod::Keep,
        )];

        let signals = make_test_signals();
        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        // Should have at least one retained candidate
        let retained = results.iter().filter(|r| r.candidate.is_some()).count();
        assert!(retained >= 1, "Expected at least 1 candidate retained");
    }

    #[test]
    fn test_lut_preserving_dominates_breaking() {
        // LUT-preserving candidate should dominate LUT-breaking when both exist
        // This test verifies that FullFrame is pruned by either LUT eligibility or structural dominance
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                },
                0.15,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Keep,
            ),
        ];

        let signals = PolicySignals {
            lut_eligibility: LutEligibility::Preserving,
            cpu_budget_class: CpuBudgetClass::Medium,
            lut_preserving: true,
            quantization_cost: crate::lut_policy::QuantizationCostClass::None,
            palette_switch_cost: 0.1,
            synthetic_transparency_risk: 0.05,
            candidate_cpu_cost: 0.5,
            taxonomy_fragility: 0.1,
            byte_cost: 0.15,
            visual_risk: 0.05,
            temporal_instability: 0.05,
        };

        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        // FullFrame should be pruned (by LUT eligibility filter since it's LUT-breaking)
        let full_frame_pruned = results.iter().any(|r| {
            r.candidate.is_none()
                && matches!(
                    r.reason,
                    PruneReason::PrunedByLutPolicy | PruneReason::PrunedByStructuralDominance
                )
        });
        assert!(full_frame_pruned, "Expected FullFrame to be pruned");

        // OpaqueBbox should be retained
        let opaque_retained = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                matches!(
                    c.representation,
                    CandidateRepresentation::ExactOpaqueBbox { .. }
                )
            } else {
                false
            }
        });
        assert!(opaque_retained, "Expected OpaqueBbox to be retained");
    }

    #[test]
    fn test_minimal_noop_dominates_others() {
        // MinimalNoOp with Keep disposal should dominate all other candidates
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::MinimalNoOp,
                0.0,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                },
                0.15,
                DisposalMethod::Keep,
            ),
        ];

        let signals = make_test_signals();
        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        // Only MinimalNoOp should be retained
        let retained_count = results.iter().filter(|r| r.candidate.is_some()).count();
        assert_eq!(retained_count, 1, "Expected only 1 candidate retained");

        let minimal_retained = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                matches!(c.representation, CandidateRepresentation::MinimalNoOp)
            } else {
                false
            }
        });
        assert!(minimal_retained, "Expected MinimalNoOp to be retained");
    }

    #[test]
    fn test_disposal_previous_prunes_sparse() {
        // Previous disposal: should also prune transparent sparse patches
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Previous,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::TransparentSparsePatch {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                    is_risky: false,
                },
                0.15,
                DisposalMethod::Previous,
            ),
        ];

        let signals = make_test_signals();
        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Previous);

        // Sparse patch should be pruned
        let sparse_pruned = results.iter().any(|r| {
            r.candidate.is_none() && matches!(r.reason, PruneReason::PrunedByTransparencyRisk)
        });
        assert!(
            sparse_pruned,
            "Expected sparse patch to be pruned in Previous disposal"
        );
    }

    #[test]
    fn test_palette_churn_filter() {
        // High palette churn should prune expensive palette-switching candidates
        // The palette proxy filter prunes LUT-breaking candidates when LUT-preserving alternatives exist
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                },
                0.15,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Keep,
            ),
        ];

        let signals = PolicySignals {
            lut_eligibility: LutEligibility::Mixed,
            cpu_budget_class: CpuBudgetClass::Medium,
            lut_preserving: true,
            quantization_cost: crate::lut_policy::QuantizationCostClass::Moderate,
            palette_switch_cost: 0.7, // High palette churn
            synthetic_transparency_risk: 0.2,
            candidate_cpu_cost: 1.5,
            taxonomy_fragility: 0.3,
            byte_cost: 0.3,
            visual_risk: 0.2,
            temporal_instability: 0.2,
        };

        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        // FullFrame should be pruned (by LUT eligibility filter since it's LUT-breaking)
        let full_frame_pruned = results.iter().any(|r| {
            r.candidate.is_none()
                && matches!(
                    r.reason,
                    PruneReason::PrunedByPaletteChurnRisk
                        | PruneReason::PrunedByStructuralDominance
                        | PruneReason::PrunedByLutPolicy
                )
        });
        assert!(full_frame_pruned, "Expected FullFrame to be pruned");
    }

    #[test]
    fn test_risky_sparse_pruned_when_safer_exists() {
        // Risky sparse patch should be pruned when a safer alternative exists
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::TransparentSparsePatch {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                    is_risky: true,
                },
                0.3,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::TransparentSparsePatch {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                    is_risky: false,
                },
                0.15,
                DisposalMethod::Keep,
            ),
        ];

        let signals = make_test_signals();
        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        // Risky sparse should be pruned
        let risky_pruned = results.iter().any(|r| {
            r.candidate.is_none() && matches!(r.reason, PruneReason::PrunedByStructuralDominance)
        });
        assert!(risky_pruned, "Expected risky sparse patch to be pruned");

        // Non-risky sparse should be retained
        let safe_sparse_retained = results.iter().any(|r| {
            if let Some(c) = &r.candidate {
                if let CandidateRepresentation::TransparentSparsePatch {
                    is_risky: false, ..
                } = c.representation
                {
                    return true;
                }
            }
            false
        });
        assert!(
            safe_sparse_retained,
            "Expected safe sparse patch to be retained"
        );
    }

    #[test]
    fn test_breaking_lut_pruned_when_preserving_exists() {
        // LUT-breaking candidates should be pruned when LUT-preserving alternatives exist
        let candidates = vec![
            make_test_candidate(
                0,
                CandidateRepresentation::ExactOpaqueBbox {
                    bbox: BoundingBox::new(10, 10, 110, 110),
                },
                0.2,
                DisposalMethod::Keep,
            ),
            make_test_candidate(
                0,
                CandidateRepresentation::FullFrame,
                1.0,
                DisposalMethod::Keep,
            ),
        ];

        let signals = PolicySignals {
            lut_eligibility: LutEligibility::Preserving,
            cpu_budget_class: CpuBudgetClass::Medium,
            lut_preserving: true,
            quantization_cost: crate::lut_policy::QuantizationCostClass::None,
            palette_switch_cost: 0.1,
            synthetic_transparency_risk: 0.05,
            candidate_cpu_cost: 0.5,
            taxonomy_fragility: 0.1,
            byte_cost: 0.2,
            visual_risk: 0.05,
            temporal_instability: 0.05,
        };

        let results = Tier1Pruner::prune_frame(&candidates, &signals, DisposalMethod::Keep);

        // FullFrame (LUT-breaking) should be pruned by LUT eligibility filter
        let full_frame_pruned = results.iter().any(|r| {
            r.candidate.is_none()
                && matches!(
                    r.reason,
                    PruneReason::PrunedByLutPolicy | PruneReason::PrunedByStructuralDominance
                )
        });
        assert!(full_frame_pruned, "Expected FullFrame to be pruned");
    }
}
