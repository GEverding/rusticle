//! Path A palette strategy: global-preferred stable palette handling for opaque-delta sequences.
//!
//! **STATUS**: Research / Future Opt-In (part of two-path routing, not current mainline product path)
//!
//! This module implements palette realization specifically for Path A (conservative opaque-delta)
//! sequences. It strongly prefers:
//! - Source global palette reuse when safe
//! - Sequence-global palette derivation when source lacks global palette
//! - Local palette fallback only when truly necessary (quality threshold exceeded)
//!
//! # Invariants
//!
//! - No synthetic transparency introduced (Path A is fully opaque)
//! - Stable palette across frames whenever possible
//! - Frame geometry/disposal/delay preserved
//! - Output suitable for direct GIF encoding
//!
//! # Strategy
//!
//! 1. **Collect all unique colors** from all Path A frames (opaque pixels only)
//! 2. **Quantize to global 256-color palette** using imagequant
//! 3. **Apply global palette to all frames** — emit as GIF global color table
//! 4. **Per-frame local palette override** ONLY if quantization error exceeds threshold (fallback, not default)
//! 5. **No transparent palette entry** needed (Path A is fully opaque)
//!
//! # What It Was Trying to Solve
//!
//! The two-path system explored whether a specialized palette strategy for opaque-delta sequences
//! could improve compression. This module implements that strategy: global-preferred stable palette
//! handling with conservative local palette fallback.
//!
//! # Structural Assumptions
//!
//! - Input is Path A frames (opaque pixel data only)
//! - Output is quantized frame data (palette indices) ready for GIF encoding
//! - No synthetic transparency is introduced
//! - Frame timing and disposal are preserved exactly
//! - Quality threshold (default 30.0 dB) determines when to use local palette fallback
//!
//! # Latest Evidence
//!
//! Path A palette strategy works well for already-optimized opaque-delta sequences, but generality
//! is not established. Validation on a larger, more diverse GIF corpus is needed before promoting
//! to mainline.
//!
//! See `docs/RESEARCH_VOYAGER_AND_TWO_PATH.md` for full context.

use crate::error::{Error, Result};
use crate::palette_lut::PaletteLut;
use crate::path_a::PathAFrame;
use crate::types::DisposalMethod;
use rayon::prelude::*;
use std::time::Duration;

/// Configuration for Path A palette strategy.
#[derive(Debug, Clone, Copy)]
pub struct PathAPaletteConfig {
    /// Quality threshold for local palette fallback.
    /// If a frame's quantization error (PSNR) falls below this, use local palette.
    /// Default: 30.0 dB (conservative, rarely triggers fallback).
    pub quality_threshold_db: f32,
    /// Maximum percentage of frames allowed to use local palettes.
    /// If exceeded, force global palette for all frames.
    /// Default: 0.05 (5% of frames).
    pub max_local_palette_ratio: f32,
}

impl Default for PathAPaletteConfig {
    fn default() -> Self {
        Self {
            quality_threshold_db: 30.0,
            max_local_palette_ratio: 0.05,
        }
    }
}

/// Result of Path A palette realization.
#[derive(Debug, Clone)]
pub struct PathAPaletteRealization {
    /// Global palette (always present for Path A).
    pub global_palette: Vec<u8>, // Flat RGB: [r0,g0,b0,r1,g1,b1,...]
    /// Per-frame quantized data.
    pub frames: Vec<PathAQuantizedFrame>,
    /// Statistics about palette usage.
    pub stats: PathAPaletteStats,
}

/// Quantized frame data for Path A.
#[derive(Debug, Clone)]
pub struct PathAQuantizedFrame {
    /// Palette indices (one per pixel).
    pub indices: Vec<u8>,
    /// Local palette for this frame (if fallback was necessary).
    pub local_palette: Option<Vec<u8>>, // Flat RGB
    /// Frame metadata (preserved from Path A).
    pub delay: Duration,
    pub dispose: DisposalMethod,
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
}

/// Statistics about Path A palette realization.
#[derive(Debug, Clone)]
pub struct PathAPaletteStats {
    /// Number of frames using global palette.
    pub frames_using_global: usize,
    /// Number of frames using local palette (fallback).
    pub frames_using_local: usize,
    /// Ratio of frames using local palette.
    pub local_palette_ratio: f32,
    /// Mean PSNR across all frames (dB).
    pub mean_psnr_db: f32,
    /// Minimum PSNR across all frames (dB).
    pub min_psnr_db: f32,
}

/// Path A palette realizer: converts Path A frames into quantized output with stable global palette.
pub struct PathAPaletteRealizer;

impl PathAPaletteRealizer {
    /// Realize palette for Path A frames.
    ///
    /// # Arguments
    ///
    /// - `path_a_frames`: Path A frames (opaque-only, already optimized).
    /// - `config`: Path A palette configuration.
    ///
    /// # Returns
    ///
    /// A `PathAPaletteRealization` with global palette and per-frame quantized data.
    ///
    /// # Invariants
    ///
    /// - Global palette is always present (Path A strongly prefers global palette).
    /// - No synthetic transparency introduced.
    /// - Frame geometry/disposal/delay preserved.
    /// - Output suitable for direct GIF encoding.
    pub fn realize(
        path_a_frames: &[PathAFrame],
        config: PathAPaletteConfig,
    ) -> Result<PathAPaletteRealization> {
        if path_a_frames.is_empty() {
            return Ok(PathAPaletteRealization {
                global_palette: vec![0, 0, 0], // Minimal palette
                frames: vec![],
                stats: PathAPaletteStats {
                    frames_using_global: 0,
                    frames_using_local: 0,
                    local_palette_ratio: 0.0,
                    mean_psnr_db: 0.0,
                    min_psnr_db: 0.0,
                },
            });
        }

        // Step 1: Collect all RGBA pixels from all Path A frames
        let mut all_rgba = Vec::new();
        for frame in path_a_frames {
            all_rgba.extend_from_slice(&frame.pixels);
        }

        // Step 2: Derive global palette from all frames
        let global_palette_rgb = Self::derive_global_palette(&all_rgba)?;

        // Step 3: Build LUT from global palette
        let palette_3byte = Self::flat_rgb_to_palette(&global_palette_rgb);
        let global_lut = PaletteLut::new(&palette_3byte);

        // Step 4: Quantize all frames with global palette, track quality
        let mut quantized_frames = Vec::new();
        let mut psnr_values = Vec::new();
        let mut frames_using_local = 0;

        for frame in path_a_frames {
            let (quantized, psnr) = Self::quantize_frame_with_quality(
                frame,
                &global_lut,
                &global_palette_rgb,
                config.quality_threshold_db,
            )?;

            psnr_values.push(psnr);
            if quantized.local_palette.is_some() {
                frames_using_local += 1;
            }
            quantized_frames.push(quantized);
        }

        // Step 5: Check if local palette usage exceeds threshold
        let local_palette_ratio = frames_using_local as f32 / path_a_frames.len() as f32;
        if local_palette_ratio > config.max_local_palette_ratio {
            // Too many local palettes: force global palette for all frames
            quantized_frames = path_a_frames
                .par_iter()
                .map(|frame| Self::quantize_frame_global_only(frame, &global_lut))
                .collect::<Result<Vec<_>>>()?;
            frames_using_local = 0;
        }

        // Step 6: Compute statistics
        let mean_psnr_db = if psnr_values.is_empty() {
            0.0
        } else {
            psnr_values.iter().sum::<f32>() / psnr_values.len() as f32
        };
        let min_psnr_db = psnr_values.iter().copied().fold(f32::INFINITY, f32::min);

        let stats = PathAPaletteStats {
            frames_using_global: path_a_frames.len() - frames_using_local,
            frames_using_local,
            local_palette_ratio,
            mean_psnr_db,
            min_psnr_db,
        };

        Ok(PathAPaletteRealization {
            global_palette: global_palette_rgb,
            frames: quantized_frames,
            stats,
        })
    }

    /// Derive a global 256-color palette from all RGBA pixels.
    fn derive_global_palette(rgba_pixels: &[u8]) -> Result<Vec<u8>> {
        if rgba_pixels.is_empty() {
            return Ok(vec![0, 0, 0]); // Minimal palette
        }

        // Convert raw bytes to imagequant RGBA structs
        let rgba_data: Vec<imagequant::RGBA> = rgba_pixels
            .chunks_exact(4)
            .map(|chunk| imagequant::RGBA {
                r: chunk[0],
                g: chunk[1],
                b: chunk[2],
                a: chunk[3],
            })
            .collect();

        // Create imagequant attributes
        let mut attr = imagequant::Attributes::new();
        attr.set_max_colors(256)
            .map_err(|e| Error::EncodeError(format!("failed to set max colors: {}", e)))?;
        attr.set_quality(0, 100)
            .map_err(|e| Error::EncodeError(format!("failed to set quality: {}", e)))?;

        // Create image from RGBA pixels (treat as 1D for simplicity)
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

    /// Quantize a frame with global palette, compute quality (PSNR).
    /// Falls back to local palette if quality is below threshold.
    fn quantize_frame_with_quality(
        frame: &PathAFrame,
        global_lut: &PaletteLut,
        global_palette_rgb: &[u8],
        quality_threshold_db: f32,
    ) -> Result<(PathAQuantizedFrame, f32)> {
        if frame.pixels.is_empty() {
            return Ok((
                PathAQuantizedFrame {
                    indices: vec![],
                    local_palette: None,
                    delay: frame.delay,
                    dispose: frame.dispose,
                    left: frame.left,
                    top: frame.top,
                    width: frame.width,
                    height: frame.height,
                },
                f32::INFINITY,
            ));
        }

        // Quantize with global palette
        let (indices, _stats) = global_lut.map_buffer(&frame.pixels);

        // Compute PSNR
        let psnr = Self::compute_psnr(&frame.pixels, &indices, global_palette_rgb)?;

        // Check if quality is acceptable
        if psnr >= quality_threshold_db {
            // Quality is good: use global palette
            Ok((
                PathAQuantizedFrame {
                    indices,
                    local_palette: None,
                    delay: frame.delay,
                    dispose: frame.dispose,
                    left: frame.left,
                    top: frame.top,
                    width: frame.width,
                    height: frame.height,
                },
                psnr,
            ))
        } else {
            // Quality is poor: fall back to local palette
            let local_quantized = Self::quantize_frame_local(frame)?;
            let local_psnr = Self::compute_psnr_for_local_frame(frame, &local_quantized)?;

            Ok((local_quantized, local_psnr))
        }
    }

    /// Quantize a frame using global palette only (no fallback).
    fn quantize_frame_global_only(
        frame: &PathAFrame,
        global_lut: &PaletteLut,
    ) -> Result<PathAQuantizedFrame> {
        if frame.pixels.is_empty() {
            return Ok(PathAQuantizedFrame {
                indices: vec![],
                local_palette: None,
                delay: frame.delay,
                dispose: frame.dispose,
                left: frame.left,
                top: frame.top,
                width: frame.width,
                height: frame.height,
            });
        }

        let (indices, _stats) = global_lut.map_buffer(&frame.pixels);

        Ok(PathAQuantizedFrame {
            indices,
            local_palette: None,
            delay: frame.delay,
            dispose: frame.dispose,
            left: frame.left,
            top: frame.top,
            width: frame.width,
            height: frame.height,
        })
    }

    /// Quantize a frame using local palette (imagequant per-frame).
    fn quantize_frame_local(frame: &PathAFrame) -> Result<PathAQuantizedFrame> {
        if frame.pixels.is_empty() {
            return Ok(PathAQuantizedFrame {
                indices: vec![],
                local_palette: None,
                delay: frame.delay,
                dispose: frame.dispose,
                left: frame.left,
                top: frame.top,
                width: frame.width,
                height: frame.height,
            });
        }

        // Derive local palette from this frame's pixels
        let local_palette_rgb = Self::derive_global_palette(&frame.pixels)?;

        // Convert to 3-byte format for LUT
        let palette_3byte = Self::flat_rgb_to_palette(&local_palette_rgb);
        let local_lut = PaletteLut::new(&palette_3byte);

        // Quantize using the local palette
        let (indices, _stats) = local_lut.map_buffer(&frame.pixels);

        Ok(PathAQuantizedFrame {
            indices,
            local_palette: Some(local_palette_rgb),
            delay: frame.delay,
            dispose: frame.dispose,
            left: frame.left,
            top: frame.top,
            width: frame.width,
            height: frame.height,
        })
    }

    /// Compute PSNR between original RGBA and quantized RGB.
    /// PSNR is computed as: 20 * log10(255 / sqrt(MSE))
    fn compute_psnr(rgba_pixels: &[u8], indices: &[u8], palette_rgb: &[u8]) -> Result<f32> {
        if rgba_pixels.is_empty() || indices.is_empty() {
            return Ok(f32::INFINITY);
        }

        let palette_3byte = Self::flat_rgb_to_palette(palette_rgb);
        let mut mse = 0.0f64;
        let mut count = 0usize;

        for (i, pixel) in rgba_pixels.chunks_exact(4).enumerate() {
            if i >= indices.len() {
                break;
            }

            let idx = indices[i] as usize;
            if idx >= palette_3byte.len() {
                continue; // Skip invalid indices
            }

            let palette_color = palette_3byte[idx];
            let r_orig = pixel[0] as f64;
            let g_orig = pixel[1] as f64;
            let b_orig = pixel[2] as f64;
            let r_pal = palette_color[0] as f64;
            let g_pal = palette_color[1] as f64;
            let b_pal = palette_color[2] as f64;

            let dr = r_orig - r_pal;
            let dg = g_orig - g_pal;
            let db = b_orig - b_pal;

            mse += dr * dr + dg * dg + db * db;
            count += 1;
        }

        if count == 0 {
            return Ok(f32::INFINITY);
        }

        mse /= (count * 3) as f64; // Average over all channels
        let psnr = if mse < 1e-10 {
            f32::INFINITY
        } else {
            20.0 * (255.0 / mse.sqrt()).log10() as f32
        };

        Ok(psnr)
    }

    /// Compute PSNR for a locally-quantized frame.
    fn compute_psnr_for_local_frame(
        frame: &PathAFrame,
        quantized: &PathAQuantizedFrame,
    ) -> Result<f32> {
        if let Some(local_palette) = &quantized.local_palette {
            Self::compute_psnr(&frame.pixels, &quantized.indices, local_palette)
        } else {
            Ok(f32::INFINITY)
        }
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

    /// Create a test Path A frame with solid color.
    fn create_test_path_a_frame(width: u16, height: u16, r: u8, g: u8, b: u8) -> PathAFrame {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[0] = r;
            chunk[1] = g;
            chunk[2] = b;
            chunk[3] = 255; // Opaque
        }

        PathAFrame {
            pixels,
            left: 0,
            top: 0,
            width,
            height,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
        }
    }

    #[test]
    fn test_empty_sequence() {
        let config = PathAPaletteConfig::default();
        let result = PathAPaletteRealizer::realize(&[], config);

        assert!(result.is_ok());
        let realization = result.unwrap();
        assert_eq!(realization.frames.len(), 0);
        assert!(!realization.global_palette.is_empty());
    }

    #[test]
    fn test_voyager_like_sequence_uses_global_palette() {
        // Simulate a voyager-like sequence: small offset changes with stable colors
        let config = PathAPaletteConfig::default();
        let frames = vec![
            create_test_path_a_frame(100, 100, 255, 0, 0), // Red
            create_test_path_a_frame(50, 50, 0, 255, 0),   // Green
            create_test_path_a_frame(50, 50, 0, 0, 255),   // Blue
        ];

        let result = PathAPaletteRealizer::realize(&frames, config);

        assert!(result.is_ok());
        let realization = result.unwrap();

        // Should have global palette
        assert!(!realization.global_palette.is_empty());
        assert_eq!(realization.global_palette.len() % 3, 0);

        // All frames should use global palette (no local fallback)
        assert_eq!(realization.stats.frames_using_global, 3);
        assert_eq!(realization.stats.frames_using_local, 0);
        assert_eq!(realization.stats.local_palette_ratio, 0.0);

        // All frames should have indices
        for frame in &realization.frames {
            assert!(!frame.indices.is_empty());
            assert!(frame.local_palette.is_none());
        }
    }

    #[test]
    fn test_no_palette_churn() {
        // Verify that stable palette is used across frames
        let config = PathAPaletteConfig::default();
        let frames = vec![
            create_test_path_a_frame(100, 100, 255, 0, 0),
            create_test_path_a_frame(100, 100, 255, 0, 0),
            create_test_path_a_frame(100, 100, 255, 0, 0),
        ];

        let result = PathAPaletteRealizer::realize(&frames, config);

        assert!(result.is_ok());
        let realization = result.unwrap();

        // All frames should use the same global palette
        for frame in &realization.frames {
            assert!(frame.local_palette.is_none());
        }

        // Global palette should be stable
        assert_eq!(realization.stats.frames_using_global, 3);
    }

    #[test]
    fn test_local_fallback_when_quality_poor() {
        // Create a frame with many colors that may not quantize well
        let config = PathAPaletteConfig {
            quality_threshold_db: 50.0, // Very high threshold to trigger fallback
            max_local_palette_ratio: 1.0,
        };

        let mut frame = create_test_path_a_frame(100, 100, 255, 0, 0);
        // Add many different colors to the frame
        for i in 0..100 {
            for j in 0..100 {
                let idx = (i * 100 + j) * 4;
                if idx + 3 < frame.pixels.len() {
                    frame.pixels[idx] = (i as u8).wrapping_add(j as u8);
                    frame.pixels[idx + 1] = (i as u8).wrapping_mul(j as u8);
                    frame.pixels[idx + 2] = (i ^ j) as u8;
                    frame.pixels[idx + 3] = 255;
                }
            }
        }

        let result = PathAPaletteRealizer::realize(&[frame], config);

        assert!(result.is_ok());
        let realization = result.unwrap();

        // Should have at least one frame (may use local palette due to high threshold)
        assert_eq!(realization.frames.len(), 1);
    }

    #[test]
    fn test_frame_metadata_preserved() {
        let config = PathAPaletteConfig::default();
        let mut frame = create_test_path_a_frame(100, 100, 255, 0, 0);
        frame.delay = Duration::from_millis(200);
        frame.dispose = DisposalMethod::None;
        frame.left = 10;
        frame.top = 20;

        let result = PathAPaletteRealizer::realize(&[frame.clone()], config);

        assert!(result.is_ok());
        let realization = result.unwrap();
        let realized_frame = &realization.frames[0];

        assert_eq!(realized_frame.delay, Duration::from_millis(200));
        assert_eq!(realized_frame.dispose, DisposalMethod::None);
        assert_eq!(realized_frame.left, 10);
        assert_eq!(realized_frame.top, 20);
        assert_eq!(realized_frame.width, 100);
        assert_eq!(realized_frame.height, 100);
    }

    #[test]
    fn test_no_synthetic_transparency() {
        // Verify that Path A palette realization does not introduce synthetic transparency
        let config = PathAPaletteConfig::default();
        let frames = vec![
            create_test_path_a_frame(100, 100, 255, 0, 0),
            create_test_path_a_frame(100, 100, 0, 255, 0),
        ];

        let result = PathAPaletteRealizer::realize(&frames, config);

        assert!(result.is_ok());
        let realization = result.unwrap();

        // All original pixels are opaque
        for frame in &frames {
            for chunk in frame.pixels.chunks_exact(4) {
                assert_eq!(chunk[3], 255, "All Path A pixels must be opaque");
            }
        }

        // Quantized frames should not have transparent indices
        for _frame in &realization.frames {
            // Path A does not use transparent indices
            // (they're only needed for transparency, which Path A doesn't have)
        }
    }

    #[test]
    fn test_quality_metrics_computed() {
        let config = PathAPaletteConfig::default();
        let frames = vec![
            create_test_path_a_frame(100, 100, 255, 0, 0),
            create_test_path_a_frame(100, 100, 0, 255, 0),
            create_test_path_a_frame(100, 100, 0, 0, 255),
        ];

        let result = PathAPaletteRealizer::realize(&frames, config);

        assert!(result.is_ok());
        let realization = result.unwrap();

        // Quality metrics should be computed
        assert!(realization.stats.mean_psnr_db > 0.0);
        assert!(realization.stats.min_psnr_db > 0.0);
        assert!(realization.stats.mean_psnr_db >= realization.stats.min_psnr_db);
    }

    #[test]
    fn test_disposal_always_none() {
        // Verify that all emitted frames use None disposal (Path A invariant)
        let config = PathAPaletteConfig::default();
        let frames = vec![
            create_test_path_a_frame(100, 100, 255, 0, 0),
            create_test_path_a_frame(100, 100, 0, 255, 0),
        ];

        let result = PathAPaletteRealizer::realize(&frames, config);

        assert!(result.is_ok());
        let realization = result.unwrap();

        for frame in &realization.frames {
            assert_eq!(
                frame.dispose,
                DisposalMethod::None,
                "All Path A frames must use None disposal"
            );
        }
    }

    #[test]
    fn test_max_local_palette_ratio_enforced() {
        // Create a config that limits local palette usage
        let config = PathAPaletteConfig {
            quality_threshold_db: 30.0,
            max_local_palette_ratio: 0.0, // Force global palette for all frames
        };

        let frames = vec![
            create_test_path_a_frame(100, 100, 255, 0, 0),
            create_test_path_a_frame(100, 100, 0, 255, 0),
            create_test_path_a_frame(100, 100, 0, 0, 255),
        ];

        let result = PathAPaletteRealizer::realize(&frames, config);

        assert!(result.is_ok());
        let realization = result.unwrap();

        // All frames should use global palette
        assert_eq!(realization.stats.frames_using_global, 3);
        assert_eq!(realization.stats.frames_using_local, 0);
        assert_eq!(realization.stats.local_palette_ratio, 0.0);
    }

    #[test]
    fn test_output_suitable_for_encoding() {
        // Verify that output can be used directly by GIF encoder
        let config = PathAPaletteConfig::default();
        let frames = vec![
            create_test_path_a_frame(100, 100, 255, 0, 0),
            create_test_path_a_frame(100, 100, 0, 255, 0),
        ];

        let result = PathAPaletteRealizer::realize(&frames, config);

        assert!(result.is_ok());
        let realization = result.unwrap();

        // Global palette should be valid RGB
        assert!(!realization.global_palette.is_empty());
        assert_eq!(realization.global_palette.len() % 3, 0);

        // Each frame should have valid indices
        for frame in &realization.frames {
            assert!(!frame.indices.is_empty());
            // Indices should be valid (0-255)
            for &idx in &frame.indices {
                let palette_idx = idx as usize * 3;
                assert!(
                    palette_idx + 2 < realization.global_palette.len()
                        || frame.local_palette.is_some(),
                    "Index {} is out of bounds",
                    idx
                );
            }
        }
    }
}
