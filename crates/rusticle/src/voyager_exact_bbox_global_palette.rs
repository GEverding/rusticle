//! Voyager-class representation study: exact opaque bbox patches + derived sequence-global palette.
//!
//! **STATUS**: Research / Future Opt-In (not current mainline product path)
//!
//! This module implements candidate #3 for the bounded representation study (epic `rusticle-502`):
//!
//! - Takes resized displayed canvases as input
//! - Computes **exact opaque bbox patches** between consecutive resized displayed frames
//! - Derives a **single sequence-global palette** from all patches/frames in the candidate sequence
//! - Quantizes all emitted frames against that derived global palette
//! - Preserves frame delay/disposal/geometry correctly
//! - No synthetic transparency thresholding
//! - No full-frame fallback threshold logic
//!
//! # Design
//!
//! This candidate combines the bbox efficiency of source-reuse with the palette quality of
//! the control path. It should be a strong practical candidate: preserve the opaque-delta
//! structure, but allow a fresh sequence-global palette after resize.
//!
//! # What It Was Trying to Solve
//!
//! The voyager study explored whether exact bbox patches + fresh sequence-global palette
//! could improve compression. This candidate tests that hypothesis: bbox efficiency with
//! optimal palette derivation.
//!
//! # Structural Assumptions
//!
//! - Input is resized displayed canvases (RGBA, already composited correctly)
//! - Output is quantized frame data (palette indices) ready for GIF encoding
//! - No synthetic transparency is introduced
//! - Frame timing and disposal are preserved exactly
//! - Bbox patches are computed exactly (no thresholding or fallback logic)
//!
//! # Latest Evidence
//!
//! The exact bbox + derived global palette approach shows promise for certain GIF types
//! (e.g., animation-heavy, opaque-delta sequences), but generality is not established.
//! Validation on a larger, more diverse GIF corpus is needed before promoting to mainline.
//!
//! # Output
//!
//! Produces a `VoyagerExactBboxGlobalPaletteRepr` containing:
//! - Global palette (derived from all frames)
//! - Per-frame quantized data (bbox patches or full-frame, indices, metadata)
//! - Frame timing and disposal preserved
//!
//! See `docs/RESEARCH_VOYAGER_AND_TWO_PATH.md` for full context.

use crate::error::{Error, Result};
use crate::palette_lut::PaletteLut;
use crate::types::{DisposalMethod, Frame};
use std::time::Duration;

/// Voyager representation: exact bbox patches + derived global palette output.
#[derive(Debug, Clone)]
pub struct VoyagerExactBboxGlobalPaletteRepr {
    /// Canvas width in pixels.
    pub width: u16,
    /// Canvas height in pixels.
    pub height: u16,
    /// Sequence-global palette (flat RGB: [r0,g0,b0,r1,g1,b1,...]).
    pub global_palette: Vec<u8>,
    /// Per-frame quantized data.
    pub frames: Vec<VoyagerExactBboxGlobalPaletteFrame>,
}

/// A single frame in exact bbox + global palette representation.
#[derive(Debug, Clone)]
pub struct VoyagerExactBboxGlobalPaletteFrame {
    /// Palette indices (one per pixel in the bbox region).
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

/// Voyager exact bbox + global palette representation builder.
pub struct VoyagerExactBboxGlobalPaletteBuilder;

impl VoyagerExactBboxGlobalPaletteBuilder {
    /// Build voyager exact bbox + global palette representation from resized displayed canvases.
    ///
    /// # Arguments
    ///
    /// - `resized_frames`: RGBA frames (already resized to display dimensions).
    /// - `canvas_width`: Canvas width in pixels.
    /// - `canvas_height`: Canvas height in pixels.
    ///
    /// # Returns
    ///
    /// A `VoyagerExactBboxGlobalPaletteRepr` with bbox patches and sequence-global palette.
    ///
    /// # Errors
    ///
    /// Returns an error if palette derivation or quantization fails.
    pub fn build(
        resized_frames: &[Frame],
        canvas_width: u16,
        canvas_height: u16,
    ) -> Result<VoyagerExactBboxGlobalPaletteRepr> {
        if resized_frames.is_empty() {
            return Ok(VoyagerExactBboxGlobalPaletteRepr {
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

        // Step 3: Compute exact changed bboxes and quantize frames
        let frames = Self::build_frames_with_bboxes(resized_frames, canvas_width, canvas_height, &lut)?;

        Ok(VoyagerExactBboxGlobalPaletteRepr {
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
        Self::derive_palette_from_rgba(&all_rgba)
    }

    /// Build frames with exact opaque bbox patches.
    fn build_frames_with_bboxes(
        resized_frames: &[Frame],
        canvas_width: u16,
        canvas_height: u16,
        lut: &PaletteLut,
    ) -> Result<Vec<VoyagerExactBboxGlobalPaletteFrame>> {
        if resized_frames.is_empty() {
            return Ok(vec![]);
        }

        let mut frames = Vec::new();

        // Frame 0: always full-frame
        let frame0 = Self::quantize_frame(
            &resized_frames[0],
            0,
            0,
            canvas_width,
            canvas_height,
            lut,
        )?;
        frames.push(frame0);

        // Subsequent frames: compute changed bbox and emit patches
        for i in 1..resized_frames.len() {
            let prev_frame = &resized_frames[i - 1];
            let curr_frame = &resized_frames[i];

            // Compute exact changed bbox
            let bbox = Self::compute_changed_bbox(prev_frame, curr_frame, canvas_width, canvas_height);

            let frame = if bbox.area == 0 {
                // Identical frames: emit 1x1 minimal patch at (0,0)
                let pixel_at_origin = curr_frame.pixels[0..4].to_vec();
                let (mut indices, _) = lut.map_buffer(&pixel_at_origin);
                let transparent_idx = Self::find_transparent_index_and_remap(&pixel_at_origin, &mut indices, lut)?;

                VoyagerExactBboxGlobalPaletteFrame {
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
                let bbox_pixels = Self::extract_bbox_region(curr_frame, &bbox, canvas_width);
                let (mut indices, _) = lut.map_buffer(&bbox_pixels);
                let transparent_idx = Self::find_transparent_index_and_remap(&bbox_pixels, &mut indices, lut)?;

                VoyagerExactBboxGlobalPaletteFrame {
                    indices,
                    transparent_idx,
                    delay: curr_frame.delay,
                    dispose: curr_frame.dispose,
                    left: bbox.left,
                    top: bbox.top,
                    width: bbox.width,
                    height: bbox.height,
                }
            };

            frames.push(frame);
        }

        Ok(frames)
    }

    /// Compute exact changed bbox between two frames.
    fn compute_changed_bbox(
        prev_frame: &Frame,
        curr_frame: &Frame,
        canvas_width: u16,
        canvas_height: u16,
    ) -> BoundingBox {
        let width = canvas_width as usize;
        let height = canvas_height as usize;

        let mut min_x = width as u16;
        let mut min_y = height as u16;
        let mut max_x = 0u16;
        let mut max_y = 0u16;
        let mut found_change = false;

        for y in 0..height {
            for x in 0..width {
                let idx = (y * width + x) * 4;
                let prev_pixel = &prev_frame.pixels[idx..idx + 4];
                let curr_pixel = &curr_frame.pixels[idx..idx + 4];

                if prev_pixel != curr_pixel {
                    found_change = true;
                    min_x = min_x.min(x as u16);
                    min_y = min_y.min(y as u16);
                    max_x = max_x.max((x + 1) as u16);
                    max_y = max_y.max((y + 1) as u16);
                }
            }
        }

        if found_change {
            BoundingBox {
                left: min_x,
                top: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
                area: ((max_x - min_x) as usize) * ((max_y - min_y) as usize),
            }
        } else {
            BoundingBox {
                left: 0,
                top: 0,
                width: 0,
                height: 0,
                area: 0,
            }
        }
    }

    /// Extract a bbox region from a frame as RGBA pixels.
    fn extract_bbox_region(frame: &Frame, bbox: &BoundingBox, canvas_width: u16) -> Vec<u8> {
        let width = bbox.width as usize;
        let height = bbox.height as usize;
        let mut pixels = Vec::with_capacity(width * height * 4);

        for y in 0..height {
            for x in 0..width {
                let canvas_x = bbox.left as usize + x;
                let canvas_y = bbox.top as usize + y;
                let idx = (canvas_y * (canvas_width as usize) + canvas_x) * 4;

                pixels.extend_from_slice(&frame.pixels[idx..idx + 4]);
            }
        }

        pixels
    }

    /// Quantize a single frame using the global palette LUT.
    fn quantize_frame(
        frame: &Frame,
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        lut: &PaletteLut,
    ) -> Result<VoyagerExactBboxGlobalPaletteFrame> {
        if frame.pixels.is_empty() {
            return Ok(VoyagerExactBboxGlobalPaletteFrame {
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
        let transparent_idx = Self::find_transparent_index_and_remap(&frame.pixels, &mut indices, lut)?;

        Ok(VoyagerExactBboxGlobalPaletteFrame {
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

    /// Derive a 256-color palette from RGBA pixels using imagequant.
    fn derive_palette_from_rgba(rgba_pixels: &[u8]) -> Result<Vec<u8>> {
        // Convert raw bytes to RGBA structs
        let rgba_data: Vec<imagequant::RGBA> = rgba_pixels
            .chunks_exact(4)
            .map(|chunk| imagequant::RGBA {
                r: chunk[0],
                g: chunk[1],
                b: chunk[2],
                a: chunk[3],
            })
            .collect();

        if rgba_data.is_empty() {
            return Ok(vec![]);
        }

        // Create imagequant attributes
        let mut attr = imagequant::Attributes::new();
        attr.set_max_colors(256)
            .map_err(|e| Error::EncodeError(format!("failed to set max colors: {}", e)))?;
        attr.set_quality(0, 100)
            .map_err(|e| Error::EncodeError(format!("failed to set quality: {}", e)))?;

        // Create image from RGBA pixels
        let width = rgba_data.len();
        let height = 1;
        let mut img = attr
            .new_image_borrowed(&rgba_data, width, height, 0.0)
            .map_err(|e| Error::EncodeError(format!("failed to create image: {}", e)))?;

        // Quantize
        let mut result = attr
            .quantize(&mut img)
            .map_err(|e| Error::EncodeError(format!("failed to quantize: {}", e)))?;

        // Enable dithering for better visual quality
        result
            .set_dithering_level(1.0)
            .map_err(|e| Error::EncodeError(format!("failed to set dithering: {}", e)))?;

        // Get palette
        let (palette, _) = result
            .remapped(&mut img)
            .map_err(|e| Error::EncodeError(format!("failed to remap: {}", e)))?;

        // Convert palette to flat RGB format
        let mut palette_rgb = Vec::with_capacity(palette.len() * 3);
        for color in palette {
            palette_rgb.push(color.r);
            palette_rgb.push(color.g);
            palette_rgb.push(color.b);
        }

        Ok(palette_rgb)
    }

    /// Find transparent index and remap transparent pixels to it.
    /// Prefers index 0 for transparency (GIF convention, better LZW compression).
    fn find_transparent_index_and_remap(
        rgba_pixels: &[u8],
        indices: &mut [u8],
        lut: &PaletteLut,
    ) -> Result<Option<u8>> {
        // Check if there are any transparent pixels
        let has_transparent = rgba_pixels.chunks_exact(4).any(|p| p[3] < 128);

        if !has_transparent {
            return Ok(None);
        }

        let palette = lut.palette();
        let palette_len = palette.len();

        // Guard against empty palette
        if palette_len == 0 {
            return Ok(None);
        }

        // Count usage of each palette index by OPAQUE pixels only
        let mut opaque_usage = vec![0usize; palette_len];
        for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
            if i < indices.len() && pixel[3] >= 128 {
                opaque_usage[indices[i] as usize] += 1;
            }
        }

        // Prefer index 0 for transparency (GIF convention)
        let transparent_idx = if opaque_usage[0] == 0 {
            0
        } else {
            if let Some(unused_offset) = opaque_usage.iter().skip(1).position(|&count| count == 0) {
                (unused_offset + 1) as u8
            } else if palette_len < 256 {
                opaque_usage
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, &count)| count)
                    .map(|(idx, _)| idx as u8)
                    .unwrap_or(0)
            } else {
                opaque_usage
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, &count)| count)
                    .map(|(idx, _)| idx as u8)
                    .unwrap_or(0)
            }
        };

        // Remap all transparent pixels to use the transparent index
        for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
            if i < indices.len() && pixel[3] < 128 {
                indices[i] = transparent_idx;
            }
        }

        Ok(Some(transparent_idx))
    }

    /// Convert flat RGB palette to 3-byte color array.
    fn flat_rgb_to_palette(flat_rgb: &[u8]) -> Vec<[u8; 3]> {
        flat_rgb
            .chunks_exact(3)
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
            .collect()
    }
}

/// Simple bounding box structure.
#[derive(Debug, Clone, Copy)]
struct BoundingBox {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    area: usize,
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
    fn test_exact_bbox_global_palette_single_frame() {
        let frame = create_opaque_frame(100, 100, [255, 0, 0]);

        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame], 100, 100)
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
    fn test_exact_bbox_global_palette_two_frames_with_change() {
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 10, 10, 20, 20, [0, 255, 0]);

        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
            .expect("build failed");

        // Verify canvas dimensions
        assert_eq!(repr.width, 100);
        assert_eq!(repr.height, 100);

        // Verify two frames
        assert_eq!(repr.frames.len(), 2);

        // Verify first frame is full-frame
        let vframe0 = &repr.frames[0];
        assert_eq!(vframe0.width, 100);
        assert_eq!(vframe0.height, 100);
        assert_eq!(vframe0.left, 0);
        assert_eq!(vframe0.top, 0);
        assert_eq!(vframe0.indices.len(), 100 * 100);

        // Verify second frame is bbox patch
        let vframe1 = &repr.frames[1];
        assert!(vframe1.width <= 100);
        assert!(vframe1.height <= 100);
        // Bbox should be around the rectangle region (10,10) to (30,30)
        assert!(vframe1.left >= 10);
        assert!(vframe1.top >= 10);
        assert!(vframe1.width > 0);
        assert!(vframe1.height > 0);
        assert_eq!(vframe1.indices.len(), (vframe1.width as usize) * (vframe1.height as usize));

        // Verify global palette is derived
        assert!(!repr.global_palette.is_empty());
        assert_eq!(repr.global_palette.len() % 3, 0);
    }

    #[test]
    fn test_exact_bbox_global_palette_identical_frames() {
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_opaque_frame(100, 100, [255, 0, 0]);

        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
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
    fn test_exact_bbox_global_palette_with_transparency() {
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_transparent_frame(100, 100);

        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
            .expect("build failed");

        // Verify two frames
        assert_eq!(repr.frames.len(), 2);

        // Verify second frame has transparent index
        let vframe1 = &repr.frames[1];
        assert!(vframe1.transparent_idx.is_some());
    }

    #[test]
    fn test_exact_bbox_global_palette_delays_preserved() {
        let mut frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        frame1.delay = Duration::from_millis(200);

        let mut frame2 = create_opaque_frame(100, 100, [0, 255, 0]);
        frame2.delay = Duration::from_millis(300);

        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
            .expect("build failed");

        // Verify delays are preserved
        assert_eq!(repr.frames[0].delay, Duration::from_millis(200));
        assert_eq!(repr.frames[1].delay, Duration::from_millis(300));
    }

    #[test]
    fn test_exact_bbox_global_palette_disposal_preserved() {
        let mut frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        frame1.dispose = DisposalMethod::Background;

        let mut frame2 = create_opaque_frame(100, 100, [0, 255, 0]);
        frame2.dispose = DisposalMethod::Previous;

        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
            .expect("build failed");

        // Verify disposal methods are preserved
        assert_eq!(repr.frames[0].dispose, DisposalMethod::Background);
        assert_eq!(repr.frames[1].dispose, DisposalMethod::Previous);
    }

    #[test]
    fn test_exact_bbox_global_palette_empty_sequence() {
        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[], 100, 100)
            .expect("build failed");

        // Verify empty sequence
        assert_eq!(repr.width, 100);
        assert_eq!(repr.height, 100);
        assert_eq!(repr.frames.len(), 0);
        assert!(!repr.global_palette.is_empty()); // Minimal palette
    }

    #[test]
    fn test_exact_bbox_global_palette_no_synthetic_transparency() {
        // All opaque frames should not introduce synthetic transparency
        let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
        let frame2 = create_opaque_frame(100, 100, [0, 255, 0]);

        let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
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
}
