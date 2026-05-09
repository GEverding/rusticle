//! Voyager-class representation study: source-global-reuse candidate.
//!
//! **STATUS**: Research / Future Opt-In (not current mainline product path)
//!
//! This module implements candidate #2 for the bounded representation study (epic `rusticle-502`):
//!
//! - Takes resized displayed canvases + source GIF as input
//! - Computes **exact opaque bbox patches** between consecutive resized displayed frames
//! - Reuses the **source GIF's global palette** directly (no derivation)
//! - Quantizes all frames against that source global palette
//! - Preserves frame delay/disposal/geometry correctly
//! - **Fails explicitly** if source has no global palette (not a silent fallback)
//!
//! # Design
//!
//! This candidate is intentionally strict: it tests whether source palette reuse avoids
//! quantization quality loss enough to offset the palette mismatch from resize interpolation.
//! If the source has no global palette, the candidate is marked as "not viable" rather than
//! silently deriving a new one.
//!
//! # What It Was Trying to Solve
//!
//! The voyager study explored whether reusing the source GIF's palette (avoiding re-quantization)
//! could improve quality. This candidate tests that hypothesis directly.
//!
//! # Structural Assumptions
//!
//! - Input is resized displayed canvases (RGBA, already composited correctly) + source GIF
//! - Source GIF must have a global palette (candidate fails explicitly if not)
//! - Output is quantized frame data (palette indices) ready for GIF encoding
//! - No synthetic transparency is introduced
//! - Frame timing and disposal are preserved exactly
//!
//! # Latest Evidence
//!
//! Source palette reuse is too strict (fails when source has no global palette) and doesn't
//! generalize well. Validation on a larger, more diverse GIF corpus is needed before promoting
//! any voyager candidate to mainline.
//!
//! # Output
//!
//! Produces a `VoyagerSourceReuseRepr` containing:
//! - Global palette (reused from source)
//! - Per-frame quantized data (bbox patches or full-frame, indices, metadata)
//! - Frame timing and disposal preserved
//! - Viability flag indicating whether source palette reuse was possible
//!
//! See `docs/research/RESEARCH_VOYAGER_AND_TWO_PATH.md` for full context.

use crate::error::Result;
use crate::gif_ops::{compute_changed_bbox, extract_bbox_region, find_transparent_index_and_remap};
use crate::palette_lut::PaletteLut;
use crate::types::{DisposalMethod, Frame, Gif};
use std::time::Duration;

/// Viability result for source-global-reuse candidate.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceReuseViability {
    /// Source palette reuse was successful.
    Viable,
    /// Source has no global palette; candidate not applicable.
    NoSourceGlobalPalette,
    /// Source palette is too small or incompatible.
    IncompatiblePalette,
    /// Candidate evaluation failed.
    Failed,
}

impl SourceReuseViability {
    /// Human-readable name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Viable => "viable",
            Self::NoSourceGlobalPalette => "no_source_palette",
            Self::IncompatiblePalette => "incompatible_palette",
            Self::Failed => "failed",
        }
    }
}

/// Voyager representation: source-global-reuse candidate output.
#[derive(Debug, Clone)]
pub struct VoyagerSourceReuseRepr {
    /// Canvas width in pixels.
    pub width: u16,
    /// Canvas height in pixels.
    pub height: u16,
    /// Reused source global palette (flat RGB: [r0,g0,b0,r1,g1,b1,...]).
    pub global_palette: Vec<u8>,
    /// Per-frame quantized data.
    pub frames: Vec<VoyagerSourceReuseFrame>,
    /// Viability of this candidate.
    pub viability: SourceReuseViability,
}

/// A single frame in source-reuse representation.
#[derive(Debug, Clone)]
pub struct VoyagerSourceReuseFrame {
    /// Palette indices (one per pixel).
    pub indices: Vec<u8>,
    /// Transparent index for this frame (if any).
    pub transparent_idx: Option<u8>,
    /// Frame delay.
    pub delay: Duration,
    /// Frame disposal method.
    pub dispose: DisposalMethod,
    /// Frame geometry (bbox patch or full-canvas).
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
}

/// Voyager source-reuse representation builder.
pub struct VoyagerSourceReuseBuilder;

impl VoyagerSourceReuseBuilder {
    /// Build voyager source-reuse representation from resized displayed canvases and source GIF.
    ///
    /// # Arguments
    ///
    /// - `resized_frames`: RGBA frames (already resized to display dimensions).
    /// - `canvas_width`: Canvas width in pixels.
    /// - `canvas_height`: Canvas height in pixels.
    /// - `source_gif`: Original GIF (for global palette extraction).
    ///
    /// # Returns
    ///
    /// A `VoyagerSourceReuseRepr` with bbox patches and source global palette.
    /// If source has no global palette, returns with `viability = NoSourceGlobalPalette`.
    ///
    /// # Errors
    ///
    /// Returns an error if quantization fails (not if source palette is missing).
    pub fn build(
        resized_frames: &[Frame],
        canvas_width: u16,
        canvas_height: u16,
        source_gif: &Gif,
    ) -> Result<VoyagerSourceReuseRepr> {
        // Step 1: Check if source has global palette
        let source_palette = match &source_gif.global_palette {
            Some(palette) => palette,
            None => {
                // Source has no global palette: candidate not viable
                return Ok(VoyagerSourceReuseRepr {
                    width: canvas_width,
                    height: canvas_height,
                    global_palette: vec![0, 0, 0], // Minimal palette
                    frames: vec![],
                    viability: SourceReuseViability::NoSourceGlobalPalette,
                });
            }
        };

        // Step 2: Validate source palette
        if source_palette.colors.is_empty() || source_palette.colors.len() > 256 {
            return Ok(VoyagerSourceReuseRepr {
                width: canvas_width,
                height: canvas_height,
                global_palette: vec![0, 0, 0],
                frames: vec![],
                viability: SourceReuseViability::IncompatiblePalette,
            });
        }

        // Step 3: Convert source palette to flat RGB
        let global_palette_flat = Self::palette_to_flat_rgb(&source_palette.colors);

        // Step 4: Build LUT from source palette
        let lut = PaletteLut::new(&source_palette.colors);

        // Step 5: Compute exact changed bboxes and quantize frames
        let frames = if resized_frames.is_empty() {
            vec![]
        } else {
            Self::build_frames_with_bboxes(resized_frames, canvas_width, canvas_height, &lut)?
        };

        Ok(VoyagerSourceReuseRepr {
            width: canvas_width,
            height: canvas_height,
            global_palette: global_palette_flat,
            frames,
            viability: SourceReuseViability::Viable,
        })
    }

    /// Build frames with exact opaque bbox patches.
    fn build_frames_with_bboxes(
        resized_frames: &[Frame],
        canvas_width: u16,
        canvas_height: u16,
        lut: &PaletteLut,
    ) -> Result<Vec<VoyagerSourceReuseFrame>> {
        if resized_frames.is_empty() {
            return Ok(vec![]);
        }

        let mut frames = Vec::new();

        // Frame 0: always full-frame
        let frame0 =
            Self::quantize_frame(&resized_frames[0], 0, 0, canvas_width, canvas_height, lut)?;
        frames.push(frame0);

        // Subsequent frames: compute changed bbox and emit patches
        for i in 1..resized_frames.len() {
            let prev_frame = &resized_frames[i - 1];
            let curr_frame = &resized_frames[i];

            // Compute exact changed bbox
            let bbox = compute_changed_bbox(
                &prev_frame.pixels,
                &curr_frame.pixels,
                canvas_width as usize,
                canvas_height as usize,
            );

            let frame = if bbox.area() == 0 {
                // Identical frames: emit 1x1 minimal patch at (0,0)
                let pixel_at_origin = curr_frame.pixels[0..4].to_vec();
                let (mut indices, _) = lut.map_buffer(&pixel_at_origin);
                let mut palette_rgb: Vec<u8> = lut
                    .palette()
                    .iter()
                    .flat_map(|color| color.iter().copied())
                    .collect();
                let transparent_idx = find_transparent_index_and_remap(
                    &pixel_at_origin,
                    &mut indices,
                    &mut palette_rgb,
                );

                VoyagerSourceReuseFrame {
                    indices,
                    transparent_idx,
                    delay: curr_frame.delay,
                    dispose: curr_frame.dispose,
                    left: 0,
                    top: 0,
                    width: 1,
                    height: 1,
                }
            } else {
                // Extract bbox region and quantize
                let bbox_pixels =
                    extract_bbox_region(&curr_frame.pixels, canvas_width as usize, &bbox);
                let (mut indices, _) = lut.map_buffer(&bbox_pixels);
                let mut palette_rgb: Vec<u8> = lut
                    .palette()
                    .iter()
                    .flat_map(|color| color.iter().copied())
                    .collect();
                let transparent_idx =
                    find_transparent_index_and_remap(&bbox_pixels, &mut indices, &mut palette_rgb);

                VoyagerSourceReuseFrame {
                    indices,
                    transparent_idx,
                    delay: curr_frame.delay,
                    dispose: curr_frame.dispose,
                    left: bbox.left,
                    top: bbox.top,
                    width: bbox.width(),
                    height: bbox.height(),
                }
            };

            frames.push(frame);
        }

        Ok(frames)
    }

    /// Quantize a single frame using the source palette LUT.
    fn quantize_frame(
        frame: &Frame,
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        lut: &PaletteLut,
    ) -> Result<VoyagerSourceReuseFrame> {
        if frame.pixels.is_empty() {
            return Ok(VoyagerSourceReuseFrame {
                indices: vec![],
                transparent_idx: None,
                delay: frame.delay,
                dispose: frame.dispose,
                left,
                top,
                width,
                height,
            });
        }

        let (mut indices, _) = lut.map_buffer(&frame.pixels);
        let mut palette_rgb: Vec<u8> = lut
            .palette()
            .iter()
            .flat_map(|color| color.iter().copied())
            .collect();
        let transparent_idx =
            find_transparent_index_and_remap(&frame.pixels, &mut indices, &mut palette_rgb);

        Ok(VoyagerSourceReuseFrame {
            indices,
            transparent_idx,
            delay: frame.delay,
            dispose: frame.dispose,
            left,
            top,
            width,
            height,
        })
    }

    /// Convert palette to flat RGB format.
    fn palette_to_flat_rgb(colors: &[[u8; 3]]) -> Vec<u8> {
        let mut flat = Vec::with_capacity(colors.len() * 3);
        for color in colors {
            flat.push(color[0]);
            flat.push(color[1]);
            flat.push(color[2]);
        }
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Palette;

    /// Create a simple test frame with opaque pixels.
    fn create_opaque_frame(width: u16, height: u16, color: [u8; 3]) -> Frame {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[0] = color[0];
            chunk[1] = color[1];
            chunk[2] = color[2];
            chunk[3] = 255; // Opaque
        }

        Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        }
    }

    /// Create a test frame with transparency.
    fn create_transparent_frame(width: u16, height: u16) -> Frame {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        for (i, chunk) in pixels.chunks_exact_mut(4).enumerate() {
            chunk[0] = 255; // Red
            chunk[1] = 0;
            chunk[2] = 0;
            chunk[3] = if i % 2 == 0 { 0 } else { 255 };
        }

        Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        }
    }

    /// Create a test GIF with a source global palette.
    fn create_test_gif_with_palette(palette_colors: Vec<[u8; 3]>) -> Gif {
        Gif {
            width: 100,
            height: 100,
            global_palette: Some(Palette {
                colors: palette_colors,
            }),
            frames: vec![],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        }
    }

    /// Create a test GIF without global palette.
    fn create_test_gif_no_palette() -> Gif {
        Gif {
            width: 100,
            height: 100,
            global_palette: None,
            frames: vec![],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        }
    }

    #[test]
    fn test_source_palette_reuse_viable() {
        let frame = create_opaque_frame(50, 50, [255, 0, 0]);
        let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let source_gif = create_test_gif_with_palette(palette);

        let repr =
            VoyagerSourceReuseBuilder::build(&[frame], 50, 50, &source_gif).expect("build failed");

        assert_eq!(repr.viability, SourceReuseViability::Viable);
        assert_eq!(repr.width, 50);
        assert_eq!(repr.height, 50);
        assert!(!repr.global_palette.is_empty());
        assert_eq!(repr.frames.len(), 1);
    }

    #[test]
    fn test_no_source_global_palette() {
        let frame = create_opaque_frame(50, 50, [255, 0, 0]);
        let source_gif = create_test_gif_no_palette();

        let repr =
            VoyagerSourceReuseBuilder::build(&[frame], 50, 50, &source_gif).expect("build failed");

        assert_eq!(repr.viability, SourceReuseViability::NoSourceGlobalPalette);
        assert_eq!(repr.frames.len(), 0);
    }

    #[test]
    fn test_exact_bbox_geometry_preserved() {
        let frame0 = create_opaque_frame(100, 100, [200, 200, 200]);
        let mut frame1 = create_opaque_frame(100, 100, [200, 200, 200]);

        // Create a small change in frame1 at (10, 10) with size 20x20
        for y in 10..30 {
            for x in 10..30 {
                let idx = (y * 100 + x) * 4;
                frame1.pixels[idx] = 100;
                frame1.pixels[idx + 1] = 100;
                frame1.pixels[idx + 2] = 100;
            }
        }

        let palette = vec![[200, 200, 200], [100, 100, 100]];
        let source_gif = create_test_gif_with_palette(palette);

        let repr = VoyagerSourceReuseBuilder::build(&[frame0, frame1], 100, 100, &source_gif)
            .expect("build failed");

        assert_eq!(repr.viability, SourceReuseViability::Viable);
        assert_eq!(repr.frames.len(), 2);

        // Frame 0 should be full-frame
        assert_eq!(repr.frames[0].left, 0);
        assert_eq!(repr.frames[0].top, 0);
        assert_eq!(repr.frames[0].width, 100);
        assert_eq!(repr.frames[0].height, 100);

        // Frame 1 should be bbox patch (20x20 at 10,10)
        assert_eq!(repr.frames[1].left, 10);
        assert_eq!(repr.frames[1].top, 10);
        assert_eq!(repr.frames[1].width, 20);
        assert_eq!(repr.frames[1].height, 20);
    }

    #[test]
    fn test_no_local_palette_churn() {
        let frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
        let frame2 = create_opaque_frame(50, 50, [0, 255, 0]);

        let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let source_gif = create_test_gif_with_palette(palette);

        let repr = VoyagerSourceReuseBuilder::build(&[frame1, frame2], 50, 50, &source_gif)
            .expect("build failed");

        assert_eq!(repr.viability, SourceReuseViability::Viable);
        // All frames should use global palette (no local_palette field in VoyagerSourceReuseFrame)
        assert_eq!(repr.frames.len(), 2);
    }

    #[test]
    fn test_identical_consecutive_frames() {
        let frame0 = create_opaque_frame(50, 50, [255, 0, 0]);
        let frame1 = frame0.clone();

        let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let source_gif = create_test_gif_with_palette(palette);

        let repr = VoyagerSourceReuseBuilder::build(&[frame0, frame1], 50, 50, &source_gif)
            .expect("build failed");

        assert_eq!(repr.viability, SourceReuseViability::Viable);
        assert_eq!(repr.frames.len(), 2);

        // Frame 1 should be 1x1 minimal patch
        assert_eq!(repr.frames[1].width, 1);
        assert_eq!(repr.frames[1].height, 1);
    }

    #[test]
    fn test_transparent_pixels_handled() {
        let frame = create_transparent_frame(50, 50);
        let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let source_gif = create_test_gif_with_palette(palette);

        let repr =
            VoyagerSourceReuseBuilder::build(&[frame], 50, 50, &source_gif).expect("build failed");

        assert_eq!(repr.viability, SourceReuseViability::Viable);
        assert_eq!(repr.frames.len(), 1);
        // Should have a transparent index assigned
        assert!(repr.frames[0].transparent_idx.is_some());
    }

    #[test]
    fn test_delays_and_disposal_preserved() {
        let mut frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
        frame1.delay = Duration::from_millis(200);
        frame1.dispose = DisposalMethod::Background;

        let mut frame2 = create_opaque_frame(50, 50, [0, 255, 0]);
        frame2.delay = Duration::from_millis(300);
        frame2.dispose = DisposalMethod::Previous;

        let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let source_gif = create_test_gif_with_palette(palette);

        let repr = VoyagerSourceReuseBuilder::build(&[frame1, frame2], 50, 50, &source_gif)
            .expect("build failed");

        assert_eq!(repr.frames[0].delay, Duration::from_millis(200));
        assert_eq!(repr.frames[0].dispose, DisposalMethod::Background);
        assert_eq!(repr.frames[1].delay, Duration::from_millis(300));
        assert_eq!(repr.frames[1].dispose, DisposalMethod::Previous);
    }

    #[test]
    fn test_empty_sequence() {
        let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let source_gif = create_test_gif_with_palette(palette);

        let repr =
            VoyagerSourceReuseBuilder::build(&[], 100, 100, &source_gif).expect("build failed");

        assert_eq!(repr.viability, SourceReuseViability::Viable);
        assert_eq!(repr.frames.len(), 0);
    }

    #[test]
    fn test_output_decodable_frame_count() {
        let frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
        let frame2 = create_opaque_frame(50, 50, [0, 255, 0]);
        let frame3 = create_opaque_frame(50, 50, [0, 0, 255]);

        let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
        let source_gif = create_test_gif_with_palette(palette);

        let repr = VoyagerSourceReuseBuilder::build(&[frame1, frame2, frame3], 50, 50, &source_gif)
            .expect("build failed");

        assert_eq!(repr.viability, SourceReuseViability::Viable);
        assert_eq!(repr.frames.len(), 3);

        // Each frame should have valid indices
        for vframe in &repr.frames {
            assert!(!vframe.indices.is_empty());
            // All indices should be valid palette indices
            for &idx in &vframe.indices {
                assert!((idx as usize) < repr.global_palette.len() / 3);
            }
        }
    }
}
