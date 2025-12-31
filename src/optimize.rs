//! Frame optimization - transparency optimization for GIF frames.
//!
//! Marks unchanged pixels as transparent to reduce file size.

use crate::simd_opt::mark_unchanged_pixels_simd;
use crate::types::{DisposalMethod, Frame, Gif, OptLevel};
use rayon::prelude::*;

impl Gif {
    /// Optimize frames by marking unchanged pixels transparent.
    ///
    /// For each frame (except the first), compares pixels to the previous frame
    /// and marks unchanged pixels as transparent. The aggressiveness of matching
    /// is controlled by the optimization level.
    ///
    /// # Arguments
    /// * `level` - Optimization level controlling pixel matching threshold
    ///
    /// # Returns
    /// A new Gif with optimized frames
    ///
    /// # Example
    /// ```ignore
    /// let gif = Gif::from_bytes(&data)?;
    /// let optimized = gif.optimize(OptLevel::O3);
    /// ```
    #[must_use]
    pub fn optimize(self, level: OptLevel) -> Gif {
        if self.frames.is_empty() {
            return self.clone();
        }

        let threshold = match level {
            OptLevel::O1 => 0,
            OptLevel::O2 => 2,
            OptLevel::O3 => 8,
        };

        let optimized_frames = self
            .frames
            .par_iter()
            .enumerate()
            .map(|(idx, frame)| {
                if idx == 0 {
                    // First frame is never optimized
                    frame.clone()
                } else {
                    let prev_frame = &self.frames[idx - 1];
                    optimize_frame(frame, prev_frame, threshold, level)
                }
            })
            .collect();

        Gif {
            width: self.width,
            height: self.height,
            global_palette: self.global_palette.clone(),
            frames: optimized_frames,
            loop_count: self.loop_count,
            original_palette: self.original_palette.clone(),
        }
    }

    /// Lossy compression - mark "close enough" pixels as transparent.
    ///
    /// Compares each frame to the previous frame and marks pixels that are
    /// within a perceptual distance threshold as transparent. This allows
    /// the encoder to skip storing those pixels, resulting in significant
    /// size reduction.
    ///
    /// Quality 100 is lossless (threshold 0). Lower quality values increase
    /// the threshold, making more pixels transparent and reducing file size.
    ///
    /// # Arguments
    /// * `quality` - Quality level 0-100 (100 = lossless, 0 = maximum loss)
    ///
    /// # Returns
    /// A new Gif with lossy compression applied
    ///
    /// # Examples
    /// ```ignore
    /// let gif = Gif::from_bytes(&data)?;
    /// let compressed = gif.lossy(80); // 20% quality loss for size reduction
    /// ```
    #[must_use]
    pub fn lossy(self, quality: u8) -> Gif {
        if self.frames.is_empty() {
            return self.clone();
        }

        // Clamp quality to 0-100 range
        let quality = quality.min(100);

        // Calculate threshold: quality 100 = 0 (lossless), quality 0 = 20 (conservative max)
        let threshold = ((100 - quality as u16) * 20 / 100) as u8;

        // Compare each frame to the previous frame, like optimize does.
        // This avoids error accumulation from maintaining a separate canvas
        // that gets out of sync with actual encode/decode pixel values.
        let lossy_frames: Vec<Frame> = self
            .frames
            .iter()
            .enumerate()
            .map(|(idx, frame)| {
                if idx == 0 {
                    // First frame: no optimization
                    frame.clone()
                } else {
                    // Compare to previous frame
                    let prev_frame = &self.frames[idx - 1];
                    apply_lossy_frame(frame, &prev_frame.pixels, threshold)
                }
            })
            .collect();

        Gif {
            width: self.width,
            height: self.height,
            global_palette: self.global_palette.clone(),
            frames: lossy_frames,
            loop_count: self.loop_count,
            original_palette: self.original_palette.clone(),
        }
    }
}

/// Optimize a single frame by comparing to the previous frame.
/// Uses SIMD-accelerated pixel comparison.
fn optimize_frame(frame: &Frame, prev_frame: &Frame, threshold: u8, level: OptLevel) -> Frame {
    let mut optimized = frame.clone();

    // Only optimize if frames have compatible dimensions
    if frame.width != prev_frame.width || frame.height != prev_frame.height {
        optimized.dispose = DisposalMethod::Keep;
        return optimized;
    }

    let width = frame.width as usize;
    let height = frame.height as usize;

    // Use SIMD to mark unchanged pixels as transparent
    let min_len = optimized.pixels.len().min(prev_frame.pixels.len());
    if min_len >= 4 && min_len % 4 == 0 {
        mark_unchanged_pixels_simd(
            &mut optimized.pixels[..min_len],
            &prev_frame.pixels[..min_len],
            threshold,
        );
    }

    // For O3, crop to bounding box of changed pixels
    if level == OptLevel::O3 {
        if let Some((left, top, right, bottom)) =
            find_bounding_box(&optimized.pixels, width, height)
        {
            optimized = crop_frame(&optimized, left, top, right, bottom);
        }
    }

    // Set disposal method to Keep (don't clear before next frame)
    optimized.dispose = DisposalMethod::Keep;

    optimized
}

/// Apply lossy compression to a single frame.
/// Uses SIMD-accelerated pixel comparison.
fn apply_lossy_frame(frame: &Frame, canvas: &[u8], threshold: u8) -> Frame {
    let mut lossy = frame.clone();

    // Use SIMD to mark similar pixels as transparent
    let min_len = lossy.pixels.len().min(canvas.len());
    if min_len >= 4 && min_len % 4 == 0 {
        mark_unchanged_pixels_simd(&mut lossy.pixels[..min_len], &canvas[..min_len], threshold);
    }

    // Set disposal method to Keep
    lossy.dispose = DisposalMethod::Keep;

    lossy
}

/// Check if two pixels are similar within threshold (per-channel).
#[inline]
fn pixels_similar(a: &[u8; 4], b: &[u8; 4], threshold: u8) -> bool {
    a[0].abs_diff(b[0]) <= threshold
        && a[1].abs_diff(b[1]) <= threshold
        && a[2].abs_diff(b[2]) <= threshold
        && a[3].abs_diff(b[3]) <= threshold
}

/// Check if two colors are similar using perceptual distance.
///
/// Uses weighted RGB distance where human eye is more sensitive to green:
/// - R: weight 0.3
/// - G: weight 0.5
/// - B: weight 0.2
#[inline]
#[allow(dead_code)]
fn colors_similar(a: &[u8; 4], b: &[u8; 4], threshold: u8) -> bool {
    // Ignore alpha channel for color comparison
    let dr = a[0].abs_diff(b[0]) as u32;
    let dg = a[1].abs_diff(b[1]) as u32;
    let db = a[2].abs_diff(b[2]) as u32;

    // Perceptual distance: weighted RGB
    // Weights: R=0.3, G=0.5, B=0.2 (normalized to sum to 1.0)
    // Multiply by 10 to avoid floating point: R=3, G=5, B=2
    let weighted_distance = (dr * 3 + dg * 5 + db * 2) / 10;

    weighted_distance <= threshold as u32
}

/// Find bounding box of non-transparent pixels.
/// Returns (left, top, right, bottom) or None if all pixels are transparent.
fn find_bounding_box(pixels: &[u8], width: usize, height: usize) -> Option<(u16, u16, u16, u16)> {
    let mut left = width;
    let mut top = height;
    let mut right = 0;
    let mut bottom = 0;

    for y in 0..height {
        for x in 0..width {
            let pixel_idx = (y * width + x) * 4;
            let alpha = pixels.get(pixel_idx + 3).copied().unwrap_or(0);

            if alpha > 0 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x);
                bottom = bottom.max(y);
            }
        }
    }

    if left <= right && top <= bottom {
        Some((
            left as u16,
            top as u16,
            (right + 1) as u16,
            (bottom + 1) as u16,
        ))
    } else {
        None
    }
}

/// Crop frame to specified bounds.
fn crop_frame(frame: &Frame, left: u16, top: u16, right: u16, bottom: u16) -> Frame {
    let new_width = (right - left) as usize;
    let new_height = (bottom - top) as usize;
    let old_width = frame.width as usize;

    let mut new_pixels = vec![0u8; new_width * new_height * 4];

    for y in 0..new_height {
        for x in 0..new_width {
            let old_x = left as usize + x;
            let old_y = top as usize + y;

            let old_idx = (old_y * old_width + old_x) * 4;
            let new_idx = (y * new_width + x) * 4;

            if old_idx + 3 < frame.pixels.len() {
                new_pixels[new_idx] = frame.pixels[old_idx];
                new_pixels[new_idx + 1] = frame.pixels[old_idx + 1];
                new_pixels[new_idx + 2] = frame.pixels[old_idx + 2];
                new_pixels[new_idx + 3] = frame.pixels[old_idx + 3];
            }
        }
    }

    Frame {
        pixels: new_pixels,
        delay: frame.delay,
        dispose: frame.dispose,
        local_palette: frame.local_palette.clone(),
        left: frame.left + left,
        top: frame.top + top,
        width: new_width as u16,
        height: new_height as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_frame(width: u16, height: u16, color: [u8; 4]) -> Frame {
        let mut pixels = Vec::new();
        for _ in 0..(width as usize * height as usize) {
            pixels.extend_from_slice(&color);
        }
        Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        }
    }

    #[test]
    fn test_pixels_similar_exact() {
        let a = [255, 128, 64, 255];
        let b = [255, 128, 64, 255];
        assert!(pixels_similar(&a, &b, 0));
    }

    #[test]
    fn test_pixels_similar_threshold() {
        let a = [255, 128, 64, 255];
        let b = [254, 129, 65, 254];
        assert!(pixels_similar(&a, &b, 2));
        assert!(!pixels_similar(&a, &b, 0));
    }

    #[test]
    fn test_colors_similar_exact() {
        let a = [255, 128, 64, 255];
        let b = [255, 128, 64, 255];
        assert!(colors_similar(&a, &b, 0));
    }

    #[test]
    fn test_colors_similar_perceptual() {
        // Same color difference in different channels should have different distances
        let base = [128, 128, 128, 255];
        let red_diff = [138, 128, 128, 255]; // +10 red
        let green_diff = [128, 138, 128, 255]; // +10 green
        let blue_diff = [128, 128, 138, 255]; // +10 blue

        // Green difference should be largest (weight 0.5)
        // Red difference should be medium (weight 0.3)
        // Blue difference should be smallest (weight 0.2)
        let threshold = 5;
        assert!(colors_similar(&base, &red_diff, threshold)); // 10*3/10 = 3, should pass
        assert!(colors_similar(&base, &green_diff, threshold)); // 10*5/10 = 5, should pass
        assert!(colors_similar(&base, &blue_diff, threshold)); // 10*2/10 = 2, should pass

        // With lower threshold, green diff should fail
        let threshold = 4;
        assert!(colors_similar(&base, &red_diff, threshold)); // 3 <= 4, pass
        assert!(!colors_similar(&base, &green_diff, threshold)); // 5 > 4, fail
        assert!(colors_similar(&base, &blue_diff, threshold)); // 2 <= 4, pass
    }

    #[test]
    fn test_optimize_first_frame_unchanged() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [0, 255, 0, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1.clone(), frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        assert_eq!(optimized.frames[0].pixels, frame1.pixels);
    }

    #[test]
    fn test_optimize_marks_unchanged_transparent() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let mut frame2_pixels = Vec::new();
        for _ in 0..4 {
            frame2_pixels.extend_from_slice(&[255, 0, 0, 255]);
        }
        // Change one pixel
        frame2_pixels[0] = 0;
        frame2_pixels[1] = 255;

        let frame2 = Frame {
            pixels: frame2_pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        };

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        let frame2_opt = &optimized.frames[1];

        // First pixel should be opaque (changed)
        assert_eq!(frame2_opt.pixels[3], 255);

        // Other pixels should be transparent (unchanged)
        assert_eq!(frame2_opt.pixels[7], 0); // Second pixel alpha
        assert_eq!(frame2_opt.pixels[11], 0); // Third pixel alpha
        assert_eq!(frame2_opt.pixels[15], 0); // Fourth pixel alpha
    }

    #[test]
    fn test_optimize_sets_disposal_keep() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [255, 0, 0, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O1);
        assert_eq!(optimized.frames[1].dispose, DisposalMethod::Keep);
    }

    #[test]
    fn test_lossy_first_frame_unchanged() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [0, 255, 0, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1.clone(), frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let lossy = gif.clone().lossy(80);
        assert_eq!(lossy.frames[0].pixels, frame1.pixels);
    }

    #[test]
    fn test_lossy_quality_100_lossless() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [254, 1, 1, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let lossy = gif.lossy(100);
        // Quality 100 should have threshold 0, so no pixels should be marked transparent
        assert_eq!(lossy.frames[1].pixels, frame2.pixels);
    }

    #[test]
    fn test_lossy_quality_0_maximum_loss() {
        let frame1 = make_frame(2, 2, [128, 128, 128, 255]);
        let frame2 = make_frame(2, 2, [129, 129, 129, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let lossy = gif.lossy(0);
        // Quality 0 should have threshold 20, so pixels with diff 1 should be transparent
        let frame2_lossy = &lossy.frames[1];
        for i in 0..4 {
            assert_eq!(
                frame2_lossy.pixels[i * 4 + 3],
                0,
                "Pixel {} should be transparent",
                i
            );
        }
    }

    #[test]
    fn test_lossy_threshold_calculation() {
        // New formula: threshold = (100 - quality) * 20 / 100
        // quality 80 = 4, quality 50 = 10, quality 0 = 20

        let frame1 = make_frame(1, 1, [128, 128, 128, 255]);
        let frame2 = make_frame(1, 1, [130, 128, 128, 255]); // +2 red

        let gif = Gif {
            width: 1,
            height: 1,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        // Quality 80: threshold = 4
        // Red diff: 2, should be transparent (2 <= 4)
        let lossy = gif.clone().lossy(80);
        assert_eq!(
            lossy.frames[1].pixels[3], 0,
            "Pixel should be transparent at quality 80 (diff 2 <= threshold 4)"
        );

        // Quality 95: threshold = 1
        // Red diff: 2, should NOT be transparent (2 > 1)
        let lossy = gif.lossy(95);
        assert_eq!(
            lossy.frames[1].pixels[3], 255,
            "Pixel should be opaque at quality 95 (diff 2 > threshold 1)"
        );
    }
}
