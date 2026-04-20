//! Integration tests for analysis kernels with adaptive IR.
//!
//! Demonstrates how SIMD analysis kernels integrate with the canonical IR
//! and candidate generation pipeline.

use rusticle::{
    adaptive_ir::Canvas,
    analysis_kernels::{
        analyze_changed_pixels_simd, analyze_transparency_simd, analyze_color_distance_simd,
    },
};

/// Create a test canvas with solid color.
fn create_solid_canvas(width: u16, height: u16, r: u8, g: u8, b: u8, a: u8) -> Canvas {
    let size = (width as usize) * (height as usize) * 4;
    let mut pixels = vec![0u8; size];
    for i in (0..size).step_by(4) {
        pixels[i] = r;
        pixels[i + 1] = g;
        pixels[i + 2] = b;
        pixels[i + 3] = a;
    }
    Canvas {
        pixels,
        width,
        height,
    }
}

#[test]
fn test_analyze_changed_pixels_with_canonical_frame() {
    // Create a simple 100x100 canonical frame
    let width = 100u16;
    let height = 100u16;

    // Pre-draw canvas: solid red
    let pre_draw = create_solid_canvas(width, height, 255, 0, 0, 255);

    // Displayed canvas: solid blue (changed)
    let displayed = create_solid_canvas(width, height, 0, 0, 255, 255);

    // Analyze changed pixels using SIMD kernel
    let stats = analyze_changed_pixels_simd(&pre_draw.pixels, &displayed.pixels, 0);

    // All pixels should have changed (red -> blue)
    assert_eq!(stats.changed_count, (width as usize) * (height as usize));
    assert_eq!(stats.became_transparent, 0);
    assert_eq!(stats.became_opaque, 0);
}

#[test]
fn test_analyze_transparency_with_source_patch() {
    // Create a source patch with mixed transparency
    let width = 100u16;
    let height = 100u16;
    let size = (width as usize) * (height as usize) * 4;

    let mut pixels = vec![0u8; size];
    // First half: opaque (alpha = 255)
    for i in (0..size / 2).step_by(4) {
        pixels[i] = 100;
        pixels[i + 1] = 100;
        pixels[i + 2] = 100;
        pixels[i + 3] = 255;
    }
    // Second half: transparent (alpha = 0)
    for i in (size / 2..size).step_by(4) {
        pixels[i] = 100;
        pixels[i + 1] = 100;
        pixels[i + 2] = 100;
        pixels[i + 3] = 0;
    }

    let stats = analyze_transparency_simd(&pixels);

    let expected_pixels = (width as usize) * (height as usize);
    assert_eq!(stats.opaque_count, expected_pixels / 2);
    assert_eq!(stats.transparent_count, expected_pixels / 2);
    assert_eq!(stats.semi_transparent_count, 0);
}

#[test]
fn test_analyze_color_distance_between_frames() {
    // Create two frames with known color difference
    let width = 100u16;
    let height = 100u16;

    // Frame 1: solid red
    let frame1 = create_solid_canvas(width, height, 255, 0, 0, 255);

    // Frame 2: solid green
    let frame2 = create_solid_canvas(width, height, 0, 255, 0, 255);

    // Analyze color distance
    let stats = analyze_color_distance_simd(&frame1.pixels, &frame2.pixels);

    // Expected distance: sqrt(255^2 + 255^2) = sqrt(130050) ≈ 360
    let expected_pixels = (width as usize) * (height as usize);
    assert_eq!(stats.pixel_count, expected_pixels);
    assert!(stats.max_distance > 350 && stats.max_distance < 370);

    // Sum of squared distances: 255^2 + 255^2 = 130050 per pixel
    let expected_sum = 130050u64 * expected_pixels as u64;
    assert_eq!(stats.sum_sq_distance, expected_sum);
}

#[test]
fn test_kernels_with_small_changes() {
    // Simulate a frame with only a small region changed
    let width = 100u16;
    let height = 100u16;

    // Pre-draw: all red
    let pre_draw = create_solid_canvas(width, height, 255, 0, 0, 255);

    // Displayed: mostly red, but a 10x10 region is blue
    let mut displayed = pre_draw.clone();
    for y in 10..20 {
        for x in 10..20 {
            let idx = (y as usize * width as usize + x as usize) * 4;
            displayed.pixels[idx] = 0;     // R
            displayed.pixels[idx + 1] = 0; // G
            displayed.pixels[idx + 2] = 255; // B
        }
    }

    // Analyze changed pixels
    let stats = analyze_changed_pixels_simd(&pre_draw.pixels, &displayed.pixels, 0);

    // Only the 10x10 region should have changed
    assert_eq!(stats.changed_count, 100);
}

#[test]
fn test_kernels_correctness_with_threshold() {
    // Test that threshold parameter works correctly
    let width = 50u16;
    let height = 50u16;

    // Frame 1: solid gray (128, 128, 128)
    let frame1 = create_solid_canvas(width, height, 128, 128, 128, 255);

    // Frame 2: slightly different gray (130, 129, 127)
    let frame2 = create_solid_canvas(width, height, 130, 129, 127, 255);

    // With threshold 0: all pixels changed
    let stats_strict = analyze_changed_pixels_simd(&frame1.pixels, &frame2.pixels, 0);
    assert_eq!(stats_strict.changed_count, (width as usize) * (height as usize));

    // With threshold 3: no pixels changed (all diffs <= 2)
    let stats_loose = analyze_changed_pixels_simd(&frame1.pixels, &frame2.pixels, 3);
    assert_eq!(stats_loose.changed_count, 0);
}

#[test]
fn test_kernels_with_alpha_transitions() {
    // Test tracking of alpha transitions
    let width = 100u16;
    let height = 100u16;

    // Frame 1: all opaque
    let frame1 = create_solid_canvas(width, height, 100, 100, 100, 255);

    // Frame 2: all transparent
    let frame2 = create_solid_canvas(width, height, 100, 100, 100, 0);

    let stats = analyze_changed_pixels_simd(&frame1.pixels, &frame2.pixels, 0);

    // All pixels changed (alpha changed)
    let total_pixels = (width as usize) * (height as usize);
    assert_eq!(stats.changed_count, total_pixels);
    assert_eq!(stats.became_transparent, total_pixels);
    assert_eq!(stats.became_opaque, 0);
}
