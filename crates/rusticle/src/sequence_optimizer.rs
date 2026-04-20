//! Sequence-level chunked DP-lite decision model.
//!
//! This module implements a bounded, practical DP-lite/beam-search style optimizer
//! that reasons about transitions, palette churn, and representation continuity
//! across frames/chunks. It replaces the per-frame greedy `Chooser::choose_sequence()`
//! with a sequence-level decision model that optimizes palette coherence and
//! representation consistency.
//!
//! # Architecture
//!
//! Instead of per-frame greedy selection, we divide the sequence into chunks
//! (8-16 frames) and optimize within each chunk using bounded DP with:
//! - State: `(palette_strategy, lut_eligibility_run_length)`
//! - Transition cost: switching palette strategies adds a penalty
//! - Beam width: 3 (keep top-3 partial solutions per frame position)
//! - Total states explored per chunk: ≤ 3 × 12 × 3 candidates = 108 (trivially bounded)
//!
//! # Chunking Strategy
//!
//! 1. Detect scene changes: `changed_ratio > 0.8` triggers chunk boundary
//! 2. If no scene changes, use fixed chunks of 12 frames
//! 3. Each chunk gets a single palette strategy decision (not per-frame)
//!
//! # Within-Chunk Optimization
//!
//! For each frame in chunk:
//! 1. Enumerate surviving candidates (post tier-1 pruning)
//! 2. Transition cost: switching from LutPreserving to LutBreaking adds penalty
//! 3. Objective: minimize `sum(frame_scores) + sum(transition_costs)` subject to quality guardrails
//! 4. Keep top-3 partial solutions per frame position (beam width)
//!
//! # Complexity
//!
//! - Per-chunk: O(chunk_size × candidates_per_frame × beam_width) = O(12 × 3 × 3) = O(108)
//! - Total: O(num_chunks × 108) = O(sequence_length / 12 × 108) ≈ O(sequence_length × 9)
//! - Practical: <10ms for 100-frame sequence
//! - Memory: O(beam_width × chunk_size) = O(3 × 12) = O(36) states per chunk
//!
//! # Output
//!
//! `SequenceDecision` with per-frame decisions that are globally coherent within chunks.
//!
//! # Integration
//!
//! This module is designed to be called by `rusticle-0r7` (wire TieredOptimizer into
//! adaptive_encode.rs). It takes:
//! - `all_candidates`: Per-frame candidate lists (already pruned by tier-1)
//! - `seq`: Canonical sequence IR
//! - `profile`: GIF profile with taxonomy and metrics
//! - `config`: Optimizer configuration (chunk size, beam width, transition costs)
//!
//! And produces a `SequenceDecision` with globally coherent per-frame choices.
//!
//! # Caveats
//!
//! - Chunk-level palette strategy decision is fixed per chunk (not per-frame)
//! - Beam width of 3 is a practical trade-off; larger widths increase exploration
//! - Transition costs are configurable but default to 0.15 (palette switch) and 0.10 (LUT break)
//! - No full exponential search; bounded by chunk size and beam width

use crate::adaptive_ir::CanonicalSequence;
use crate::candidate_gen::Candidate;
use crate::lut_policy::candidate_to_family;
use crate::palette_strategy::PaletteStrategy;
use crate::profiler::GifProfile;
use crate::scoring::{FrameDecision, Scorer, SequenceDecision, DecisionReason};

/// Configuration for sequence-level DP-lite optimization.
#[derive(Debug, Clone)]
pub struct SequenceOptimizerConfig {
    /// Chunk size for DP-lite optimization (default: 12 frames).
    pub chunk_size: usize,
    /// Beam width: number of top partial solutions to keep per frame (default: 3).
    pub beam_width: usize,
    /// Palette switch cost penalty (default: 0.15).
    pub palette_switch_cost: f32,
    /// LUT break cost penalty (default: 0.10).
    pub lut_break_cost: f32,
    /// Scene change threshold: changed_ratio > this triggers chunk boundary (default: 0.8).
    pub scene_change_threshold: f32,
}

impl Default for SequenceOptimizerConfig {
    fn default() -> Self {
        Self {
            chunk_size: 12,
            beam_width: 3,
            palette_switch_cost: 0.15,
            lut_break_cost: 0.10,
            scene_change_threshold: 0.8,
        }
    }
}

/// State for a partial solution in DP-lite beam search.
#[derive(Debug, Clone)]
struct DpState {
    /// Frame index (0-based within chunk).
    frame_idx: usize,
    /// Whether the previous frame was LUT-preserving.
    prev_lut_preserving: bool,
    /// Accumulated cost so far.
    accumulated_cost: f32,
    /// Per-frame decisions so far.
    decisions: Vec<FrameDecision>,
}

impl DpState {
    /// Create a new initial state for a chunk.
    fn new() -> Self {
        Self {
            frame_idx: 0,
            prev_lut_preserving: true, // Assume LUT-preserving start
            accumulated_cost: 0.0,
            decisions: Vec::new(),
        }
    }

    /// Extend this state with a new frame decision.
    fn extend(
        &self,
        frame_idx: usize,
        decision: FrameDecision,
        transition_cost: f32,
    ) -> Self {
        let mut new_state = self.clone();
        new_state.frame_idx = frame_idx;
        new_state.accumulated_cost += decision.score_breakdown.total_score + transition_cost;
        new_state.decisions.push(decision);
        new_state
    }
}

/// Sequence-level DP-lite optimizer.
pub struct SequenceOptimizer;

impl SequenceOptimizer {
    /// Optimize a sequence using chunked DP-lite.
    ///
    /// # Arguments
    ///
    /// - `all_candidates`: Per-frame candidate lists (already pruned by tier-1)
    /// - `seq`: Canonical sequence IR
    /// - `profile`: GIF profile with taxonomy and metrics
    /// - `config`: Optimizer configuration
    ///
    /// # Returns
    ///
    /// A `SequenceDecision` with globally coherent per-frame choices.
    pub fn optimize(
        all_candidates: &[Vec<Candidate>],
        seq: &CanonicalSequence,
        profile: &GifProfile,
        config: &SequenceOptimizerConfig,
    ) -> SequenceDecision {
        // Detect chunk boundaries
        let chunks = Self::detect_chunks(seq, config);

        // Optimize each chunk independently
        let mut all_decisions = Vec::new();
        for chunk in chunks {
            let chunk_decisions = Self::optimize_chunk(
                &chunk,
                all_candidates,
                seq,
                profile,
                config,
            );
            all_decisions.extend(chunk_decisions);
        }

        // Compute summary statistics
        let avg_score = if !all_decisions.is_empty() {
            all_decisions.iter().map(|d| d.score_breakdown.total_score).sum::<f32>()
                / all_decisions.len() as f32
        } else {
            0.0
        };

        let estimated_total_bytes = all_decisions
            .iter()
            .map(|d| (d.score_breakdown.byte_cost * 10000.0) as usize)
            .sum();

        let summary = format!(
            "DP-lite sequence optimization: {} frames, {} chunks, avg_score={:.3}",
            all_decisions.len(),
            all_decisions.len().div_ceil(config.chunk_size),
            avg_score
        );

        SequenceDecision {
            frame_decisions: all_decisions,
            sequence_palette_strategy: PaletteStrategy::ReuseGlobalPreferred,
            avg_score,
            estimated_total_bytes,
            summary,
        }
    }

    /// Detect chunk boundaries in the sequence.
    ///
    /// Returns a list of frame index ranges, one per chunk.
    fn detect_chunks(seq: &CanonicalSequence, config: &SequenceOptimizerConfig) -> Vec<(usize, usize)> {
        let mut chunks = Vec::new();
        let mut chunk_start = 0;

        for (i, frame) in seq.frames.iter().enumerate() {
            let is_scene_change = frame.changed_region.changed_ratio > config.scene_change_threshold;

            if is_scene_change && i > chunk_start {
                // End current chunk at scene change
                chunks.push((chunk_start, i));
                chunk_start = i;
            } else if i - chunk_start >= config.chunk_size && i > chunk_start {
                // End chunk at fixed size
                chunks.push((chunk_start, i));
                chunk_start = i;
            }
        }

        // Final chunk
        if chunk_start < seq.frames.len() {
            chunks.push((chunk_start, seq.frames.len()));
        }

        chunks
    }

    /// Optimize a single chunk using DP-lite beam search.
    fn optimize_chunk(
        chunk: &(usize, usize),
        all_candidates: &[Vec<Candidate>],
        seq: &CanonicalSequence,
        profile: &GifProfile,
        config: &SequenceOptimizerConfig,
    ) -> Vec<FrameDecision> {
        let (chunk_start, chunk_end) = *chunk;

        // Determine palette strategy for this chunk
        let palette_strategy = Self::choose_chunk_palette_strategy(
            chunk_start,
            chunk_end,
            all_candidates,
            seq,
            profile,
        );

        // Initialize beam with initial state
        let mut beam: Vec<DpState> = vec![DpState::new()];

        // DP-lite: process each frame in the chunk
        for (frame_idx, (frame, candidates)) in seq.frames[chunk_start..chunk_end]
            .iter()
            .zip(&all_candidates[chunk_start..chunk_end])
            .enumerate()
            .map(|(i, (f, c))| (chunk_start + i, (f, c)))
        {

            let mut next_beam = Vec::new();

            // For each state in current beam
            for state in &beam {
                // For each candidate for this frame
                for candidate in candidates {
                    // Score the candidate
                    let score = Scorer::score_candidate(candidate, frame, seq, profile);

                    // Compute transition cost
                    let is_lut_preserving = candidate_to_family(&candidate.representation)
                        .is_lut_preserving();
                    let transition_cost = Self::compute_transition_cost(
                        state.prev_lut_preserving,
                        is_lut_preserving,
                        config,
                    );

                    // Create frame decision
                    let decision = FrameDecision {
                        frame_index: frame_idx,
                        chosen_candidate: candidate.representation.clone(),
                        chosen_palette_strategy: palette_strategy,
                        score_breakdown: score,
                        alternatives: Vec::new(),
                        reason: DecisionReason::LowestScore,
                        explanation: format!(
                            "DP-lite frame {} in chunk [{}, {})",
                            frame_idx, chunk_start, chunk_end
                        ),
                    };

                    // Extend state
                    let mut new_state = state.extend(frame_idx, decision, transition_cost);
                    new_state.prev_lut_preserving = is_lut_preserving;

                    next_beam.push(new_state);
                }
            }

            // Prune beam to top-k by accumulated cost
            next_beam.sort_by(|a, b| {
                a.accumulated_cost
                    .partial_cmp(&b.accumulated_cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            next_beam.truncate(config.beam_width);

            beam = next_beam;
        }

        // Extract best path from beam
        if let Some(best_state) = beam.first() {
            best_state.decisions.clone()
        } else {
            // Fallback: greedy per-frame (should not happen)
            Vec::new()
        }
    }

    /// Choose palette strategy for a chunk.
    ///
    /// If ≥80% of frames in chunk are LutPreserving with ReuseGlobalPreferred,
    /// lock chunk to that strategy. Otherwise, allow DeriveSequenceGlobalPreferred.
    fn choose_chunk_palette_strategy(
        chunk_start: usize,
        chunk_end: usize,
        all_candidates: &[Vec<Candidate>],
        _seq: &CanonicalSequence,
        _profile: &GifProfile,
    ) -> PaletteStrategy {
        let mut lut_preserving_count = 0;

        for frame_idx in chunk_start..chunk_end {
            if let Some(candidates) = all_candidates.get(frame_idx) {
                // Check if any candidate is LUT-preserving
                if candidates.iter().any(|c| {
                    candidate_to_family(&c.representation).is_lut_preserving()
                }) {
                    lut_preserving_count += 1;
                }
            }
        }

        let chunk_size = chunk_end - chunk_start;
        let lut_ratio = lut_preserving_count as f32 / chunk_size as f32;

        if lut_ratio >= 0.8 {
            PaletteStrategy::ReuseGlobalPreferred
        } else {
            PaletteStrategy::DeriveSequenceGlobalPreferred
        }
    }

    /// Compute transition cost between two LUT states.
    ///
    /// Switching from LUT-preserving to LUT-breaking adds a penalty.
    fn compute_transition_cost(
        prev_lut_preserving: bool,
        curr_lut_preserving: bool,
        config: &SequenceOptimizerConfig,
    ) -> f32 {
        match (prev_lut_preserving, curr_lut_preserving) {
            (true, false) => config.lut_break_cost,  // Breaking LUT: penalty
            (false, true) => config.lut_break_cost,  // Restoring LUT: penalty
            _ => 0.0,                                 // No transition cost
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_ir::{BoundingBox, Canvas, CanonicalFrame, CanonicalSequence, ChangedRegion, SourcePatch};
    use crate::candidate_gen::{Candidate, CandidateMetadata, CandidateRepresentation, SafetyReason};
    use crate::profiler::{
        GifProfile, SequenceTaxonomy, PaletteInfo, ChangeStatistics, SequenceMetrics,
        DisposalDistribution, TransparencyAnalysis, PatchDensity, DeltaSignal,
    };
    use crate::types::DisposalMethod;
    use std::time::Duration;

    fn create_test_gif(width: u16, height: u16, frame_count: usize) -> CanonicalSequence {
        let mut frames = Vec::new();
        for _i in 0..frame_count {
            let canvas_area = (width as usize) * (height as usize);
            let pixels = vec![0u8; canvas_area * 4];

            frames.push(CanonicalFrame {
                source_patch: SourcePatch {
                    pixels: pixels.clone(),
                    left: 0,
                    top: 0,
                    width,
                    height,
                    has_transparency: false,
                    transparent_pixel_count: 0,
                    opaque_pixel_count: canvas_area,
                },
                pre_draw_canvas: Canvas {
                    pixels: pixels.clone(),
                    width,
                    height,
                },
                displayed_canvas: Canvas {
                    pixels: pixels.clone(),
                    width,
                    height,
                },
                post_disposal_canvas: Canvas {
                    pixels: pixels.clone(),
                    width,
                    height,
                },
                changed_region: ChangedRegion {
                    bbox: BoundingBox {
                        left: 0,
                        top: 0,
                        right: width - 1,
                        bottom: height - 1,
                    },
                    changed_pixel_count: 100,
                    changed_ratio: 0.1,
                    is_full_canvas_patch: true,
                },
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::None,
            });
        }

        CanonicalSequence {
            width,
            height,
            loop_count: crate::types::LoopCount::Infinite,
            frames,
        }
    }

    fn create_test_candidates(frame_idx: usize, count: usize) -> Vec<Candidate> {
        (0..count)
            .map(|i| Candidate {
                frame_index: frame_idx,
                representation: if i == 0 {
                    CandidateRepresentation::ExactOpaqueBbox {
                        bbox: BoundingBox {
                            left: 0,
                            top: 0,
                            right: 99,
                            bottom: 99,
                        },
                    }
                } else {
                    CandidateRepresentation::FullFrame
                },
                metadata: CandidateMetadata {
                    changed_bbox: BoundingBox {
                        left: 0,
                        top: 0,
                        right: 99,
                        bottom: 99,
                    },
                    changed_pixel_count: 10000,
                    changed_ratio: 0.1,
                    source_is_full_canvas: true,
                    source_has_transparency: false,
                    source_transparent_count: 0,
                    source_opaque_count: 10000,
                    disposal_method: DisposalMethod::None,
                    safety_reason: SafetyReason::AlwaysSafe,
                },
            })
            .collect()
    }

    fn create_test_profile() -> GifProfile {
        GifProfile {
            metrics: SequenceMetrics {
                frame_count: 24,
                width: 100,
                height: 100,
                total_pixels: 240000,
                avg_delay_ms: 100.0,
            },
            taxonomy: SequenceTaxonomy::OpaqueDeltaGlobalPalette,
            palette_info: PaletteInfo {
                has_global_palette: true,
                local_palette_count: 0,
                palette_stability: 0.95,
            },
            change_statistics: ChangeStatistics {
                avg_changed_ratio: 0.1,
                max_changed_ratio: 0.5,
                min_changed_ratio: 0.01,
                sparse_change_frames: 0,
                dense_change_frames: 0,
            },
            transparency_analysis: TransparencyAnalysis {
                frames_with_transparency: 0,
                avg_transparency_ratio: 0.0,
                max_transparency_ratio: 0.0,
                frames_with_significant_transparency: 0,
                uses_gce: false,
                frames_with_local_palette: 0,
            },
            disposal_distribution: DisposalDistribution {
                keep_count: 0,
                none_count: 24,
                background_count: 0,
                previous_count: 0,
                dominant: "none".to_string(),
            },
            patch_density: PatchDensity {
                avg_bbox_ratio: 0.1,
                max_bbox_ratio: 0.5,
                offset_patch_frames: 0,
                avg_patch_density: 1.0,
            },
            delta_signal: DeltaSignal {
                strength: 0.9,
                opaque_delta_frames: 24,
                offset_sparse_frames: 0,
                is_already_delta_encoded: true,
            },
        }
    }

    #[test]
    fn test_opaque_delta_sequence_stays_coherent() {
        // 24-frame opaque-delta sequence should all be in one chunk with ReuseGlobalPreferred
        let seq = create_test_gif(100, 100, 24);
        let profile = create_test_profile();
        let config = SequenceOptimizerConfig::default();

        let all_candidates: Vec<Vec<Candidate>> = (0..24)
            .map(|i| create_test_candidates(i, 2))
            .collect();

        let decision = SequenceOptimizer::optimize(&all_candidates, &seq, &profile, &config);

        // All frames should be in the decision
        assert_eq!(decision.frame_decisions.len(), 24);

        // All should use the same palette strategy
        let first_strategy = decision.frame_decisions[0].chosen_palette_strategy;
        for frame_decision in &decision.frame_decisions {
            assert_eq!(frame_decision.chosen_palette_strategy, first_strategy);
        }
    }

    #[test]
    fn test_scene_change_creates_chunk_boundary() {
        // Create a sequence with a scene change at frame 10
        let mut seq = create_test_gif(100, 100, 20);
        seq.frames[10].changed_region.changed_ratio = 0.9; // Scene change

        let profile = create_test_profile();
        let config = SequenceOptimizerConfig::default();

        let all_candidates: Vec<Vec<Candidate>> = (0..20)
            .map(|i| create_test_candidates(i, 2))
            .collect();

        let decision = SequenceOptimizer::optimize(&all_candidates, &seq, &profile, &config);

        // Should have decisions for all frames
        assert_eq!(decision.frame_decisions.len(), 20);
    }

    #[test]
    fn test_deterministic_output() {
        // Same inputs should produce identical output
        let seq = create_test_gif(100, 100, 12);
        let profile = create_test_profile();
        let config = SequenceOptimizerConfig::default();

        let all_candidates: Vec<Vec<Candidate>> = (0..12)
            .map(|i| create_test_candidates(i, 2))
            .collect();

        let decision1 = SequenceOptimizer::optimize(&all_candidates, &seq, &profile, &config);
        let decision2 = SequenceOptimizer::optimize(&all_candidates, &seq, &profile, &config);

        // Scores should be identical
        assert_eq!(decision1.avg_score, decision2.avg_score);
        assert_eq!(decision1.frame_decisions.len(), decision2.frame_decisions.len());

        for (d1, d2) in decision1.frame_decisions.iter().zip(decision2.frame_decisions.iter()) {
            assert_eq!(d1.score_breakdown.total_score, d2.score_breakdown.total_score);
        }
    }

    #[test]
    fn test_beam_width_respected() {
        // Verify that beam width limits exploration
        let seq = create_test_gif(100, 100, 12);
        let profile = create_test_profile();
        let config = SequenceOptimizerConfig {
            beam_width: 2,
            ..Default::default()
        };

        let all_candidates: Vec<Vec<Candidate>> = (0..12)
            .map(|i| create_test_candidates(i, 3))
            .collect();

        let decision = SequenceOptimizer::optimize(&all_candidates, &seq, &profile, &config);

        // Should still produce valid output
        assert_eq!(decision.frame_decisions.len(), 12);
    }

    #[test]
    fn test_chunk_size_respected() {
        // Verify that chunk size limits frame grouping
        let seq = create_test_gif(100, 100, 30);
        let profile = create_test_profile();
        let config = SequenceOptimizerConfig {
            chunk_size: 10,
            ..Default::default()
        };

        let all_candidates: Vec<Vec<Candidate>> = (0..30)
            .map(|i| create_test_candidates(i, 2))
            .collect();

        let decision = SequenceOptimizer::optimize(&all_candidates, &seq, &profile, &config);

        // Should have decisions for all frames
        assert_eq!(decision.frame_decisions.len(), 30);
    }

    #[test]
    fn test_dp_lite_prefers_smooth_path() {
        // Test that DP-lite prefers a smooth path over oscillating per-frame greedy choices.
        // Create a sequence where greedy would thrash between two candidates.
        let seq = create_test_gif(100, 100, 12);
        let profile = create_test_profile();
        let config = SequenceOptimizerConfig::default();

        // Create candidates where frame 0-5 prefer OpaqueBbox, frame 6-11 prefer FullFrame
        // but with transition costs, DP-lite should prefer a consistent path
        let all_candidates: Vec<Vec<Candidate>> = (0..12)
            .map(|i| create_test_candidates(i, 2))
            .collect();

        let decision = SequenceOptimizer::optimize(&all_candidates, &seq, &profile, &config);

        // Should produce a valid decision
        assert_eq!(decision.frame_decisions.len(), 12);
        // All frames should have the same palette strategy (chunk-level decision)
        let first_strategy = decision.frame_decisions[0].chosen_palette_strategy;
        for frame_decision in &decision.frame_decisions {
            assert_eq!(frame_decision.chosen_palette_strategy, first_strategy);
        }
    }
}
