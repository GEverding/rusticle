//! Integration tests for voyager exact bbox + global palette + fallback representation.

use rusticle::VoyagerExactBboxGlobalPaletteFallbackBuilder;
use std::time::Duration;

mod common;
use common::make_frame;

#[test]
fn test_fallback_repr_produces_valid_structure() {
    // Create a simple 2-frame sequence with a small change
    let frame1 = make_frame(100, 100, [255, 0, 0, 255]);
    let mut frame2 = make_frame(100, 100, [255, 0, 0, 255]);
    // Modify a small region
    for i in 0..400 {
        frame2.pixels[i] = 0;
    }

    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1, frame2],
        100,
        100,
        0.7,
    )
    .expect("build failed");

    // Verify structure
    assert_eq!(repr.width, 100);
    assert_eq!(repr.height, 100);
    assert_eq!(repr.frames.len(), 2);
    assert!(!repr.global_palette.is_empty());
    assert_eq!(repr.global_palette.len() % 3, 0); // RGB triplets

    // Verify first frame is full-frame
    let frame0 = &repr.frames[0];
    assert_eq!(frame0.width, 100);
    assert_eq!(frame0.height, 100);
    assert_eq!(frame0.left, 0);
    assert_eq!(frame0.top, 0);
    assert_eq!(frame0.indices.len(), 10000);

    // Verify second frame is bbox patch (small change)
    let frame1 = &repr.frames[1];
    assert!(frame1.width < 100 || frame1.height < 100);
    assert_eq!(frame1.indices.len(), (frame1.width as usize) * (frame1.height as usize));
}

#[test]
fn test_fallback_repr_large_change_triggers_full_frame() {
    // Create a 2-frame sequence with a large change
    let frame1 = make_frame(100, 100, [255, 0, 0, 255]);
    let mut frame2 = make_frame(100, 100, [255, 0, 0, 255]);
    // Modify a large region (80x80 = 6400 pixels)
    for y in 10..90 {
        for x in 10..90 {
            let idx = (y * 100 + x) * 4;
            frame2.pixels[idx] = 0;
            frame2.pixels[idx + 1] = 255;
            frame2.pixels[idx + 2] = 0;
        }
    }

    // Threshold 0.5 = 5000 pixels, bbox = 6400 pixels -> should trigger full-frame
    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1, frame2],
        100,
        100,
        0.5,
    )
    .expect("build failed");

    // Verify second frame is full-frame (fallback triggered)
    let frame1 = &repr.frames[1];
    assert_eq!(frame1.width, 100);
    assert_eq!(frame1.height, 100);
    assert_eq!(frame1.left, 0);
    assert_eq!(frame1.top, 0);
    assert_eq!(frame1.indices.len(), 10000);
}

#[test]
fn test_fallback_repr_threshold_boundary() {
    // Create a 2-frame sequence with a change that's exactly at the boundary
    let frame1 = make_frame(100, 100, [255, 0, 0, 255]);
    let mut frame2 = make_frame(100, 100, [255, 0, 0, 255]);
    // Modify a region that's 4999 pixels (just under 50% of 10000)
    for y in 0..70 {
        for x in 0..71 {
            if y * 71 + x < 4999 {
                let idx = (y * 100 + x) * 4;
                frame2.pixels[idx] = 0;
            }
        }
    }

    // Threshold 0.5 = 5000 pixels, bbox area < 5000 -> should stay as patch
    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1, frame2],
        100,
        100,
        0.5,
    )
    .expect("build failed");

    let frame1 = &repr.frames[1];
    // Should be bbox patch, not full-frame
    assert!(frame1.width < 100 || frame1.height < 100);
}

#[test]
fn test_fallback_repr_preserves_delays() {
    let mut frame1 = make_frame(100, 100, [255, 0, 0, 255]);
    frame1.delay = Duration::from_millis(150);

    let mut frame2 = make_frame(100, 100, [0, 255, 0, 255]);
    frame2.delay = Duration::from_millis(250);

    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1, frame2],
        100,
        100,
        0.7,
    )
    .expect("build failed");

    assert_eq!(repr.frames[0].delay, Duration::from_millis(150));
    assert_eq!(repr.frames[1].delay, Duration::from_millis(250));
}

#[test]
fn test_fallback_repr_preserves_disposal() {
    use rusticle::DisposalMethod;

    let mut frame1 = make_frame(100, 100, [255, 0, 0, 255]);
    frame1.dispose = DisposalMethod::Background;

    let mut frame2 = make_frame(100, 100, [0, 255, 0, 255]);
    frame2.dispose = DisposalMethod::Previous;

    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1, frame2],
        100,
        100,
        0.7,
    )
    .expect("build failed");

    assert_eq!(repr.frames[0].dispose, DisposalMethod::Background);
    assert_eq!(repr.frames[1].dispose, DisposalMethod::Previous);
}

#[test]
fn test_fallback_repr_multi_frame_sequence() {
    // Create a 4-frame sequence with varying change sizes
    let frame1 = make_frame(100, 100, [255, 0, 0, 255]);

    let mut frame2 = make_frame(100, 100, [255, 0, 0, 255]);
    // Small change (10x10 = 100 pixels)
    for i in 0..400 {
        frame2.pixels[i] = 0;
    }

    let mut frame3 = make_frame(100, 100, [255, 0, 0, 255]);
    // Large change (80x80 = 6400 pixels)
    for y in 10..90 {
        for x in 10..90 {
            let idx = (y * 100 + x) * 4;
            frame3.pixels[idx] = 0;
            frame3.pixels[idx + 1] = 255;
        }
    }

    let mut frame4 = make_frame(100, 100, [255, 0, 0, 255]);
    // Same large change as frame3
    for y in 10..90 {
        for x in 10..90 {
            let idx = (y * 100 + x) * 4;
            frame4.pixels[idx] = 0;
            frame4.pixels[idx + 1] = 255;
        }
    }

    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1, frame2, frame3, frame4],
        100,
        100,
        0.5,
    )
    .expect("build failed");

    assert_eq!(repr.frames.len(), 4);

    // Frame 0: full-frame
    assert_eq!(repr.frames[0].width, 100);
    assert_eq!(repr.frames[0].height, 100);

    // Frame 1: small change -> bbox patch
    assert!(repr.frames[1].width < 100 || repr.frames[1].height < 100);

    // Frame 2: large change -> full-frame (fallback)
    assert_eq!(repr.frames[2].width, 100);
    assert_eq!(repr.frames[2].height, 100);

    // Frame 3: no change (identical to frame 2) -> minimal 1x1 patch
    assert_eq!(repr.frames[3].width, 1);
    assert_eq!(repr.frames[3].height, 1);
}

#[test]
fn test_fallback_repr_different_thresholds() {
    // Create a sequence with a 50% change
    let frame1 = make_frame(100, 100, [255, 0, 0, 255]);
    let mut frame2 = make_frame(100, 100, [255, 0, 0, 255]);
    // Modify 50% of the canvas (5000 pixels)
    for i in 0..20000 {
        if i % 2 == 0 {
            frame2.pixels[i] = 0;
        }
    }

    // Threshold 0.4 (4000 pixels) -> should trigger full-frame
    let repr_low = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1.clone(), frame2.clone()],
        100,
        100,
        0.4,
    )
    .expect("build failed");
    assert_eq!(repr_low.frames[1].width, 100);
    assert_eq!(repr_low.frames[1].height, 100);

    // Threshold 0.6 (6000 pixels) -> should stay as patch
    let repr_high = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1, frame2],
        100,
        100,
        0.6,
    )
    .expect("build failed");
    assert!(repr_high.frames[1].width < 100 || repr_high.frames[1].height < 100);
}

#[test]
fn test_fallback_repr_empty_sequence() {
    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[], 100, 100, 0.7)
        .expect("build failed");

    assert_eq!(repr.width, 100);
    assert_eq!(repr.height, 100);
    assert_eq!(repr.frames.len(), 0);
    assert!(!repr.global_palette.is_empty());
}

#[test]
fn test_fallback_repr_single_frame() {
    let frame = make_frame(100, 100, [255, 0, 0, 255]);

    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(&[frame], 100, 100, 0.7)
        .expect("build failed");

    assert_eq!(repr.frames.len(), 1);
    assert_eq!(repr.frames[0].width, 100);
    assert_eq!(repr.frames[0].height, 100);
}

#[test]
fn test_fallback_repr_with_transparency() {
    let frame1 = make_frame(100, 100, [255, 0, 0, 255]);

    let mut frame2 = make_frame(100, 100, [255, 0, 0, 255]);
    // Add some transparent pixels
    for i in 0..100 {
        frame2.pixels[i * 4 + 3] = 0; // Set alpha to 0
    }

    let repr = VoyagerExactBboxGlobalPaletteFallbackBuilder::build(
        &[frame1, frame2],
        100,
        100,
        0.7,
    )
    .expect("build failed");

    // Verify second frame has transparent index
    assert!(repr.frames[1].transparent_idx.is_some());
}
