//! Fast palette lookup table for nearest-neighbor color mapping.
//!
//! Uses a 6-bit-per-channel lookup table (64×64×64 = 262KB) for O(1) color lookup.
//! Pre-computes the nearest palette index for every RGB cell to avoid O(pixels × 256)
//! distance calculations.

/// Fast palette lookup table for nearest-neighbor color mapping.
///
/// Uses 6 bits per channel (262KB table) for O(1) color lookup with improved accuracy.
///
/// # Example
/// ```ignore
/// let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
/// let lut = PaletteLut::new(&palette);
/// let idx = lut.map(255, 0, 0);  // Returns 0 (red)
/// ```
pub struct PaletteLut {
    /// 64×64×64 table mapping RGB666 to palette index (262144 bytes)
    table: Box<[u8; 262144]>,
    /// Original palette for distance calculations
    palette: Vec<[u8; 3]>,
}

impl PaletteLut {
    /// Build LUT from a palette (max 256 colors).
    ///
    /// # Panics
    /// Panics if palette has more than 256 colors.
    pub fn new(palette: &[[u8; 3]]) -> Self {
        assert!(palette.len() <= 256, "Palette must have at most 256 colors");

        let mut table = Box::new([0u8; 262144]);

        // For each cell in 64×64×64 space
        for r in 0..64u8 {
            for g in 0..64u8 {
                for b in 0..64u8 {
                    // Cell center in 8-bit space
                    // Expand 6-bit value to 8-bit by shifting left 2 and replicating high bits
                    let r8 = (r << 2) | (r >> 4);
                    let g8 = (g << 2) | (g >> 4);
                    let b8 = (b << 2) | (b >> 4);

                    // Find nearest palette entry
                    let idx = Self::find_nearest(palette, r8, g8, b8);

                    let table_idx = ((r as usize) << 12) | ((g as usize) << 6) | (b as usize);
                    table[table_idx] = idx;
                }
            }
        }

        Self {
            table,
            palette: palette.to_vec(),
        }
    }

    /// Find nearest palette index for an RGB color.
    #[inline]
    fn find_nearest(palette: &[[u8; 3]], r: u8, g: u8, b: u8) -> u8 {
        let mut best_idx = 0u8;
        let mut best_dist = u32::MAX;

        for (idx, color) in palette.iter().enumerate() {
            let dr = r as i32 - color[0] as i32;
            let dg = g as i32 - color[1] as i32;
            let db = b as i32 - color[2] as i32;
            let dist = (dr * dr + dg * dg + db * db) as u32;

            if dist < best_dist {
                best_dist = dist;
                best_idx = idx as u8;
            }
        }

        best_idx
    }

    /// Map an RGB pixel to palette index. O(1).
    ///
    /// # Example
    /// ```ignore
    /// let idx = lut.map(255, 0, 0);
    /// ```
    #[inline]
    pub fn map(&self, r: u8, g: u8, b: u8) -> u8 {
        let idx = ((r as usize >> 2) << 12) | ((g as usize >> 2) << 6) | (b as usize >> 2);
        self.table[idx]
    }

    /// Map RGB and return (index, squared_distance).
    ///
    /// Used for quality detection to measure how well the palette matches the color.
    ///
    /// # Example
    /// ```ignore
    /// let (idx, dist_sq) = lut.map_with_distance(255, 0, 0);
    /// ```
    #[inline]
    pub fn map_with_distance(&self, r: u8, g: u8, b: u8) -> (u8, u32) {
        let idx = self.map(r, g, b);
        let color = &self.palette[idx as usize];

        let dr = r as i32 - color[0] as i32;
        let dg = g as i32 - color[1] as i32;
        let db = b as i32 - color[2] as i32;

        (idx, (dr * dr + dg * dg + db * db) as u32)
    }

    /// Map entire RGBA buffer to indices.
    ///
    /// Returns (indices, quality stats).
    ///
    /// # Panics
    /// Panics if `rgba.len()` is not divisible by 4.
    pub fn map_buffer(&self, rgba: &[u8]) -> (Vec<u8>, PaletteMapStats) {
        let pixel_count = rgba.len() / 4;
        let mut indices = Vec::with_capacity(pixel_count);

        let mut total_dist: u64 = 0;
        let mut max_dist: u32 = 0;
        let mut outliers: usize = 0;
        let mut used_colors = [false; 256];

        for pixel in rgba.chunks_exact(4) {
            let (idx, dist) = self.map_with_distance(pixel[0], pixel[1], pixel[2]);
            indices.push(idx);

            total_dist += dist as u64;
            max_dist = max_dist.max(dist);
            if dist > 2500 {
                // sqrt(2500) = 50 color distance
                outliers += 1;
            }
            used_colors[idx as usize] = true;
        }

        let stats = PaletteMapStats {
            avg_distance_sq: total_dist as f64 / pixel_count as f64,
            max_distance_sq: max_dist,
            outlier_ratio: outliers as f64 / pixel_count as f64,
            palette_utilization: used_colors.iter().filter(|&&x| x).count() as f64
                / self.palette.len() as f64,
        };

        (indices, stats)
    }

    /// Get the palette.
    #[must_use]
    pub fn palette(&self) -> &[[u8; 3]] {
        &self.palette
    }
}

/// Statistics from palette mapping.
#[derive(Debug, Clone, Copy)]
pub struct PaletteMapStats {
    /// Average squared distance from pixel to palette color.
    pub avg_distance_sq: f64,
    /// Maximum squared distance (worst pixel).
    pub max_distance_sq: u32,
    /// Ratio of pixels with distance > 50 (squared > 2500).
    pub outlier_ratio: f64,
    /// Ratio of palette colors used in output.
    pub palette_utilization: f64,
}

impl PaletteMapStats {
    /// Returns true if the mapping quality is acceptable.
    ///
    /// Use this to decide whether to fallback to full re-quantization.
    #[must_use]
    pub fn is_acceptable(&self) -> bool {
        self.avg_distance_sq < 150.0 && self.outlier_ratio < 0.05 && self.palette_utilization > 0.3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_simple_palette() -> Vec<[u8; 3]> {
        vec![
            [0, 0, 0],       // Black
            [255, 0, 0],     // Red
            [0, 255, 0],     // Green
            [0, 0, 255],     // Blue
            [255, 255, 255], // White
        ]
    }

    #[test]
    fn test_lut_exact_match() {
        let palette = make_simple_palette();
        let lut = PaletteLut::new(&palette);

        // Exact matches should map to themselves
        assert_eq!(lut.map(0, 0, 0), 0); // Black
        assert_eq!(lut.map(255, 0, 0), 1); // Red
        assert_eq!(lut.map(0, 255, 0), 2); // Green
        assert_eq!(lut.map(0, 0, 255), 3); // Blue
        assert_eq!(lut.map(255, 255, 255), 4); // White
    }

    #[test]
    fn test_lut_nearest_neighbor() {
        let palette = make_simple_palette();
        let lut = PaletteLut::new(&palette);

        // Close to red should map to red
        assert_eq!(lut.map(254, 0, 0), 1);
        assert_eq!(lut.map(255, 1, 0), 1);

        // Close to green should map to green
        assert_eq!(lut.map(0, 254, 0), 2);
        assert_eq!(lut.map(1, 255, 0), 2);
    }

    #[test]
    fn test_map_with_distance() {
        let palette = make_simple_palette();
        let lut = PaletteLut::new(&palette);

        // Exact match has distance 0
        let (idx, dist) = lut.map_with_distance(255, 0, 0);
        assert_eq!(idx, 1);
        assert_eq!(dist, 0);

        // Off by 1 in each channel: distance = 1 + 1 + 1 = 3
        let (idx, dist) = lut.map_with_distance(254, 1, 1);
        assert_eq!(idx, 1); // Still closest to red
        assert!(dist > 0);
    }

    #[test]
    fn test_map_buffer() {
        let palette = make_simple_palette();
        let lut = PaletteLut::new(&palette);

        // RGBA buffer: 2 pixels
        // Pixel 1: Red (255, 0, 0, 255)
        // Pixel 2: Green (0, 255, 0, 255)
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];

        let (indices, stats) = lut.map_buffer(&rgba);

        assert_eq!(indices.len(), 2);
        assert_eq!(indices[0], 1); // Red
        assert_eq!(indices[1], 2); // Green

        // Both are exact matches, so avg distance should be 0
        assert_eq!(stats.avg_distance_sq, 0.0);
        assert_eq!(stats.max_distance_sq, 0);
        assert_eq!(stats.outlier_ratio, 0.0);
        assert!(stats.palette_utilization > 0.0);
    }

    #[test]
    fn test_palette_utilization() {
        let palette = make_simple_palette();
        let lut = PaletteLut::new(&palette);

        // Only use red and green
        let rgba = vec![
            255, 0, 0, 255, // Red
            0, 255, 0, 255, // Green
        ];

        let (_indices, stats) = lut.map_buffer(&rgba);

        // 2 out of 5 colors used
        assert!(stats.palette_utilization >= 0.4 && stats.palette_utilization <= 0.41);
    }

    #[test]
    fn test_is_acceptable_quality() {
        let palette = make_simple_palette();
        let lut = PaletteLut::new(&palette);

        // Exact matches should be acceptable
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let (_indices, stats) = lut.map_buffer(&rgba);
        assert!(stats.is_acceptable());
    }

    #[test]
    fn test_lut_size() {
        let palette = make_simple_palette();
        let lut = PaletteLut::new(&palette);

        // Table should be exactly 262KB (64^3)
        assert_eq!(std::mem::size_of_val(&*lut.table), 262144);
    }

    #[test]
    fn test_palette_max_256_colors() {
        let mut palette = vec![[0u8; 3]; 256];
        for i in 0..256 {
            palette[i] = [i as u8, i as u8, i as u8];
        }

        let lut = PaletteLut::new(&palette);
        assert_eq!(lut.palette().len(), 256);
    }

    #[test]
    #[should_panic]
    fn test_palette_too_many_colors() {
        let palette = vec![[0u8; 3]; 257];
        let _lut = PaletteLut::new(&palette);
    }
}
