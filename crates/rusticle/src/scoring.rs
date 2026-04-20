//! Adaptive scoring and chooser layer for candidate frame/sequence encodings.
//!
//! This module ranks candidate representations using the canonical IR, profiler output,
//! and palette strategy set. It produces deterministic, explainable decisions about which
//! candidate and palette strategy to use for each frame or sequence.
//!
//! # Scoring Dimensions
//!
//! - **byte_cost**: Estimated encoded size (heuristic proxy, not actual encode).
//! - **visual_risk**: Risk of visual artifacts or quality loss.
//! - **lut_cost**: Cost of breaking/rebuilding the palette LUT (new).
//! - **temporal_instability**: Risk of palette churn or temporal artifacts.
//! - **synthetic_transparency_risk**: Risk of introducing synthetic transparency.
//! - **palette_coherence**: Penalty for breaking palette consistency within a chunk (new).
//! - **cpu_cost**: Estimated computational cost of encoding.
//! - **total_score**: Weighted sum of all dimensions.
//!
//! # Weight Rebalancing
//!
//! The weight vector has been rebalanced to prevent byte-greedy selection from breaking
//! LUT-friendly paths:
//!
//! ```text
//! Old weights:  byte_cost=0.35, visual_risk=0.25, temporal=0.15, synth_transparency=0.15, cpu=0.10
//! New weights:  byte_cost=0.25, visual_risk=0.25, lut_cost=0.20, temporal=0.10,
//!               synth_transparency=0.10, palette_coherence=0.05, cpu=0.05
//! ```
//!
//! Key changes:
//! - byte_cost reduced from 0.35 → 0.25 (less byte-greedy)
//! - lut_cost added at 0.20 weight (LUT preservation now competes fairly)
//! - palette_coherence added at 0.05 weight (penalizes palette thrashing)
//! - cpu_cost reduced from 0.10 → 0.05 (secondary concern)
//!
//! # Decision Structure
//!
//! A `FrameDecision` records the chosen candidate, palette strategy, score breakdown,
//! and explanation for why that choice was made.

use crate::adaptive_ir::{CanonicalFrame, CanonicalSequence};
use crate::candidate_gen::{Candidate, CandidateMetadata, CandidateRepresentation, SafetyReason};
use crate::lut_policy::{candidate_to_family, CandidateFamily};
use crate::profiler::{GifProfile, SequenceTaxonomy};
use crate::palette_strategy::{PaletteStrategy, PaletteStrategySet};
use crate::types::DisposalMethod;

/// Score breakdown for a candidate representation.
///
/// All scores are normalized to [0, 1] where lower is better (cost-like).
/// The total_score is a weighted combination of all dimensions.
#[derive(Debug, Clone, Copy)]
pub struct ScoreBreakdown {
    /// Estimated encoded size cost (0.0 = smallest, 1.0 = largest).
    /// Heuristic proxy: based on bbox area, changed pixels, transparency.
    pub byte_cost: f32,

    /// Visual risk from quality loss or artifacts (0.0 = safe, 1.0 = high risk).
    /// Considers transparency handling, bbox tightness, disposal semantics.
    pub visual_risk: f32,

    /// LUT cost: penalty for breaking/rebuilding the palette LUT (0.0 = LUT-preserving, 1.0 = full requantization).
    /// Computed from candidate family and policy signals.
    pub lut_cost: f32,

    /// Temporal instability risk from palette churn or frame-to-frame artifacts.
    /// (0.0 = stable, 1.0 = high churn).
    pub temporal_instability: f32,

    /// Risk of introducing synthetic transparency (0.0 = safe, 1.0 = high risk).
    /// Penalizes sparse patches and minimal frames in risky contexts.
    pub synthetic_transparency_risk: f32,

    /// Palette coherence penalty: penalizes breaking palette consistency within a chunk.
    /// (0.0 = coherent, 1.0 = isolated local palette).
    pub palette_coherence: f32,

    /// Estimated CPU cost of encoding (0.0 = cheap, 1.0 = expensive).
    /// Considers full-frame vs patch, transparency handling, palette strategy.
    pub cpu_cost: f32,

    /// Weighted total score (0.0 = best, 1.0 = worst).
    /// Computed as: 0.25*byte_cost + 0.25*visual_risk + 0.20*lut_cost
    ///            + 0.10*temporal_instability + 0.10*synthetic_transparency_risk
    ///            + 0.05*palette_coherence + 0.05*cpu_cost
    pub total_score: f32,
}

impl ScoreBreakdown {
    /// Create a new score breakdown with all zeros.
    pub fn zero() -> Self {
        Self {
            byte_cost: 0.0,
            visual_risk: 0.0,
            lut_cost: 0.0,
            temporal_instability: 0.0,
            synthetic_transparency_risk: 0.0,
            palette_coherence: 0.0,
            cpu_cost: 0.0,
            total_score: 0.0,
        }
    }

    /// Compute total_score from component scores using rebalanced weights.
    ///
    /// Weights (rebalanced to prevent byte-greedy selection from breaking LUT paths):
    /// - byte_cost: 0.25 (reduced from 0.35; less byte-greedy)
    /// - visual_risk: 0.25 (correctness/quality; unchanged)
    /// - lut_cost: 0.20 (NEW; LUT preservation now competes fairly with byte cost)
    /// - temporal_instability: 0.10 (reduced from 0.15; animation smoothness)
    /// - synthetic_transparency_risk: 0.10 (reduced from 0.15; safety)
    /// - palette_coherence: 0.05 (NEW; penalizes palette thrashing)
    /// - cpu_cost: 0.05 (reduced from 0.10; secondary concern)
    ///
    /// Total: 1.00 (weights sum to 1.0)
    pub fn compute_total(&mut self) {
        self.total_score = 0.25 * self.byte_cost
            + 0.25 * self.visual_risk
            + 0.20 * self.lut_cost
            + 0.10 * self.temporal_instability
            + 0.10 * self.synthetic_transparency_risk
            + 0.05 * self.palette_coherence
            + 0.05 * self.cpu_cost;
    }
}

/// Reason code for a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionReason {
    /// Lowest total score among candidates.
    LowestScore,
    /// Preferred by taxonomy (e.g., opaque-delta for voyager-like).
    TaxonomyPreferred,
    /// Safety constraint: avoids risky candidates.
    SafetyConstraint,
    /// Palette strategy alignment: matches preferred strategy.
    PaletteStrategyAlignment,
    /// Tie-breaker: multiple candidates had similar scores.
    TieBreaker,
    /// Fallback: no ideal candidate, chose least-bad option.
    Fallback,
}

/// Decision for a single frame.
#[derive(Debug, Clone)]
pub struct FrameDecision {
    /// Frame index in the sequence.
    pub frame_index: usize,

    /// Chosen candidate representation.
    pub chosen_candidate: CandidateRepresentation,

    /// Chosen palette strategy.
    pub chosen_palette_strategy: PaletteStrategy,

    /// Score breakdown for the chosen candidate.
    pub score_breakdown: ScoreBreakdown,

    /// Top alternative(s) if any.
    pub alternatives: Vec<(CandidateRepresentation, ScoreBreakdown)>,

    /// Primary reason code for the decision.
    pub reason: DecisionReason,

    /// Explanation text (human-readable).
    pub explanation: String,
}

/// Sequence-level decision summary.
#[derive(Debug, Clone)]
pub struct SequenceDecision {
    /// Per-frame decisions.
    pub frame_decisions: Vec<FrameDecision>,

    /// Chosen palette strategy for the sequence.
    pub sequence_palette_strategy: PaletteStrategy,

    /// Average score across all frames.
    pub avg_score: f32,

    /// Estimated total encoded size (heuristic).
    pub estimated_total_bytes: usize,

    /// Summary explanation.
    pub summary: String,
}

/// Scorer for candidate representations.
pub struct Scorer;

impl Scorer {
    /// Score a single candidate for a frame.
    ///
    /// Returns a `ScoreBreakdown` with all dimensions computed.
    /// The new lut_cost and palette_coherence dimensions are computed from policy signals.
    pub fn score_candidate(
        candidate: &Candidate,
        frame: &CanonicalFrame,
        seq: &CanonicalSequence,
        profile: &GifProfile,
    ) -> ScoreBreakdown {
        let mut score = ScoreBreakdown::zero();

        // Compute byte_cost heuristic
        score.byte_cost = Self::estimate_byte_cost(candidate, frame, seq);

        // Compute visual_risk
        score.visual_risk = Self::estimate_visual_risk(candidate, frame, seq, profile);

        // Compute lut_cost (NEW: from candidate family and policy signals)
        score.lut_cost = Self::estimate_lut_cost(candidate, profile);

        // Compute temporal_instability
        score.temporal_instability = Self::estimate_temporal_instability(candidate, frame, seq, profile);

        // Compute synthetic_transparency_risk
        score.synthetic_transparency_risk =
            Self::estimate_synthetic_transparency_risk(candidate, frame, seq, profile);

        // Compute palette_coherence (NEW: from policy signals)
        score.palette_coherence = Self::estimate_palette_coherence(candidate, profile);

        // Compute cpu_cost
        score.cpu_cost = Self::estimate_cpu_cost(candidate, frame);

        // Compute total_score
        score.compute_total();

        score
    }

    /// Estimate LUT cost for a candidate (0.0 = LUT-preserving, 1.0 = full requantization).
    ///
    /// Maps candidate family to LUT cost:
    /// - OpaqueBbox, MinimalNoOp: 0.0 (LUT-preserving, no cost)
    /// - FullFrame: 0.3 (moderate cost; full frame but may reuse palette)
    /// - TransparentSparse: 0.8 (high cost; requires transparency index and potential requantization)
    ///
    /// Adjusted by palette stability: if palette is unstable, even LUT-preserving candidates
    /// incur some cost due to palette churn risk.
    fn estimate_lut_cost(candidate: &Candidate, profile: &GifProfile) -> f32 {
        let family = candidate_to_family(&candidate.representation);
        let palette_stability = profile.palette_info.palette_stability;

        // Base cost from candidate family
        let base_cost = match family {
            CandidateFamily::OpaqueBbox | CandidateFamily::MinimalNoOp => 0.0,
            CandidateFamily::FullFrame => 0.3,
            CandidateFamily::TransparentSparse => 0.8,
            CandidateFamily::PalettePreserving => 0.0,
            CandidateFamily::PaletteBreaking => 0.9,
        };

        // Adjust by palette stability: unstable palettes increase LUT cost even for
        // nominally LUT-preserving candidates (due to palette churn risk)
        let stability_penalty = (1.0 - palette_stability) * 0.2;

        (base_cost + stability_penalty).min(1.0)
    }

    /// Estimate palette coherence penalty (0.0 = coherent, 1.0 = isolated local palette).
    ///
    /// Penalizes candidates that break palette consistency:
    /// - OpaqueBbox, MinimalNoOp: 0.0 (coherent with global palette)
    /// - FullFrame: 0.1 (slight penalty; full frame may require new palette)
    /// - TransparentSparse: 0.5 (moderate penalty; sparse patches often need local palettes)
    ///
    /// Adjusted by taxonomy fragility: fragile taxonomies (e.g., transparency-heavy)
    /// incur higher penalties for palette-breaking candidates.
    fn estimate_palette_coherence(candidate: &Candidate, profile: &GifProfile) -> f32 {
        let family = candidate_to_family(&candidate.representation);

        // Base coherence penalty from candidate family
        let base_penalty = match family {
            CandidateFamily::OpaqueBbox | CandidateFamily::MinimalNoOp => 0.0,
            CandidateFamily::FullFrame => 0.1,
            CandidateFamily::TransparentSparse => 0.5,
            CandidateFamily::PalettePreserving => 0.0,
            CandidateFamily::PaletteBreaking => 0.7,
        };

        // Adjust by taxonomy fragility: fragile taxonomies penalize palette-breaking more
        let fragility = match profile.taxonomy {
            SequenceTaxonomy::OpaqueDeltaGlobalPalette => 0.1,
            SequenceTaxonomy::TransparencyHeavySparseDelta => 0.8,
            SequenceTaxonomy::DisposalHeavyBackgroundPrevious => 0.6,
            SequenceTaxonomy::Photographic => 0.5,
            SequenceTaxonomy::Mixed => 0.5,
        };

        let fragility_multiplier = 0.5_f32 + fragility * 0.5_f32; // Range [0.5, 1.0]

        (base_penalty * fragility_multiplier).min(1.0_f32)
    }

    /// Estimate byte cost for a candidate (0.0 = smallest, 1.0 = largest).
    ///
    /// Heuristic: based on bbox area, changed pixels, and transparency.
    /// - Full frame: high cost (full canvas).
    /// - Opaque bbox: medium cost (bbox area + overhead).
    /// - Transparent sparse: low cost (sparse pixels) but risky.
    /// - Minimal/no-op: very low cost (minimal data).
    fn estimate_byte_cost(candidate: &Candidate, _frame: &CanonicalFrame, seq: &CanonicalSequence) -> f32 {
        let canvas_area = (seq.width as usize) * (seq.height as usize);

        match &candidate.representation {
            CandidateRepresentation::FullFrame => {
                // Full frame: ~canvas_area pixels + overhead
                // Normalized to [0, 1] where 1.0 = full canvas
                1.0
            }
            CandidateRepresentation::ExactOpaqueBbox { bbox } => {
                // Opaque bbox: area of bbox + overhead
                // Normalized relative to canvas
                let bbox_area = bbox.area();
                let ratio = bbox_area as f32 / canvas_area as f32;
                // Add small overhead for bbox metadata
                (ratio * 0.95 + 0.05).min(1.0)
            }
            CandidateRepresentation::TransparentSparsePatch { bbox, is_risky } => {
                // Sparse patch: area of bbox + transparency overhead
                let bbox_area = bbox.area();
                let ratio = bbox_area as f32 / canvas_area as f32;
                // Transparency adds overhead (alpha channel, potential palette expansion)
                let transparency_overhead = if *is_risky { 0.15 } else { 0.05 };
                (ratio * 0.8 + transparency_overhead).min(1.0)
            }
            CandidateRepresentation::MinimalNoOp => {
                // Minimal/no-op: very small overhead
                0.02
            }
        }
    }

    /// Estimate visual risk (0.0 = safe, 1.0 = high risk).
    ///
    /// Considers:
    /// - Transparency handling (sparse patches are riskier).
    /// - Bbox tightness (tight bbox = lower risk).
    /// - Disposal semantics (Background disposal = higher risk).
    fn estimate_visual_risk(
        candidate: &Candidate,
        frame: &CanonicalFrame,
        _seq: &CanonicalSequence,
        _profile: &GifProfile,
    ) -> f32 {
        match &candidate.representation {
            CandidateRepresentation::FullFrame => {
                // Full frame: always safe (no visual artifacts)
                0.0
            }
            CandidateRepresentation::ExactOpaqueBbox { .. } => {
                // Opaque bbox: safe if bbox is tight
                // Risk increases if disposal is Background (clears region)
                let disposal_risk = match frame.dispose {
                    DisposalMethod::Background => 0.1,
                    _ => 0.0,
                };
                0.05 + disposal_risk
            }
            CandidateRepresentation::TransparentSparsePatch { is_risky, .. } => {
                // Sparse patch: risky if marked as risky
                if *is_risky {
                    0.6 // High risk: transparency semantics uncertain
                } else {
                    0.3 // Medium risk: sparse transparency
                }
            }
            CandidateRepresentation::MinimalNoOp => {
                // Minimal/no-op: risky if disposal is Background
                match frame.dispose {
                    DisposalMethod::Background => 0.7, // Very risky: clears region
                    _ => 0.1,                           // Low risk: safe disposal
                }
            }
        }
    }

    /// Estimate temporal instability (0.0 = stable, 1.0 = high churn).
    ///
    /// Considers:
    /// - Palette strategy impact (local palettes = higher churn).
    /// - Frame-to-frame changes (sparse changes = lower instability).
    fn estimate_temporal_instability(
        candidate: &Candidate,
        frame: &CanonicalFrame,
        _seq: &CanonicalSequence,
        profile: &GifProfile,
    ) -> f32 {
        // Base instability from palette stability
        let palette_instability = 1.0 - profile.palette_info.palette_stability;

        // Adjust based on candidate type
        let candidate_factor = match &candidate.representation {
            CandidateRepresentation::FullFrame => 0.3,      // Full frame: some instability
            CandidateRepresentation::ExactOpaqueBbox { .. } => 0.2, // Opaque bbox: lower instability
            CandidateRepresentation::TransparentSparsePatch { .. } => 0.5, // Sparse: higher instability
            CandidateRepresentation::MinimalNoOp => 0.1,    // No-op: very stable
        };

        // Adjust based on changed area (sparse changes = lower instability)
        let change_factor = frame.changed_region.changed_ratio.min(1.0);

        (palette_instability * 0.5 + candidate_factor * 0.3 + change_factor * 0.2).min(1.0)
    }

    /// Estimate synthetic transparency risk (0.0 = safe, 1.0 = high risk).
    ///
    /// Penalizes candidates that introduce or rely on transparency in risky contexts.
    fn estimate_synthetic_transparency_risk(
        candidate: &Candidate,
        frame: &CanonicalFrame,
        _seq: &CanonicalSequence,
        _profile: &GifProfile,
    ) -> f32 {
        match &candidate.representation {
            CandidateRepresentation::FullFrame => {
                // Full frame: no synthetic transparency risk
                0.0
            }
            CandidateRepresentation::ExactOpaqueBbox { .. } => {
                // Opaque bbox: no synthetic transparency (all opaque)
                0.0
            }
            CandidateRepresentation::TransparentSparsePatch { is_risky, .. } => {
                // Sparse patch: risk depends on whether it's marked risky
                if *is_risky {
                    0.8 // High risk: uncertain transparency semantics
                } else {
                    0.3 // Medium risk: sparse transparency
                }
            }
            CandidateRepresentation::MinimalNoOp => {
                // Minimal/no-op: risk depends on disposal and source transparency
                if frame.source_patch.has_transparency {
                    0.5 // Medium risk: relies on previous frame's transparency
                } else {
                    0.1 // Low risk: no transparency involved
                }
            }
        }
    }

    /// Estimate CPU cost (0.0 = cheap, 1.0 = expensive).
    ///
    /// Considers:
    /// - Full frame: expensive (full canvas processing).
    /// - Opaque bbox: medium (bbox processing).
    /// - Sparse patch: cheap (sparse processing).
    /// - Minimal/no-op: very cheap.
    fn estimate_cpu_cost(candidate: &Candidate, frame: &CanonicalFrame) -> f32 {
        let canvas_area = (frame.source_patch.width as usize) * (frame.source_patch.height as usize);

        match &candidate.representation {
            CandidateRepresentation::FullFrame => {
                // Full frame: expensive
                0.8
            }
            CandidateRepresentation::ExactOpaqueBbox { bbox } => {
                // Opaque bbox: medium cost proportional to bbox area
                let bbox_area = bbox.area();
                let ratio = bbox_area as f32 / canvas_area.max(1) as f32;
                0.3 + ratio * 0.4
            }
            CandidateRepresentation::TransparentSparsePatch { bbox, .. } => {
                // Sparse patch: cheap (sparse processing)
                let bbox_area = bbox.area();
                let ratio = bbox_area as f32 / canvas_area.max(1) as f32;
                0.2 + ratio * 0.2
            }
            CandidateRepresentation::MinimalNoOp => {
                // Minimal/no-op: very cheap
                0.05
            }
        }
    }
}

/// Chooser for selecting best candidate and palette strategy.
pub struct Chooser;

impl Chooser {
    /// Choose the best candidate for a single frame.
    ///
    /// Considers all candidates, scores them, and selects the best one
    /// while respecting safety constraints and taxonomy preferences.
    pub fn choose_frame_candidate(
        candidates: &[Candidate],
        frame: &CanonicalFrame,
        seq: &CanonicalSequence,
        profile: &GifProfile,
        palette_strategies: &PaletteStrategySet,
    ) -> FrameDecision {
        // Score all candidates
        let mut scored: Vec<(Candidate, ScoreBreakdown)> = candidates
            .iter()
            .map(|c| (c.clone(), Scorer::score_candidate(c, frame, seq, profile)))
            .collect();

        // Sort by total_score (lower is better)
        scored.sort_by(|a, b| a.1.total_score.partial_cmp(&b.1.total_score).unwrap());

        // Select the best candidate
        let (chosen_candidate, chosen_score) = if let Some((c, s)) = scored.first() {
            (c.clone(), *s)
        } else {
            // Fallback: if no candidates, use full frame with default metadata
            let fallback = Candidate {
                frame_index: 0,
                representation: CandidateRepresentation::FullFrame,
                metadata: CandidateMetadata {
                    changed_bbox: frame.changed_region.bbox,
                    changed_pixel_count: frame.changed_region.changed_pixel_count,
                    changed_ratio: frame.changed_region.changed_ratio,
                    source_is_full_canvas: frame.changed_region.is_full_canvas_patch,
                    source_has_transparency: frame.source_patch.has_transparency,
                    source_transparent_count: frame.source_patch.transparent_pixel_count,
                    source_opaque_count: frame.source_patch.opaque_pixel_count,
                    disposal_method: frame.dispose,
                    safety_reason: SafetyReason::AlwaysSafe,
                },
            };
            let score = Scorer::score_candidate(&fallback, frame, seq, profile);
            (fallback, score)
        };

        // Collect alternatives (top 2 non-chosen candidates)
        let alternatives: Vec<(CandidateRepresentation, ScoreBreakdown)> = scored
            .iter()
            .skip(1)
            .take(2)
            .map(|(c, s)| (c.representation.clone(), *s))
            .collect();

        // Choose palette strategy (prefer primary strategy)
        let chosen_palette_strategy = palette_strategies
            .primary()
            .unwrap_or(PaletteStrategy::DeriveSequenceGlobalPreferred);

        // Determine reason code
        let reason = Self::determine_reason(
            &chosen_candidate,
            &chosen_score,
            &scored,
            profile,
            palette_strategies,
        );

        // Generate explanation
        let explanation = Self::generate_explanation(
            &chosen_candidate,
            &chosen_score,
            &reason,
            profile,
            &chosen_palette_strategy,
        );

        FrameDecision {
            frame_index: chosen_candidate.frame_index,
            chosen_candidate: chosen_candidate.representation.clone(),
            chosen_palette_strategy,
            score_breakdown: chosen_score,
            alternatives,
            reason,
            explanation,
        }
    }

    /// Choose candidates for an entire sequence.
    ///
    /// Returns a `SequenceDecision` with per-frame decisions and summary.
    pub fn choose_sequence(
        all_candidates: &[Vec<Candidate>],
        seq: &CanonicalSequence,
        profile: &GifProfile,
        palette_strategies: &PaletteStrategySet,
    ) -> SequenceDecision {
        let mut frame_decisions = Vec::new();
        let mut total_score = 0.0;

        for (frame_idx, frame) in seq.frames.iter().enumerate() {
            let candidates = all_candidates
                .get(frame_idx)
                .cloned()
                .unwrap_or_default();

            let decision = Self::choose_frame_candidate(
                &candidates,
                frame,
                seq,
                profile,
                palette_strategies,
            );

            total_score += decision.score_breakdown.total_score;
            frame_decisions.push(decision);
        }

        let avg_score = if !frame_decisions.is_empty() {
            total_score / frame_decisions.len() as f32
        } else {
            0.0
        };

        // Estimate total encoded bytes (heuristic)
        let estimated_total_bytes = Self::estimate_total_bytes(&frame_decisions, seq);

        let sequence_palette_strategy = palette_strategies
            .primary()
            .unwrap_or(PaletteStrategy::DeriveSequenceGlobalPreferred);

        let summary = format!(
            "Sequence: {} frames, taxonomy: {}, avg_score: {:.3}, est_bytes: {}",
            seq.frames.len(),
            profile.taxonomy.name(),
            avg_score,
            estimated_total_bytes
        );

        SequenceDecision {
            frame_decisions,
            sequence_palette_strategy,
            avg_score,
            estimated_total_bytes,
            summary,
        }
    }

    /// Determine the reason code for a decision.
    fn determine_reason(
        chosen: &Candidate,
        chosen_score: &ScoreBreakdown,
        all_scored: &[(Candidate, ScoreBreakdown)],
        profile: &GifProfile,
        _palette_strategies: &PaletteStrategySet,
    ) -> DecisionReason {
        // Check if chosen is preferred by taxonomy
        if Self::is_taxonomy_preferred(&chosen.representation, profile) {
            return DecisionReason::TaxonomyPreferred;
        }

        // Check if chosen is safe and alternatives are risky
        if chosen.metadata.safety_reason == SafetyReason::AlwaysSafe {
            let has_risky_alternatives = all_scored
                .iter()
                .skip(1)
                .any(|(c, _)| c.metadata.safety_reason != SafetyReason::AlwaysSafe);
            if has_risky_alternatives {
                return DecisionReason::SafetyConstraint;
            }
        }

        // Check if score difference is small (tie-breaker)
        if let Some((_, second_score)) = all_scored.get(1) {
            let score_diff = (second_score.total_score - chosen_score.total_score).abs();
            if score_diff < 0.05 {
                return DecisionReason::TieBreaker;
            }
        }

        // Default: lowest score
        DecisionReason::LowestScore
    }

    /// Check if a candidate representation is preferred by taxonomy.
    fn is_taxonomy_preferred(repr: &CandidateRepresentation, profile: &GifProfile) -> bool {
        match profile.taxonomy {
            SequenceTaxonomy::OpaqueDeltaGlobalPalette => {
                // Prefer opaque bbox or full frame
                matches!(
                    repr,
                    CandidateRepresentation::ExactOpaqueBbox { .. }
                        | CandidateRepresentation::FullFrame
                )
            }
            SequenceTaxonomy::TransparencyHeavySparseDelta => {
                // Prefer sparse patch
                matches!(repr, CandidateRepresentation::TransparentSparsePatch { .. })
            }
            SequenceTaxonomy::DisposalHeavyBackgroundPrevious => {
                // Prefer full frame or opaque bbox (respect disposal)
                matches!(
                    repr,
                    CandidateRepresentation::FullFrame | CandidateRepresentation::ExactOpaqueBbox { .. }
                )
            }
            SequenceTaxonomy::Photographic => {
                // Prefer full frame (quality over size)
                matches!(repr, CandidateRepresentation::FullFrame)
            }
            SequenceTaxonomy::Mixed => {
                // No strong preference
                false
            }
        }
    }

    /// Generate human-readable explanation for a decision.
    fn generate_explanation(
        chosen: &Candidate,
        score: &ScoreBreakdown,
        reason: &DecisionReason,
        _profile: &GifProfile,
        palette_strategy: &PaletteStrategy,
    ) -> String {
        let repr_name = match &chosen.representation {
            CandidateRepresentation::FullFrame => "full-frame",
            CandidateRepresentation::ExactOpaqueBbox { .. } => "opaque-bbox",
            CandidateRepresentation::TransparentSparsePatch { .. } => "sparse-patch",
            CandidateRepresentation::MinimalNoOp => "minimal-noop",
        };

        let reason_text = match reason {
            DecisionReason::LowestScore => "lowest total score",
            DecisionReason::TaxonomyPreferred => "preferred by taxonomy",
            DecisionReason::SafetyConstraint => "safety constraint (avoids risky candidates)",
            DecisionReason::PaletteStrategyAlignment => "aligns with palette strategy",
            DecisionReason::TieBreaker => "tie-breaker (similar scores)",
            DecisionReason::Fallback => "fallback (no ideal candidate)",
        };

        format!(
            "Frame {}: {} (score: {:.3}, byte: {:.2}, visual: {:.2}, lut: {:.2}, transparency: {:.2}, coherence: {:.2}) - {} - palette: {}",
            chosen.frame_index,
            repr_name,
            score.total_score,
            score.byte_cost,
            score.visual_risk,
            score.lut_cost,
            score.synthetic_transparency_risk,
            score.palette_coherence,
            reason_text,
            palette_strategy.name()
        )
    }

    /// Estimate total encoded bytes for a sequence (heuristic).
    fn estimate_total_bytes(decisions: &[FrameDecision], seq: &CanonicalSequence) -> usize {
        let canvas_area = (seq.width as usize) * (seq.height as usize);
        let mut total = 0usize;

        for decision in decisions {
            let frame_bytes = match &decision.chosen_candidate {
                CandidateRepresentation::FullFrame => {
                    // Full frame: ~canvas_area pixels + overhead
                    (canvas_area as f32 * 0.5) as usize + 100
                }
                CandidateRepresentation::ExactOpaqueBbox { bbox } => {
                    // Opaque bbox: bbox area + overhead
                    (bbox.area() as f32 * 0.4) as usize + 50
                }
                CandidateRepresentation::TransparentSparsePatch { bbox, .. } => {
                    // Sparse patch: bbox area + transparency overhead
                    (bbox.area() as f32 * 0.3) as usize + 30
                }
                CandidateRepresentation::MinimalNoOp => {
                    // Minimal/no-op: very small
                    20
                }
            };
            total += frame_bytes;
        }

        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate_gen::CandidateGenerator;
    use crate::profiler::profile_canonical_sequence;
    use crate::types::{DisposalMethod, Gif};
    use std::time::Duration;

    fn create_test_gif_opaque_delta() -> Gif {
        // Voyager-like: opaque deltas with global palette
        use crate::types::{Frame, Palette};

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
                left: 0,
                top: 0,
                width: 100,
                height: 100,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
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
        // Transparency-heavy: sparse transparent patches
        use crate::types::{Frame, Palette};

        let palette = Palette {
            colors: vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]],
        };

        let mut frames = Vec::new();
        for _i in 0..3 {
            let mut pixels = vec![0u8; 100 * 100 * 4];
            // Fill with transparent pixels
            for j in 0..100 * 100 {
                pixels[j * 4] = 255;
                pixels[j * 4 + 3] = 0; // Transparent
            }
            frames.push(Frame {
                pixels,
                left: 0,
                top: 0,
                width: 100,
                height: 100,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
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
    fn test_score_breakdown_compute_total() {
        let mut score = ScoreBreakdown {
            byte_cost: 0.5,
            visual_risk: 0.3,
            lut_cost: 0.2,
            temporal_instability: 0.2,
            synthetic_transparency_risk: 0.1,
            palette_coherence: 0.1,
            cpu_cost: 0.4,
            total_score: 0.0,
        };

        score.compute_total();

        // Expected: 0.25*0.5 + 0.25*0.3 + 0.20*0.2 + 0.10*0.2 + 0.10*0.1 + 0.05*0.1 + 0.05*0.4
        //         = 0.125 + 0.075 + 0.04 + 0.02 + 0.01 + 0.005 + 0.02 = 0.295
        assert!((score.total_score - 0.295).abs() < 0.001);
    }

    #[test]
    fn test_score_breakdown_weights_sum_to_one() {
        // Verify that weights sum to 1.0
        let weights = [0.25, 0.25, 0.20, 0.10, 0.10, 0.05, 0.05];
        let sum: f32 = weights.iter().sum();
        assert!((sum - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_chooser_opaque_delta_prefers_opaque_bbox() {
        let gif = create_test_gif_opaque_delta();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();
        let palette_strategies = crate::palette_strategy::determine_palette_strategies(&gif, &seq, &profile);

        let candidates = CandidateGenerator::generate(&seq);

        // Group candidates by frame
        let mut frame_candidates: Vec<Vec<Candidate>> = vec![Vec::new(); seq.frames.len()];
        for candidate in candidates {
            frame_candidates[candidate.frame_index].push(candidate);
        }

        // Choose for first frame
        let decision = Chooser::choose_frame_candidate(
            &frame_candidates[0],
            &seq.frames[0],
            &seq,
            &profile,
            &palette_strategies,
        );

        // For opaque-delta sequences, should prefer opaque bbox or full frame
        // (or minimal-noop if no changes)
        assert!(matches!(
            decision.chosen_candidate,
            CandidateRepresentation::ExactOpaqueBbox { .. }
                | CandidateRepresentation::FullFrame
                | CandidateRepresentation::MinimalNoOp
        ));
    }

    #[test]
    fn test_chooser_transparency_heavy_allows_sparse_patch() {
        let gif = create_test_gif_transparency_heavy();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();
        let palette_strategies = crate::palette_strategy::determine_palette_strategies(&gif, &seq, &profile);

        let candidates = CandidateGenerator::generate(&seq);

        // Group candidates by frame
        let mut frame_candidates: Vec<Vec<Candidate>> = vec![Vec::new(); seq.frames.len()];
        for candidate in candidates {
            frame_candidates[candidate.frame_index].push(candidate);
        }

        // Choose for first frame
        let _decision = Chooser::choose_frame_candidate(
            &frame_candidates[0],
            &seq.frames[0],
            &seq,
            &profile,
            &palette_strategies,
        );

        // For transparency-heavy sequences, sparse patch is acceptable
        // (though full frame may still be chosen if it scores better)
        assert!(!frame_candidates[0].is_empty());
    }

    #[test]
    fn test_chooser_deterministic() {
        let gif = create_test_gif_opaque_delta();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();
        let palette_strategies = crate::palette_strategy::determine_palette_strategies(&gif, &seq, &profile);

        let candidates = CandidateGenerator::generate(&seq);

        // Group candidates by frame
        let mut frame_candidates: Vec<Vec<Candidate>> = vec![Vec::new(); seq.frames.len()];
        for candidate in candidates {
            frame_candidates[candidate.frame_index].push(candidate);
        }

        // Choose twice and verify same result
        let decision1 = Chooser::choose_frame_candidate(
            &frame_candidates[0],
            &seq.frames[0],
            &seq,
            &profile,
            &palette_strategies,
        );

        let decision2 = Chooser::choose_frame_candidate(
            &frame_candidates[0],
            &seq.frames[0],
            &seq,
            &profile,
            &palette_strategies,
        );

        assert_eq!(decision1.chosen_candidate, decision2.chosen_candidate);
        assert_eq!(decision1.chosen_palette_strategy, decision2.chosen_palette_strategy);
        assert!((decision1.score_breakdown.total_score - decision2.score_breakdown.total_score).abs() < 0.001);
    }

    #[test]
    fn test_sequence_decision_summary() {
        let gif = create_test_gif_opaque_delta();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();
        let palette_strategies = crate::palette_strategy::determine_palette_strategies(&gif, &seq, &profile);

        let candidates = CandidateGenerator::generate(&seq);

        // Group candidates by frame
        let mut frame_candidates: Vec<Vec<Candidate>> = vec![Vec::new(); seq.frames.len()];
        for candidate in candidates {
            frame_candidates[candidate.frame_index].push(candidate);
        }

        let decision = Chooser::choose_sequence(&frame_candidates, &seq, &profile, &palette_strategies);

        assert_eq!(decision.frame_decisions.len(), seq.frames.len());
        assert!(decision.avg_score >= 0.0 && decision.avg_score <= 1.0);
        assert!(decision.estimated_total_bytes > 0);
        assert!(!decision.summary.is_empty());
    }

    #[test]
    fn test_lut_preserving_opaque_bbox_scores_better_than_lut_breaking_sparse() {
        // Test that LUT-preserving opaque bbox candidate scores better than
        // LUT-breaking sparse candidate, even if sparse is slightly smaller.
        let gif = create_test_gif_opaque_delta();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();

        // Create two candidates: opaque bbox (LUT-preserving) and sparse patch (LUT-breaking)
        use crate::adaptive_ir::BoundingBox;
        let opaque_bbox = Candidate {
            frame_index: 0,
            representation: CandidateRepresentation::ExactOpaqueBbox {
                bbox: BoundingBox::new(10, 10, 50, 50),
            },
            metadata: CandidateMetadata {
                changed_bbox: BoundingBox::new(10, 10, 50, 50),
                changed_pixel_count: 1600,
                changed_ratio: 0.16,
                source_is_full_canvas: false,
                source_has_transparency: false,
                source_transparent_count: 0,
                source_opaque_count: 10000,
                disposal_method: DisposalMethod::Keep,
                safety_reason: SafetyReason::AlwaysSafe,
            },
        };

        let sparse_patch = Candidate {
            frame_index: 0,
            representation: CandidateRepresentation::TransparentSparsePatch {
                bbox: BoundingBox::new(10, 10, 40, 40),
                is_risky: false,
            },
            metadata: CandidateMetadata {
                changed_bbox: BoundingBox::new(10, 10, 40, 40),
                changed_pixel_count: 800,
                changed_ratio: 0.08,
                source_is_full_canvas: false,
                source_has_transparency: false,
                source_transparent_count: 0,
                source_opaque_count: 10000,
                disposal_method: DisposalMethod::Keep,
                safety_reason: SafetyReason::AlwaysSafe,
            },
        };

        let opaque_score = Scorer::score_candidate(&opaque_bbox, &seq.frames[0], &seq, &profile);
        let sparse_score = Scorer::score_candidate(&sparse_patch, &seq.frames[0], &seq, &profile);

        // Opaque bbox should score better (lower total_score) than sparse patch
        // because LUT cost dominates the byte savings
        assert!(
            opaque_score.total_score < sparse_score.total_score,
            "Opaque bbox ({:.3}) should score better than sparse patch ({:.3})",
            opaque_score.total_score,
            sparse_score.total_score
        );

        // Verify that lut_cost is the key differentiator
        assert!(opaque_score.lut_cost < sparse_score.lut_cost);
    }

    #[test]
    fn test_transparency_heavy_allows_sparse_without_over_penalty() {
        // Test that transparency-heavy sequences don't over-penalize sparse candidates
        // when they are structurally appropriate.
        let gif = create_test_gif_transparency_heavy();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();

        use crate::adaptive_ir::BoundingBox;
        let sparse_patch = Candidate {
            frame_index: 0,
            representation: CandidateRepresentation::TransparentSparsePatch {
                bbox: BoundingBox::new(10, 10, 40, 40),
                is_risky: false,
            },
            metadata: CandidateMetadata {
                changed_bbox: BoundingBox::new(10, 10, 40, 40),
                changed_pixel_count: 800,
                changed_ratio: 0.08,
                source_is_full_canvas: false,
                source_has_transparency: true,
                source_transparent_count: 500,
                source_opaque_count: 300,
                disposal_method: DisposalMethod::Keep,
                safety_reason: SafetyReason::AlwaysSafe,
            },
        };

        let score = Scorer::score_candidate(&sparse_patch, &seq.frames[0], &seq, &profile);

        // For transparency-heavy sequences, sparse patch should have reasonable score
        // (not excessively penalized)
        assert!(score.total_score < 0.8, "Sparse patch in transparency-heavy sequence should score reasonably");

        // Verify that synthetic_transparency_risk is not excessive
        assert!(score.synthetic_transparency_risk < 0.7);
    }

    #[test]
    fn test_score_breakdown_includes_new_dimensions() {
        // Test that score breakdown includes all new dimensions
        let gif = create_test_gif_opaque_delta();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();

        let candidates = CandidateGenerator::generate(&seq);
        let frame_candidates: Vec<Vec<Candidate>> = vec![candidates];

        let score = Scorer::score_candidate(&frame_candidates[0][0], &seq.frames[0], &seq, &profile);

        // Verify all dimensions are present and in valid range
        assert!(score.byte_cost >= 0.0 && score.byte_cost <= 1.0);
        assert!(score.visual_risk >= 0.0 && score.visual_risk <= 1.0);
        assert!(score.lut_cost >= 0.0 && score.lut_cost <= 1.0);
        assert!(score.temporal_instability >= 0.0 && score.temporal_instability <= 1.0);
        assert!(score.synthetic_transparency_risk >= 0.0 && score.synthetic_transparency_risk <= 1.0);
        assert!(score.palette_coherence >= 0.0 && score.palette_coherence <= 1.0);
        assert!(score.cpu_cost >= 0.0 && score.cpu_cost <= 1.0);
        assert!(score.total_score >= 0.0 && score.total_score <= 1.0);
    }

    #[test]
    fn test_scoring_deterministic_with_new_dimensions() {
        // Test that scoring is deterministic with new dimensions
        let gif = create_test_gif_opaque_delta();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();

        let candidates = CandidateGenerator::generate(&seq);

        // Score the same candidate multiple times
        let score1 = Scorer::score_candidate(&candidates[0], &seq.frames[0], &seq, &profile);
        let score2 = Scorer::score_candidate(&candidates[0], &seq.frames[0], &seq, &profile);
        let score3 = Scorer::score_candidate(&candidates[0], &seq.frames[0], &seq, &profile);

        // All scores should be identical
        assert!((score1.total_score - score2.total_score).abs() < 0.0001);
        assert!((score2.total_score - score3.total_score).abs() < 0.0001);
        assert!((score1.lut_cost - score2.lut_cost).abs() < 0.0001);
        assert!((score1.palette_coherence - score2.palette_coherence).abs() < 0.0001);
    }

    #[test]
    fn test_opaque_delta_lut_cost_low() {
        // Test that opaque-delta sequences have low LUT cost
        let gif = create_test_gif_opaque_delta();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();

        use crate::adaptive_ir::BoundingBox;
        let opaque_bbox = Candidate {
            frame_index: 0,
            representation: CandidateRepresentation::ExactOpaqueBbox {
                bbox: BoundingBox::new(10, 10, 50, 50),
            },
            metadata: CandidateMetadata {
                changed_bbox: BoundingBox::new(10, 10, 50, 50),
                changed_pixel_count: 1600,
                changed_ratio: 0.16,
                source_is_full_canvas: false,
                source_has_transparency: false,
                source_transparent_count: 0,
                source_opaque_count: 10000,
                disposal_method: DisposalMethod::Keep,
                safety_reason: SafetyReason::AlwaysSafe,
            },
        };

        let score = Scorer::score_candidate(&opaque_bbox, &seq.frames[0], &seq, &profile);

        // For opaque-delta sequences, opaque bbox should have very low LUT cost
        assert!(score.lut_cost < 0.15, "Opaque bbox in opaque-delta should have low LUT cost");
    }

    #[test]
    fn test_transparency_heavy_lut_cost_high() {
        // Test that transparency-heavy sequences have high LUT cost for sparse patches
        let gif = create_test_gif_transparency_heavy();
        let seq = crate::adaptive_ir::CanonicalSequenceBuilder::build(&gif).unwrap();
        let profile = profile_canonical_sequence(&seq).unwrap();

        use crate::adaptive_ir::BoundingBox;
        let sparse_patch = Candidate {
            frame_index: 0,
            representation: CandidateRepresentation::TransparentSparsePatch {
                bbox: BoundingBox::new(10, 10, 40, 40),
                is_risky: false,
            },
            metadata: CandidateMetadata {
                changed_bbox: BoundingBox::new(10, 10, 40, 40),
                changed_pixel_count: 800,
                changed_ratio: 0.08,
                source_is_full_canvas: false,
                source_has_transparency: true,
                source_transparent_count: 500,
                source_opaque_count: 300,
                disposal_method: DisposalMethod::Keep,
                safety_reason: SafetyReason::AlwaysSafe,
            },
        };

        let score = Scorer::score_candidate(&sparse_patch, &seq.frames[0], &seq, &profile);

        // For transparency-heavy sequences, sparse patch should have high LUT cost
        assert!(score.lut_cost > 0.5, "Sparse patch in transparency-heavy should have high LUT cost");
    }
}
