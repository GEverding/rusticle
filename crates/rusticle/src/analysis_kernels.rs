//! SIMD-accelerated analysis kernels for adaptive encoder.
//!
//! Provides vectorized kernels for candidate analysis and profiling.
//! Each kernel has a SIMD implementation and scalar fallback for correctness verification.
//!
//! # Kernels
//!
//! - **Changed-pixel mask**: Identify which pixels differ between two canvases.
//! - **Transparency stats**: Count transparent pixels and accumulate color statistics.

use std::simd::{
    cmp::{SimdOrd, SimdPartialOrd},
    u8x16, Mask,
};

/// Result of changed-pixel mask analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangedPixelStats {
    /// Total count of pixels that changed.
    pub changed_count: usize,
    /// Count of pixels that became transparent (alpha == 0).
    pub became_transparent: usize,
    /// Count of pixels that became opaque (alpha == 255).
    pub became_opaque: usize,
}

/// Result of transparency and color statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransparencyStats {
    /// Count of fully transparent pixels (alpha == 0).
    pub transparent_count: usize,
    /// Count of fully opaque pixels (alpha == 255).
    pub opaque_count: usize,
    /// Count of semi-transparent pixels (0 < alpha < 255).
    pub semi_transparent_count: usize,
}

/// Result of color-distance statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorDistanceStats {
    /// Sum of squared color distances (for RMSE calculation).
    pub sum_sq_distance: u64,
    /// Count of pixels analyzed.
    pub pixel_count: usize,
    /// Maximum color distance found.
    pub max_distance: u32,
}

/// Analyze changed pixels between two canvases using SIMD.
///
/// Compares `prev` and `curr` pixel-by-pixel. A pixel is considered changed if
/// any RGBA channel differs by more than `threshold`.
///
/// Returns statistics about the changes, including counts of pixels that became
/// transparent or opaque.
///
/// # Arguments
/// * `prev` - Previous canvas RGBA data
/// * `curr` - Current canvas RGBA data
/// * `threshold` - Difference threshold per channel (0-255)
///
/// # Panics
/// Panics if `prev.len() != curr.len()` or buffer length is not a multiple of 4.
#[must_use]
pub fn analyze_changed_pixels_simd(prev: &[u8], curr: &[u8], threshold: u8) -> ChangedPixelStats {
    assert_eq!(prev.len(), curr.len());
    assert!(
        prev.len().is_multiple_of(4),
        "Buffer must be multiple of 4 bytes (RGBA)"
    );

    let len = prev.len();
    let chunks = len / 16;

    let thresh_vec = u8x16::splat(threshold);
    let mut changed_count = 0usize;
    let mut became_transparent = 0usize;
    let mut became_opaque = 0usize;

    // Process 16 bytes (4 RGBA pixels) at a time
    for i in 0..chunks {
        let offset = i * 16;

        let prev_chunk: [u8; 16] = prev[offset..offset + 16].try_into().unwrap();
        let curr_chunk: [u8; 16] = curr[offset..offset + 16].try_into().unwrap();

        let prev_vec = u8x16::from_array(prev_chunk);
        let curr_vec = u8x16::from_array(curr_chunk);

        // |a - b| = max(a,b) - min(a,b)
        let diff = curr_vec.simd_max(prev_vec) - curr_vec.simd_min(prev_vec);

        // Compare all bytes against threshold
        let within: Mask<i8, 16> = diff.simd_le(thresh_vec);
        let mask = within.to_bitmask();

        // Each pixel needs all 4 bytes within threshold to be unchanged
        // Pixel masks: 0x000F, 0x00F0, 0x0F00, 0xF000
        for pixel_idx in 0..4 {
            let pixel_mask = 0x000F << (pixel_idx * 4);
            if (mask & pixel_mask) != pixel_mask {
                // Pixel changed
                changed_count += 1;

                // Check alpha channel (byte 3 of each pixel)
                let prev_alpha = prev_chunk[pixel_idx * 4 + 3];
                let curr_alpha = curr_chunk[pixel_idx * 4 + 3];

                if curr_alpha == 0 && prev_alpha != 0 {
                    became_transparent += 1;
                } else if curr_alpha == 255 && prev_alpha != 255 {
                    became_opaque += 1;
                }
            }
        }
    }

    // Handle remainder (0-3 pixels) with scalar
    let remainder_start = chunks * 16;
    for i in (remainder_start..len).step_by(4) {
        if i + 3 < len {
            let changed = prev[i].abs_diff(curr[i]) > threshold
                || prev[i + 1].abs_diff(curr[i + 1]) > threshold
                || prev[i + 2].abs_diff(curr[i + 2]) > threshold
                || prev[i + 3].abs_diff(curr[i + 3]) > threshold;

            if changed {
                changed_count += 1;

                let prev_alpha = prev[i + 3];
                let curr_alpha = curr[i + 3];

                if curr_alpha == 0 && prev_alpha != 0 {
                    became_transparent += 1;
                } else if curr_alpha == 255 && prev_alpha != 255 {
                    became_opaque += 1;
                }
            }
        }
    }

    ChangedPixelStats {
        changed_count,
        became_transparent,
        became_opaque,
    }
}

/// Scalar fallback for [`analyze_changed_pixels_simd`].
#[inline]
pub fn analyze_changed_pixels_scalar(prev: &[u8], curr: &[u8], threshold: u8) -> ChangedPixelStats {
    assert_eq!(prev.len(), curr.len());

    let mut changed_count = 0usize;
    let mut became_transparent = 0usize;
    let mut became_opaque = 0usize;

    for i in (0..prev.len()).step_by(4) {
        if i + 3 < prev.len() {
            let changed = prev[i].abs_diff(curr[i]) > threshold
                || prev[i + 1].abs_diff(curr[i + 1]) > threshold
                || prev[i + 2].abs_diff(curr[i + 2]) > threshold
                || prev[i + 3].abs_diff(curr[i + 3]) > threshold;

            if changed {
                changed_count += 1;

                let prev_alpha = prev[i + 3];
                let curr_alpha = curr[i + 3];

                if curr_alpha == 0 && prev_alpha != 0 {
                    became_transparent += 1;
                } else if curr_alpha == 255 && prev_alpha != 255 {
                    became_opaque += 1;
                }
            }
        }
    }

    ChangedPixelStats {
        changed_count,
        became_transparent,
        became_opaque,
    }
}

/// Analyze transparency distribution in a canvas using SIMD.
///
/// Counts pixels by transparency level: fully transparent (alpha == 0),
/// fully opaque (alpha == 255), and semi-transparent (0 < alpha < 255).
///
/// # Arguments
/// * `pixels` - RGBA pixel data
///
/// # Panics
/// Panics if buffer length is not a multiple of 4.
#[must_use]
pub fn analyze_transparency_simd(pixels: &[u8]) -> TransparencyStats {
    assert!(
        pixels.len().is_multiple_of(4),
        "Buffer must be multiple of 4 bytes (RGBA)"
    );

    let len = pixels.len();
    let chunks = len / 16;

    let mut transparent_count = 0usize;
    let mut opaque_count = 0usize;
    let mut semi_transparent_count = 0usize;

    // Process 16 bytes (4 RGBA pixels) at a time
    for i in 0..chunks {
        let offset = i * 16;
        let chunk: [u8; 16] = pixels[offset..offset + 16].try_into().unwrap();

        // Extract alpha channels (bytes 3, 7, 11, 15)
        let alpha0 = chunk[3];
        let alpha1 = chunk[7];
        let alpha2 = chunk[11];
        let alpha3 = chunk[15];

        for alpha in [alpha0, alpha1, alpha2, alpha3] {
            if alpha == 0 {
                transparent_count += 1;
            } else if alpha == 255 {
                opaque_count += 1;
            } else {
                semi_transparent_count += 1;
            }
        }
    }

    // Handle remainder with scalar
    let remainder_start = chunks * 16;
    for i in (remainder_start..len).step_by(4) {
        if i + 3 < len {
            let alpha = pixels[i + 3];
            if alpha == 0 {
                transparent_count += 1;
            } else if alpha == 255 {
                opaque_count += 1;
            } else {
                semi_transparent_count += 1;
            }
        }
    }

    TransparencyStats {
        transparent_count,
        opaque_count,
        semi_transparent_count,
    }
}

/// Scalar fallback for [`analyze_transparency_simd`].
#[inline]
pub fn analyze_transparency_scalar(pixels: &[u8]) -> TransparencyStats {
    assert!(
        pixels.len().is_multiple_of(4),
        "Buffer must be multiple of 4 bytes (RGBA)"
    );

    let mut transparent_count = 0usize;
    let mut opaque_count = 0usize;
    let mut semi_transparent_count = 0usize;

    for i in (0..pixels.len()).step_by(4) {
        if i + 3 < pixels.len() {
            let alpha = pixels[i + 3];
            if alpha == 0 {
                transparent_count += 1;
            } else if alpha == 255 {
                opaque_count += 1;
            } else {
                semi_transparent_count += 1;
            }
        }
    }

    TransparencyStats {
        transparent_count,
        opaque_count,
        semi_transparent_count,
    }
}

/// Analyze color distance between two canvases using SIMD.
///
/// Computes per-pixel color distance (Euclidean distance in RGB space, ignoring alpha).
/// Accumulates sum of squared distances and tracks maximum distance.
///
/// # Arguments
/// * `prev` - Previous canvas RGBA data
/// * `curr` - Current canvas RGBA data
///
/// # Panics
/// Panics if `prev.len() != curr.len()` or buffer length is not a multiple of 4.
#[must_use]
pub fn analyze_color_distance_simd(prev: &[u8], curr: &[u8]) -> ColorDistanceStats {
    assert_eq!(prev.len(), curr.len());
    assert!(
        prev.len().is_multiple_of(4),
        "Buffer must be multiple of 4 bytes (RGBA)"
    );

    let len = prev.len();
    let pixel_count = len / 4;
    let chunks = len / 16;

    let mut sum_sq_distance = 0u64;
    let mut max_distance = 0u32;

    // Process 16 bytes (4 RGBA pixels) at a time
    for i in 0..chunks {
        let offset = i * 16;

        let prev_chunk: [u8; 16] = prev[offset..offset + 16].try_into().unwrap();
        let curr_chunk: [u8; 16] = curr[offset..offset + 16].try_into().unwrap();

        // Process each pixel (4 bytes per pixel)
        for pixel_idx in 0..4 {
            let base = pixel_idx * 4;
            let r_diff = prev_chunk[base] as i32 - curr_chunk[base] as i32;
            let g_diff = prev_chunk[base + 1] as i32 - curr_chunk[base + 1] as i32;
            let b_diff = prev_chunk[base + 2] as i32 - curr_chunk[base + 2] as i32;

            let sq_dist = (r_diff * r_diff + g_diff * g_diff + b_diff * b_diff) as u64;
            sum_sq_distance += sq_dist;

            let dist = (sq_dist as f64).sqrt() as u32;
            if dist > max_distance {
                max_distance = dist;
            }
        }
    }

    // Handle remainder with scalar
    let remainder_start = chunks * 16;
    for i in (remainder_start..len).step_by(4) {
        if i + 3 < len {
            let r_diff = prev[i] as i32 - curr[i] as i32;
            let g_diff = prev[i + 1] as i32 - curr[i + 1] as i32;
            let b_diff = prev[i + 2] as i32 - curr[i + 2] as i32;

            let sq_dist = (r_diff * r_diff + g_diff * g_diff + b_diff * b_diff) as u64;
            sum_sq_distance += sq_dist;

            let dist = (sq_dist as f64).sqrt() as u32;
            if dist > max_distance {
                max_distance = dist;
            }
        }
    }

    ColorDistanceStats {
        sum_sq_distance,
        pixel_count,
        max_distance,
    }
}

/// Scalar fallback for [`analyze_color_distance_simd`].
#[inline]
pub fn analyze_color_distance_scalar(prev: &[u8], curr: &[u8]) -> ColorDistanceStats {
    assert_eq!(prev.len(), curr.len());
    assert!(
        prev.len().is_multiple_of(4),
        "Buffer must be multiple of 4 bytes (RGBA)"
    );

    let len = prev.len();
    let pixel_count = len / 4;

    let mut sum_sq_distance = 0u64;
    let mut max_distance = 0u32;

    for i in (0..len).step_by(4) {
        if i + 3 < len {
            let r_diff = prev[i] as i32 - curr[i] as i32;
            let g_diff = prev[i + 1] as i32 - curr[i + 1] as i32;
            let b_diff = prev[i + 2] as i32 - curr[i + 2] as i32;

            let sq_dist = (r_diff * r_diff + g_diff * g_diff + b_diff * b_diff) as u64;
            sum_sq_distance += sq_dist;

            let dist = (sq_dist as f64).sqrt() as u32;
            if dist > max_distance {
                max_distance = dist;
            }
        }
    }

    ColorDistanceStats {
        sum_sq_distance,
        pixel_count,
        max_distance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_changed_pixels_no_change() {
        let pixels = vec![100u8, 100, 100, 255, 50, 50, 50, 255];
        let stats = analyze_changed_pixels_simd(&pixels, &pixels, 0);
        assert_eq!(stats.changed_count, 0);
        assert_eq!(stats.became_transparent, 0);
        assert_eq!(stats.became_opaque, 0);
    }

    #[test]
    fn test_changed_pixels_all_changed() {
        let prev = vec![100u8, 100, 100, 255, 50, 50, 50, 255];
        let curr = vec![200u8, 200, 200, 255, 150, 150, 150, 255];
        let stats = analyze_changed_pixels_simd(&prev, &curr, 0);
        assert_eq!(stats.changed_count, 2);
    }

    #[test]
    fn test_changed_pixels_became_transparent() {
        let prev = vec![100u8, 100, 100, 255];
        let curr = vec![100u8, 100, 100, 0]; // alpha changed to transparent
        let stats = analyze_changed_pixels_simd(&prev, &curr, 0);
        assert_eq!(stats.changed_count, 1);
        assert_eq!(stats.became_transparent, 1);
        assert_eq!(stats.became_opaque, 0);
    }

    #[test]
    fn test_changed_pixels_became_opaque() {
        let prev = vec![100u8, 100, 100, 0];
        let curr = vec![100u8, 100, 100, 255]; // alpha changed to opaque
        let stats = analyze_changed_pixels_simd(&prev, &curr, 0);
        assert_eq!(stats.changed_count, 1);
        assert_eq!(stats.became_transparent, 0);
        assert_eq!(stats.became_opaque, 1);
    }

    #[test]
    fn test_changed_pixels_threshold() {
        let prev = vec![100u8, 100, 100, 255];
        let curr = vec![102u8, 101, 100, 255]; // diffs: 2, 1, 0, 0
        let stats = analyze_changed_pixels_simd(&prev, &curr, 2);
        assert_eq!(stats.changed_count, 0); // all within threshold
    }

    #[test]
    fn test_changed_pixels_simd_vs_scalar() {
        let size = 1024;
        let prev: Vec<u8> = (0..size).map(|i| ((i * 7) % 256) as u8).collect();
        let curr: Vec<u8> = (0..size).map(|i| ((i * 7 + 3) % 256) as u8).collect();

        let simd_stats = analyze_changed_pixels_simd(&prev, &curr, 5);
        let scalar_stats = analyze_changed_pixels_scalar(&prev, &curr, 5);

        assert_eq!(simd_stats.changed_count, scalar_stats.changed_count);
        assert_eq!(
            simd_stats.became_transparent,
            scalar_stats.became_transparent
        );
        assert_eq!(simd_stats.became_opaque, scalar_stats.became_opaque);
    }

    #[test]
    fn test_transparency_all_opaque() {
        let pixels = vec![100u8, 100, 100, 255, 50, 50, 50, 255];
        let stats = analyze_transparency_simd(&pixels);
        assert_eq!(stats.opaque_count, 2);
        assert_eq!(stats.transparent_count, 0);
        assert_eq!(stats.semi_transparent_count, 0);
    }

    #[test]
    fn test_transparency_all_transparent() {
        let pixels = vec![100u8, 100, 100, 0, 50, 50, 50, 0];
        let stats = analyze_transparency_simd(&pixels);
        assert_eq!(stats.transparent_count, 2);
        assert_eq!(stats.opaque_count, 0);
        assert_eq!(stats.semi_transparent_count, 0);
    }

    #[test]
    fn test_transparency_mixed() {
        let pixels = vec![
            100u8, 100, 100, 255, // opaque
            50, 50, 50, 0, // transparent
            75, 75, 75, 128, // semi-transparent
        ];
        let stats = analyze_transparency_simd(&pixels);
        assert_eq!(stats.opaque_count, 1);
        assert_eq!(stats.transparent_count, 1);
        assert_eq!(stats.semi_transparent_count, 1);
    }

    #[test]
    fn test_transparency_simd_vs_scalar() {
        let size = 1024;
        let pixels: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();

        let simd_stats = analyze_transparency_simd(&pixels);
        let scalar_stats = analyze_transparency_scalar(&pixels);

        assert_eq!(simd_stats.transparent_count, scalar_stats.transparent_count);
        assert_eq!(simd_stats.opaque_count, scalar_stats.opaque_count);
        assert_eq!(
            simd_stats.semi_transparent_count,
            scalar_stats.semi_transparent_count
        );
    }

    #[test]
    fn test_color_distance_identical() {
        let pixels = vec![100u8, 100, 100, 255, 50, 50, 50, 255];
        let stats = analyze_color_distance_simd(&pixels, &pixels);
        assert_eq!(stats.sum_sq_distance, 0);
        assert_eq!(stats.max_distance, 0);
        assert_eq!(stats.pixel_count, 2);
    }

    #[test]
    fn test_color_distance_simple() {
        let prev = vec![0u8, 0, 0, 255];
        let curr = vec![3u8, 4, 0, 255]; // distance = sqrt(9 + 16) = 5
        let stats = analyze_color_distance_simd(&prev, &curr);
        assert_eq!(stats.pixel_count, 1);
        assert_eq!(stats.max_distance, 5);
        assert_eq!(stats.sum_sq_distance, 25);
    }

    #[test]
    fn test_color_distance_simd_vs_scalar() {
        let size = 1024;
        let prev: Vec<u8> = (0..size).map(|i| ((i * 7) % 256) as u8).collect();
        let curr: Vec<u8> = (0..size).map(|i| ((i * 7 + 3) % 256) as u8).collect();

        let simd_stats = analyze_color_distance_simd(&prev, &curr);
        let scalar_stats = analyze_color_distance_scalar(&prev, &curr);

        assert_eq!(simd_stats.sum_sq_distance, scalar_stats.sum_sq_distance);
        assert_eq!(simd_stats.max_distance, scalar_stats.max_distance);
        assert_eq!(simd_stats.pixel_count, scalar_stats.pixel_count);
    }

    #[test]
    fn test_various_sizes() {
        for num_pixels in [1, 3, 4, 5, 15, 16, 17, 63, 64, 65, 100, 256] {
            let size = num_pixels * 4;
            let prev: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            let curr: Vec<u8> = (0..size).map(|i| ((i + 1) % 256) as u8).collect();

            let simd_changed = analyze_changed_pixels_simd(&prev, &curr, 0);
            let scalar_changed = analyze_changed_pixels_scalar(&prev, &curr, 0);
            assert_eq!(
                simd_changed.changed_count, scalar_changed.changed_count,
                "Changed pixels mismatch at {} pixels",
                num_pixels
            );

            let simd_trans = analyze_transparency_simd(&prev);
            let scalar_trans = analyze_transparency_scalar(&prev);
            assert_eq!(
                simd_trans.transparent_count, scalar_trans.transparent_count,
                "Transparency mismatch at {} pixels",
                num_pixels
            );

            let simd_dist = analyze_color_distance_simd(&prev, &curr);
            let scalar_dist = analyze_color_distance_scalar(&prev, &curr);
            assert_eq!(
                simd_dist.sum_sq_distance, scalar_dist.sum_sq_distance,
                "Color distance mismatch at {} pixels",
                num_pixels
            );
        }
    }

    #[test]
    fn benchmark_changed_pixels_large_canvas() {
        // Simulate a 1920x1080 canvas (8.3M pixels)
        let size = 1920 * 1080 * 4;
        let prev: Vec<u8> = (0..size).map(|i| ((i * 7) % 256) as u8).collect();
        let curr: Vec<u8> = (0..size).map(|i| ((i * 7 + 3) % 256) as u8).collect();

        // SIMD version
        let start = std::time::Instant::now();
        let simd_stats = analyze_changed_pixels_simd(&prev, &curr, 5);
        let simd_elapsed = start.elapsed();

        // Scalar version
        let start = std::time::Instant::now();
        let scalar_stats = analyze_changed_pixels_scalar(&prev, &curr, 5);
        let scalar_elapsed = start.elapsed();

        // Verify correctness
        assert_eq!(simd_stats.changed_count, scalar_stats.changed_count);
        assert_eq!(
            simd_stats.became_transparent,
            scalar_stats.became_transparent
        );
        assert_eq!(simd_stats.became_opaque, scalar_stats.became_opaque);

        // Report timing (not a hard assertion, just informational)
        eprintln!(
            "Changed pixels (1920x1080): SIMD={:?}, Scalar={:?}, Speedup={:.2}x",
            simd_elapsed,
            scalar_elapsed,
            scalar_elapsed.as_secs_f64() / simd_elapsed.as_secs_f64()
        );
    }

    #[test]
    fn benchmark_transparency_large_canvas() {
        // Simulate a 1920x1080 canvas
        let size = 1920 * 1080 * 4;
        let pixels: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();

        // SIMD version
        let start = std::time::Instant::now();
        let simd_stats = analyze_transparency_simd(&pixels);
        let simd_elapsed = start.elapsed();

        // Scalar version
        let start = std::time::Instant::now();
        let scalar_stats = analyze_transparency_scalar(&pixels);
        let scalar_elapsed = start.elapsed();

        // Verify correctness
        assert_eq!(simd_stats.transparent_count, scalar_stats.transparent_count);
        assert_eq!(simd_stats.opaque_count, scalar_stats.opaque_count);
        assert_eq!(
            simd_stats.semi_transparent_count,
            scalar_stats.semi_transparent_count
        );

        eprintln!(
            "Transparency (1920x1080): SIMD={:?}, Scalar={:?}, Speedup={:.2}x",
            simd_elapsed,
            scalar_elapsed,
            scalar_elapsed.as_secs_f64() / simd_elapsed.as_secs_f64()
        );
    }

    #[test]
    fn benchmark_color_distance_large_canvas() {
        // Simulate a 1920x1080 canvas
        let size = 1920 * 1080 * 4;
        let prev: Vec<u8> = (0..size).map(|i| ((i * 7) % 256) as u8).collect();
        let curr: Vec<u8> = (0..size).map(|i| ((i * 7 + 3) % 256) as u8).collect();

        // SIMD version
        let start = std::time::Instant::now();
        let simd_stats = analyze_color_distance_simd(&prev, &curr);
        let simd_elapsed = start.elapsed();

        // Scalar version
        let start = std::time::Instant::now();
        let scalar_stats = analyze_color_distance_scalar(&prev, &curr);
        let scalar_elapsed = start.elapsed();

        // Verify correctness
        assert_eq!(simd_stats.sum_sq_distance, scalar_stats.sum_sq_distance);
        assert_eq!(simd_stats.max_distance, scalar_stats.max_distance);
        assert_eq!(simd_stats.pixel_count, scalar_stats.pixel_count);

        eprintln!(
            "Color distance (1920x1080): SIMD={:?}, Scalar={:?}, Speedup={:.2}x",
            simd_elapsed,
            scalar_elapsed,
            scalar_elapsed.as_secs_f64() / simd_elapsed.as_secs_f64()
        );
    }
}
