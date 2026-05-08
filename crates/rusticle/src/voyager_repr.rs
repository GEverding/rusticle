//! Voyager-class representation study: control path.
//!
//! **STATUS**: Research / Future Opt-In (not current mainline product path)
//!
//! This module implements the intentionally simple control candidate for the bounded
//! representation study (epic `rusticle-502`):
//!
//! - Takes resized displayed canvases as input
//! - Emits **full-frame** output for every frame
//! - Derives a **single sequence-global palette** from the resized frames
//! - Quantizes all frames against that derived global palette
//! - Preserves frame delay/disposal/geometry correctly
//!
//! # Design
//!
//! This is the control path: no bbox reconstruction, no source palette reuse, no fallback
//! threshold logic. It establishes a clean shared representation abstraction that later
//! candidates can build on.
//!
//! # What It Was Trying to Solve
//!
//! The voyager study explored whether alternative representation strategies (different ways
//! to structure GIF frames and palettes) could improve compression or quality. This control
//! path is the baseline: full-frame output with sequence-global palette.
//!
//! # Structural Assumptions
//!
//! - Input is resized displayed canvases (RGBA, already composited correctly)
//! - Output is quantized frame data (palette indices) ready for GIF encoding
//! - No synthetic transparency is introduced
//! - Frame timing and disposal are preserved exactly
//!
//! # Latest Evidence
//!
//! The control path is a clean baseline but doesn't offer compression wins over the corrected
//! default path. Validation on a larger, more diverse GIF corpus is needed before promoting
//! any voyager candidate to mainline.
//!
//! # Output
//!
//! Produces a `VoyagerRepr` containing:
//! - Global palette (derived from all frames)
//! - Per-frame quantized data (full-frame geometry, indices, metadata)
//! - Frame timing and disposal preserved
//!
//! See `docs/RESEARCH_VOYAGER_AND_TWO_PATH.md` for full context.

use crate::error::{Error, Result};
use crate::palette_lut::PaletteLut;
use crate::types::{DisposalMethod, Frame};
use rayon::prelude::*;
use std::time::Duration;

/// Voyager representation: full-frame control path output.
#[derive(Debug, Clone)]
pub struct VoyagerRepr {
    /// Canvas width in pixels.
    pub width: u16,
    /// Canvas height in pixels.
    pub height: u16,
    /// Sequence-global palette (flat RGB: [r0,g0,b0,r1,g1,b1,...]).
    pub global_palette: Vec<u8>,
    /// Per-frame quantized data.
    pub frames: Vec<VoyagerFrame>,
}

/// A single frame in voyager representation.
#[derive(Debug, Clone)]
pub struct VoyagerFrame {
    /// Palette indices (one per pixel, full-frame).
    pub indices: Vec<u8>,
    /// Transparent index for this frame (if any).
    pub transparent_idx: Option<u8>,
    /// Frame delay.
    pub delay: Duration,
    /// Frame disposal method.
    pub dispose: DisposalMethod,
    /// Frame geometry (always full-canvas for control path).
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
}

/// Voyager representation builder.
pub struct VoyagerBuilder;

impl VoyagerBuilder {
    /// Build voyager representation from resized displayed canvases.
    ///
    /// # Arguments
    ///
    /// - `resized_frames`: RGBA frames (already resized to display dimensions).
    /// - `canvas_width`: Canvas width in pixels.
    /// - `canvas_height`: Canvas height in pixels.
    ///
    /// # Returns
    ///
    /// A `VoyagerRepr` with full-frame geometry and sequence-global palette.
    ///
    /// # Errors
    ///
    /// Returns an error if palette derivation or quantization fails.
    pub fn build(
        resized_frames: &[Frame],
        canvas_width: u16,
        canvas_height: u16,
    ) -> Result<VoyagerRepr> {
        if resized_frames.is_empty() {
            return Ok(VoyagerRepr {
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

        // Step 3: Quantize all frames using the global palette (in parallel)
        let frames: Vec<VoyagerFrame> = resized_frames
            .par_iter()
            .map(|frame| Self::quantize_frame(frame, &lut, canvas_width, canvas_height))
            .collect::<Result<Vec<_>>>()?;

        Ok(VoyagerRepr {
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

    /// Quantize a single frame using the global palette LUT.
    fn quantize_frame(
        frame: &Frame,
        lut: &PaletteLut,
        canvas_width: u16,
        canvas_height: u16,
    ) -> Result<VoyagerFrame> {
        if frame.pixels.is_empty() {
            // Empty frame
            return Ok(VoyagerFrame {
                indices: vec![],
                transparent_idx: None,
                delay: frame.delay,
                dispose: frame.dispose,
                left: 0,
                top: 0,
                width: canvas_width,
                height: canvas_height,
            });
        }

        // Map pixels using LUT
        let (mut indices, _stats) = lut.map_buffer(&frame.pixels);

        // Find and remap transparent index
        let transparent_idx = Self::find_transparent_index_and_remap(&frame.pixels, &mut indices, lut)?;

        Ok(VoyagerFrame {
            indices,
            transparent_idx,
            delay: frame.delay,
            dispose: frame.dispose,
            left: 0,
            top: 0,
            width: canvas_width,
            height: canvas_height,
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

        // Prefer index 0 for transparency (GIF convention, better LZW compression)
        let transparent_idx = if opaque_usage[0] == 0 {
            // Index 0 is unused by opaque pixels - perfect!
            0
        } else {
            // Index 0 is used - find an unused index to swap with
            if let Some(unused_offset) = opaque_usage.iter().skip(1).position(|&count| count == 0) {
                let swap_idx = (unused_offset + 1) as u8; // +1 because we skipped index 0
                swap_idx
            } else if palette_len < 256 {
                // No unused index - would need to add new entry
                // For now, use the least-used index
                opaque_usage
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, &count)| count)
                    .map(|(idx, _)| idx as u8)
                    .unwrap_or(0)
            } else {
                // Full palette, all used - use least-used as fallback
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

    /// Convert flat RGB palette to 3-byte format.
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
    use std::time::Duration;

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
            // Alternate transparent and opaque
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
    fn test_single_frame_full_canvas_geometry() {
        let frame = create_opaque_frame(100, 100, [255, 0, 0]);
        let repr = VoyagerBuilder::build(&[frame], 100, 100).expect("build failed");

        assert_eq!(repr.width, 100);
        assert_eq!(repr.height, 100);
        assert_eq!(repr.frames.len(), 1);

        let vframe = &repr.frames[0];
        assert_eq!(vframe.width, 100);
        assert_eq!(vframe.height, 100);
        assert_eq!(vframe.left, 0);
        assert_eq!(vframe.top, 0);
        assert_eq!(vframe.indices.len(), 100 * 100);
    }

    #[test]
    fn test_multi_frame_single_global_palette() {
        let frame1 = create_opaque_frame(50, 50, [255, 0, 0]); // Red
        let frame2 = create_opaque_frame(50, 50, [0, 255, 0]); // Green
        let frame3 = create_opaque_frame(50, 50, [0, 0, 255]); // Blue

        let repr = VoyagerBuilder::build(&[frame1, frame2, frame3], 50, 50)
            .expect("build failed");

        // All frames should use the same global palette
        assert_eq!(repr.frames.len(), 3);
        assert!(!repr.global_palette.is_empty());

        // Each frame should have full-canvas geometry
        for vframe in &repr.frames {
            assert_eq!(vframe.width, 50);
            assert_eq!(vframe.height, 50);
            assert_eq!(vframe.indices.len(), 50 * 50);
        }
    }

    #[test]
    fn test_no_local_palette_churn() {
        let frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
        let frame2 = create_opaque_frame(50, 50, [0, 255, 0]);

        let repr = VoyagerBuilder::build(&[frame1, frame2], 50, 50).expect("build failed");

        // Control path should never use local palettes
        // (This is implicit in VoyagerFrame structure - no local_palette field)
        assert_eq!(repr.frames.len(), 2);
        // Verify global palette is used
        assert!(!repr.global_palette.is_empty());
    }

    #[test]
    fn test_delays_and_disposal_preserved() {
        let mut frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
        frame1.delay = Duration::from_millis(200);
        frame1.dispose = DisposalMethod::Background;

        let mut frame2 = create_opaque_frame(50, 50, [0, 255, 0]);
        frame2.delay = Duration::from_millis(300);
        frame2.dispose = DisposalMethod::Previous;

        let repr = VoyagerBuilder::build(&[frame1, frame2], 50, 50).expect("build failed");

        assert_eq!(repr.frames[0].delay, Duration::from_millis(200));
        assert_eq!(repr.frames[0].dispose, DisposalMethod::Background);
        assert_eq!(repr.frames[1].delay, Duration::from_millis(300));
        assert_eq!(repr.frames[1].dispose, DisposalMethod::Previous);
    }

    #[test]
    fn test_transparent_pixels_handled() {
        let frame = create_transparent_frame(50, 50);
        let repr = VoyagerBuilder::build(&[frame], 50, 50).expect("build failed");

        assert_eq!(repr.frames.len(), 1);
        // Should have a transparent index assigned
        assert!(repr.frames[0].transparent_idx.is_some());
    }

    #[test]
    fn test_empty_sequence() {
        let repr = VoyagerBuilder::build(&[], 100, 100).expect("build failed");

        assert_eq!(repr.width, 100);
        assert_eq!(repr.height, 100);
        assert_eq!(repr.frames.len(), 0);
        // Should have minimal palette
        assert!(!repr.global_palette.is_empty());
    }

    #[test]
    fn test_output_decodable_frame_count() {
        let frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
        let frame2 = create_opaque_frame(50, 50, [0, 255, 0]);
        let frame3 = create_opaque_frame(50, 50, [0, 0, 255]);

        let repr = VoyagerBuilder::build(&[frame1, frame2, frame3], 50, 50)
            .expect("build failed");

        // Frame count should match input
        assert_eq!(repr.frames.len(), 3);

        // Each frame should have valid indices
        for vframe in &repr.frames {
            assert_eq!(vframe.indices.len(), 50 * 50);
            // All indices should be valid palette indices
            for &idx in &vframe.indices {
                assert!((idx as usize) < repr.global_palette.len() / 3);
            }
        }
    }
}
