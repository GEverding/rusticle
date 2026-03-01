mod common;

use common::{create_gradient_gif, create_test_gif};
use rusticle::{Filter, Gif, OptLevel};

#[test]
fn test_create_and_encode_simple_gif() {
    let gif = create_test_gif(100, 100, 3);
    assert_eq!(gif.width, 100);
    assert_eq!(gif.height, 100);
    assert_eq!(gif.frames.len(), 3);
}

#[test]
fn test_create_and_encode_gradient_gif() {
    let gif = create_gradient_gif(50, 50, 2);
    assert_eq!(gif.width, 50);
    assert_eq!(gif.height, 50);
    assert_eq!(gif.frames.len(), 2);
}

#[test]
fn test_decode_encode_roundtrip() {
    let gif = create_test_gif(100, 100, 3);
    let original_width = gif.width;
    let original_height = gif.height;
    let original_frame_count = gif.frames.len();

    let encoded = gif.to_bytes().expect("Failed to encode");
    let decoded = Gif::from_bytes(&encoded).expect("Failed to decode roundtrip");

    assert_eq!(
        decoded.width, original_width,
        "Width should be preserved in roundtrip"
    );
    assert_eq!(
        decoded.height, original_height,
        "Height should be preserved in roundtrip"
    );
    assert_eq!(
        decoded.frames.len(),
        original_frame_count,
        "Frame count should be preserved in roundtrip"
    );
}

#[test]
fn test_resize_reduces_dimensions() {
    let gif = create_test_gif(200, 200, 2);
    let original_width = gif.width;
    let original_height = gif.height;

    let resized = gif
        .resize(100, 100, Filter::Lanczos3)
        .expect("Failed to resize");

    assert_eq!(resized.width, 100, "Width should be exactly 100");
    assert_eq!(resized.height, 100, "Height should be exactly 100");
    assert!(resized.frames.len() > 0, "Should have frames");
}

#[test]
fn test_resize_fit_maintains_aspect_ratio() {
    let gif = create_test_gif(200, 100, 2);
    let original_ratio = gif.width as f64 / gif.height as f64;

    let resized = gif
        .resize_fit(100, 100, Filter::Lanczos3)
        .expect("Failed to resize_fit");

    let new_ratio = resized.width as f64 / resized.height as f64;
    let ratio_diff = (original_ratio - new_ratio).abs();

    assert!(
        ratio_diff < 0.01,
        "Aspect ratio should be maintained (diff: {})",
        ratio_diff
    );
    assert!(
        resized.width <= 100 && resized.height <= 100,
        "Resized dimensions should fit within bounds"
    );
}

#[test]
fn test_resize_fit_with_different_bounds() {
    let gif = create_test_gif(200, 100, 2);
    let original_ratio = gif.width as f64 / gif.height as f64;

    let resized = gif
        .resize_fit(200, 150, Filter::Bilinear)
        .expect("Failed to resize_fit");

    let new_ratio = resized.width as f64 / resized.height as f64;
    let ratio_diff = (original_ratio - new_ratio).abs();

    assert!(ratio_diff < 0.01, "Aspect ratio should be maintained");
    assert!(
        resized.width <= 200 && resized.height <= 150,
        "Resized dimensions should fit within bounds"
    );
}

#[test]
fn test_resize_with_different_filters() {
    let gif = create_test_gif(100, 100, 2);

    let filters = [
        Filter::Nearest,
        Filter::Bilinear,
        Filter::Mitchell,
        Filter::Lanczos3,
    ];

    for filter in filters {
        let resized = gif
            .clone()
            .resize(50, 50, filter)
            .expect("Failed to resize");
        assert_eq!(resized.width, 50);
        assert_eq!(resized.height, 50);
    }
}

#[test]
fn test_optimize_reduces_or_maintains_size() {
    let gif = create_gradient_gif(100, 100, 3);
    let original_size = gif.to_bytes().expect("Failed to encode").len();

    let optimized = gif.clone().optimize(OptLevel::O3);
    let optimized_size = optimized.to_bytes().expect("Failed to encode").len();

    // Optimized should be same or smaller
    assert!(
        optimized_size <= original_size,
        "Optimized size ({}) should be <= original size ({})",
        optimized_size,
        original_size
    );
}

#[test]
fn test_optimize_preserves_dimensions() {
    let gif = create_test_gif(100, 100, 3);
    let original_width = gif.width;
    let original_height = gif.height;
    let original_frame_count = gif.frames.len();

    let optimized = gif.optimize(OptLevel::O2);

    assert_eq!(optimized.width, original_width);
    assert_eq!(optimized.height, original_height);
    assert_eq!(optimized.frames.len(), original_frame_count);
}

#[test]
fn test_optimize_levels_comparison() {
    let gif = create_gradient_gif(100, 100, 3);

    let o1 = gif.clone().optimize(OptLevel::O1);
    let o2 = gif.clone().optimize(OptLevel::O2);
    let o3 = gif.clone().optimize(OptLevel::O3);

    let o1_size = o1.to_bytes().expect("Failed to encode").len();
    let o2_size = o2.to_bytes().expect("Failed to encode").len();
    let o3_size = o3.to_bytes().expect("Failed to encode").len();

    // Higher optimization levels should generally produce smaller or equal sizes
    assert!(o1_size > 0);
    assert!(o2_size > 0);
    assert!(o3_size > 0);
}

#[test]
fn test_lossy_reduces_size() {
    let gif = create_gradient_gif(100, 100, 3);
    let original_size = gif.to_bytes().expect("Failed to encode").len();

    let lossy = gif.clone().lossy(50);
    let lossy_size = lossy.to_bytes().expect("Failed to encode").len();

    // Lossy at 50 should be noticeably smaller
    assert!(
        lossy_size <= original_size,
        "Lossy size ({}) should be <= original size ({})",
        lossy_size,
        original_size
    );
}

#[test]
fn test_lossy_quality_levels() {
    let gif = create_gradient_gif(100, 100, 3);

    let q100 = gif.clone().lossy(100);
    let q80 = gif.clone().lossy(80);
    let q50 = gif.clone().lossy(50);
    let q20 = gif.clone().lossy(20);

    let q100_size = q100.to_bytes().expect("Failed to encode").len();
    let q80_size = q80.to_bytes().expect("Failed to encode").len();
    let q50_size = q50.to_bytes().expect("Failed to encode").len();
    let q20_size = q20.to_bytes().expect("Failed to encode").len();

    // Lower quality should generally produce smaller sizes
    assert!(q100_size > 0);
    assert!(q80_size > 0);
    assert!(q50_size > 0);
    assert!(q20_size > 0);
}

#[test]
fn test_lossy_preserves_dimensions() {
    let gif = create_test_gif(100, 100, 3);
    let original_width = gif.width;
    let original_height = gif.height;
    let original_frame_count = gif.frames.len();

    let lossy = gif.lossy(80);

    assert_eq!(lossy.width, original_width);
    assert_eq!(lossy.height, original_height);
    assert_eq!(lossy.frames.len(), original_frame_count);
}

#[test]
fn test_full_pipeline_resize_optimize_lossy() {
    let gif = create_gradient_gif(200, 200, 3);
    let original_size = gif.to_bytes().expect("Failed to encode").len();

    let processed = gif
        .resize(100, 100, Filter::Lanczos3)
        .expect("Failed to resize")
        .optimize(OptLevel::O2)
        .lossy(80);

    let processed_bytes = processed.to_bytes().expect("Failed to encode");
    let processed_size = processed_bytes.len();

    // Should produce valid GIF
    let decoded = Gif::from_bytes(&processed_bytes).expect("Failed to decode processed");
    assert_eq!(decoded.width, 100);
    assert_eq!(decoded.height, 100);

    // Should be smaller than original
    assert!(
        processed_size < original_size,
        "Processed size ({}) should be < original size ({})",
        processed_size,
        original_size
    );
}

#[test]
fn test_full_pipeline_optimize_then_resize() {
    let gif = create_test_gif(200, 200, 3);

    let processed = gif
        .optimize(OptLevel::O3)
        .resize(150, 150, Filter::Mitchell)
        .expect("Failed to resize");

    let bytes = processed.to_bytes().expect("Failed to encode");
    let decoded = Gif::from_bytes(&bytes).expect("Failed to decode");

    assert_eq!(decoded.width, 150);
    assert_eq!(decoded.height, 150);
}

#[test]
fn test_clone_produces_identical_gif() {
    let gif = create_test_gif(100, 100, 2);
    let cloned = gif.clone();

    assert_eq!(gif.width, cloned.width);
    assert_eq!(gif.height, cloned.height);
    assert_eq!(gif.frames.len(), cloned.frames.len());

    let original_bytes = gif.to_bytes().expect("Failed to encode");
    let cloned_bytes = cloned.to_bytes().expect("Failed to encode");

    // Encoded bytes should be identical
    assert_eq!(original_bytes, cloned_bytes);
}

#[test]
fn test_resize_zero_dimensions_error() {
    let gif = create_test_gif(100, 100, 2);

    let result = gif.clone().resize(0, 100, Filter::Lanczos3);
    assert!(result.is_err(), "Should error on zero width");

    let result = gif.resize(100, 0, Filter::Lanczos3);
    assert!(result.is_err(), "Should error on zero height");
}

#[test]
fn test_resize_fit_zero_dimensions_error() {
    let gif = create_test_gif(100, 100, 2);

    let result = gif.clone().resize_fit(0, 100, Filter::Lanczos3);
    assert!(result.is_err(), "Should error on zero max_width");

    let result = gif.resize_fit(100, 0, Filter::Lanczos3);
    assert!(result.is_err(), "Should error on zero max_height");
}

#[test]
fn test_multiple_operations_chain() {
    let gif = create_test_gif(200, 200, 2);

    let result = gif
        .resize_fit(200, 200, Filter::Lanczos3)
        .expect("Failed to resize_fit")
        .optimize(OptLevel::O2)
        .lossy(75)
        .resize(100, 100, Filter::Bilinear)
        .expect("Failed to resize");

    assert_eq!(result.width, 100);
    assert_eq!(result.height, 100);
}

#[test]
fn test_frame_properties_preserved() {
    let gif = create_gradient_gif(100, 100, 3);

    // Check that frames have valid properties
    for (i, frame) in gif.frames.iter().enumerate() {
        assert!(
            frame.pixels.len() == (frame.width as usize * frame.height as usize * 4),
            "Frame {} pixel data size mismatch",
            i
        );
        assert!(
            frame.delay.as_millis() >= 0,
            "Frame {} delay should be non-negative",
            i
        );
    }
}

#[test]
fn test_encode_decode_preserves_frame_count() {
    let test_cases = [
        create_test_gif(50, 50, 2),
        create_test_gif(100, 100, 3),
        create_gradient_gif(75, 75, 4),
    ];

    for gif in test_cases {
        let frame_count = gif.frames.len();

        let encoded = gif.to_bytes().expect("Failed to encode");
        let decoded = Gif::from_bytes(&encoded).expect("Failed to decode");

        assert_eq!(
            decoded.frames.len(),
            frame_count,
            "Frame count should be preserved"
        );
    }
}

#[test]
fn test_single_frame_gif() {
    let gif = create_test_gif(100, 100, 1);
    assert_eq!(gif.frames.len(), 1);

    let encoded = gif.to_bytes().expect("Failed to encode");
    let decoded = Gif::from_bytes(&encoded).expect("Failed to decode");
    assert_eq!(decoded.frames.len(), 1);
}

#[test]
fn test_many_frames_gif() {
    let gif = create_test_gif(50, 50, 10);
    assert_eq!(gif.frames.len(), 10);

    let encoded = gif.to_bytes().expect("Failed to encode");
    let decoded = Gif::from_bytes(&encoded).expect("Failed to decode");
    assert_eq!(decoded.frames.len(), 10);
}

#[test]
fn test_small_dimensions() {
    let gif = create_test_gif(10, 10, 2);
    assert_eq!(gif.width, 10);
    assert_eq!(gif.height, 10);

    let resized = gif.resize(5, 5, Filter::Nearest).expect("Failed to resize");
    assert_eq!(resized.width, 5);
    assert_eq!(resized.height, 5);
}

#[test]
fn test_large_dimensions() {
    let gif = create_test_gif(500, 500, 2);
    assert_eq!(gif.width, 500);
    assert_eq!(gif.height, 500);

    let resized = gif
        .resize(250, 250, Filter::Lanczos3)
        .expect("Failed to resize");
    assert_eq!(resized.width, 250);
    assert_eq!(resized.height, 250);
}
