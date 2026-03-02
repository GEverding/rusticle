//! SIMD-accelerated frame optimization.
//!
//! Provides vectorized pixel comparison and transparency marking.
//! Uses std::simd for portable SIMD across ARM NEON and x86 SSE/AVX.

use std::simd::{cmp::SimdOrd, cmp::SimdPartialOrd, u8x16, Mask};

/// Bounding box of the region that differs between two frames.
///
/// Returned by [`find_diff_bounding_box`]. Coordinates are in pixels
/// relative to the canvas origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffRect {
    /// Horizontal offset of the diff region (pixels from left edge).
    pub left: u16,
    /// Vertical offset of the diff region (pixels from top edge).
    pub top: u16,
    /// Width of the diff region in pixels.
    pub width: u16,
    /// Height of the diff region in pixels.
    pub height: u16,
}

/// Mark unchanged pixels as transparent using SIMD comparison.
///
/// Compares `current` against `previous` pixel-by-pixel. If all four RGBA
/// channels differ by at most `threshold`, the pixel is zeroed (transparent).
/// Returns the number of pixels marked transparent.
///
/// Processes 4 pixels (16 bytes) at a time via portable SIMD, with a scalar
/// fallback for remainder pixels.
///
/// # Panics
///
/// Panics if `current.len() != previous.len()` or buffer length is not a
/// multiple of 4.
#[inline]
pub fn mark_unchanged_pixels_simd(current: &mut [u8], previous: &[u8], threshold: u8) -> usize {
    assert_eq!(current.len(), previous.len());
    assert!(
        current.len().is_multiple_of(4),
        "Buffer must be multiple of 4 bytes (RGBA)"
    );

    let len = current.len();
    let chunks = len / 16;

    let thresh_vec = u8x16::splat(threshold);
    let mut transparent_count = 0usize;

    // Process 16 bytes (4 RGBA pixels) at a time
    for i in 0..chunks {
        let offset = i * 16;

        // Load 16 bytes
        let curr_chunk: [u8; 16] = current[offset..offset + 16].try_into().unwrap();
        let prev_chunk: [u8; 16] = previous[offset..offset + 16].try_into().unwrap();

        let curr = u8x16::from_array(curr_chunk);
        let prev = u8x16::from_array(prev_chunk);

        // |a - b| = max(a,b) - min(a,b)
        let diff = curr.simd_max(prev) - curr.simd_min(prev);

        // Compare all bytes against threshold
        let within: Mask<i8, 16> = diff.simd_le(thresh_vec);
        let mask = within.to_bitmask();

        // Each pixel needs all 4 bytes within threshold
        // Pixel masks: 0x000F, 0x00F0, 0x0F00, 0xF000
        if (mask & 0x000F) == 0x000F {
            current[offset..offset + 4].copy_from_slice(&[0, 0, 0, 0]);
            transparent_count += 1;
        }
        if (mask & 0x00F0) == 0x00F0 {
            current[offset + 4..offset + 8].copy_from_slice(&[0, 0, 0, 0]);
            transparent_count += 1;
        }
        if (mask & 0x0F00) == 0x0F00 {
            current[offset + 8..offset + 12].copy_from_slice(&[0, 0, 0, 0]);
            transparent_count += 1;
        }
        if (mask & 0xF000) == 0xF000 {
            current[offset + 12..offset + 16].copy_from_slice(&[0, 0, 0, 0]);
            transparent_count += 1;
        }
    }

    // Handle remainder (0-3 pixels) with scalar
    let remainder_start = chunks * 16;
    for i in (remainder_start..len).step_by(4) {
        if i + 3 < len {
            let similar = current[i].abs_diff(previous[i]) <= threshold
                && current[i + 1].abs_diff(previous[i + 1]) <= threshold
                && current[i + 2].abs_diff(previous[i + 2]) <= threshold
                && current[i + 3].abs_diff(previous[i + 3]) <= threshold;

            if similar {
                current[i] = 0;
                current[i + 1] = 0;
                current[i + 2] = 0;
                current[i + 3] = 0;
                transparent_count += 1;
            }
        }
    }

    transparent_count
}

/// Scalar fallback for [`mark_unchanged_pixels_simd`].
///
/// Same behavior, without SIMD. Useful for benchmarking the SIMD speedup.
///
/// # Panics
///
/// Panics if `current.len() != previous.len()`.
#[inline]
pub fn mark_unchanged_pixels_scalar(current: &mut [u8], previous: &[u8], threshold: u8) -> usize {
    assert_eq!(current.len(), previous.len());

    let mut transparent_count = 0;

    for i in (0..current.len()).step_by(4) {
        if i + 3 < current.len() {
            let similar = current[i].abs_diff(previous[i]) <= threshold
                && current[i + 1].abs_diff(previous[i + 1]) <= threshold
                && current[i + 2].abs_diff(previous[i + 2]) <= threshold
                && current[i + 3].abs_diff(previous[i + 3]) <= threshold;

            if similar {
                current[i] = 0;
                current[i + 1] = 0;
                current[i + 2] = 0;
                current[i + 3] = 0;
                transparent_count += 1;
            }
        }
    }

    transparent_count
}

/// Check if a row has any pixels that differ beyond threshold.
/// Uses SIMD for fast comparison.
#[inline]
fn row_has_diff(prev_row: &[u8], curr_row: &[u8], threshold: u8) -> bool {
    assert_eq!(prev_row.len(), curr_row.len());

    let len = prev_row.len();
    let chunks = len / 16;
    let thresh_vec = u8x16::splat(threshold);

    // Check 16 bytes (4 pixels) at a time
    for i in 0..chunks {
        let offset = i * 16;

        let curr_chunk: [u8; 16] = curr_row[offset..offset + 16].try_into().unwrap();
        let prev_chunk: [u8; 16] = prev_row[offset..offset + 16].try_into().unwrap();

        let curr = u8x16::from_array(curr_chunk);
        let prev = u8x16::from_array(prev_chunk);

        // |a - b| = max(a,b) - min(a,b)
        let diff = curr.simd_max(prev) - curr.simd_min(prev);

        // Check if any byte exceeds threshold
        let exceeds: Mask<i8, 16> = diff.simd_gt(thresh_vec);
        if exceeds.any() {
            return true;
        }
    }

    // Handle remainder with scalar
    let remainder_start = chunks * 16;
    for i in (remainder_start..len).step_by(4) {
        if i + 3 < len
            && (curr_row[i].abs_diff(prev_row[i]) > threshold
                || curr_row[i + 1].abs_diff(prev_row[i + 1]) > threshold
                || curr_row[i + 2].abs_diff(prev_row[i + 2]) > threshold
                || curr_row[i + 3].abs_diff(prev_row[i + 3]) > threshold)
        {
            return true;
        }
    }

    false
}

/// Scalar version of row_has_diff for benchmarking comparison.
#[inline]
fn row_has_diff_scalar(prev_row: &[u8], curr_row: &[u8], threshold: u8) -> bool {
    assert_eq!(prev_row.len(), curr_row.len());

    for i in (0..prev_row.len()).step_by(4) {
        if i + 3 < prev_row.len()
            && (curr_row[i].abs_diff(prev_row[i]) > threshold
                || curr_row[i + 1].abs_diff(prev_row[i + 1]) > threshold
                || curr_row[i + 2].abs_diff(prev_row[i + 2]) > threshold
                || curr_row[i + 3].abs_diff(prev_row[i + 3]) > threshold)
        {
            return true;
        }
    }
    false
}

/// Find the minimal bounding box containing all pixels that differ between two frames.
/// Returns None if frames are identical (within threshold).
///
/// Uses SIMD to compare 4 pixels at a time for performance.
///
/// # Arguments
/// * `prev` - Previous frame data (RGBA bytes)
/// * `curr` - Current frame data (RGBA bytes)
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
/// * `threshold` - Difference threshold per channel (0-255)
///
/// # Panics
/// Panics if `prev.len() != curr.len() != width * height * 4`
#[must_use]
pub fn find_diff_bounding_box(
    prev: &[u8],
    curr: &[u8],
    width: usize,
    height: usize,
    threshold: u8,
) -> Option<DiffRect> {
    let expected_len = width * height * 4;
    assert_eq!(prev.len(), expected_len, "prev buffer size mismatch");
    assert_eq!(curr.len(), expected_len, "curr buffer size mismatch");

    if width == 0 || height == 0 {
        return None;
    }

    let bytes_per_row = width * 4;

    // Find top row with diff
    let mut top = height;
    for y in 0..height {
        let row_start = y * bytes_per_row;
        let row_end = row_start + bytes_per_row;
        if row_has_diff(
            &prev[row_start..row_end],
            &curr[row_start..row_end],
            threshold,
        ) {
            top = y;
            break;
        }
    }

    // If no diff found, return None
    if top == height {
        return None;
    }

    // Find bottom row with diff (scan from bottom)
    let mut bottom = top;
    for y in (top..height).rev() {
        let row_start = y * bytes_per_row;
        let row_end = row_start + bytes_per_row;
        if row_has_diff(
            &prev[row_start..row_end],
            &curr[row_start..row_end],
            threshold,
        ) {
            bottom = y;
            break;
        }
    }

    // Find left column with diff
    let mut left = width;
    for x in 0..width {
        let mut has_diff = false;
        for y in top..=bottom {
            let idx = y * bytes_per_row + x * 4;
            if curr[idx].abs_diff(prev[idx]) > threshold
                || curr[idx + 1].abs_diff(prev[idx + 1]) > threshold
                || curr[idx + 2].abs_diff(prev[idx + 2]) > threshold
                || curr[idx + 3].abs_diff(prev[idx + 3]) > threshold
            {
                has_diff = true;
                break;
            }
        }
        if has_diff {
            left = x;
            break;
        }
    }

    // Find right column with diff (scan from right)
    let mut right = left;
    for x in (left..width).rev() {
        let mut has_diff = false;
        for y in top..=bottom {
            let idx = y * bytes_per_row + x * 4;
            if curr[idx].abs_diff(prev[idx]) > threshold
                || curr[idx + 1].abs_diff(prev[idx + 1]) > threshold
                || curr[idx + 2].abs_diff(prev[idx + 2]) > threshold
                || curr[idx + 3].abs_diff(prev[idx + 3]) > threshold
            {
                has_diff = true;
                break;
            }
        }
        if has_diff {
            right = x;
            break;
        }
    }

    Some(DiffRect {
        left: left as u16,
        top: top as u16,
        width: (right - left + 1) as u16,
        height: (bottom - top + 1) as u16,
    })
}

/// Scalar version of find_diff_bounding_box for benchmarking comparison.
///
/// # Arguments
/// * `prev` - Previous frame data (RGBA bytes)
/// * `curr` - Current frame data (RGBA bytes)
/// * `width` - Frame width in pixels
/// * `height` - Frame height in pixels
/// * `threshold` - Difference threshold per channel (0-255)
///
/// # Panics
/// Panics if `prev.len() != curr.len() != width * height * 4`
#[must_use]
pub fn find_diff_bounding_box_scalar(
    prev: &[u8],
    curr: &[u8],
    width: usize,
    height: usize,
    threshold: u8,
) -> Option<DiffRect> {
    let expected_len = width * height * 4;
    assert_eq!(prev.len(), expected_len, "prev buffer size mismatch");
    assert_eq!(curr.len(), expected_len, "curr buffer size mismatch");

    if width == 0 || height == 0 {
        return None;
    }

    let bytes_per_row = width * 4;

    // Find top row with diff
    let mut top = height;
    for y in 0..height {
        let row_start = y * bytes_per_row;
        let row_end = row_start + bytes_per_row;
        if row_has_diff_scalar(
            &prev[row_start..row_end],
            &curr[row_start..row_end],
            threshold,
        ) {
            top = y;
            break;
        }
    }

    // If no diff found, return None
    if top == height {
        return None;
    }

    // Find bottom row with diff (scan from bottom)
    let mut bottom = top;
    for y in (top..height).rev() {
        let row_start = y * bytes_per_row;
        let row_end = row_start + bytes_per_row;
        if row_has_diff_scalar(
            &prev[row_start..row_end],
            &curr[row_start..row_end],
            threshold,
        ) {
            bottom = y;
            break;
        }
    }

    // Find left column with diff
    let mut left = width;
    for x in 0..width {
        let mut has_diff = false;
        for y in top..=bottom {
            let idx = y * bytes_per_row + x * 4;
            if curr[idx].abs_diff(prev[idx]) > threshold
                || curr[idx + 1].abs_diff(prev[idx + 1]) > threshold
                || curr[idx + 2].abs_diff(prev[idx + 2]) > threshold
                || curr[idx + 3].abs_diff(prev[idx + 3]) > threshold
            {
                has_diff = true;
                break;
            }
        }
        if has_diff {
            left = x;
            break;
        }
    }

    // Find right column with diff (scan from right)
    let mut right = left;
    for x in (left..width).rev() {
        let mut has_diff = false;
        for y in top..=bottom {
            let idx = y * bytes_per_row + x * 4;
            if curr[idx].abs_diff(prev[idx]) > threshold
                || curr[idx + 1].abs_diff(prev[idx + 1]) > threshold
                || curr[idx + 2].abs_diff(prev[idx + 2]) > threshold
                || curr[idx + 3].abs_diff(prev[idx + 3]) > threshold
            {
                has_diff = true;
                break;
            }
        }
        if has_diff {
            right = x;
            break;
        }
    }

    Some(DiffRect {
        left: left as u16,
        top: top as u16,
        width: (right - left + 1) as u16,
        height: (bottom - top + 1) as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simd_exact_match() {
        let prev = vec![255u8, 128, 64, 255, 100, 100, 100, 255];
        let mut curr = prev.clone();

        let count = mark_unchanged_pixels_simd(&mut curr, &prev, 0);

        assert_eq!(count, 2);
        assert_eq!(curr, vec![0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_simd_threshold() {
        let prev = vec![100u8, 100, 100, 255];
        let mut curr = vec![102u8, 101, 100, 254]; // diff: 2, 1, 0, 1

        // Threshold 2: all diffs <= 2, should be transparent
        let count = mark_unchanged_pixels_simd(&mut curr, &prev, 2);
        assert_eq!(count, 1);
        assert_eq!(curr, vec![0, 0, 0, 0]);

        // Reset and try threshold 0
        let mut curr = vec![102u8, 101, 100, 254];
        let count = mark_unchanged_pixels_simd(&mut curr, &prev, 0);
        assert_eq!(count, 0);
        assert_eq!(curr, vec![102, 101, 100, 254]); // unchanged
    }

    #[test]
    fn test_simd_partial_match() {
        // 4 pixels: first two match, last two don't
        let prev = vec![
            100, 100, 100, 255, // pixel 0
            100, 100, 100, 255, // pixel 1
            100, 100, 100, 255, // pixel 2
            100, 100, 100, 255, // pixel 3
        ];
        let mut curr = vec![
            100, 100, 100, 255, // pixel 0 - exact match
            101, 100, 100, 255, // pixel 1 - within threshold 2
            110, 100, 100, 255, // pixel 2 - diff 10 > threshold
            100, 100, 100, 0, // pixel 3 - alpha diff 255 > threshold
        ];

        let count = mark_unchanged_pixels_simd(&mut curr, &prev, 2);

        assert_eq!(count, 2); // only pixels 0 and 1
        assert_eq!(&curr[0..4], &[0, 0, 0, 0]); // transparent
        assert_eq!(&curr[4..8], &[0, 0, 0, 0]); // transparent
        assert_eq!(&curr[8..12], &[110, 100, 100, 255]); // unchanged
        assert_eq!(&curr[12..16], &[100, 100, 100, 0]); // unchanged
    }

    #[test]
    fn test_simd_large_buffer() {
        // 100x100 = 10000 pixels = 40000 bytes
        let size = 40000;
        let prev: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        let mut curr = prev.clone();

        let count = mark_unchanged_pixels_simd(&mut curr, &prev, 0);

        assert_eq!(count, 10000);
        assert!(curr.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_simd_matches_scalar() {
        // Verify SIMD produces same results as scalar
        let size = 1024; // 256 pixels
        let prev: Vec<u8> = (0..size).map(|i| ((i * 7) % 256) as u8).collect();
        let curr_orig: Vec<u8> = (0..size).map(|i| ((i * 7 + 3) % 256) as u8).collect();

        let mut curr_simd = curr_orig.clone();
        let mut curr_scalar = curr_orig.clone();

        let count_simd = mark_unchanged_pixels_simd(&mut curr_simd, &prev, 5);
        let count_scalar = mark_unchanged_pixels_scalar(&mut curr_scalar, &prev, 5);

        assert_eq!(count_simd, count_scalar);
        assert_eq!(curr_simd, curr_scalar);
    }

    #[test]
    fn test_simd_various_sizes() {
        // Test various buffer sizes to hit different code paths
        for num_pixels in [1, 3, 4, 5, 15, 16, 17, 63, 64, 65, 100, 256] {
            let size = num_pixels * 4;
            let prev: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
            let mut curr_simd = prev.clone();
            let mut curr_scalar = prev.clone();

            let count_simd = mark_unchanged_pixels_simd(&mut curr_simd, &prev, 0);
            let count_scalar = mark_unchanged_pixels_scalar(&mut curr_scalar, &prev, 0);

            assert_eq!(
                count_simd, count_scalar,
                "Mismatch at {} pixels",
                num_pixels
            );
            assert_eq!(
                curr_simd, curr_scalar,
                "Data mismatch at {} pixels",
                num_pixels
            );
        }
    }

    #[test]
    fn test_diff_bbox_identical_frames() {
        // 2x2 frame, identical
        let width = 2;
        let height = 2;
        let prev = vec![
            255, 0, 0, 255, // pixel (0,0) red
            0, 255, 0, 255, // pixel (1,0) green
            0, 0, 255, 255, // pixel (0,1) blue
            255, 255, 0, 255, // pixel (1,1) yellow
        ];
        let curr = prev.clone();

        let result = find_diff_bounding_box(&prev, &curr, width, height, 0);
        assert_eq!(result, None);
    }

    #[test]
    fn test_diff_bbox_full_diff() {
        // 2x2 frame, all pixels different
        let width = 2;
        let height = 2;
        let prev = vec![
            255, 0, 0, 255, // pixel (0,0) red
            0, 255, 0, 255, // pixel (1,0) green
            0, 0, 255, 255, // pixel (0,1) blue
            255, 255, 0, 255, // pixel (1,1) yellow
        ];
        let curr = vec![
            0, 0, 0, 255, // pixel (0,0) black
            255, 255, 255, 255, // pixel (1,0) white
            128, 128, 128, 255, // pixel (0,1) gray
            64, 64, 64, 255, // pixel (1,1) dark gray
        ];

        let result = find_diff_bounding_box(&prev, &curr, width, height, 0);
        assert_eq!(
            result,
            Some(DiffRect {
                left: 0,
                top: 0,
                width: 2,
                height: 2
            })
        );
    }

    #[test]
    fn test_diff_bbox_top_left_corner() {
        // 3x3 frame, only top-left pixel differs
        let width = 3;
        let height = 3;
        let prev = vec![100u8; 36]; // 3x3 * 4 bytes
        let mut curr = prev.clone();

        // Change only top-left pixel (0,0)
        curr[0] = 200; // R channel differs by 100

        let result = find_diff_bounding_box(&prev, &curr, width, height, 0);
        assert_eq!(
            result,
            Some(DiffRect {
                left: 0,
                top: 0,
                width: 1,
                height: 1
            })
        );
    }

    #[test]
    fn test_diff_bbox_bottom_right_corner() {
        // 3x3 frame, only bottom-right pixel differs
        let width = 3;
        let height = 3;
        let prev = vec![100u8; 36]; // 3x3 * 4 bytes
        let mut curr = prev.clone();

        // Change only bottom-right pixel (2,2)
        let idx = (2 * width + 2) * 4; // row 2, col 2
        curr[idx] = 200; // R channel differs

        let result = find_diff_bounding_box(&prev, &curr, width, height, 0);
        assert_eq!(
            result,
            Some(DiffRect {
                left: 2,
                top: 2,
                width: 1,
                height: 1
            })
        );
    }

    #[test]
    fn test_diff_bbox_center_diff() {
        // 5x5 frame, center 2x2 region differs
        let width = 5;
        let height = 5;
        let prev = vec![100u8; 100]; // 5x5 * 4 bytes
        let mut curr = prev.clone();

        // Change center 2x2 region (rows 2-3, cols 2-3)
        for y in 2..4 {
            for x in 2..4 {
                let idx = (y * width + x) * 4;
                curr[idx] = 200;
            }
        }

        let result = find_diff_bounding_box(&prev, &curr, width, height, 0);
        assert_eq!(
            result,
            Some(DiffRect {
                left: 2,
                top: 2,
                width: 2,
                height: 2
            })
        );
    }

    #[test]
    fn test_diff_bbox_threshold() {
        // 2x2 frame, pixels within threshold don't count as diff
        let width = 2;
        let height = 2;
        let prev = vec![
            100, 100, 100, 255, // pixel (0,0)
            100, 100, 100, 255, // pixel (1,0)
            100, 100, 100, 255, // pixel (0,1)
            100, 100, 100, 255, // pixel (1,1)
        ];
        let curr = vec![
            102, 101, 100, 255, // pixel (0,0) - diffs: 2, 1, 0
            100, 100, 100, 255, // pixel (1,0) - identical
            100, 100, 100, 255, // pixel (0,1) - identical
            100, 100, 100, 255, // pixel (1,1) - identical
        ];

        // Threshold 2: all diffs in (0,0) are <= 2, so no diff
        let result = find_diff_bounding_box(&prev, &curr, width, height, 2);
        assert_eq!(result, None);

        // Threshold 1: diff of 2 in R channel exceeds threshold
        let result = find_diff_bounding_box(&prev, &curr, width, height, 1);
        assert_eq!(
            result,
            Some(DiffRect {
                left: 0,
                top: 0,
                width: 1,
                height: 1
            })
        );
    }

    #[test]
    fn test_diff_bbox_single_pixel_frame() {
        // 1x1 frame
        let width = 1;
        let height = 1;
        let prev = vec![100, 100, 100, 255];
        let curr = vec![200, 100, 100, 255];

        let result = find_diff_bounding_box(&prev, &curr, width, height, 0);
        assert_eq!(
            result,
            Some(DiffRect {
                left: 0,
                top: 0,
                width: 1,
                height: 1
            })
        );
    }

    #[test]
    fn test_diff_bbox_horizontal_strip() {
        // 5x5 frame, middle row differs
        let width = 5;
        let height = 5;
        let prev = vec![100u8; 100];
        let mut curr = prev.clone();

        // Change entire middle row (row 2)
        for x in 0..width {
            let idx = (2 * width + x) * 4;
            curr[idx] = 200;
        }

        let result = find_diff_bounding_box(&prev, &curr, width, height, 0);
        assert_eq!(
            result,
            Some(DiffRect {
                left: 0,
                top: 2,
                width: 5,
                height: 1
            })
        );
    }

    #[test]
    fn test_diff_bbox_vertical_strip() {
        // 5x5 frame, middle column differs
        let width = 5;
        let height = 5;
        let prev = vec![100u8; 100];
        let mut curr = prev.clone();

        // Change entire middle column (col 2)
        for y in 0..height {
            let idx = (y * width + 2) * 4;
            curr[idx] = 200;
        }

        let result = find_diff_bounding_box(&prev, &curr, width, height, 0);
        assert_eq!(
            result,
            Some(DiffRect {
                left: 2,
                top: 0,
                width: 1,
                height: 5
            })
        );
    }
}
