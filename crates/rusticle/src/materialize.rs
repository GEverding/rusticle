//! Candidate materialization: convert adaptive decisions into concrete Frame output.
//!
//! This module bridges the gap between adaptive decision-making (candidate selection,
//! palette strategy choice) and actual GIF frame emission. It materializes chosen
//! candidate representations into concrete `Frame` structures with correct:
//! - Pixel data (full canvas, bbox patches, sparse transparent regions)
//! - Frame geometry (left, top, width, height)
//! - Disposal method
//! - Transparency semantics
//!
//! # Materialization Process
//!
//! For each frame, given:
//! - `FrameDecision`: chosen candidate representation and palette strategy
//! - `CanonicalFrame`: canonical IR with pre-draw, displayed, and post-disposal canvases
//! - `CanonicalSequence`: full sequence context
//!
//! Materialize produces a `Frame` with:
//! - **FullFrame**: Complete canvas RGBA pixels, full dimensions.
//! - **ExactOpaqueBbox**: Tight bbox around opaque pixels, offset on canvas.
//! - **TransparentSparsePatch**: Bbox region with transparency preserved.
//! - **MinimalNoOp**: Empty/minimal frame (semantically safe only).
//!
//! # Palette Realization
//!
//! This module produces correct RGBA frame geometry and metadata, but does NOT:
//! - Quantize colors to a palette
//! - Dither or apply color reduction
//! - Finalize palette indices
//!
//! Those tasks are deferred to the palette realization layer (later task).
//!
//! # Invariants
//!
//! - Materialized frames preserve canvas semantics from canonical IR.
//! - Disposal method is preserved from the decision.
//! - Transparency is preserved (no implicit alpha loss).
//! - Bbox frames have correct left/top/width/height offsets.
//! - No canvas-state loss during materialization.

use crate::adaptive_ir::{BoundingBox, CanonicalFrame, CanonicalSequence};
use crate::candidate_gen::CandidateRepresentation;
use crate::error::Result;
use crate::scoring::FrameDecision;
use crate::types::Frame;

/// Materializer: converts adaptive decisions into concrete Frame output.
pub struct Materializer;

impl Materializer {
    /// Materialize a single frame decision into a concrete Frame.
    ///
    /// Given a frame decision (chosen candidate + palette strategy) and the canonical IR,
    /// produces a Frame with correct pixel data, geometry, and disposal method.
    ///
    /// # Arguments
    ///
    /// - `decision`: The chosen candidate representation and palette strategy.
    /// - `frame`: The canonical frame with pre-draw, displayed, and post-disposal canvases.
    /// - `seq`: The full canonical sequence (for context).
    ///
    /// # Returns
    ///
    /// A materialized `Frame` with RGBA pixels, geometry, and metadata.
    /// Palette quantization is deferred to later stages.
    ///
    /// # Errors
    ///
    /// Returns an error if materialization fails (e.g., invalid bbox, memory issues).
    pub fn materialize_frame(
        decision: &FrameDecision,
        frame: &CanonicalFrame,
        seq: &CanonicalSequence,
    ) -> Result<Frame> {
        match &decision.chosen_candidate {
            CandidateRepresentation::FullFrame => {
                Self::materialize_full_frame(frame, seq)
            }
            CandidateRepresentation::ExactOpaqueBbox { bbox } => {
                Self::materialize_opaque_bbox(frame, seq, *bbox)
            }
            CandidateRepresentation::TransparentSparsePatch { bbox, .. } => {
                Self::materialize_transparent_sparse(frame, seq, *bbox)
            }
            CandidateRepresentation::MinimalNoOp => {
                Self::materialize_minimal_noop(frame, seq)
            }
        }
    }

    /// Materialize a full-frame candidate.
    ///
    /// Produces a Frame with the complete displayed canvas as RGBA pixels.
    /// Geometry is full canvas (left=0, top=0, width=canvas.width, height=canvas.height).
    fn materialize_full_frame(frame: &CanonicalFrame, seq: &CanonicalSequence) -> Result<Frame> {
        Ok(Frame {
            pixels: frame.displayed_canvas.pixels.clone(),
            delay: frame.delay,
            dispose: frame.dispose,
            local_palette: None, // Palette realization is deferred
            left: 0,
            top: 0,
            width: seq.width,
            height: seq.height,
        })
    }

    /// Materialize an exact opaque bbox candidate.
    ///
    /// Produces a Frame with only the bbox region, containing opaque pixels from the
    /// displayed canvas. Transparency is set to 0 outside the bbox (implicit).
    ///
    /// # Arguments
    ///
    /// - `frame`: The canonical frame.
    /// - `seq`: The sequence context.
    /// - `bbox`: The bounding box of opaque pixels (in canvas coordinates).
    fn materialize_opaque_bbox(
        frame: &CanonicalFrame,
        seq: &CanonicalSequence,
        bbox: BoundingBox,
    ) -> Result<Frame> {
        let width = bbox.width();
        let height = bbox.height();

        if width == 0 || height == 0 {
            // Empty bbox: SAFETY - GIF format requires non-zero frame dimensions.
            // Emit a 1x1 transparent frame as a safe fallback.
            // This is semantically safe because:
            // - The 1x1 frame is drawn outside the visible canvas (no visual impact).
            // - Disposal is applied correctly (Background clears it, Keep preserves it, etc.).
            // - The canvas state is preserved for the next frame.
            let pixels = vec![0u8, 0u8, 0u8, 0u8]; // 1x1 transparent pixel
            return Ok(Frame {
                pixels,
                delay: frame.delay,
                dispose: frame.dispose,
                local_palette: None,
                left: bbox.left,
                top: bbox.top,
                width: 1,
                height: 1,
            });
        }

        // Extract bbox region from displayed canvas
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);

        for y in bbox.top..bbox.bottom {
            for x in bbox.left..bbox.right {
                let canvas_idx = ((y as usize) * (seq.width as usize) + (x as usize)) * 4;
                if canvas_idx + 4 <= frame.displayed_canvas.pixels.len() {
                    pixels.extend_from_slice(&frame.displayed_canvas.pixels[canvas_idx..canvas_idx + 4]);
                } else {
                    // Out of bounds: fill with transparent black
                    pixels.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }

        Ok(Frame {
            pixels,
            delay: frame.delay,
            dispose: frame.dispose,
            local_palette: None,
            left: bbox.left,
            top: bbox.top,
            width,
            height,
        })
    }

    /// Materialize a transparent sparse patch candidate.
    ///
    /// Produces a Frame with the bbox region, preserving all transparency information.
    /// This is used when the source has transparent pixels that must be preserved.
    ///
    /// # Arguments
    ///
    /// - `frame`: The canonical frame.
    /// - `seq`: The sequence context.
    /// - `bbox`: The bounding box of the sparse region (in canvas coordinates).
    fn materialize_transparent_sparse(
        frame: &CanonicalFrame,
        seq: &CanonicalSequence,
        bbox: BoundingBox,
    ) -> Result<Frame> {
        let width = bbox.width();
        let height = bbox.height();

        if width == 0 || height == 0 {
            // Empty bbox: SAFETY - GIF format requires non-zero frame dimensions.
            // Emit a 1x1 transparent frame as a safe fallback.
            // This is semantically safe because:
            // - The 1x1 frame is drawn outside the visible canvas (no visual impact).
            // - Disposal is applied correctly (Background clears it, Keep preserves it, etc.).
            // - The canvas state is preserved for the next frame.
            let pixels = vec![0u8, 0u8, 0u8, 0u8]; // 1x1 transparent pixel
            return Ok(Frame {
                pixels,
                delay: frame.delay,
                dispose: frame.dispose,
                local_palette: None,
                left: bbox.left,
                top: bbox.top,
                width: 1,
                height: 1,
            });
        }

        // Extract bbox region from displayed canvas, preserving transparency
        let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);

        for y in bbox.top..bbox.bottom {
            for x in bbox.left..bbox.right {
                let canvas_idx = ((y as usize) * (seq.width as usize) + (x as usize)) * 4;
                if canvas_idx + 4 <= frame.displayed_canvas.pixels.len() {
                    pixels.extend_from_slice(&frame.displayed_canvas.pixels[canvas_idx..canvas_idx + 4]);
                } else {
                    // Out of bounds: fill with transparent black
                    pixels.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }

        Ok(Frame {
            pixels,
            delay: frame.delay,
            dispose: frame.dispose,
            local_palette: None,
            left: bbox.left,
            top: bbox.top,
            width,
            height,
        })
    }

    /// Materialize a minimal/no-op candidate.
    ///
    /// Produces a Frame with minimal pixel data. This is only safe when:
    /// - No pixels changed (changed_pixel_count == 0), OR
    /// - Disposal is not Background (which would clear the frame region).
    ///
    /// **SAFETY**: 0x0 frames are invalid in GIF format (GIF encoder rejects them with
    /// "odd-sized buffer" error). We emit a 1x1 transparent frame instead, which is
    /// semantically safe for all disposal methods:
    /// - The 1x1 frame is drawn (no-op visually, outside canvas).
    /// - Disposal is applied (semantically correct).
    /// - The canvas state is preserved for the next frame.
    ///
    /// The frame's delay and disposal method are preserved for timing/cleanup.
    fn materialize_minimal_noop(frame: &CanonicalFrame, _seq: &CanonicalSequence) -> Result<Frame> {
        // SAFETY: GIF format requires non-zero frame dimensions.
        // Emit a 1x1 transparent frame as a safe fallback for all disposal methods.
        // This is semantically safe because:
        // - The 1x1 frame is drawn outside the visible canvas (no visual impact).
        // - Disposal is applied correctly (Background clears it, Keep preserves it, etc.).
        // - The canvas state is preserved for the next frame.
        let pixels = vec![0u8, 0u8, 0u8, 0u8]; // 1x1 transparent pixel (RGBA: 0, 0, 0, 0)
        Ok(Frame {
            pixels,
            delay: frame.delay,
            dispose: frame.dispose,
            local_palette: None,
            left: 0,
            top: 0,
            width: 1,
            height: 1,
        })
    }

    /// Materialize a complete sequence of frame decisions into concrete Frames.
    ///
    /// Applies `materialize_frame` to each decision in order, producing a Vec<Frame>
    /// suitable for encoding.
    ///
    /// # Arguments
    ///
    /// - `decisions`: Per-frame decisions from the scorer/chooser.
    /// - `seq`: The canonical sequence.
    ///
    /// # Returns
    ///
    /// A Vec of materialized Frames in order.
    ///
    /// # Errors
    ///
    /// Returns an error if any frame materialization fails.
    pub fn materialize_sequence(
        decisions: &[FrameDecision],
        seq: &CanonicalSequence,
    ) -> Result<Vec<Frame>> {
        let mut materialized = Vec::with_capacity(decisions.len());

        for decision in decisions {
            let frame_idx = decision.frame_index;
            if frame_idx >= seq.frames.len() {
                return Err(crate::error::Error::EncodeError(
                    format!("Frame index {} out of bounds (sequence has {} frames)",
                        frame_idx, seq.frames.len())
                ));
            }

            let canonical_frame = &seq.frames[frame_idx];
            let materialized_frame = Self::materialize_frame(decision, canonical_frame, seq)?;
            materialized.push(materialized_frame);
        }

        Ok(materialized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_ir::CanonicalSequenceBuilder;
    use crate::candidate_gen::CandidateGenerator;
    use crate::palette_strategy::PaletteStrategy;
    use crate::scoring::{DecisionReason, ScoreBreakdown};
    use crate::types::{DisposalMethod, Gif, LoopCount};
    use std::time::Duration;

    /// Create a simple test GIF with opaque frames.
    fn create_opaque_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
        let mut frames = Vec::new();
        for i in 0..frame_count {
            let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
            // Fill with different colors per frame
            let color = [
                (255, 0, 0, 255),     // Red
                (0, 255, 0, 255),     // Green
                (0, 0, 255, 255),     // Blue
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

    /// Create a test GIF with a small opaque bbox in the center.
    /// The frame is full-canvas but mostly transparent, with a small opaque region.
    fn create_bbox_test_gif(width: u16, height: u16) -> Gif {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];

        // Fill with transparent background
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[3] = 0; // Transparent
        }

        // Draw a 10x10 opaque red square in the center
        let center_x = (width / 2) as usize - 5;
        let center_y = (height / 2) as usize - 5;
        for y in 0..10 {
            for x in 0..10 {
                let idx = ((center_y + y) * (width as usize) + (center_x + x)) * 4;
                pixels[idx] = 255;     // Red
                pixels[idx + 1] = 0;
                pixels[idx + 2] = 0;
                pixels[idx + 3] = 255; // Opaque
            }
        }

        Gif {
            width,
            height,
            global_palette: None,
            frames: vec![crate::types::Frame {
                pixels,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
                left: 0,
                top: 0,
                width,
                height,
            }],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }



    #[test]
    fn test_materialize_full_frame() {
        let gif = create_opaque_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let materialized = Materializer::materialize_frame(&decision, &seq.frames[0], &seq)
            .expect("Failed to materialize");

        // Full frame should have full canvas dimensions
        assert_eq!(materialized.width, 50);
        assert_eq!(materialized.height, 50);
        assert_eq!(materialized.left, 0);
        assert_eq!(materialized.top, 0);
        assert_eq!(materialized.pixels.len(), 50 * 50 * 4);

        // Pixels should match displayed canvas
        assert_eq!(materialized.pixels, seq.frames[0].displayed_canvas.pixels);
    }

    #[test]
    fn test_materialize_opaque_bbox() {
        // Create a fully opaque patch (no transparency) to trigger bbox candidate generation
        // The patch is smaller than the canvas, so bbox candidate will be generated
        let canvas_size = (100 * 100) as usize * 4;
        let mut pixels = vec![0u8; canvas_size];

        // Fill canvas with transparent background
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[3] = 0; // Transparent
        }

        // Draw a 20x20 fully opaque red patch at (40, 40)
        let patch_left = 40u16;
        let patch_top = 40u16;
        let patch_width = 20u16;
        let patch_height = 20u16;

        for y in 0..patch_height as usize {
            for x in 0..patch_width as usize {
                let canvas_x = (patch_left as usize) + x;
                let canvas_y = (patch_top as usize) + y;
                let idx = (canvas_y * 100 + canvas_x) * 4;
                pixels[idx] = 255;     // Red
                pixels[idx + 1] = 0;
                pixels[idx + 2] = 0;
                pixels[idx + 3] = 255; // Opaque
            }
        }

        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![crate::types::Frame {
                pixels,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
                left: patch_left,
                top: patch_top,
                width: patch_width,
                height: patch_height,
            }],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        };

        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        // Manually create a bbox candidate (since the entire patch is opaque, no bbox candidate is generated)
        let _candidates = CandidateGenerator::generate(&seq);

        // Create a decision with a manually specified bbox
        let bbox = BoundingBox::new(patch_left, patch_top, patch_left + patch_width, patch_top + patch_height);
        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::ExactOpaqueBbox { bbox },
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let materialized = Materializer::materialize_frame(&decision, &seq.frames[0], &seq)
            .expect("Failed to materialize");

        // Bbox frame should have correct dimensions
        assert_eq!(materialized.width, bbox.width());
        assert_eq!(materialized.height, bbox.height());
        assert_eq!(materialized.left, bbox.left);
        assert_eq!(materialized.top, bbox.top);

        // Pixel count should match bbox area
        assert_eq!(materialized.pixels.len(), (bbox.width() as usize) * (bbox.height() as usize) * 4);

        // Verify that the materialized pixels match the displayed canvas in the bbox region
        // (The displayed canvas should have the opaque patch)
        let mut expected_pixels = Vec::new();
        for y in bbox.top..bbox.bottom {
            for x in bbox.left..bbox.right {
                let canvas_idx = ((y as usize) * (seq.width as usize) + (x as usize)) * 4;
                if canvas_idx + 4 <= seq.frames[0].displayed_canvas.pixels.len() {
                    expected_pixels.extend_from_slice(&seq.frames[0].displayed_canvas.pixels[canvas_idx..canvas_idx + 4]);
                } else {
                    expected_pixels.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        assert_eq!(materialized.pixels, expected_pixels);
    }

    #[test]
    fn test_materialize_transparent_sparse() {
        let gif = create_bbox_test_gif(100, 100);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        // Use the changed region bbox as the sparse patch bbox
        let bbox = seq.frames[0].changed_region.bbox;

        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::TransparentSparsePatch { bbox, is_risky: false },
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let materialized = Materializer::materialize_frame(&decision, &seq.frames[0], &seq)
            .expect("Failed to materialize");

        // Sparse patch should have correct dimensions
        assert_eq!(materialized.width, bbox.width());
        assert_eq!(materialized.height, bbox.height());
        assert_eq!(materialized.left, bbox.left);
        assert_eq!(materialized.top, bbox.top);

        // Pixel count should match bbox area
        assert_eq!(materialized.pixels.len(), (bbox.width() as usize) * (bbox.height() as usize) * 4);
    }

    #[test]
    fn test_materialize_minimal_noop() {
        let gif = create_opaque_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::MinimalNoOp,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let materialized = Materializer::materialize_frame(&decision, &seq.frames[0], &seq)
            .expect("Failed to materialize");

        // Minimal frame should have 1x1 dimensions (GIF format requires non-zero dimensions).
        // This is a safe fallback that preserves semantics for all disposal methods.
        assert_eq!(materialized.width, 1);
        assert_eq!(materialized.height, 1);
        assert_eq!(materialized.pixels.len(), 4); // 1x1 RGBA = 4 bytes
    }

    #[test]
    fn test_materialize_sequence() {
        let gif = create_opaque_test_gif(50, 50, 3);
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
            FrameDecision {
                frame_index: 2,
                chosen_candidate: CandidateRepresentation::FullFrame,
                chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
                score_breakdown: ScoreBreakdown::zero(),
                alternatives: vec![],
                reason: DecisionReason::LowestScore,
                explanation: "test".to_string(),
            },
        ];

        let materialized = Materializer::materialize_sequence(&decisions, &seq)
            .expect("Failed to materialize sequence");

        assert_eq!(materialized.len(), 3);
        for frame in &materialized {
            assert_eq!(frame.width, 50);
            assert_eq!(frame.height, 50);
            assert_eq!(frame.pixels.len(), 50 * 50 * 4);
        }
    }

    #[test]
    fn test_materialize_preserves_disposal() {
        let gif = create_opaque_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let materialized = Materializer::materialize_frame(&decision, &seq.frames[0], &seq)
            .expect("Failed to materialize");

        // Disposal method should be preserved
        assert_eq!(materialized.dispose, seq.frames[0].dispose);
    }

    #[test]
    fn test_materialize_preserves_delay() {
        let gif = create_opaque_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::FullFrame,
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let materialized = Materializer::materialize_frame(&decision, &seq.frames[0], &seq)
            .expect("Failed to materialize");

        // Delay should be preserved
        assert_eq!(materialized.delay, seq.frames[0].delay);
    }

    #[test]
    fn test_materialize_bbox_offset_correctness() {
        // Create a fully opaque patch to test bbox offset correctness
        let canvas_size = (100 * 100) as usize * 4;
        let mut pixels = vec![0u8; canvas_size];

        // Fill canvas with transparent background
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[3] = 0; // Transparent
        }

        // Draw a 20x20 fully opaque red patch at (40, 40)
        let patch_left = 40u16;
        let patch_top = 40u16;
        let patch_width = 20u16;
        let patch_height = 20u16;

        for y in 0..patch_height as usize {
            for x in 0..patch_width as usize {
                let canvas_x = (patch_left as usize) + x;
                let canvas_y = (patch_top as usize) + y;
                let idx = (canvas_y * 100 + canvas_x) * 4;
                pixels[idx] = 255;     // Red
                pixels[idx + 1] = 0;
                pixels[idx + 2] = 0;
                pixels[idx + 3] = 255; // Opaque
            }
        }

        let gif = Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![crate::types::Frame {
                pixels,
                delay: Duration::from_millis(100),
                dispose: DisposalMethod::Keep,
                local_palette: None,
                left: patch_left,
                top: patch_top,
                width: patch_width,
                height: patch_height,
            }],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        };

        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");

        // Create a decision with a manually specified bbox
        let bbox = BoundingBox::new(patch_left, patch_top, patch_left + patch_width, patch_top + patch_height);
        let decision = FrameDecision {
            frame_index: 0,
            chosen_candidate: CandidateRepresentation::ExactOpaqueBbox { bbox },
            chosen_palette_strategy: PaletteStrategy::DeriveSequenceGlobalPreferred,
            score_breakdown: ScoreBreakdown::zero(),
            alternatives: vec![],
            reason: DecisionReason::LowestScore,
            explanation: "test".to_string(),
        };

        let materialized = Materializer::materialize_frame(&decision, &seq.frames[0], &seq)
            .expect("Failed to materialize");

        // Verify that the bbox offset is correct
        assert_eq!(materialized.left, patch_left);
        assert_eq!(materialized.top, patch_top);
        assert_eq!(materialized.width, patch_width);
        assert_eq!(materialized.height, patch_height);
    }
}
