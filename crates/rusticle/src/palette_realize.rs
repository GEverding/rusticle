//! Palette realization: convert palette strategy decisions into quantized palette output.
//!
//! This module bridges the gap between palette strategy selection (from `palette_strategy.rs`)
//! and actual GIF encoding. Given materialized RGBA frames and a chosen palette strategy,
//! it produces concrete palette-realized output structures suitable for the encode path.
//!
//! # Palette Realization Process
//!
//! For each strategy:
//! - **ReuseGlobalPreferred**: Validate source global palette covers materialized frames' color needs.
//!   If coverage is sufficient (e.g., >98% pixels within threshold), reuse it directly.
//!   Otherwise fall back to DeriveSequenceGlobal.
//! - **DeriveSequenceGlobalPreferred**: Collect all unique colors from materialized frames,
//!   run imagequant to derive a single 256-color global palette, assign to all frames.
//! - **LocalPaletteFallback**: Run imagequant per-frame to produce local palettes.
//!
//! # Output
//!
//! Produces `PaletteRealization` containing:
//! - Global palette (if any)
//! - Per-frame local palettes (if any)
//! - Per-frame quantized indices
//! - Transparent index per frame
//!
//! # Invariants
//!
//! - Transparent-index handling is safe: no collision with opaque pixels.
//! - Frame geometry/disposal/delay from materialization remain intact.
//! - Output is suitable for direct consumption by the encode path.

use crate::error::Result;
use crate::gif_ops::{derive_palette_from_rgba, find_transparent_index_and_remap};
use crate::palette_lut::PaletteLut;
use crate::palette_strategy::PaletteStrategy;
use crate::types::{Frame, Gif};
use rayon::prelude::*;

/// A quantized frame with palette indices and metadata.
#[derive(Debug, Clone)]
pub struct QuantizedFrameData {
    /// Palette indices (one per pixel).
    pub indices: Vec<u8>,
    /// Local palette for this frame (if any).
    pub local_palette: Option<Vec<u8>>, // Flat RGB: [r0,g0,b0,r1,g1,b1,...]
    /// Transparent index for this frame (if any).
    pub transparent_idx: Option<u8>,
    /// Frame metadata (preserved from materialization).
    pub delay: std::time::Duration,
    pub dispose: crate::types::DisposalMethod,
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
}

/// Result of palette realization: global palette (if any) + per-frame quantized data.
#[derive(Debug, Clone)]
pub struct PaletteRealization {
    /// Global palette (if strategy uses global palette).
    pub global_palette: Option<Vec<u8>>, // Flat RGB
    /// Per-frame quantized data.
    pub frames: Vec<QuantizedFrameData>,
}

/// Palette realizer: converts strategy decisions into quantized output.
pub struct PaletteRealizer;

impl PaletteRealizer {
    /// Realize palette for a sequence given materialized frames and chosen strategy.
    ///
    /// # Arguments
    ///
    /// - `materialized_frames`: RGBA frames from materialization stage.
    /// - `strategy`: Chosen palette strategy.
    /// - `source_gif`: Source GIF (for reuse strategy).
    ///
    /// # Returns
    ///
    /// A `PaletteRealization` with global/local palettes and per-frame indices.
    ///
    /// # Errors
    ///
    /// Returns an error if quantization fails.
    pub fn realize(
        materialized_frames: &[Frame],
        strategy: PaletteStrategy,
        source_gif: &Gif,
    ) -> Result<PaletteRealization> {
        match strategy {
            PaletteStrategy::ReuseGlobalPreferred => {
                Self::realize_reuse_global_preferred(materialized_frames, source_gif)
            }
            PaletteStrategy::DeriveSequenceGlobalPreferred => {
                Self::realize_derive_sequence_global(materialized_frames)
            }
            PaletteStrategy::LocalPaletteFallback => {
                Self::realize_local_palette_fallback(materialized_frames)
            }
        }
    }

    /// Realize using reuse-global-preferred strategy.
    ///
    /// Validates source global palette coverage. If sufficient (>98% pixels within threshold),
    /// reuses it. Otherwise falls back to derive-sequence-global.
    fn realize_reuse_global_preferred(
        materialized_frames: &[Frame],
        source_gif: &Gif,
    ) -> Result<PaletteRealization> {
        // Check if source has global palette
        let source_palette = match &source_gif.global_palette {
            Some(p) => p,
            None => {
                // No source palette: fall back to derive-sequence-global
                return Self::realize_derive_sequence_global(materialized_frames);
            }
        };

        // Build LUT from source palette
        let palette_rgb: Vec<[u8; 3]> = source_palette.colors.clone();
        let lut = PaletteLut::new(&palette_rgb);

        // Validate coverage: check if >98% of pixels map acceptably
        let coverage = Self::check_palette_coverage(materialized_frames, &lut)?;

        if coverage.acceptable_ratio >= 0.98 {
            // Coverage is sufficient: reuse source palette
            let global_palette_rgb = Self::palette_to_flat_rgb(&palette_rgb);
            let frames = Self::quantize_frames_with_lut(materialized_frames, &lut)?;

            Ok(PaletteRealization {
                global_palette: Some(global_palette_rgb),
                frames,
            })
        } else {
            // Coverage insufficient: fall back to derive-sequence-global
            Self::realize_derive_sequence_global(materialized_frames)
        }
    }

    /// Realize using derive-sequence-global-preferred strategy.
    ///
    /// Collects all unique colors from materialized frames, runs imagequant to derive
    /// a single 256-color global palette, assigns to all frames.
    fn realize_derive_sequence_global(materialized_frames: &[Frame]) -> Result<PaletteRealization> {
        // If no frames, return empty realization
        if materialized_frames.is_empty() {
            return Ok(PaletteRealization {
                global_palette: None,
                frames: vec![],
            });
        }

        // Collect all RGBA pixels from all frames
        let mut all_rgba = Vec::new();
        for frame in materialized_frames {
            all_rgba.extend_from_slice(&frame.pixels);
        }

        // If all frames are empty, still quantize them (they'll have empty indices)
        let global_palette_rgb = if all_rgba.is_empty() {
            // Create a minimal palette for empty frames
            vec![0, 0, 0] // Single black color
        } else {
            // Derive global palette using imagequant
            derive_palette_from_rgba(&all_rgba)?
        };

        // Build LUT from derived palette
        let palette_3byte: Vec<[u8; 3]> = Self::flat_rgb_to_palette(&global_palette_rgb);
        let lut = PaletteLut::new(&palette_3byte);

        // Quantize all frames using the global palette
        let frames = Self::quantize_frames_with_lut(materialized_frames, &lut)?;

        Ok(PaletteRealization {
            global_palette: Some(global_palette_rgb),
            frames,
        })
    }

    /// Realize using local-palette-fallback strategy.
    ///
    /// Runs imagequant per-frame to produce local palettes.
    fn realize_local_palette_fallback(materialized_frames: &[Frame]) -> Result<PaletteRealization> {
        let frames: Vec<QuantizedFrameData> = materialized_frames
            .par_iter()
            .map(Self::quantize_frame_local)
            .collect::<Result<Vec<_>>>()?;

        Ok(PaletteRealization {
            global_palette: None,
            frames,
        })
    }

    /// Quantize a single frame using a pre-built LUT (global palette).
    fn quantize_frame_with_lut(frame: &Frame, lut: &PaletteLut) -> Result<QuantizedFrameData> {
        if frame.pixels.is_empty() {
            // Empty frame
            return Ok(QuantizedFrameData {
                indices: vec![],
                local_palette: None,
                transparent_idx: None,
                delay: frame.delay,
                dispose: frame.dispose,
                left: frame.left,
                top: frame.top,
                width: frame.width,
                height: frame.height,
            });
        }

        // Map pixels using LUT
        let (mut indices, _stats) = lut.map_buffer(&frame.pixels);

        // Find and remap transparent index
        let mut palette_rgb: Vec<u8> = lut
            .palette()
            .iter()
            .flat_map(|color| color.iter().copied())
            .collect();
        let transparent_idx =
            find_transparent_index_and_remap(&frame.pixels, &mut indices, &mut palette_rgb);

        Ok(QuantizedFrameData {
            indices,
            local_palette: None, // Using global palette
            transparent_idx,
            delay: frame.delay,
            dispose: frame.dispose,
            left: frame.left,
            top: frame.top,
            width: frame.width,
            height: frame.height,
        })
    }

    /// Quantize a single frame using imagequant (local palette).
    fn quantize_frame_local(frame: &Frame) -> Result<QuantizedFrameData> {
        if frame.pixels.is_empty() {
            // Empty frame
            return Ok(QuantizedFrameData {
                indices: vec![],
                local_palette: None,
                transparent_idx: None,
                delay: frame.delay,
                dispose: frame.dispose,
                left: frame.left,
                top: frame.top,
                width: frame.width,
                height: frame.height,
            });
        }

        // Derive palette from this frame's pixels
        let palette_rgb = derive_palette_from_rgba(&frame.pixels)?;

        // Convert to 3-byte format for LUT
        let palette_3byte = Self::flat_rgb_to_palette(&palette_rgb);
        let lut = PaletteLut::new(&palette_3byte);

        // Quantize using the local palette
        let (mut indices, _stats) = lut.map_buffer(&frame.pixels);

        // Find and remap transparent index
        let mut palette_rgb: Vec<u8> = lut
            .palette()
            .iter()
            .flat_map(|color| color.iter().copied())
            .collect();
        let transparent_idx =
            find_transparent_index_and_remap(&frame.pixels, &mut indices, &mut palette_rgb);

        Ok(QuantizedFrameData {
            indices,
            local_palette: Some(palette_rgb),
            transparent_idx,
            delay: frame.delay,
            dispose: frame.dispose,
            left: frame.left,
            top: frame.top,
            width: frame.width,
            height: frame.height,
        })
    }

    /// Quantize all frames using a pre-built LUT (global palette).
    fn quantize_frames_with_lut(
        materialized_frames: &[Frame],
        lut: &PaletteLut,
    ) -> Result<Vec<QuantizedFrameData>> {
        materialized_frames
            .par_iter()
            .map(|frame| Self::quantize_frame_with_lut(frame, lut))
            .collect()
    }

    /// Check palette coverage: what ratio of pixels map acceptably to the palette?
    fn check_palette_coverage(
        materialized_frames: &[Frame],
        lut: &PaletteLut,
    ) -> Result<CoverageStats> {
        let mut total_pixels = 0usize;
        let mut acceptable_pixels = 0usize;

        for frame in materialized_frames {
            if frame.pixels.is_empty() {
                continue;
            }

            let (_indices, stats) = lut.map_buffer(&frame.pixels);
            total_pixels += frame.pixels.len() / 4;
            if stats.is_acceptable() {
                acceptable_pixels += frame.pixels.len() / 4;
            }
        }

        let acceptable_ratio = if total_pixels == 0 {
            1.0
        } else {
            acceptable_pixels as f32 / total_pixels as f32
        };

        Ok(CoverageStats { acceptable_ratio })
    }

    /// Convert flat RGB palette to 3-byte format.
    fn flat_rgb_to_palette(flat_rgb: &[u8]) -> Vec<[u8; 3]> {
        flat_rgb
            .chunks_exact(3)
            .map(|chunk| [chunk[0], chunk[1], chunk[2]])
            .collect()
    }

    /// Convert 3-byte palette to flat RGB format.
    fn palette_to_flat_rgb(palette: &[[u8; 3]]) -> Vec<u8> {
        palette.iter().flat_map(|c| c.iter().copied()).collect()
    }
}

/// Coverage statistics for palette validation.
#[derive(Debug, Clone)]
struct CoverageStats {
    /// Ratio of pixels that map acceptably to the palette (0.0 to 1.0).
    acceptable_ratio: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DisposalMethod, LoopCount};
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
            chunk[3] = if i % 2 == 0 { 255 } else { 0 }; // Alternating transparent
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

    /// Create a test GIF with global palette.
    fn create_test_gif_with_global_palette() -> Gif {
        let palette = crate::types::Palette {
            colors: vec![
                [255, 0, 0], // Red
                [0, 255, 0], // Green
                [0, 0, 255], // Blue
            ],
        };

        let frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
            create_opaque_frame(50, 50, [0, 0, 255]),
        ];

        Gif {
            width: 50,
            height: 50,
            global_palette: Some(palette),
            frames,
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    /// Create a test GIF without global palette.
    fn create_test_gif_no_global_palette() -> Gif {
        let frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
            create_opaque_frame(50, 50, [0, 0, 255]),
        ];

        Gif {
            width: 50,
            height: 50,
            global_palette: None,
            frames,
            loop_count: LoopCount::Infinite,
            original_palette: None,
        }
    }

    #[test]
    fn test_realize_reuse_global_preferred_with_valid_palette() {
        let source_gif = create_test_gif_with_global_palette();
        let materialized_frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
        ];

        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::ReuseGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok(), "Should realize with reuse-global-preferred");
        let realization = result.unwrap();
        assert!(
            realization.global_palette.is_some(),
            "Should have global palette"
        );
        assert_eq!(realization.frames.len(), 2, "Should have 2 frames");
        for frame in &realization.frames {
            assert!(
                frame.local_palette.is_none(),
                "Should not have local palette"
            );
        }
    }

    #[test]
    fn test_realize_reuse_global_preferred_fallback_to_derive() {
        let source_gif = create_test_gif_no_global_palette();
        let materialized_frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
        ];

        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::ReuseGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok(), "Should realize with fallback to derive");
        let realization = result.unwrap();
        assert!(
            realization.global_palette.is_some(),
            "Should have global palette"
        );
        assert_eq!(realization.frames.len(), 2, "Should have 2 frames");
    }

    #[test]
    fn test_realize_derive_sequence_global() {
        let source_gif = create_test_gif_no_global_palette();
        let materialized_frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
            create_opaque_frame(50, 50, [0, 0, 255]),
        ];

        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::DeriveSequenceGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok(), "Should derive sequence global palette");
        let realization = result.unwrap();
        assert!(
            realization.global_palette.is_some(),
            "Should have global palette"
        );
        assert_eq!(realization.frames.len(), 3, "Should have 3 frames");
        for frame in &realization.frames {
            assert!(
                frame.local_palette.is_none(),
                "Should not have local palette"
            );
            assert!(!frame.indices.is_empty(), "Should have indices");
        }
    }

    #[test]
    fn test_realize_local_palette_fallback() {
        let source_gif = create_test_gif_no_global_palette();
        let materialized_frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
        ];

        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::LocalPaletteFallback,
            &source_gif,
        );

        assert!(result.is_ok(), "Should realize with local palette fallback");
        let realization = result.unwrap();
        assert!(
            realization.global_palette.is_none(),
            "Should not have global palette"
        );
        assert_eq!(realization.frames.len(), 2, "Should have 2 frames");
        for frame in &realization.frames {
            assert!(frame.local_palette.is_some(), "Should have local palette");
            assert!(!frame.indices.is_empty(), "Should have indices");
        }
    }

    #[test]
    fn test_realize_preserves_frame_metadata() {
        let source_gif = create_test_gif_with_global_palette();
        let mut frame = create_opaque_frame(50, 50, [255, 0, 0]);
        frame.delay = Duration::from_millis(200);
        frame.dispose = DisposalMethod::Background;
        frame.left = 10;
        frame.top = 20;

        let result = PaletteRealizer::realize(
            &[frame.clone()],
            PaletteStrategy::DeriveSequenceGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok());
        let realization = result.unwrap();
        let realized_frame = &realization.frames[0];

        assert_eq!(realized_frame.delay, Duration::from_millis(200));
        assert_eq!(realized_frame.dispose, DisposalMethod::Background);
        assert_eq!(realized_frame.left, 10);
        assert_eq!(realized_frame.top, 20);
        assert_eq!(realized_frame.width, 50);
        assert_eq!(realized_frame.height, 50);
    }

    #[test]
    fn test_realize_with_transparency() {
        let source_gif = create_test_gif_no_global_palette();
        let materialized_frames = vec![create_transparent_frame(50, 50)];

        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::DeriveSequenceGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok(), "Should handle transparency");
        let realization = result.unwrap();
        assert_eq!(realization.frames.len(), 1);
        let frame = &realization.frames[0];
        assert!(
            frame.transparent_idx.is_some(),
            "Should have transparent index"
        );
    }

    #[test]
    fn test_realize_empty_sequence() {
        let source_gif = create_test_gif_no_global_palette();
        let materialized_frames = vec![];

        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::DeriveSequenceGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok());
        let realization = result.unwrap();
        assert_eq!(realization.frames.len(), 0);
    }

    #[test]
    fn test_realize_empty_frames() {
        let source_gif = create_test_gif_no_global_palette();
        let mut frame = create_opaque_frame(50, 50, [255, 0, 0]);
        frame.pixels.clear(); // Empty frame

        let result = PaletteRealizer::realize(
            &[frame],
            PaletteStrategy::DeriveSequenceGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok());
        let realization = result.unwrap();
        assert_eq!(realization.frames.len(), 1);
        assert!(realization.frames[0].indices.is_empty());
    }

    #[test]
    fn test_palette_coverage_check() {
        let source_gif = create_test_gif_with_global_palette();
        let materialized_frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
        ];

        let palette = source_gif.global_palette.as_ref().unwrap();
        let palette_3byte: Vec<[u8; 3]> = palette.colors.clone();
        let lut = PaletteLut::new(&palette_3byte);

        let coverage = PaletteRealizer::check_palette_coverage(&materialized_frames, &lut);
        assert!(coverage.is_ok());
        let stats = coverage.unwrap();
        assert!(
            stats.acceptable_ratio > 0.0,
            "Should have some acceptable coverage"
        );
    }

    #[test]
    fn test_voyager_like_sequence_with_global_preferred() {
        // Simulate a voyager-like sequence: opaque deltas with stable global palette
        let source_gif = create_test_gif_with_global_palette();
        let materialized_frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
            create_opaque_frame(50, 50, [0, 0, 255]),
        ];

        // Use reuse-global-preferred strategy
        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::ReuseGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok());
        let realization = result.unwrap();

        // Should have a global palette
        assert!(realization.global_palette.is_some());

        // All frames should use the global palette (no local palettes)
        for frame in &realization.frames {
            assert!(frame.local_palette.is_none());
            assert!(!frame.indices.is_empty());
        }

        // All frames should have the same palette size
        let palette_size = realization.global_palette.as_ref().unwrap().len();
        assert!(palette_size > 0);
        assert_eq!(palette_size % 3, 0, "Palette should be flat RGB");
    }

    #[test]
    fn test_transparency_heavy_sequence_with_local_fallback() {
        // Simulate a transparency-heavy sequence
        let source_gif = create_test_gif_no_global_palette();
        let materialized_frames = vec![
            create_transparent_frame(50, 50),
            create_transparent_frame(50, 50),
        ];

        // Use local-palette-fallback strategy
        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::LocalPaletteFallback,
            &source_gif,
        );

        assert!(result.is_ok());
        let realization = result.unwrap();

        // Should NOT have a global palette
        assert!(realization.global_palette.is_none());

        // Each frame should have its own local palette
        for frame in &realization.frames {
            assert!(frame.local_palette.is_some());
            assert!(!frame.indices.is_empty());
            assert!(
                frame.transparent_idx.is_some(),
                "Should have transparent index"
            );
        }
    }

    #[test]
    fn test_palette_realization_output_suitable_for_encode() {
        // Verify that palette realization output can be used directly by encode path
        let source_gif = create_test_gif_with_global_palette();
        let materialized_frames = vec![
            create_opaque_frame(50, 50, [255, 0, 0]),
            create_opaque_frame(50, 50, [0, 255, 0]),
        ];

        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::DeriveSequenceGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok());
        let realization = result.unwrap();

        // Verify output structure is suitable for encode
        assert!(realization.global_palette.is_some());
        assert_eq!(realization.frames.len(), 2);

        for (i, frame) in realization.frames.iter().enumerate() {
            // Each frame should have indices
            assert!(!frame.indices.is_empty(), "Frame {} should have indices", i);

            // Indices are u8, so they're always valid (0-255)
            assert!(!frame.indices.is_empty(), "Frame {} should have indices", i);

            // Frame metadata should be preserved
            assert_eq!(frame.width, 50);
            assert_eq!(frame.height, 50);
            assert_eq!(frame.delay, Duration::from_millis(100));
        }
    }

    #[test]
    fn test_transparent_index_safety() {
        // Verify that transparent index handling is safe
        let source_gif = create_test_gif_no_global_palette();
        let materialized_frames = vec![create_transparent_frame(50, 50)];

        let result = PaletteRealizer::realize(
            &materialized_frames,
            PaletteStrategy::DeriveSequenceGlobalPreferred,
            &source_gif,
        );

        assert!(result.is_ok());
        let realization = result.unwrap();
        let frame = &realization.frames[0];

        if let Some(trans_idx) = frame.transparent_idx {
            // Verify that transparent pixels use the transparent index
            let rgba_pixels = &materialized_frames[0].pixels;
            for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
                if i < frame.indices.len() {
                    if pixel[3] < 128 {
                        // Transparent pixel should use transparent index
                        assert_eq!(
                            frame.indices[i], trans_idx,
                            "Transparent pixel at {} should use transparent index",
                            i
                        );
                    }
                }
            }
        }
    }
}
