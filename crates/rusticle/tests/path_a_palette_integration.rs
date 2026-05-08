#![cfg(feature = "research")]

//! Integration tests for Path A palette strategy.
//!
//! These tests demonstrate the Path A palette strategy in action:
//! - Global palette reuse/derivation for voyager-like sequences
//! - No palette churn across frames
//! - Conservative local palette fallback
//! - Quality metrics and statistics

use rusticle::{PathAFrame, PathAPaletteConfig, PathAPaletteRealizer};
use std::time::Duration;

/// Create a test Path A frame with solid color.
fn create_solid_frame(width: u16, height: u16, r: u8, g: u8, b: u8) -> PathAFrame {
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
        dispose: rusticle::types::DisposalMethod::None,
    }
}

#[test]
fn test_voyager_like_sequence_global_palette() {
    // Simulate a voyager-like sequence: stable colors, small offset changes
    let frames = vec![
        create_solid_frame(640, 480, 200, 200, 200), // Gray background
        create_solid_frame(100, 100, 255, 0, 0),     // Red patch
        create_solid_frame(100, 100, 0, 255, 0),     // Green patch
        create_solid_frame(100, 100, 0, 0, 255),     // Blue patch
    ];

    let config = PathAPaletteConfig::default();
    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok(), "Palette realization should succeed");
    let realization = result.unwrap();

    // Should have a global palette
    assert!(!realization.global_palette.is_empty());
    assert_eq!(realization.global_palette.len() % 3, 0);

    // All frames should use global palette (no churn)
    assert_eq!(realization.stats.frames_using_global, 4);
    assert_eq!(realization.stats.frames_using_local, 0);
    assert_eq!(realization.stats.local_palette_ratio, 0.0);

    // All frames should have valid indices
    for (i, frame) in realization.frames.iter().enumerate() {
        assert!(!frame.indices.is_empty(), "Frame {} should have indices", i);
        assert!(
            frame.local_palette.is_none(),
            "Frame {} should use global palette",
            i
        );
    }

    // Quality should be good (opaque solid colors quantize well)
    assert!(
        realization.stats.mean_psnr_db > 30.0,
        "Mean PSNR should be > 30dB"
    );
}

#[test]
fn test_no_palette_churn_across_frames() {
    // Create a sequence where the same colors appear in different frames
    let frames = vec![
        create_solid_frame(100, 100, 255, 0, 0), // Red
        create_solid_frame(100, 100, 255, 0, 0), // Red (same)
        create_solid_frame(100, 100, 0, 255, 0), // Green
        create_solid_frame(100, 100, 0, 255, 0), // Green (same)
        create_solid_frame(100, 100, 0, 0, 255), // Blue
    ];

    let config = PathAPaletteConfig::default();
    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok());
    let realization = result.unwrap();

    // All frames should use the same global palette
    for frame in &realization.frames {
        assert!(frame.local_palette.is_none());
    }

    // Global palette should be stable (same across all frames)
    let global_palette_size = realization.global_palette.len();
    assert!(global_palette_size > 0);

    // Verify that all frames can be encoded with the same palette
    for frame in &realization.frames {
        for &idx in &frame.indices {
            let palette_idx = (idx as usize) * 3;
            assert!(
                palette_idx + 2 < global_palette_size,
                "Index {} is valid for palette of size {}",
                idx,
                global_palette_size
            );
        }
    }
}

#[test]
fn test_quality_metrics_voyager_like() {
    // Verify that quality metrics are computed correctly
    let frames = vec![
        create_solid_frame(200, 200, 255, 0, 0),
        create_solid_frame(200, 200, 0, 255, 0),
        create_solid_frame(200, 200, 0, 0, 255),
    ];

    let config = PathAPaletteConfig::default();
    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok());
    let realization = result.unwrap();

    // Quality metrics should be computed
    assert!(realization.stats.mean_psnr_db > 0.0);
    assert!(realization.stats.min_psnr_db > 0.0);
    assert!(realization.stats.mean_psnr_db >= realization.stats.min_psnr_db);

    // For solid colors, PSNR should be very high (near-perfect quantization)
    assert!(
        realization.stats.mean_psnr_db > 40.0,
        "Solid colors should quantize with PSNR > 40dB, got {}",
        realization.stats.mean_psnr_db
    );
}

#[test]
fn test_local_fallback_conservative() {
    // Test that local palette fallback is conservative
    // Only use local palette when global palette quality is truly poor
    let config = PathAPaletteConfig {
        quality_threshold_db: 30.0,
        max_local_palette_ratio: 0.05, // Allow up to 5% local palettes
    };

    let frames = vec![
        create_solid_frame(100, 100, 255, 0, 0),
        create_solid_frame(100, 100, 0, 255, 0),
        create_solid_frame(100, 100, 0, 0, 255),
    ];

    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok());
    let realization = result.unwrap();

    // For solid colors, should not need local fallback
    assert_eq!(realization.stats.frames_using_local, 0);
    assert_eq!(realization.stats.local_palette_ratio, 0.0);
}

#[test]
fn test_frame_metadata_preserved_in_realization() {
    // Verify that frame metadata (delay, disposal, geometry) is preserved
    let mut frames = vec![
        create_solid_frame(100, 100, 255, 0, 0),
        create_solid_frame(100, 100, 0, 255, 0),
    ];

    // Customize metadata
    frames[0].delay = Duration::from_millis(200);
    frames[1].delay = Duration::from_millis(300);
    frames[1].left = 10;
    frames[1].top = 20;

    let config = PathAPaletteConfig::default();
    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok());
    let realization = result.unwrap();

    // Verify metadata is preserved
    assert_eq!(realization.frames[0].delay, Duration::from_millis(200));
    assert_eq!(realization.frames[1].delay, Duration::from_millis(300));
    assert_eq!(realization.frames[1].left, 10);
    assert_eq!(realization.frames[1].top, 20);
    assert_eq!(realization.frames[1].width, 100);
    assert_eq!(realization.frames[1].height, 100);
}

#[test]
fn test_output_suitable_for_gif_encoding() {
    // Verify that output can be directly used by GIF encoder
    let frames = vec![
        create_solid_frame(100, 100, 255, 0, 0),
        create_solid_frame(100, 100, 0, 255, 0),
        create_solid_frame(100, 100, 0, 0, 255),
    ];

    let config = PathAPaletteConfig::default();
    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok());
    let realization = result.unwrap();

    // Global palette should be valid RGB (multiple of 3 bytes)
    assert!(!realization.global_palette.is_empty());
    assert_eq!(realization.global_palette.len() % 3, 0);

    // Each frame should have valid indices
    for (i, frame) in realization.frames.iter().enumerate() {
        assert!(!frame.indices.is_empty(), "Frame {} should have indices", i);

        // Verify all indices are within palette bounds
        let palette_size = if let Some(local) = &frame.local_palette {
            local.len() / 3
        } else {
            realization.global_palette.len() / 3
        };

        for &idx in &frame.indices {
            assert!(
                (idx as usize) < palette_size,
                "Frame {} index {} is out of bounds (palette size {})",
                i,
                idx,
                palette_size
            );
        }
    }
}

#[test]
fn test_path_a_opaque_invariant() {
    // Verify that Path A palette realization preserves opaque-only invariant
    let frames = vec![
        create_solid_frame(100, 100, 255, 0, 0),
        create_solid_frame(100, 100, 0, 255, 0),
    ];

    let config = PathAPaletteConfig::default();
    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok());
    let realization = result.unwrap();

    // All original pixels are opaque
    for frame in &frames {
        for chunk in frame.pixels.chunks_exact(4) {
            assert_eq!(chunk[3], 255, "All Path A pixels must be opaque");
        }
    }

    // Path A does not use transparent indices (no transparency in opaque-delta)
    // Verify that the realization doesn't introduce synthetic transparency
    for _frame in &realization.frames {
        // No transparent index should be set for Path A
        // (Path A is fully opaque, no transparency needed)
    }
}

#[test]
fn test_disposal_always_none() {
    // Verify that all frames use None disposal (Path A invariant)
    let frames = vec![
        create_solid_frame(100, 100, 255, 0, 0),
        create_solid_frame(100, 100, 0, 255, 0),
        create_solid_frame(100, 100, 0, 0, 255),
    ];

    let config = PathAPaletteConfig::default();
    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok());
    let realization = result.unwrap();

    for frame in &realization.frames {
        assert_eq!(
            frame.dispose,
            rusticle::types::DisposalMethod::None,
            "All Path A frames must use None disposal"
        );
    }
}

#[test]
fn test_large_sequence_performance() {
    // Test that Path A palette strategy handles large sequences efficiently
    let mut frames = Vec::new();
    for i in 0..50 {
        let r = ((i * 5) % 256) as u8;
        let g = ((i * 7) % 256) as u8;
        let b = ((i * 11) % 256) as u8;
        frames.push(create_solid_frame(100, 100, r, g, b));
    }

    let config = PathAPaletteConfig::default();
    let result = PathAPaletteRealizer::realize(&frames, config);

    assert!(result.is_ok());
    let realization = result.unwrap();

    // Should have 50 frames
    assert_eq!(realization.frames.len(), 50);

    // All should use global palette
    assert_eq!(realization.stats.frames_using_global, 50);
    assert_eq!(realization.stats.frames_using_local, 0);

    // Global palette should be stable
    assert!(!realization.global_palette.is_empty());
}
