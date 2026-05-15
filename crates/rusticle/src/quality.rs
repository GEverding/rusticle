//! Image quality metrics for comparing GIF frames.
//!
//! Provides PSNR, SSIM, mean color distance, and optional Butteraugli perceptual distance metrics.
//!
//! ## Metric Directionality
//!
//! - **PSNR** (Peak Signal-to-Noise Ratio): Higher is better. Typical range 30–50 dB.
//! - **SSIM** (Structural Similarity Index): Higher is better. Range 0–1, with > 0.95 excellent.
//! - **Butteraugli** (perceptual distance): **Lower is better** (opposite of PSNR/SSIM).
//!   Requires `butteraugli` feature and image dimensions ≥ 8×8.
//!   Typical range: < 1.0 imperceptible, 1.0–2.0 good, > 3.0 noticeable.
//!
//! ## Butteraugli Availability
//!
//! Butteraugli scores are computed only when:
//! 1. The `butteraugli` feature is enabled at compile time
//! 2. Image dimensions are at least 8×8 pixels
//! 3. Using [`QualityMetrics::compare_with_dimensions()`] (legacy [`QualityMetrics::compare()`] never computes Butteraugli)
//!
//! When unavailable, the `butteraugli` field is `None`.

/// Convert RGBA buffer to RGB by compositing onto white background.
///
/// Each pixel is composited using alpha blending:
/// `out = (color * alpha + white * (255 - alpha)) / 255`
///
/// Returns a packed RGB buffer (3 bytes per pixel).
#[cfg(feature = "butteraugli")]
fn rgba_to_rgb_composited(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let expected_len = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected_len {
        return Vec::new(); // Invalid buffer, return empty
    }

    let pixel_count = (width as usize) * (height as usize);
    let mut rgb = Vec::with_capacity(pixel_count * 3);

    for i in 0..pixel_count {
        let idx = i * 4;
        let r = rgba[idx] as u32;
        let g = rgba[idx + 1] as u32;
        let b = rgba[idx + 2] as u32;
        let a = rgba[idx + 3] as u32;

        // Composite onto white (255, 255, 255)
        // out = (color * alpha + 255 * (255 - alpha)) / 255
        let out_r = ((r * a + 255 * (255 - a)) / 255) as u8;
        let out_g = ((g * a + 255 * (255 - a)) / 255) as u8;
        let out_b = ((b * a + 255 * (255 - a)) / 255) as u8;

        rgb.push(out_r);
        rgb.push(out_g);
        rgb.push(out_b);
    }

    rgb
}

/// Compute Butteraugli perceptual distance score.
///
/// Returns `None` if:
/// - Image dimensions < 8x8 (Butteraugli minimum)
/// - Buffer lengths don't match expected size
/// - Butteraugli computation fails
#[cfg(feature = "butteraugli")]
fn compute_butteraugli(original: &[u8], processed: &[u8], width: u32, height: u32) -> Option<f64> {
    // Guard: minimum image size
    if width < 8 || height < 8 {
        return None;
    }

    // Guard: buffer length validation
    let expected_len = (width as usize) * (height as usize) * 4;
    if original.len() != expected_len || processed.len() != expected_len {
        return None;
    }

    // Convert RGBA to RGB via white compositing
    let orig_rgb = rgba_to_rgb_composited(original, width, height);
    let proc_rgb = rgba_to_rgb_composited(processed, width, height);

    if orig_rgb.is_empty() || proc_rgb.is_empty() {
        return None;
    }

    // Convert RGB byte vectors to RGB8 slices for butteraugli
    // RGB8 is repr(C) with 3 u8 fields, matching our byte layout
    let orig_pixels: &[butteraugli::RGB8] =
        unsafe { std::slice::from_raw_parts(orig_rgb.as_ptr() as *const _, orig_rgb.len() / 3) };
    let proc_pixels: &[butteraugli::RGB8] =
        unsafe { std::slice::from_raw_parts(proc_rgb.as_ptr() as *const _, proc_rgb.len() / 3) };

    // Create ImgRef from RGB buffers
    let orig_img = butteraugli::ImgRef::new(orig_pixels, width as usize, height as usize);
    let proc_img = butteraugli::ImgRef::new(proc_pixels, width as usize, height as usize);

    // Compute Butteraugli score (scalar only, no diffmap)
    let params = butteraugli::ButteraugliParams::new();
    butteraugli::butteraugli(orig_img, proc_img, &params)
        .ok()
        .map(|result| result.score)
}

/// Quality metrics comparing two images.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

    /// Butteraugli perceptual distance score. Lower is better.
    /// None if feature disabled, image dimensions < 8x8, or computation failed.
    /// Typical values: < 1.0 imperceptible, < 2.0 good, > 3.0 noticeable.
    #[cfg_attr(feature = "serde", serde(default))]
    pub butteraugli: Option<f64>,

    /// Whether this comparison is valid. False if buffers had mismatched lengths,
    /// non-RGBA alignment, or other validation failures.
    /// When false, metric values should be treated as N/A and not reported as quality scores.
    #[cfg_attr(feature = "serde", serde(default))]
    pub valid: bool,
}

impl QualityMetrics {
    /// Compare two RGBA buffers and compute quality metrics.
    ///
    /// Both buffers must have the same length and be RGBA (4 bytes per pixel).
    /// Computes PSNR, SSIM, MSE, and color distance metrics over RGB channels
    /// (alpha is ignored).
    ///
    /// # Panics
    ///
    /// Panics if buffers have different lengths or length is not a multiple of 4.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn main() -> rusticle::Result<()> {
    /// use rusticle::QualityMetrics;
    ///
    /// let original = std::fs::read("original.raw")?;
    /// let processed = std::fs::read("processed.raw")?;
    /// let metrics = QualityMetrics::compare(&original, &processed);
    ///
    /// println!("PSNR: {:.1} dB, SSIM: {:.4}", metrics.psnr, metrics.ssim);
    /// if metrics.is_good() {
    ///     println!("Quality: GOOD");
    /// }
    /// # Ok(())
    /// # }
    /// ```
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
            butteraugli: None,
            valid: true,
        }
    }

    /// Compare two RGBA buffers with known dimensions and compute quality metrics including Butteraugli.
    ///
    /// Both buffers must have the same length and be RGBA (4 bytes per pixel).
    /// Computes PSNR, SSIM, MSE, color distance metrics, and optionally Butteraugli score.
    ///
    /// If buffers have different lengths or are not a multiple of 4 bytes, returns metrics
    /// with `valid: false` instead of panicking. Callers should check `valid` before
    /// reporting metrics. This gracefully handles subframed GIFs where individual frame
    /// buffers may be smaller than the canvas dimensions.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # fn main() -> rusticle::Result<()> {
    /// use rusticle::QualityMetrics;
    ///
    /// let original = std::fs::read("original.raw")?;
    /// let processed = std::fs::read("processed.raw")?;
    /// let metrics = QualityMetrics::compare_with_dimensions(&original, &processed, 640, 480);
    ///
    /// if metrics.valid {
    ///     println!("PSNR: {:.1} dB, SSIM: {:.4}", metrics.psnr, metrics.ssim);
    ///     if let Some(ba) = metrics.butteraugli {
    ///         println!("Butteraugli: {:.2} (lower is better)", ba);
    ///     }
    /// } else {
    ///     println!("Comparison invalid (buffer mismatch)");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    #[cfg_attr(not(feature = "butteraugli"), allow(unused_variables))]
    pub fn compare_with_dimensions(
        original: &[u8],
        processed: &[u8],
        width: u32,
        height: u32,
    ) -> Self {
        // Guard: buffers must have same length and be RGBA (4 bytes per pixel)
        if original.len() != processed.len() || !original.len().is_multiple_of(4) {
            // Return invalid metrics for mismatched buffers
            return Self::invalid();
        }

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

        // Compute Butteraugli if feature enabled
        #[cfg(feature = "butteraugli")]
        let butteraugli = compute_butteraugli(original, processed, width, height);
        #[cfg(not(feature = "butteraugli"))]
        let butteraugli = None;

        Self {
            psnr,
            ssim: ssim.clamp(0.0, 1.0),
            mse,
            mean_color_distance: sum_distance / n,
            max_color_distance: max_distance,
            outlier_ratio: outliers as f64 / n,
            butteraugli,
            valid: true,
        }
    }

    /// Create a valid zero-error (perfect match) metrics struct.
    fn zero() -> Self {
        Self {
            psnr: f64::INFINITY,
            ssim: 1.0,
            mse: 0.0,
            mean_color_distance: 0.0,
            max_color_distance: 0.0,
            outlier_ratio: 0.0,
            butteraugli: None,
            valid: true,
        }
    }

    /// Create an invalid metrics struct (buffer mismatch, non-RGBA, etc).
    fn invalid() -> Self {
        Self {
            psnr: f64::NAN,
            ssim: f64::NAN,
            mse: f64::NAN,
            mean_color_distance: f64::NAN,
            max_color_distance: f64::NAN,
            outlier_ratio: f64::NAN,
            butteraugli: None,
            valid: false,
        }
    }

    /// Returns true if quality meets typical "good" thresholds.
    ///
    /// Checks PSNR >= 30dB, SSIM >= 0.90, outlier_ratio < 5%.
    /// If Butteraugli score is available (feature enabled, dimensions ≥ 8×8),
    /// also requires Butteraugli < 2.0 (lower is better).
    #[must_use]
    pub fn is_good(&self) -> bool {
        let base = self.psnr >= 30.0 && self.ssim >= 0.90 && self.outlier_ratio < 0.05;
        if let Some(ba) = self.butteraugli {
            base && ba < 2.0
        } else {
            base
        }
    }

    /// Returns true if quality meets "excellent" thresholds.
    ///
    /// Checks PSNR >= 40dB, SSIM >= 0.95, outlier_ratio < 1%.
    /// If Butteraugli score is available (feature enabled, dimensions ≥ 8×8),
    /// also requires Butteraugli < 1.0 (lower is better).
    #[must_use]
    pub fn is_excellent(&self) -> bool {
        let base = self.psnr >= 40.0 && self.ssim >= 0.95 && self.outlier_ratio < 0.01;
        if let Some(ba) = self.butteraugli {
            base && ba < 1.0
        } else {
            base
        }
    }
}

impl std::fmt::Display for QualityMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.valid {
            write!(f, "INVALID (buffer mismatch or non-RGBA)")?;
            return Ok(());
        }

        write!(
            f,
            "PSNR: {:.2}dB, SSIM: {:.4}, MSE: {:.2}, MeanDist: {:.2}, MaxDist: {:.2}, Outliers: {:.2}%",
            self.psnr,
            self.ssim,
            self.mse,
            self.mean_color_distance,
            self.max_color_distance,
            self.outlier_ratio * 100.0
        )?;
        if let Some(ba) = self.butteraugli {
            write!(f, ", Butteraugli: {:.2}", ba)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Solid RGBA image: every pixel is `(r, g, b, a)`.
    fn solid_rgba(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
        let n = (width * height) as usize;
        let mut buf = Vec::with_capacity(n * 4);
        for _ in 0..n {
            buf.extend_from_slice(&[r, g, b, a]);
        }
        buf
    }

    // ── pre-existing tests (kept) ─────────────────────────────────────────

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

    // ── A1: compare() always yields butteraugli == None ───────────────────

    #[test]
    fn test_compare_legacy_butteraugli_always_none() {
        // compare() is the legacy path — butteraugli must be None regardless
        // of whether the feature is compiled in.
        let img = solid_rgba(8, 8, 128, 64, 32, 255);
        let metrics = QualityMetrics::compare(&img, &img);
        assert_eq!(
            metrics.butteraugli, None,
            "compare() must always return butteraugli == None (legacy contract)"
        );
    }

    #[test]
    fn test_compare_legacy_butteraugli_none_for_different_images() {
        let orig = solid_rgba(8, 8, 0, 0, 0, 255);
        let proc = solid_rgba(8, 8, 255, 255, 255, 255);
        let metrics = QualityMetrics::compare(&orig, &proc);
        assert_eq!(
            metrics.butteraugli, None,
            "compare() must always return butteraugli == None even for different images"
        );
    }

    // ── A2: compare_with_dimensions() returns None for <8×8 (no panic) ───

    #[test]
    fn test_compare_with_dimensions_sub8x8_returns_none_butteraugli() {
        // 7×7 is below the Butteraugli minimum — must return None, not panic.
        let img = solid_rgba(7, 7, 100, 100, 100, 255);
        let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 7, 7);
        assert_eq!(
            metrics.butteraugli, None,
            "butteraugli must be None for 7×7 image (below 8×8 minimum)"
        );
    }

    // ── A7: graceful handling of buffer size mismatch (subframed GIFs) ────

    #[test]
    fn test_compare_with_dimensions_buffer_size_mismatch_returns_invalid() {
        // Subframed GIF: frame buffer is 8×8 but canvas is 16×16
        // Should return invalid metrics instead of panicking
        let small_buf = solid_rgba(8, 8, 100, 100, 100, 255);
        let large_buf = solid_rgba(16, 16, 100, 100, 100, 255);

        let metrics = QualityMetrics::compare_with_dimensions(&small_buf, &large_buf, 16, 16);
        assert!(
            !metrics.valid,
            "buffer size mismatch should return invalid metrics"
        );
        assert!(
            metrics.psnr.is_nan(),
            "invalid metrics should have NaN psnr"
        );
    }

    #[test]
    fn test_compare_with_dimensions_different_lengths_returns_invalid() {
        // Different buffer lengths should return invalid metrics, not panic
        let buf1 = solid_rgba(4, 4, 100, 100, 100, 255);
        let buf2 = solid_rgba(8, 8, 100, 100, 100, 255);

        let metrics = QualityMetrics::compare_with_dimensions(&buf1, &buf2, 8, 8);
        assert!(
            !metrics.valid,
            "different buffer lengths should return invalid metrics"
        );
    }

    #[test]
    fn test_compare_with_dimensions_non_rgba_length_returns_invalid() {
        // Buffer length not multiple of 4 should return invalid metrics, not panic
        let buf1 = vec![100, 100, 100, 255, 100]; // 5 bytes, not multiple of 4
        let buf2 = vec![100, 100, 100, 255, 100];

        let metrics = QualityMetrics::compare_with_dimensions(&buf1, &buf2, 1, 1);
        assert!(
            !metrics.valid,
            "non-RGBA buffer length should return invalid metrics"
        );
    }

    #[test]
    fn test_compare_with_dimensions_1x1_returns_none_butteraugli() {
        let img = solid_rgba(1, 1, 255, 0, 0, 255);
        let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 1, 1);
        assert_eq!(
            metrics.butteraugli, None,
            "butteraugli must be None for 1×1 image"
        );
    }

    #[test]
    fn test_compare_with_dimensions_7x8_returns_none_butteraugli() {
        // Width < 8 even though height == 8
        let img = solid_rgba(7, 8, 50, 50, 50, 255);
        let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 7, 8);
        assert_eq!(
            metrics.butteraugli, None,
            "butteraugli must be None when width < 8"
        );
    }

    #[test]
    fn test_compare_with_dimensions_8x7_returns_none_butteraugli() {
        // Height < 8 even though width == 8
        let img = solid_rgba(8, 7, 50, 50, 50, 255);
        let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 8, 7);
        assert_eq!(
            metrics.butteraugli, None,
            "butteraugli must be None when height < 8"
        );
    }

    // ── A2 (also): non-butteraugli metrics still work for sub-8×8 ─────────

    #[test]
    fn test_compare_with_dimensions_sub8x8_psnr_still_computed() {
        let img = solid_rgba(4, 4, 200, 100, 50, 255);
        let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 4, 4);
        assert!(metrics.valid, "valid comparison should have valid=true");
        assert!(
            metrics.psnr.is_infinite(),
            "PSNR should be infinite for identical images even at 4×4"
        );
        assert_eq!(metrics.mse, 0.0, "MSE should be 0 for identical images");
    }

    // ── Valid comparisons still work correctly ────────────────────────────

    #[test]
    fn test_compare_with_dimensions_valid_identical_images() {
        let img = solid_rgba(8, 8, 128, 64, 32, 255);
        let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 8, 8);
        assert!(metrics.valid, "identical images should be valid");
        assert!(
            metrics.psnr.is_infinite(),
            "identical should have infinite PSNR"
        );
        assert_eq!(metrics.ssim, 1.0, "identical should have SSIM=1.0");
    }

    #[test]
    fn test_compare_with_dimensions_valid_different_images() {
        let orig = solid_rgba(8, 8, 0, 0, 0, 255);
        let proc = solid_rgba(8, 8, 255, 255, 255, 255);
        let metrics = QualityMetrics::compare_with_dimensions(&orig, &proc, 8, 8);
        assert!(metrics.valid, "different images should still be valid");
        assert!(metrics.psnr < 20.0, "large difference should have low PSNR");
        assert!(metrics.ssim < 0.5, "large difference should have low SSIM");
    }

    // ── Feature-gated tests ───────────────────────────────────────────────

    #[cfg(feature = "butteraugli")]
    mod butteraugli_tests {
        use super::*;

        // A3: identical 8×8 gives Some(score) near zero
        #[test]
        fn test_butteraugli_identical_8x8_near_zero() {
            let img = solid_rgba(8, 8, 128, 64, 200, 255);
            let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 8, 8);
            let score = metrics
                .butteraugli
                .expect("butteraugli should be Some for identical 8×8 image with feature enabled");
            assert!(
                score < 1.0,
                "Butteraugli score for identical images should be near zero, got {score}"
            );
        }

        // A3: minimum valid size (exactly 8×8) returns Some
        #[test]
        fn test_butteraugli_exactly_8x8_returns_some() {
            let img = solid_rgba(8, 8, 255, 128, 0, 255);
            let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 8, 8);
            assert!(
                metrics.butteraugli.is_some(),
                "butteraugli should be Some for exactly 8×8 image"
            );
        }

        // A3: larger identical image also near zero
        #[test]
        fn test_butteraugli_identical_32x32_near_zero() {
            let img = solid_rgba(32, 32, 100, 150, 200, 255);
            let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 32, 32);
            let score = metrics
                .butteraugli
                .expect("butteraugli should be Some for 32×32 image");
            assert!(
                score < 1.0,
                "Butteraugli score for identical 32×32 images should be near zero, got {score}"
            );
        }

        // A4: strongly different images give larger score than identical
        #[test]
        fn test_butteraugli_different_images_score_larger_than_identical() {
            let white = solid_rgba(16, 16, 255, 255, 255, 255);
            let black = solid_rgba(16, 16, 0, 0, 0, 255);

            let identical_metrics = QualityMetrics::compare_with_dimensions(&white, &white, 16, 16);
            let different_metrics = QualityMetrics::compare_with_dimensions(&white, &black, 16, 16);

            let identical_score = identical_metrics
                .butteraugli
                .expect("butteraugli should be Some for identical 16×16");
            let different_score = different_metrics
                .butteraugli
                .expect("butteraugli should be Some for different 16×16");

            assert!(
                different_score > identical_score,
                "Different images (score={different_score}) should score higher than identical (score={identical_score})"
            );
        }

        // A4: score ordering is monotone — more different = higher score
        #[test]
        fn test_butteraugli_score_increases_with_difference() {
            let base = solid_rgba(16, 16, 128, 128, 128, 255);
            // small perturbation
            let mut small_diff = base.clone();
            for i in (0..small_diff.len()).step_by(4) {
                small_diff[i] = small_diff[i].saturating_add(10);
            }
            // large perturbation
            let large_diff = solid_rgba(16, 16, 0, 0, 0, 255);

            let m_base = QualityMetrics::compare_with_dimensions(&base, &base, 16, 16);
            let m_small = QualityMetrics::compare_with_dimensions(&base, &small_diff, 16, 16);
            let m_large = QualityMetrics::compare_with_dimensions(&base, &large_diff, 16, 16);

            let s_base = m_base.butteraugli.expect("base identical should be Some");
            let s_small = m_small.butteraugli.expect("small diff should be Some");
            let s_large = m_large.butteraugli.expect("large diff should be Some");

            assert!(
                s_base <= s_small,
                "Identical ({s_base}) should score <= small diff ({s_small})"
            );
            assert!(
                s_small < s_large,
                "Small diff ({s_small}) should score < large diff ({s_large})"
            );
        }

        // A5: white-matte compositing — transparent black vs white opaque
        //     After compositing onto white, both become white → same score as identical.
        #[test]
        fn test_butteraugli_transparent_black_vs_white_composited_same() {
            // Fully transparent black: after white matte → (255,255,255)
            let transparent_black = solid_rgba(16, 16, 0, 0, 0, 0);
            // Opaque white: after white matte → (255,255,255)
            let opaque_white = solid_rgba(16, 16, 255, 255, 255, 255);

            let metrics =
                QualityMetrics::compare_with_dimensions(&transparent_black, &opaque_white, 16, 16);
            let score = metrics
                .butteraugli
                .expect("butteraugli should be Some for 16×16");

            // Both composite to white, so score should be near zero
            assert!(
                score < 1.0,
                "Transparent black vs opaque white should composite to same white matte, score={score}"
            );
        }

        // A5: transparent black vs opaque black — after compositing:
        //     transparent black → white, opaque black → black → should differ
        #[test]
        fn test_butteraugli_transparent_black_vs_opaque_black_differ() {
            // Fully transparent black → composites to white
            let transparent_black = solid_rgba(16, 16, 0, 0, 0, 0);
            // Opaque black → composites to black
            let opaque_black = solid_rgba(16, 16, 0, 0, 0, 255);

            let metrics =
                QualityMetrics::compare_with_dimensions(&transparent_black, &opaque_black, 16, 16);
            let score = metrics
                .butteraugli
                .expect("butteraugli should be Some for 16×16");

            // white vs black is a large perceptual difference
            assert!(
                score > 1.0,
                "Transparent black (→white) vs opaque black should differ perceptually, score={score}"
            );
        }

        // A6: dimension/length mismatch guard returns None (not panic)
        #[test]
        fn test_butteraugli_dimension_mismatch_returns_none() {
            // Buffer is sized for 8×8 but we claim 16×16 — mismatch guard must fire.
            let img_8x8 = solid_rgba(8, 8, 100, 100, 100, 255);
            // Pass wrong dimensions (16×16 would need 16*16*4 = 1024 bytes, we have 256)
            let metrics = QualityMetrics::compare_with_dimensions(&img_8x8, &img_8x8, 16, 16);
            assert_eq!(
                metrics.butteraugli, None,
                "butteraugli must be None when buffer length doesn't match claimed dimensions"
            );
        }

        #[test]
        fn test_butteraugli_zero_dimension_returns_none() {
            // Width=0 is below the 8×8 minimum
            let img = solid_rgba(8, 8, 100, 100, 100, 255);
            let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 0, 8);
            assert_eq!(
                metrics.butteraugli, None,
                "butteraugli must be None when width=0"
            );
        }
    }
}
