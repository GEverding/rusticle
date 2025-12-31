//! SIMD-accelerated frame optimization.
//!
//! Provides vectorized pixel comparison and transparency marking.
//! Uses std::simd for portable SIMD across ARM NEON and x86 SSE/AVX.

use std::simd::{cmp::SimdOrd, cmp::SimdPartialOrd, u8x16, Mask};

/// Process pixels using 16-byte SIMD (4 pixels at a time).
/// Uses bitmask operations for faster pixel-level checks.
#[inline]
pub fn mark_unchanged_pixels_simd(
    current: &mut [u8],
    previous: &[u8],
    threshold: u8,
) -> usize {
    assert_eq!(current.len(), previous.len());
    assert!(current.len() % 4 == 0, "Buffer must be multiple of 4 bytes (RGBA)");

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
            let similar = 
                current[i].abs_diff(previous[i]) <= threshold &&
                current[i + 1].abs_diff(previous[i + 1]) <= threshold &&
                current[i + 2].abs_diff(previous[i + 2]) <= threshold &&
                current[i + 3].abs_diff(previous[i + 3]) <= threshold;
            
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

/// Scalar fallback implementation for comparison.
#[inline]
pub fn mark_unchanged_pixels_scalar(
    current: &mut [u8],
    previous: &[u8],
    threshold: u8,
) -> usize {
    assert_eq!(current.len(), previous.len());
    
    let mut transparent_count = 0;
    
    for i in (0..current.len()).step_by(4) {
        if i + 3 < current.len() {
            let similar = 
                current[i].abs_diff(previous[i]) <= threshold &&
                current[i + 1].abs_diff(previous[i + 1]) <= threshold &&
                current[i + 2].abs_diff(previous[i + 2]) <= threshold &&
                current[i + 3].abs_diff(previous[i + 3]) <= threshold;
            
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
            100, 100, 100, 255,  // pixel 0
            100, 100, 100, 255,  // pixel 1
            100, 100, 100, 255,  // pixel 2
            100, 100, 100, 255,  // pixel 3
        ];
        let mut curr = vec![
            100, 100, 100, 255,  // pixel 0 - exact match
            101, 100, 100, 255,  // pixel 1 - within threshold 2
            110, 100, 100, 255,  // pixel 2 - diff 10 > threshold
            100, 100, 100, 0,    // pixel 3 - alpha diff 255 > threshold
        ];
        
        let count = mark_unchanged_pixels_simd(&mut curr, &prev, 2);
        
        assert_eq!(count, 2); // only pixels 0 and 1
        assert_eq!(&curr[0..4], &[0, 0, 0, 0]);   // transparent
        assert_eq!(&curr[4..8], &[0, 0, 0, 0]);   // transparent
        assert_eq!(&curr[8..12], &[110, 100, 100, 255]); // unchanged
        assert_eq!(&curr[12..16], &[100, 100, 100, 0]);  // unchanged
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
            
            assert_eq!(count_simd, count_scalar, "Mismatch at {} pixels", num_pixels);
            assert_eq!(curr_simd, curr_scalar, "Data mismatch at {} pixels", num_pixels);
        }
    }
}
