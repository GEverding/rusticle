#![cfg(feature = "research")]

//! Integration tests for two-path router.
//!
//! Tests covering:
//! - Legacy strategy produces valid output
//! - Auto strategy routes correctly based on classifier
//! - Forced Path A and Path B produce valid output
//! - Fallback works when Path A fails
//! - Telemetry is emitted correctly

use rusticle::{route_optimize, Gif, OptLevel, OptimizerStrategy, TwoPathConfig};
use std::time::Duration;

fn make_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
    use rusticle::types::{DisposalMethod, Frame};

    let frames = (0..frame_count)
        .map(|i| {
            // Create simple gradient frames
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let idx = (y * width as usize + x) * 4;
                    pixels[idx] = (x as u8).wrapping_add(i as u8); // R
                    pixels[idx + 1] = (y as u8).wrapping_add(i as u8); // G
                    pixels[idx + 2] = 128; // B
                    pixels[idx + 3] = 255; // A (opaque)
                }
            }

            Frame {
                pixels,
                delay: Duration::from_millis(100),
                dispose: if i == 0 {
                    DisposalMethod::None
                } else {
                    DisposalMethod::Keep
                },
                local_palette: None,
                left: 0,
                top: 0,
                width,
                height,
            }
        })
        .collect();

    Gif {
        width,
        height,
        global_palette: None,
        frames,
        loop_count: rusticle::types::LoopCount::Infinite,
        original_palette: None,
    }
}

#[test]
fn test_legacy_strategy_produces_valid_output() {
    let gif = make_test_gif(100, 100, 3);
    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Legacy,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = route_optimize(&gif, OptLevel::O3, config);
    assert!(result.is_ok(), "Legacy strategy should succeed");

    let result = result.unwrap();
    assert!(!result.frames.is_empty(), "Should produce frames");
    assert_eq!(result.telemetry.strategy, OptimizerStrategy::Legacy);
    assert_eq!(result.telemetry.selected_path, None);
    assert!(!result.telemetry.fallback_used);
}

#[test]
fn test_auto_strategy_routes_correctly() {
    let gif = make_test_gif(100, 100, 3);
    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = route_optimize(&gif, OptLevel::O3, config);
    assert!(result.is_ok(), "Auto strategy should succeed");

    let result = result.unwrap();
    assert!(!result.frames.is_empty(), "Should produce frames");
    assert_eq!(result.telemetry.strategy, OptimizerStrategy::Auto);
    assert!(
        result.telemetry.selected_path.is_some(),
        "Should select a path"
    );
    assert!(
        result.telemetry.classification.is_some(),
        "Should have classification"
    );
}

#[test]
fn test_forced_path_a_produces_valid_output() {
    let gif = make_test_gif(100, 100, 3);
    let config = TwoPathConfig {
        strategy: OptimizerStrategy::PathA,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = route_optimize(&gif, OptLevel::O3, config);
    assert!(result.is_ok(), "Forced Path A should succeed");

    let result = result.unwrap();
    assert!(!result.frames.is_empty(), "Should produce frames");
    assert_eq!(result.telemetry.strategy, OptimizerStrategy::PathA);
    assert_eq!(
        result.telemetry.selected_path,
        Some(rusticle::OptimizerPath::PathA)
    );
}

#[test]
fn test_forced_path_b_produces_valid_output() {
    let gif = make_test_gif(100, 100, 3);
    let config = TwoPathConfig {
        strategy: OptimizerStrategy::PathB,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = route_optimize(&gif, OptLevel::O3, config);
    assert!(result.is_ok(), "Forced Path B should succeed");

    let result = result.unwrap();
    assert!(!result.frames.is_empty(), "Should produce frames");
    assert_eq!(result.telemetry.strategy, OptimizerStrategy::PathB);
    assert_eq!(
        result.telemetry.selected_path,
        Some(rusticle::OptimizerPath::PathB)
    );
}

#[test]
fn test_telemetry_emission() {
    let gif = make_test_gif(100, 100, 3);
    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: true,
        ..Default::default()
    };

    let result = route_optimize(&gif, OptLevel::O3, config);
    assert!(result.is_ok());

    let result = result.unwrap();
    assert_eq!(result.telemetry.strategy, OptimizerStrategy::Auto);
    assert!(result.telemetry.classification.is_some());

    // Telemetry should have been emitted to stderr (we can't easily capture that here,
    // but we can verify the structure is populated)
    let classification = result.telemetry.classification.unwrap();
    assert!(
        !classification.reasons.is_empty(),
        "Should have classification reasons"
    );
}

#[test]
fn test_all_strategies_produce_encodable_output() {
    let gif = make_test_gif(100, 100, 3);

    for strategy in &[
        OptimizerStrategy::Legacy,
        OptimizerStrategy::Auto,
        OptimizerStrategy::PathA,
        OptimizerStrategy::PathB,
    ] {
        let config = TwoPathConfig {
            strategy: *strategy,
            emit_telemetry: false,
            ..Default::default()
        };

        let result = route_optimize(&gif, OptLevel::O3, config);
        assert!(result.is_ok(), "Strategy {:?} should succeed", strategy);

        let result = result.unwrap();
        assert!(
            !result.frames.is_empty(),
            "Should produce frames for {:?}",
            strategy
        );

        // Verify frames have valid pixel data
        for frame in &result.frames {
            assert!(!frame.pixels.is_empty(), "Frame should have pixels");
            assert_eq!(
                frame.pixels.len() % 4,
                0,
                "Pixel data should be RGBA (multiple of 4)"
            );
        }
    }
}

#[test]
fn test_path_a_fallback_on_error() {
    // Create a GIF that might cause Path A to fail (e.g., with transparency)

    let mut gif = make_test_gif(100, 100, 2);

    // Add transparency to the second frame
    if let Some(frame) = gif.frames.get_mut(1) {
        for chunk in frame.pixels.chunks_exact_mut(4) {
            chunk[3] = 128; // Semi-transparent
        }
    }

    let config = TwoPathConfig {
        strategy: OptimizerStrategy::PathA,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = route_optimize(&gif, OptLevel::O3, config);
    // Should either succeed or fall back gracefully
    assert!(result.is_ok(), "Should handle Path A gracefully");

    let result = result.unwrap();
    assert!(
        !result.frames.is_empty(),
        "Should produce frames even if Path A fails"
    );
}

#[test]
fn test_classification_features_populated() {
    let gif = make_test_gif(100, 100, 3);
    let config = TwoPathConfig {
        strategy: OptimizerStrategy::Auto,
        emit_telemetry: false,
        ..Default::default()
    };

    let result = route_optimize(&gif, OptLevel::O3, config);
    assert!(result.is_ok());

    let result = result.unwrap();
    let classification = result.telemetry.classification.unwrap();

    // Verify all features are populated
    assert!(classification.features.has_transparent_gce == false); // Our test GIF has no transparency
    assert!(classification.features.keep_none_disposal_ratio >= 0.0);
    assert!(classification.features.keep_none_disposal_ratio <= 1.0);
    assert!(classification.features.palette_stability >= 0.0);
    assert!(classification.features.palette_stability <= 1.0);
    assert!(classification.features.offset_patch_ratio >= 0.0);
    assert!(classification.features.offset_patch_ratio <= 1.0);
    assert!(classification.features.median_changed_area_ratio >= 0.0);
    assert!(classification.features.median_changed_area_ratio <= 1.0);
}
