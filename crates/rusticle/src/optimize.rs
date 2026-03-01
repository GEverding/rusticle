//! Frame optimization - transparency optimization for GIF frames.
//!
//! Marks unchanged pixels as transparent to reduce file size.

use crate::simd_opt::{find_diff_bounding_box, mark_unchanged_pixels_simd, DiffRect};
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

        // Only crop to bounding box for O3 (most aggressive)
        let crop_to_bbox = level == OptLevel::O3;

        let optimized_frames = optimize_frames_internal(&self.frames, threshold, crop_to_bbox);

        Gif {
            width: self.width,
            height: self.height,
            global_palette: self.global_palette.clone(),
            frames: optimized_frames,
            loop_count: self.loop_count,
            // Optimization changes pixel content and can introduce sparse/cropped frames,
            // so the decode-time source palette is no longer a reliable fast-path palette.
            // Force re-quantization on encode to avoid stale-palette quality collapse.
            original_palette: None,
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

        // Lossy diffing assumes each frame buffer covers the same canvas region.
        // For pre-cropped/sub-frame data (e.g. output of optimize O3), a second
        // diff pass can compare spatially mismatched buffers and over-remove detail.
        // In that case, keep current frames unchanged.
        let has_subframes = self
            .frames
            .iter()
            .any(|f| f.left != 0 || f.top != 0 || f.width != self.width || f.height != self.height);
        if has_subframes {
            return self;
        }

        // Clamp quality to 0-100 range
        let quality = quality.min(100);

        // Calculate threshold: quality 100 = 0 (lossless), quality 0 = 20 (conservative max)
        let threshold = ((100 - quality as u16) * 20 / 100) as u8;

        // Always crop to bounding box for lossy (maximize compression)
        let crop_to_bbox = true;

        let lossy_frames = optimize_frames_internal(&self.frames, threshold, crop_to_bbox);

        Gif {
            width: self.width,
            height: self.height,
            global_palette: self.global_palette.clone(),
            frames: lossy_frames,
            loop_count: self.loop_count,
            // Lossy modifies frame pixels by thresholding to transparency.
            // Keep encode quality stable by disabling stale decode-palette fast path.
            original_palette: None,
        }
    }
}

/// Internal optimization with configurable threshold and cropping.
/// Shared by both optimize() and lossy() methods.
fn optimize_frames_internal(frames: &[Frame], threshold: u8, crop_to_bbox: bool) -> Vec<Frame> {
    frames
        .par_iter()
        .enumerate()
        .map(|(idx, frame)| {
            if idx == 0 {
                // First frame is never optimized
                frame.clone()
            } else {
                let prev_frame = &frames[idx - 1];
                optimize_frame_internal(frame, prev_frame, threshold, crop_to_bbox)
            }
        })
        .collect()
}

/// Optimize a single frame by comparing to the previous frame.
/// Uses SIMD-accelerated pixel comparison and diff-based bounding box.
fn optimize_frame_internal(
    frame: &Frame,
    prev_frame: &Frame,
    threshold: u8,
    crop_to_bbox: bool,
) -> Frame {
    // Only optimize if frames represent the same region.
    // For cropped/sub-frame pipelines, same dimensions are not enough:
    // left/top must also match, otherwise pixel-wise comparison is spatially invalid.
    if frame.width != prev_frame.width
        || frame.height != prev_frame.height
        || frame.left != prev_frame.left
        || frame.top != prev_frame.top
    {
        let mut optimized = frame.clone();
        optimized.dispose = DisposalMethod::Keep;
        return optimized;
    }

    let width = frame.width as usize;
    let height = frame.height as usize;

    // Find diff bounding box FIRST
    let diff_rect =
        match find_diff_bounding_box(&prev_frame.pixels, &frame.pixels, width, height, threshold) {
            Some(rect) => rect,
            None => {
                // Frames are identical - return minimal frame (1x1 transparent)
                return Frame {
                    pixels: vec![0, 0, 0, 0],
                    delay: frame.delay,
                    dispose: DisposalMethod::Keep,
                    local_palette: None,
                    left: 0,
                    top: 0,
                    width: 1,
                    height: 1,
                };
            }
        };

    // If crop_to_bbox: extract and process only the diff region
    // Otherwise: process full frame but mark unchanged pixels
    if crop_to_bbox {
        extract_and_optimize_subframe(frame, prev_frame, &diff_rect, threshold)
    } else {
        // Full frame processing (existing behavior)
        let mut optimized = frame.clone();
        let min_len = optimized.pixels.len().min(prev_frame.pixels.len());
        if min_len >= 4 && min_len.is_multiple_of(4) {
            mark_unchanged_pixels_simd(
                &mut optimized.pixels[..min_len],
                &prev_frame.pixels[..min_len],
                threshold,
            );
        }
        optimized.dispose = DisposalMethod::Keep;
        optimized
    }
}

/// Extract sub-frame at diff_rect and mark unchanged pixels transparent.
fn extract_and_optimize_subframe(
    frame: &Frame,
    prev_frame: &Frame,
    diff_rect: &DiffRect,
    threshold: u8,
) -> Frame {
    let src_width = frame.width as usize;
    let new_width = diff_rect.width as usize;
    let new_height = diff_rect.height as usize;

    // Extract pixels from both frames at the diff region
    let mut new_pixels = vec![0u8; new_width * new_height * 4];
    let mut prev_pixels = vec![0u8; new_width * new_height * 4];

    for y in 0..new_height {
        for x in 0..new_width {
            let src_x = diff_rect.left as usize + x;
            let src_y = diff_rect.top as usize + y;
            let src_idx = (src_y * src_width + src_x) * 4;
            let dst_idx = (y * new_width + x) * 4;

            new_pixels[dst_idx..dst_idx + 4].copy_from_slice(&frame.pixels[src_idx..src_idx + 4]);
            prev_pixels[dst_idx..dst_idx + 4]
                .copy_from_slice(&prev_frame.pixels[src_idx..src_idx + 4]);
        }
    }

    // Apply transparency marking within the extracted region
    let min_len = new_pixels.len();
    if min_len >= 4 && min_len.is_multiple_of(4) {
        mark_unchanged_pixels_simd(&mut new_pixels, &prev_pixels, threshold);
    }

    Frame {
        pixels: new_pixels,
        delay: frame.delay,
        dispose: DisposalMethod::Keep,
        local_palette: None,
        left: diff_rect.left,
        top: diff_rect.top,
        width: diff_rect.width,
        height: diff_rect.height,
    }
}

/// Check if two pixels are similar within threshold (per-channel).
#[inline]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
        // Quality 0 should have threshold 20, so pixels with diff 1 should be transparent.
        // With cropping enabled for lossy, all pixels are identical within threshold,
        // so the frame becomes minimal (1x1 transparent).
        let frame2_lossy = &lossy.frames[1];
        assert_eq!(frame2_lossy.width, 1);
        assert_eq!(frame2_lossy.height, 1);
        assert_eq!(frame2_lossy.pixels.len(), 4); // 1x1 RGBA
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

    #[test]
    fn test_optimize_identical_frames_returns_minimal() {
        let frame1 = make_frame(10, 10, [255, 0, 0, 255]);
        let frame2 = make_frame(10, 10, [255, 0, 0, 255]); // identical

        let gif = Gif {
            width: 10,
            height: 10,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: None,
        };

        let optimized = gif.optimize(OptLevel::O3);
        // Second frame should be minimal (1x1)
        assert_eq!(optimized.frames[1].width, 1);
        assert_eq!(optimized.frames[1].height, 1);
        assert_eq!(optimized.frames[1].pixels.len(), 4); // 1x1 RGBA
    }

    #[test]
    fn test_optimize_clears_original_palette_for_safe_requantization() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [0, 255, 0, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: Some(vec![[255, 0, 0], [0, 255, 0]]),
        };

        let optimized = gif.optimize(OptLevel::O3);
        assert!(optimized.original_palette.is_none());
    }

    #[test]
    fn test_lossy_clears_original_palette_for_safe_requantization() {
        let frame1 = make_frame(2, 2, [255, 0, 0, 255]);
        let frame2 = make_frame(2, 2, [254, 1, 1, 255]);

        let gif = Gif {
            width: 2,
            height: 2,
            global_palette: None,
            frames: vec![frame1, frame2],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: Some(vec![[255, 0, 0], [0, 255, 0]]),
        };

        let lossy = gif.lossy(80);
        assert!(lossy.original_palette.is_none());
    }

    #[test]
    fn test_optimize_skips_misaligned_subframes() {
        let prev_frame = Frame {
            pixels: vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 0,
            top: 0,
            width: 2,
            height: 2,
        };

        let frame = Frame {
            pixels: vec![
                0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255,
            ],
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 1,
            top: 0,
            width: 2,
            height: 2,
        };

        let optimized = optimize_frame_internal(&frame, &prev_frame, 8, true);
        assert_eq!(optimized.pixels, frame.pixels);
        assert_eq!(optimized.left, frame.left);
        assert_eq!(optimized.top, frame.top);
        assert_eq!(optimized.width, frame.width);
        assert_eq!(optimized.height, frame.height);
    }

    #[test]
    fn test_lossy_noop_on_subframes() {
        let frame = Frame {
            pixels: vec![
                0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255,
            ],
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::Keep,
            local_palette: None,
            left: 1,
            top: 0,
            width: 2,
            height: 2,
        };

        let gif = Gif {
            width: 4,
            height: 4,
            global_palette: None,
            frames: vec![frame.clone()],
            loop_count: crate::types::LoopCount::Infinite,
            original_palette: Some(vec![[0, 255, 0]]),
        };

        let lossy = gif.lossy(80);
        assert_eq!(lossy.frames[0].pixels, frame.pixels);
        assert_eq!(lossy.frames[0].left, 1);
        assert_eq!(lossy.frames[0].top, 0);
    }
}
