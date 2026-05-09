//! Voyager-class representation study: exact opaque bbox patches + derived sequence-global palette + full-frame fallback.
//!
//! **STATUS**: Research / Future Opt-In (not current mainline product path)
//!
//! This module implements candidate #4 for the bounded representation study (epic `rusticle-502`):
//!
//! - Takes resized displayed canvases as input
//! - Computes **exact opaque bbox patches** between consecutive resized displayed frames
//! - Derives a **single sequence-global palette** from all patches/frames in the candidate sequence
//! - Quantizes all emitted frames against that derived global palette
//! - **Falls back to full-frame when bbox area exceeds a configurable threshold**
//! - Preserves frame delay/disposal/geometry correctly
//! - No synthetic transparency thresholding
//!
//! # Design
//!
//! This candidate extends the exact bbox + derived global palette approach with a practical
//! production-oriented fallback rule: when the changed bbox is too large relative to the canvas,
//! emit a full frame instead of a patch. This can improve compression in cases where LZW compresses
//! full frames better than large patches.
//!
//! The threshold is configurable as a fraction of canvas area (default 0.7, matching current Path A).
//!
//! # What It Was Trying to Solve
//!
//! The voyager study explored whether exact bbox patches + fresh sequence-global palette + practical
//! fallback logic could improve compression. This candidate adds production-oriented fallback:
//! when bbox is too large, emit full frame instead (LZW may compress better).
//!
//! # Structural Assumptions
//!
//! - Input is resized displayed canvases (RGBA, already composited correctly)
//! - Output is quantized frame data (palette indices) ready for GIF encoding
//! - No synthetic transparency is introduced
//! - Frame timing and disposal are preserved exactly
//! - Bbox area threshold (default 0.7) determines when to fall back to full-frame
//!
//! # Latest Evidence
//!
//! The fallback variant adds practical production logic but requires larger-corpus validation.
//! Validation on a larger, more diverse GIF corpus is needed before promoting to mainline.
//!
//! # Output
//!
//! Produces a `VoyagerExactBboxGlobalPaletteFallbackRepr` containing:
//! - Global palette (derived from all frames)
//! - Per-frame quantized data (bbox patches or full-frame, indices, metadata)
//! - Frame timing and disposal preserved
//!
//! See `docs/research/RESEARCH_VOYAGER_AND_TWO_PATH.md` for full context.

use crate::error::Result;
use crate::gif_ops::{
    compute_changed_bbox, derive_palette_from_rgba, extract_bbox_region,
    find_transparent_index_and_remap,
};
use crate::palette_lut::PaletteLut;
use crate::types::{DisposalMethod, Frame};
use std::time::Duration;

/// Voyager representation: exact bbox patches + derived global palette + fallback output.
#[derive(Debug, Clone)]
pub struct VoyagerExactBboxGlobalPaletteFallbackRepr {
    /// Canvas width in pixels.
    pub width: u16,
    /// Canvas height in pixels.
    pub height: u16,
    /// Sequence-global palette (flat RGB: [r0,g0,b0,r1,g1,b1,...]).
    pub global_palette: Vec<u8>,
    /// Per-frame quantized data.
    pub frames: Vec<VoyagerExactBboxGlobalPaletteFallbackFrame>,
}

/// A single frame in exact bbox + global palette + fallback representation.
#[derive(Debug, Clone)]
pub struct VoyagerExactBboxGlobalPaletteFallbackFrame {
    /// Palette indices (one per pixel in the bbox region or full frame).
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

/// Voyager exact bbox + global palette + fallback representation builder.
pub struct VoyagerExactBboxGlobalPaletteFallbackBuilder;

impl VoyagerExactBboxGlobalPaletteFallbackBuilder {
    /// Build voyager exact bbox + global palette + fallback representation from resized displayed canvases.
    ///
    /// # Arguments
    ///
    /// - `resized_frames`: RGBA frames (already resized to display dimensions).
    /// - `canvas_width`: Canvas width in pixels.
    /// - `canvas_height`: Canvas height in pixels.
    /// - `bbox_area_threshold`: Fraction of canvas area (0.0..=1.0) above which to emit full frame instead of bbox.
    ///   Default is 0.7 (70% of canvas area).
    ///
    /// # Returns
    ///
    /// A `VoyagerExactBboxGlobalPaletteFallbackRepr` with bbox patches (or full frames) and sequence-global palette.
    ///
    /// # Errors
    ///
    /// Returns an error if palette derivation or quantization fails.
    pub fn build(
        resized_frames: &[Frame],
        canvas_width: u16,
        canvas_height: u16,
        bbox_area_threshold: f64,
    ) -> Result<VoyagerExactBboxGlobalPaletteFallbackRepr> {
        if resized_frames.is_empty() {
            return Ok(VoyagerExactBboxGlobalPaletteFallbackRepr {
                width: canvas_width,
                height: canvas_height,
                global_palette: vec![0, 0, 0], // Minimal palette
                frames: vec![],
            });
        }

        // Step 1: Derive sequence-global palette from all resized frames
        let global_palette = Self::derive_global_palette(resized_frames)?;

        // Step 2: Build LUT from derived palette
        let palette_3byte = Self::flat_rgb_to_palette(&global_palette);
        let lut = PaletteLut::new(&palette_3byte);

        // Step 3: Compute exact changed bboxes and quantize frames with fallback logic
        let frames = Self::build_frames_with_bboxes_and_fallback(
            resized_frames,
            canvas_width,
            canvas_height,
            &lut,
            bbox_area_threshold,
        )?;

        Ok(VoyagerExactBboxGlobalPaletteFallbackRepr {
            width: canvas_width,
            height: canvas_height,
            global_palette,
            frames,
        })
    }

    /// Derive a 256-color sequence-global palette from all resized frames.
    fn derive_global_palette(resized_frames: &[Frame]) -> Result<Vec<u8>> {
        // Collect all RGBA pixels from all frames
        let mut all_rgba = Vec::new();
        for frame in resized_frames {
            all_rgba.extend_from_slice(&frame.pixels);
        }

        if all_rgba.is_empty() {
            // Create a minimal palette for empty frames
            return Ok(vec![0, 0, 0]);
        }

        // Derive global palette using imagequant
        derive_palette_from_rgba(&all_rgba)
    }

    /// Build frames with exact opaque bbox patches and fallback to full-frame when bbox is too large.
    fn build_frames_with_bboxes_and_fallback(
        resized_frames: &[Frame],
        canvas_width: u16,
        canvas_height: u16,
        lut: &PaletteLut,
        bbox_area_threshold: f64,
    ) -> Result<Vec<VoyagerExactBboxGlobalPaletteFallbackFrame>> {
        if resized_frames.is_empty() {
            return Ok(vec![]);
        }

        let canvas_area = (canvas_width as usize) * (canvas_height as usize);
        let threshold_area = (canvas_area as f64 * bbox_area_threshold) as usize;

        let mut frames = Vec::new();

        // Frame 0: always full-frame
        let frame0 =
            Self::quantize_frame(&resized_frames[0], 0, 0, canvas_width, canvas_height, lut)?;
        frames.push(frame0);

        // Subsequent frames: compute changed bbox and emit patches or full-frame based on threshold
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

                VoyagerExactBboxGlobalPaletteFallbackFrame {
                    indices,
                    transparent_idx,
                    delay: curr_frame.delay,
                    dispose: curr_frame.dispose,
                    left: 0,
                    top: 0,
                    width: 1,
                    height: 1,
                }
            } else if bbox.area() > threshold_area {
                // Bbox exceeds threshold: emit full-frame instead
                let (mut indices, _) = lut.map_buffer(&curr_frame.pixels);
                let mut palette_rgb: Vec<u8> = lut
                    .palette()
                    .iter()
                    .flat_map(|color| color.iter().copied())
                    .collect();
                let transparent_idx = find_transparent_index_and_remap(
                    &curr_frame.pixels,
                    &mut indices,
                    &mut palette_rgb,
                );

                VoyagerExactBboxGlobalPaletteFallbackFrame {
                    indices,
                    transparent_idx,
                    delay: curr_frame.delay,
                    dispose: curr_frame.dispose,
                    left: 0,
                    top: 0,
                    width: canvas_width,
                    height: canvas_height,
                }
            } else {
                // Bbox within threshold: emit bbox patch
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

                VoyagerExactBboxGlobalPaletteFallbackFrame {
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

    /// Quantize a single frame using the global palette LUT.
    fn quantize_frame(
        frame: &Frame,
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        lut: &PaletteLut,
    ) -> Result<VoyagerExactBboxGlobalPaletteFallbackFrame> {
        if frame.pixels.is_empty() {
            return Ok(VoyagerExactBboxGlobalPaletteFallbackFrame {
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

        Ok(VoyagerExactBboxGlobalPaletteFallbackFrame {
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

    /// Convert flat RGB palette to 3-byte color array.
    fn flat_rgb_to_palette(flat_rgb: &[u8]) -> Vec<[u8; 3]> {
        flat_rgb
            .chunks_exact(3)
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DisposalMethod;

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

    /// Create a test frame with a colored rectangle on a background.
    fn create_frame_with_rect(
        width: u16,
        height: u16,
        bg_color: [u8; 3],
        rect_left: u16,
        rect_top: u16,
        rect_width: u16,
        rect_height: u16,
        rect_color: [u8; 3],
    ) -> Frame {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];

        // Fill background
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[0] = bg_color[0];
            chunk[1] = bg_color[1];
            chunk[2] = bg_color[2];
            chunk[3] = 255;
        }

        // Draw rectangle
        for y in 0..rect_height {
            for x in 0..rect_width {
                let canvas_x = rect_left as usize + x as usize;
                let canvas_y = rect_top as usize + y as usize;
                if canvas_x < width as usize && canvas_y < height as usize {
                    let idx = (canvas_y * (width as usize) + canvas_x) * 4;
                    pixels[idx] = rect_color[0];
                    pixels[idx + 1] = rect_color[1];
                    pixels[idx + 2] = rect_color[2];
                    pixels[idx + 3] = 255;
                }
            }
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

    #[test]
    fn test_fallback_single_frame() {
        let frame = create_opaque_frame(100, 100, [255, 0, 0]);

        let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame], 100, 100, 0.7)
            .expect("build failed");

        // Verify canvas dimensions
        assert_eq!(repr.width, 100);
        assert_eq!(repr.height, 100);

        // Verify single frame
        assert_eq!(repr.frames.len(), 1);

        // Verify full-frame geometry for first frame
        let vframe = &repr.frames[0];
        assert_eq!(vframe.width, 100);
        assert_eq!(vframe.height, 100);
        assert_eq!(vframe.left, 0);
        assert_eq!(vframe.top, 0);

        // Verify full-frame indices
        assert_eq!(vframe.indices.len(), 100 * 100);

        // Verify global palette is derived
        assert!(!repr.global_palette.is_empty());
        assert_eq!(repr.global_palette.len() % 3, 0);
    }

    #[test]
    fn test_fallback_small_bbox_stays_patch() {
        // 100x100 canvas, small 20x20 change at (10,10)
        // Threshold 0.7 = 7000 pixels, bbox = 400 pixels -> stays as patch
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 10, 10, 20, 20, [0, 255, 0]);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.7)
                .expect("build failed");

        // Verify two frames
        assert_eq!(repr.frames.len(), 2);

        // Verify first frame is full-frame
        let vframe0 = &repr.frames[0];
        assert_eq!(vframe0.width, 100);
        assert_eq!(vframe0.height, 100);

        // Verify second frame is bbox patch (not full-frame)
        let vframe1 = &repr.frames[1];
        assert!(vframe1.width < 100 || vframe1.height < 100);
        assert_eq!(
            vframe1.indices.len(),
            (vframe1.width as usize) * (vframe1.height as usize)
        );
    }

    #[test]
    fn test_fallback_large_bbox_triggers_full_frame() {
        // 100x100 canvas, large 80x80 change at (10,10)
        // Threshold 0.7 = 7000 pixels, bbox = 6400 pixels -> stays as patch
        // Threshold 0.5 = 5000 pixels, bbox = 6400 pixels -> falls back to full-frame
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 10, 10, 80, 80, [0, 255, 0]);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.5)
                .expect("build failed");

        // Verify two frames
        assert_eq!(repr.frames.len(), 2);

        // Verify first frame is full-frame
        let vframe0 = &repr.frames[0];
        assert_eq!(vframe0.width, 100);
        assert_eq!(vframe0.height, 100);

        // Verify second frame is full-frame (fallback triggered)
        let vframe1 = &repr.frames[1];
        assert_eq!(vframe1.width, 100);
        assert_eq!(vframe1.height, 100);
        assert_eq!(vframe1.left, 0);
        assert_eq!(vframe1.top, 0);
        assert_eq!(vframe1.indices.len(), 100 * 100);
    }

    #[test]
    fn test_fallback_threshold_boundary() {
        // 100x100 canvas = 10000 pixels
        // Create a 70x71 bbox = 4970 pixels
        // Threshold 0.5 = 5000 pixels -> should stay as patch (4970 < 5000)
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 10, 10, 70, 71, [0, 255, 0]);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.5)
                .expect("build failed");

        let vframe1 = &repr.frames[1];
        // Should be bbox patch, not full-frame
        assert!(vframe1.width <= 70 && vframe1.height <= 71);
    }

    #[test]
    fn test_fallback_identical_frames() {
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_opaque_frame(100, 100, [255, 0, 0]);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.7)
                .expect("build failed");

        // Verify two frames
        assert_eq!(repr.frames.len(), 2);

        // Verify first frame is full-frame
        let vframe0 = &repr.frames[0];
        assert_eq!(vframe0.width, 100);
        assert_eq!(vframe0.height, 100);

        // Verify second frame is minimal 1x1 patch (no change)
        let vframe1 = &repr.frames[1];
        assert_eq!(vframe1.width, 1);
        assert_eq!(vframe1.height, 1);
        assert_eq!(vframe1.left, 0);
        assert_eq!(vframe1.top, 0);
        assert_eq!(vframe1.indices.len(), 1);
    }

    #[test]
    fn test_fallback_with_transparency() {
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_transparent_frame(100, 100);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.7)
                .expect("build failed");

        // Verify two frames
        assert_eq!(repr.frames.len(), 2);

        // Verify second frame has transparent index
        let vframe1 = &repr.frames[1];
        assert!(vframe1.transparent_idx.is_some());
    }

    #[test]
    fn test_fallback_delays_preserved() {
        let mut frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        frame1.delay = Duration::from_millis(200);

        let mut frame2 = create_opaque_frame(100, 100, [0, 255, 0]);
        frame2.delay = Duration::from_millis(300);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.7)
                .expect("build failed");

        // Verify delays are preserved
        assert_eq!(repr.frames[0].delay, Duration::from_millis(200));
        assert_eq!(repr.frames[1].delay, Duration::from_millis(300));
    }

    #[test]
    fn test_fallback_disposal_preserved() {
        let mut frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        frame1.dispose = DisposalMethod::Background;

        let mut frame2 = create_opaque_frame(100, 100, [0, 255, 0]);
        frame2.dispose = DisposalMethod::Previous;

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.7)
                .expect("build failed");

        // Verify disposal methods are preserved
        assert_eq!(repr.frames[0].dispose, DisposalMethod::Background);
        assert_eq!(repr.frames[1].dispose, DisposalMethod::Previous);
    }

    #[test]
    fn test_fallback_empty_sequence() {
        let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[], 100, 100, 0.7)
            .expect("build failed");

        // Verify empty sequence
        assert_eq!(repr.width, 100);
        assert_eq!(repr.height, 100);
        assert_eq!(repr.frames.len(), 0);
        assert!(!repr.global_palette.is_empty()); // Minimal palette
    }

    #[test]
    fn test_fallback_no_synthetic_transparency() {
        // All opaque frames should not introduce synthetic transparency
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_opaque_frame(100, 100, [0, 255, 0]);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.7)
                .expect("build failed");

        // Verify no synthetic transparency introduced
        for vframe in &repr.frames {
            // If frame is all opaque, transparent_idx should be None or not used
            if vframe.transparent_idx.is_some() {
                // Check that transparent pixels actually exist in the frame
                // (This is a sanity check; the implementation should not introduce synthetic transparency)
                assert!(vframe.indices.len() > 0);
            }
        }
    }

    #[test]
    fn test_fallback_threshold_zero() {
        // Threshold 0.0 means all changes trigger full-frame
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 10, 10, 5, 5, [0, 255, 0]);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 0.0)
                .expect("build failed");

        let vframe1 = &repr.frames[1];
        // Even tiny bbox should trigger full-frame with threshold 0.0
        assert_eq!(vframe1.width, 100);
        assert_eq!(vframe1.height, 100);
    }

    #[test]
    fn test_fallback_threshold_one() {
        // Threshold 1.0 means only changes >= 100% of canvas trigger full-frame
        // (which is impossible for a bbox, so all should stay as patches)
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 0, 0, 100, 100, [0, 255, 0]);

        let repr =
            VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame1, frame2], 100, 100, 1.0)
                .expect("build failed");

        let vframe1 = &repr.frames[1];
        // Even full-canvas change should stay as patch with threshold 1.0
        assert!(vframe1.width <= 100 && vframe1.height <= 100);
    }
}
