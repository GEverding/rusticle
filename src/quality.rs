//! Image quality metrics for comparing GIF frames.
//!
//! Provides PSNR, SSIM, and mean color distance metrics.

/// Quality metrics comparing two images.
#[derive(Debug, Clone, Copy)]
pub struct QualityMetrics {
    /// Peak Signal-to-Noise Ratio (dB). Higher is better.
    /// Typical values: 30-50 dB for good quality.
    pub psnr: f64,

    /// Structural Similarity Index. Range 0-1, higher is better.
    /// > 0.95 is excellent, > 0.90 is good.
    pub ssim: f64,

    /// Mean squared error per pixel.
    pub mse: f64,

    /// Mean color distance (Euclidean RGB).
    pub mean_color_distance: f64,

    /// Max color distance (worst pixel).
    pub max_color_distance: f64,

    /// Percentage of pixels with distance > 50.
    pub outlier_ratio: f64,
}

impl QualityMetrics {
    /// Compare two RGBA buffers of the same dimensions.
    ///
    /// # Panics
    /// Panics if buffers have different lengths.
    #[must_use]
    pub fn compare(original: &[u8], processed: &[u8]) -> Self {
        assert_eq!(original.len(), processed.len(), "Buffer size mismatch");
        assert_eq!(original.len() % 4, 0, "Buffer must be RGBA");

        let pixel_count = original.len() / 4;
        if pixel_count == 0 {
            return Self::zero();
        }

        let mut sum_sq_error: f64 = 0.0;
        let mut sum_distance: f64 = 0.0;
        let mut max_distance: f64 = 0.0;
        let mut outliers: usize = 0;

        // For SSIM
        let mut sum_orig: f64 = 0.0;
        let mut sum_proc: f64 = 0.0;
        let mut sum_orig_sq: f64 = 0.0;
        let mut sum_proc_sq: f64 = 0.0;
        let mut sum_orig_proc: f64 = 0.0;

        for i in 0..pixel_count {
            let idx = i * 4;

            // RGB values (ignore alpha for quality metrics)
            let or = original[idx] as f64;
            let og = original[idx + 1] as f64;
            let ob = original[idx + 2] as f64;

            let pr = processed[idx] as f64;
            let pg = processed[idx + 1] as f64;
            let pb = processed[idx + 2] as f64;

            // MSE components
            let dr = or - pr;
            let dg = og - pg;
            let db = ob - pb;
            sum_sq_error += dr * dr + dg * dg + db * db;

            // Color distance (Euclidean)
            let dist = (dr * dr + dg * dg + db * db).sqrt();
            sum_distance += dist;
            max_distance = max_distance.max(dist);
            if dist > 50.0 {
                outliers += 1;
            }

            // SSIM uses luminance
            let orig_lum = 0.299 * or + 0.587 * og + 0.114 * ob;
            let proc_lum = 0.299 * pr + 0.587 * pg + 0.114 * pb;

            sum_orig += orig_lum;
            sum_proc += proc_lum;
            sum_orig_sq += orig_lum * orig_lum;
            sum_proc_sq += proc_lum * proc_lum;
            sum_orig_proc += orig_lum * proc_lum;
        }

        let n = pixel_count as f64;

        // MSE (per channel, so divide by 3)
        let mse = sum_sq_error / (n * 3.0);

        // PSNR
        let psnr = if mse > 0.0 {
            10.0 * (255.0_f64 * 255.0 / mse).log10()
        } else {
            f64::INFINITY // Perfect match
        };

        // SSIM
        let mean_orig = sum_orig / n;
        let mean_proc = sum_proc / n;
        let var_orig = (sum_orig_sq / n) - (mean_orig * mean_orig);
        let var_proc = (sum_proc_sq / n) - (mean_proc * mean_proc);
        let covar = (sum_orig_proc / n) - (mean_orig * mean_proc);

        // SSIM constants (for 8-bit images)
        let c1 = 6.5025; // (0.01 * 255)^2
        let c2 = 58.5225; // (0.03 * 255)^2

        let ssim = ((2.0 * mean_orig * mean_proc + c1) * (2.0 * covar + c2))
            / ((mean_orig * mean_orig + mean_proc * mean_proc + c1) * (var_orig + var_proc + c2));

        Self {
            psnr,
            ssim: ssim.clamp(0.0, 1.0),
            mse,
            mean_color_distance: sum_distance / n,
            max_color_distance: max_distance,
            outlier_ratio: outliers as f64 / n,
        }
    }

    fn zero() -> Self {
        Self {
            psnr: f64::INFINITY,
            ssim: 1.0,
            mse: 0.0,
            mean_color_distance: 0.0,
            max_color_distance: 0.0,
            outlier_ratio: 0.0,
        }
    }

    /// Returns true if quality meets typical "good" thresholds.
    #[must_use]
    pub fn is_good(&self) -> bool {
        self.psnr >= 30.0 && self.ssim >= 0.90 && self.outlier_ratio < 0.05
    }

    /// Returns true if quality meets "excellent" thresholds.
    #[must_use]
    pub fn is_excellent(&self) -> bool {
        self.psnr >= 40.0 && self.ssim >= 0.95 && self.outlier_ratio < 0.01
    }
}

impl std::fmt::Display for QualityMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PSNR: {:.2}dB, SSIM: {:.4}, MSE: {:.2}, MeanDist: {:.2}, MaxDist: {:.2}, Outliers: {:.2}%",
            self.psnr,
            self.ssim,
            self.mse,
            self.mean_color_distance,
            self.max_color_distance,
            self.outlier_ratio * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_images() {
        let img = vec![255, 0, 0, 255, 0, 255, 0, 255]; // 2 pixels
        let metrics = QualityMetrics::compare(&img, &img);

        assert!(metrics.psnr.is_infinite());
        assert!((metrics.ssim - 1.0).abs() < 0.001);
        assert_eq!(metrics.mse, 0.0);
    }

    #[test]
    fn test_small_difference() {
        let orig = vec![100, 100, 100, 255];
        let proc = vec![102, 100, 100, 255]; // +2 in red
        let metrics = QualityMetrics::compare(&orig, &proc);

        assert!(metrics.psnr > 40.0); // Small diff = high PSNR
        assert!(metrics.ssim > 0.99);
        assert!(metrics.mean_color_distance < 3.0);
    }

    #[test]
    fn test_large_difference() {
        let orig = vec![0, 0, 0, 255];
        let proc = vec![255, 255, 255, 255]; // Black vs white
        let metrics = QualityMetrics::compare(&orig, &proc);

        assert!(metrics.psnr < 10.0); // Big diff = low PSNR
        assert!(metrics.ssim < 0.1);
        assert!(metrics.max_color_distance > 400.0);
    }

    #[test]
    fn test_is_good() {
        // Good quality image
        let orig: Vec<u8> = (0..400).map(|i| (i % 256) as u8).collect();
        let proc: Vec<u8> = orig.iter().map(|&v| v.saturating_add(1)).collect();
        let metrics = QualityMetrics::compare(&orig, &proc);

        assert!(metrics.is_good());
    }
}
