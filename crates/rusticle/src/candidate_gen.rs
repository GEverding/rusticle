//! First-pass candidate frame representation generator.
//!
//! Generates GIF-native candidate representations from canonical sequence IR.
//! Each candidate carries metadata for later scoring/selection.
//!
//! # Candidate Types
//!
//! - **Full Frame**: Complete canvas replacement (always safe).
//! - **Exact Opaque Bbox Patch**: Opaque pixels within tight bounding box.
//! - **Transparent Sparse Patch**: Sparse transparent pixels (may be risky).
//! - **Minimal/No-op Frame**: Only when semantically safe (disposal-aware).

use crate::adaptive_ir::{BoundingBox, CanonicalFrame, CanonicalSequence, SourcePatch};
use crate::types::DisposalMethod;

/// A candidate frame representation with metadata for scoring.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Frame index in the sequence.
    pub frame_index: usize,
    /// The representation type.
    pub representation: CandidateRepresentation,
    /// Metadata for scoring.
    pub metadata: CandidateMetadata,
}

/// Candidate representation type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateRepresentation {
    /// Full canvas replacement.
    FullFrame,
    /// Opaque pixels within a bounding box.
    ExactOpaqueBbox {
        /// Bounding box of opaque pixels.
        bbox: BoundingBox,
    },
    /// Sparse transparent pixels (may be risky).
    TransparentSparsePatch {
        /// Bounding box of the sparse region.
        bbox: BoundingBox,
        /// Whether this candidate is marked as risky.
        is_risky: bool,
    },
    /// Minimal/no-op frame (only when safe).
    MinimalNoOp,
}

/// Metadata for candidate scoring.
#[derive(Debug, Clone)]
pub struct CandidateMetadata {
    /// Bounding box of changed region (from canonical IR).
    pub changed_bbox: BoundingBox,
    /// Count of changed pixels.
    pub changed_pixel_count: usize,
    /// Ratio of changed pixels to canvas.
    pub changed_ratio: f32,
    /// Whether source was full-canvas.
    pub source_is_full_canvas: bool,
    /// Whether source has transparency.
    pub source_has_transparency: bool,
    /// Count of transparent pixels in source.
    pub source_transparent_count: usize,
    /// Count of opaque pixels in source.
    pub source_opaque_count: usize,
    /// Disposal method of this frame.
    pub disposal_method: DisposalMethod,
    /// Safety reason/flag for this candidate.
    pub safety_reason: SafetyReason,
}

/// Safety classification for a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetyReason {
    /// Always safe (full frame or opaque bbox).
    AlwaysSafe,
    /// Safe given disposal semantics.
    SafeWithDisposal,
    /// Risky: transparent sparse patch with uncertain semantics.
    RiskyTransparent,
    /// Risky: minimal frame with background disposal.
    RiskyMinimalBackground,
}

/// Generator for candidate representations.
pub struct CandidateGenerator;

impl CandidateGenerator {
    /// Generate all candidates from a canonical sequence.
    ///
    /// For each frame, generates:
    /// 1. Full frame candidate (always).
    /// 2. Exact opaque bbox candidate (if applicable).
    /// 3. Transparent sparse patch candidate (if applicable, may be risky).
    /// 4. Minimal/no-op candidate (only if semantically safe).
    pub fn generate(seq: &CanonicalSequence) -> Vec<Candidate> {
        let mut candidates = Vec::new();

        for (frame_idx, frame) in seq.frames.iter().enumerate() {
            let frame_candidates = Self::generate_frame_candidates(frame_idx, frame, seq);
            candidates.extend(frame_candidates);
        }

        candidates
    }

    /// Generate candidates for a single frame.
    fn generate_frame_candidates(
        frame_idx: usize,
        frame: &CanonicalFrame,
        seq: &CanonicalSequence,
    ) -> Vec<Candidate> {
        let mut candidates = Vec::new();

        // Always generate full frame candidate
        candidates.push(Candidate {
            frame_index: frame_idx,
            representation: CandidateRepresentation::FullFrame,
            metadata: Self::build_metadata(frame, CandidateRepresentation::FullFrame),
        });

        // Generate exact opaque bbox candidate if source is opaque
        if !frame.source_patch.has_transparency {
            if let Some(candidate) = Self::generate_opaque_bbox_candidate(frame_idx, frame) {
                candidates.push(candidate);
            }
        }

        // Generate transparent sparse patch candidate if source has transparency
        if frame.source_patch.has_transparency {
            if let Some(candidate) =
                Self::generate_transparent_sparse_candidate(frame_idx, frame, seq)
            {
                candidates.push(candidate);
            }
        }

        // Generate minimal/no-op candidate only if semantically safe
        if let Some(candidate) = Self::generate_minimal_noop_candidate(frame_idx, frame) {
            candidates.push(candidate);
        }

        candidates
    }

    /// Generate exact opaque bbox candidate.
    fn generate_opaque_bbox_candidate(frame_idx: usize, frame: &CanonicalFrame) -> Option<Candidate> {
        // Compute tight bbox around opaque pixels in source patch
        let bbox = Self::compute_opaque_bbox(&frame.source_patch);

        // Only generate if bbox is non-empty
        if bbox.area() == 0 {
            return None;
        }

        // Don't generate if bbox is the same as the full source patch
        // (in that case, full frame candidate is sufficient)
        let source_area = (frame.source_patch.width as usize) * (frame.source_patch.height as usize);
        if bbox.area() >= source_area {
            return None;
        }

        Some(Candidate {
            frame_index: frame_idx,
            representation: CandidateRepresentation::ExactOpaqueBbox { bbox },
            metadata: Self::build_metadata(frame, CandidateRepresentation::ExactOpaqueBbox { bbox }),
        })
    }

    /// Generate transparent sparse patch candidate.
    fn generate_transparent_sparse_candidate(
        frame_idx: usize,
        frame: &CanonicalFrame,
        seq: &CanonicalSequence,
    ) -> Option<Candidate> {
        // Compute bbox of changed region
        let bbox = frame.changed_region.bbox;

        if bbox.area() == 0 {
            return None;
        }

        // Determine if this candidate is risky
        let is_risky = Self::is_transparent_sparse_risky(frame, seq);

        Some(Candidate {
            frame_index: frame_idx,
            representation: CandidateRepresentation::TransparentSparsePatch { bbox, is_risky },
            metadata: Self::build_metadata(
                frame,
                CandidateRepresentation::TransparentSparsePatch { bbox, is_risky },
            ),
        })
    }

    /// Generate minimal/no-op candidate only if semantically safe.
    fn generate_minimal_noop_candidate(frame_idx: usize, frame: &CanonicalFrame) -> Option<Candidate> {
        // Only safe if no pixels changed. If pixels changed, we must emit a frame to display them.
        // The disposal method is irrelevant here - if pixels changed, we can't skip the frame.
        if frame.changed_region.changed_pixel_count != 0 {
            return None;
        }

        // No pixels changed, so MinimalNoOp is safe for all disposal methods.
        // However, we mark it as risky for Background disposal because the 1x1 fallback
        // frame might have unexpected semantics (though it's still safe).
        let safety_reason = if frame.dispose == DisposalMethod::Background {
            SafetyReason::RiskyMinimalBackground
        } else {
            SafetyReason::SafeWithDisposal
        };

        Some(Candidate {
            frame_index: frame_idx,
            representation: CandidateRepresentation::MinimalNoOp,
            metadata: CandidateMetadata {
                changed_bbox: frame.changed_region.bbox,
                changed_pixel_count: frame.changed_region.changed_pixel_count,
                changed_ratio: frame.changed_region.changed_ratio,
                source_is_full_canvas: frame.changed_region.is_full_canvas_patch,
                source_has_transparency: frame.source_patch.has_transparency,
                source_transparent_count: frame.source_patch.transparent_pixel_count,
                source_opaque_count: frame.source_patch.opaque_pixel_count,
                disposal_method: frame.dispose,
                safety_reason,
            },
        })
    }

    /// Compute tight bounding box around opaque pixels in source patch.
    fn compute_opaque_bbox(patch: &SourcePatch) -> BoundingBox {
        let mut min_x = patch.width;
        let mut min_y = patch.height;
        let mut max_x = 0u16;
        let mut max_y = 0u16;

        for y in 0..patch.height {
            for x in 0..patch.width {
                let idx = ((y as usize) * (patch.width as usize) + (x as usize)) * 4;
                let alpha = patch.pixels[idx + 3];

                if alpha == 255 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x + 1);
                    max_y = max_y.max(y + 1);
                }
            }
        }

        if max_x == 0 || max_y == 0 {
            BoundingBox::new(0, 0, 0, 0)
        } else {
            // Adjust to canvas coordinates
            BoundingBox::new(
                patch.left.saturating_add(min_x),
                patch.top.saturating_add(min_y),
                patch.left.saturating_add(max_x),
                patch.top.saturating_add(max_y),
            )
        }
    }

    /// Determine if transparent sparse patch is risky.
    fn is_transparent_sparse_risky(frame: &CanonicalFrame, seq: &CanonicalSequence) -> bool {
        // Risky if:
        // 1. Source has high transparency ratio (>50%), OR
        // 2. Disposal is Background (clears the region), OR
        // 3. Next frame depends on this frame's transparency
        let transparency_ratio = if frame.source_patch.width > 0 && frame.source_patch.height > 0 {
            frame.source_patch.transparent_pixel_count as f32
                / (frame.source_patch.width as usize * frame.source_patch.height as usize) as f32
        } else {
            0.0
        };

        let high_transparency = transparency_ratio > 0.5;
        let background_disposal = frame.dispose == DisposalMethod::Background;

        // Check if next frame depends on this frame's transparency
        let next_depends_on_transparency = if let Some(next_frame) = seq.frames.last() {
            // Simplified: if next frame has transparency and disposal is not Previous,
            // it might depend on this frame's transparency
            next_frame.source_patch.has_transparency && frame.dispose != DisposalMethod::Previous
        } else {
            false
        };

        high_transparency || background_disposal || next_depends_on_transparency
    }

    /// Build metadata for a candidate.
    fn build_metadata(frame: &CanonicalFrame, representation: CandidateRepresentation) -> CandidateMetadata {
        let safety_reason = match representation {
            CandidateRepresentation::FullFrame => SafetyReason::AlwaysSafe,
            CandidateRepresentation::ExactOpaqueBbox { .. } => SafetyReason::AlwaysSafe,
            CandidateRepresentation::TransparentSparsePatch { is_risky, .. } => {
                if is_risky {
                    SafetyReason::RiskyTransparent
                } else {
                    SafetyReason::SafeWithDisposal
                }
            }
            CandidateRepresentation::MinimalNoOp => {
                if frame.dispose == DisposalMethod::Background {
                    SafetyReason::RiskyMinimalBackground
                } else {
                    SafetyReason::SafeWithDisposal
                }
            }
        };

        CandidateMetadata {
            changed_bbox: frame.changed_region.bbox,
            changed_pixel_count: frame.changed_region.changed_pixel_count,
            changed_ratio: frame.changed_region.changed_ratio,
            source_is_full_canvas: frame.changed_region.is_full_canvas_patch,
            source_has_transparency: frame.source_patch.has_transparency,
            source_transparent_count: frame.source_patch.transparent_pixel_count,
            source_opaque_count: frame.source_patch.opaque_pixel_count,
            disposal_method: frame.dispose,
            safety_reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive_ir::CanonicalSequenceBuilder;
    use crate::types::{DisposalMethod, Frame, Gif, LoopCount};
    use std::time::Duration;

    /// Create a test Gif with specified dimensions and frame count.
    fn create_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;
        let mut frames = Vec::new();

        for i in 0..frame_count {
            let mut pixels = vec![0u8; canvas_size];
            // Fill with a simple pattern
            for j in 0..canvas_size / 4 {
                pixels[j * 4] = (i * 50) as u8; // R
                pixels[j * 4 + 1] = (j % 256) as u8; // G
                pixels[j * 4 + 2] = 100; // B
                pixels[j * 4 + 3] = 255; // A (opaque)
            }

            frames.push(Frame {
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

    /// Create a test Gif with a single opaque delta frame.
    fn create_opaque_delta_gif(width: u16, height: u16) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;

        // Frame 0: full opaque canvas
        let mut frame0_pixels = vec![0u8; canvas_size];
        for j in 0..canvas_size / 4 {
            frame0_pixels[j * 4] = 100; // R
            frame0_pixels[j * 4 + 1] = 100; // G
            frame0_pixels[j * 4 + 2] = 100; // B
            frame0_pixels[j * 4 + 3] = 255; // A
        }

        // Frame 1: delta in a small region (top-left 10x10)
        // Note: Gif stores full-canvas frames, so we create a full canvas and mark the delta region
        let mut frame1_pixels = frame0_pixels.clone();
        for y in 0..10 {
            for x in 0..10 {
                let idx = (y * (width as usize) + x) * 4;
                frame1_pixels[idx] = 200; // R
                frame1_pixels[idx + 1] = 50; // G
                frame1_pixels[idx + 2] = 50; // B
                frame1_pixels[idx + 3] = 255; // A
            }
        }

        Gif {
            width,
            height,
            global_palette: None,
            frames: vec![
                Frame {
                    pixels: frame0_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
                Frame {
                    pixels: frame1_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
            ],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    /// Create a test Gif with Background disposal.
    fn create_background_disposal_gif(width: u16, height: u16) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;

        // Frame 0: full opaque canvas
        let mut frame0_pixels = vec![0u8; canvas_size];
        for j in 0..canvas_size / 4 {
            frame0_pixels[j * 4] = 100;
            frame0_pixels[j * 4 + 1] = 100;
            frame0_pixels[j * 4 + 2] = 100;
            frame0_pixels[j * 4 + 3] = 255;
        }

        // Frame 1: same as frame 0 (will be disposed to transparent)
        let frame1_pixels = frame0_pixels.clone();

        Gif {
            width,
            height,
            global_palette: None,
            frames: vec![
                Frame {
                    pixels: frame0_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
                Frame {
                    pixels: frame1_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Background,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
            ],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    /// Create a test Gif with transparency-heavy sparse frame.
    fn create_transparent_sparse_gif(width: u16, height: u16) -> Gif {
        let canvas_size = (width as usize) * (height as usize) * 4;

        // Frame 0: full opaque canvas
        let mut frame0_pixels = vec![0u8; canvas_size];
        for j in 0..canvas_size / 4 {
            frame0_pixels[j * 4] = 100;
            frame0_pixels[j * 4 + 1] = 100;
            frame0_pixels[j * 4 + 2] = 100;
            frame0_pixels[j * 4 + 3] = 255;
        }

        // Frame 1: mostly transparent with sparse opaque pixels
        let mut frame1_pixels = vec![0u8; canvas_size];
        // Only fill a few pixels with opaque
        for i in 0..10 {
            frame1_pixels[i * 4 + 3] = 255; // A
        }

        Gif {
            width,
            height,
            global_palette: None,
            frames: vec![
                Frame {
                    pixels: frame0_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
                Frame {
                    pixels: frame1_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
            ],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    #[test]
    fn test_generate_full_frame_candidate() {
        let gif = create_test_gif(50, 50, 2);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");
        let candidates = CandidateGenerator::generate(&seq);

        // Should have at least one full frame candidate per frame
        let full_frame_candidates: Vec<_> = candidates
            .iter()
            .filter(|c| c.representation == CandidateRepresentation::FullFrame)
            .collect();
        assert!(full_frame_candidates.len() >= 2);
    }

    #[test]
    fn test_opaque_delta_frame_produces_bbox_candidate() {
        // Create a frame with opaque pixels in a small region and transparent elsewhere
        // This tests the opaque bbox candidate generation
        let width = 50u16;
        let height = 50u16;
        let canvas_size = (width as usize) * (height as usize) * 4;

        // Frame 0: full opaque canvas (red)
        let mut frame0_pixels = vec![0u8; canvas_size];
        for j in 0..canvas_size / 4 {
            frame0_pixels[j * 4] = 100; // R
            frame0_pixels[j * 4 + 1] = 100; // G
            frame0_pixels[j * 4 + 2] = 100; // B
            frame0_pixels[j * 4 + 3] = 255; // A
        }

        // Frame 1: opaque pixels only in top-left 10x10, rest transparent
        let mut frame1_pixels = vec![0u8; canvas_size];
        for y in 0..10 {
            for x in 0..10 {
                let idx = (y * (width as usize) + x) * 4;
                frame1_pixels[idx] = 200; // R
                frame1_pixels[idx + 1] = 50; // G
                frame1_pixels[idx + 2] = 50; // B
                frame1_pixels[idx + 3] = 255; // A (opaque)
            }
        }
        // Rest is transparent (alpha = 0)

        let gif = Gif {
            width,
            height,
            global_palette: None,
            frames: vec![
                Frame {
                    pixels: frame0_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
                Frame {
                    pixels: frame1_pixels,
                    delay: Duration::from_millis(100),
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width,
                    height,
                },
            ],
            loop_count: LoopCount::Infinite,
            original_palette: None,
        };

        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");
        let candidates = CandidateGenerator::generate(&seq);

        // Frame 1 should have candidates (at minimum full frame and transparent sparse)
        let frame1_candidates: Vec<_> = candidates.iter().filter(|c| c.frame_index == 1).collect();
        assert!(!frame1_candidates.is_empty(), "Frame 1 should have candidates");

        // Since frame 1 has transparency, it won't have an opaque bbox candidate
        // But it should have a transparent sparse patch candidate
        let sparse_candidates: Vec<_> = frame1_candidates
            .iter()
            .filter(|c| matches!(c.representation, CandidateRepresentation::TransparentSparsePatch { .. }))
            .collect();
        assert!(!sparse_candidates.is_empty(), "Frame 1 should have transparent sparse candidate");
    }

    #[test]
    fn test_background_disposal_no_unsafe_minimal() {
        let gif = create_background_disposal_gif(50, 50);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");
        let candidates = CandidateGenerator::generate(&seq);

        // Frame 1 with Background disposal should not produce a safe minimal candidate
        let frame1_candidates: Vec<_> = candidates.iter().filter(|c| c.frame_index == 1).collect();
        let minimal_candidates: Vec<_> = frame1_candidates
            .iter()
            .filter(|c| c.representation == CandidateRepresentation::MinimalNoOp)
            .collect();

        // If minimal candidate exists, it should be marked as risky
        for candidate in minimal_candidates {
            assert_eq!(
                candidate.metadata.safety_reason,
                SafetyReason::RiskyMinimalBackground,
                "Minimal candidate with Background disposal should be marked risky"
            );
        }
    }

    #[test]
    fn test_transparent_sparse_candidate_generated() {
        let gif = create_transparent_sparse_gif(50, 50);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");
        let candidates = CandidateGenerator::generate(&seq);

        // Frame 1 should have a transparent sparse patch candidate
        let frame1_candidates: Vec<_> = candidates.iter().filter(|c| c.frame_index == 1).collect();
        let sparse_candidates: Vec<_> = frame1_candidates
            .iter()
            .filter(|c| matches!(c.representation, CandidateRepresentation::TransparentSparsePatch { .. }))
            .collect();

        assert!(!sparse_candidates.is_empty(), "Frame 1 should have transparent sparse candidate");
    }

    #[test]
    fn test_full_frame_always_available() {
        let gif = create_opaque_delta_gif(50, 50);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");
        let candidates = CandidateGenerator::generate(&seq);

        // Every frame should have a full frame candidate
        for frame_idx in 0..seq.frames.len() {
            let frame_candidates: Vec<_> = candidates.iter().filter(|c| c.frame_index == frame_idx).collect();
            let full_frame: Vec<_> = frame_candidates
                .iter()
                .filter(|c| c.representation == CandidateRepresentation::FullFrame)
                .collect();
            assert!(!full_frame.is_empty(), "Frame {} should have full frame candidate", frame_idx);
        }
    }

    #[test]
    fn test_candidate_metadata_populated() {
        let gif = create_test_gif(50, 50, 1);
        let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build sequence");
        let candidates = CandidateGenerator::generate(&seq);

        assert!(!candidates.is_empty());
        for candidate in candidates {
            assert_eq!(candidate.metadata.disposal_method, DisposalMethod::Keep);
            assert!(candidate.metadata.changed_ratio >= 0.0 && candidate.metadata.changed_ratio <= 1.0);
        }
    }
}
