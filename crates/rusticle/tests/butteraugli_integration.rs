//! Integration tests for Butteraugli quality metrics.
//!
//! Tests cover:
//! - `compare_with_dimensions` returning `Some` for valid ≥8×8 frames
//! - Mismatch guard returning `None` without panicking
//! - Real GIF decode + compare path (when benchmark_suite is available)

use rusticle::QualityMetrics;

// ── helpers ──────────────────────────────────────────────────────────────────

fn solid_rgba(width: u32, height: u32, r: u8, g: u8, b: u8, a: u8) -> Vec<u8> {
    let n = (width * height) as usize;
    let mut buf = Vec::with_capacity(n * 4);
    for _ in 0..n {
        buf.extend_from_slice(&[r, g, b, a]);
    }
    buf
}

// ── Mismatch guard (feature-independent) ─────────────────────────────────────

/// Buffer sized for 8×8 but dimensions claim 16×16 → must return None, not panic.
#[test]
fn test_compare_with_dimensions_mismatch_guard_returns_none() {
    let img_8x8 = solid_rgba(8, 8, 128, 128, 128, 255);
    // 16×16 would need 1024 bytes; we only have 256
    let metrics = QualityMetrics::compare_with_dimensions(&img_8x8, &img_8x8, 16, 16);
    assert_eq!(
        metrics.butteraugli, None,
        "butteraugli must be None when buffer length doesn't match claimed dimensions (mismatch guard)"
    );
}

/// Mismatched original vs processed buffer sizes must not panic (compare() panics by contract,
/// but compare_with_dimensions with matching sizes and wrong dims must not).
#[test]
fn test_compare_with_dimensions_sub8_no_panic() {
    // 4×4 is below the 8×8 minimum — must not panic regardless of feature flag.
    let img = solid_rgba(4, 4, 200, 100, 50, 255);
    let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 4, 4);
    assert_eq!(
        metrics.butteraugli, None,
        "butteraugli must be None for 4×4 (below minimum)"
    );
    // Non-butteraugli metrics should still be valid
    assert!(
        metrics.psnr.is_infinite(),
        "PSNR should be infinite for identical images"
    );
}

// ── Feature-gated integration tests ──────────────────────────────────────────

#[cfg(feature = "butteraugli")]
mod butteraugli_integration {
    use super::*;
    use rusticle::{Filter, Gif};

    /// Identical synthetic 8×8 frame → Some(score) near zero.
    #[test]
    fn test_butteraugli_synthetic_identical_returns_some() {
        let img = solid_rgba(8, 8, 180, 90, 45, 255);
        let metrics = QualityMetrics::compare_with_dimensions(&img, &img, 8, 8);
        assert!(
            metrics.butteraugli.is_some(),
            "butteraugli must be Some for valid 8×8 image with feature enabled"
        );
        let score = metrics.butteraugli.unwrap();
        assert!(
            score < 1.0,
            "Identical image butteraugli score should be near zero, got {score}"
        );
    }

    /// Strongly different synthetic frames → score is larger than identical.
    #[test]
    fn test_butteraugli_synthetic_different_larger_than_identical() {
        let white = solid_rgba(32, 32, 255, 255, 255, 255);
        let black = solid_rgba(32, 32, 0, 0, 0, 255);

        let m_identical = QualityMetrics::compare_with_dimensions(&white, &white, 32, 32);
        let m_different = QualityMetrics::compare_with_dimensions(&white, &black, 32, 32);

        let s_identical = m_identical
            .butteraugli
            .expect("identical 32×32 should return Some");
        let s_different = m_different
            .butteraugli
            .expect("different 32×32 should return Some");

        assert!(
            s_different > s_identical,
            "Different images (score={s_different}) must score higher than identical (score={s_identical})"
        );
    }

    /// Decode a real GIF from benchmark_suite, compare first frame to itself.
    /// Asserts `Some` is returned (dimensions are well above 8×8).
    /// Skips gracefully if the file is absent.
    #[test]
    fn test_butteraugli_real_gif_first_frame_identical() {
        let suite_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_gifs/benchmark_suite");

        let gif_path = suite_dir.join("cartoon_02.gif");
        if !gif_path.exists() {
            eprintln!("Skipping: benchmark_suite not available at {gif_path:?}");
            return;
        }

        let data =
            std::fs::read(&gif_path).unwrap_or_else(|e| panic!("Failed to read {gif_path:?}: {e}"));
        let gif =
            Gif::from_bytes(&data).unwrap_or_else(|e| panic!("Failed to decode {gif_path:?}: {e}"));

        assert!(!gif.frames.is_empty(), "GIF must have at least one frame");

        let frame = &gif.frames[0];
        let w = frame.width as u32;
        let h = frame.height as u32;

        // Only run butteraugli comparison if frame is large enough
        if w < 8 || h < 8 {
            eprintln!("Skipping: first frame is {w}×{h}, below 8×8 minimum");
            return;
        }

        let metrics = QualityMetrics::compare_with_dimensions(&frame.pixels, &frame.pixels, w, h);

        assert!(
            metrics.butteraugli.is_some(),
            "butteraugli must be Some for {w}×{h} frame from real GIF"
        );
        let score = metrics.butteraugli.unwrap();
        assert!(
            score < 1.0,
            "Identical frame butteraugli score should be near zero, got {score}"
        );
    }

    /// Decode a real GIF, resize first frame down then back up, compare original vs
    /// round-tripped. Asserts `Some` is returned and score is finite (no threshold).
    #[test]
    fn test_butteraugli_real_gif_resize_roundtrip_returns_some() {
        let suite_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_gifs/benchmark_suite");

        let gif_path = suite_dir.join("cartoon_02.gif");
        if !gif_path.exists() {
            eprintln!("Skipping: benchmark_suite not available at {gif_path:?}");
            return;
        }

        let data =
            std::fs::read(&gif_path).unwrap_or_else(|e| panic!("Failed to read {gif_path:?}: {e}"));
        let gif =
            Gif::from_bytes(&data).unwrap_or_else(|e| panic!("Failed to decode {gif_path:?}: {e}"));

        assert!(!gif.frames.is_empty(), "GIF must have at least one frame");

        let orig_w = gif.width as u32;
        let orig_h = gif.height as u32;

        if orig_w < 16 || orig_h < 16 {
            eprintln!("Skipping: GIF is {orig_w}×{orig_h}, too small for resize roundtrip");
            return;
        }

        let half_w = (orig_w / 2).max(8);
        let half_h = (orig_h / 2).max(8);

        // Resize down then back up
        let resized_down = gif
            .clone()
            .resize(half_w, half_h, Filter::Lanczos3)
            .unwrap_or_else(|e| panic!("Failed to resize down: {e}"));

        let resized_back = resized_down
            .resize(orig_w, orig_h, Filter::Lanczos3)
            .unwrap_or_else(|e| panic!("Failed to resize back: {e}"));

        let orig_frame = &gif.frames[0];
        let back_frame = &resized_back.frames[0];

        assert_eq!(
            back_frame.width as u32, orig_w,
            "Round-tripped frame width should match original"
        );
        assert_eq!(
            back_frame.height as u32, orig_h,
            "Round-tripped frame height should match original"
        );

        let metrics = QualityMetrics::compare_with_dimensions(
            &orig_frame.pixels,
            &back_frame.pixels,
            orig_w,
            orig_h,
        );

        assert!(
            metrics.butteraugli.is_some(),
            "butteraugli must be Some for {orig_w}×{orig_h} resize-roundtrip comparison"
        );

        let score = metrics.butteraugli.unwrap();
        assert!(
            score.is_finite(),
            "butteraugli score must be finite, got {score}"
        );
        // Loose sanity bound: resize roundtrip degrades quality but shouldn't be catastrophic
        assert!(
            score < 20.0,
            "butteraugli score for resize roundtrip should be reasonable, got {score}"
        );
    }

    /// Mismatch guard: buffer sized for 8×8 but dimensions claim 32×32 → None.
    #[test]
    fn test_butteraugli_integration_mismatch_guard() {
        let img_8x8 = solid_rgba(8, 8, 100, 150, 200, 255);
        // Claim 32×32 (needs 4096 bytes, have 256)
        let metrics = QualityMetrics::compare_with_dimensions(&img_8x8, &img_8x8, 32, 32);
        assert_eq!(
            metrics.butteraugli, None,
            "butteraugli must be None when buffer/dimension mismatch (integration)"
        );
    }
}
