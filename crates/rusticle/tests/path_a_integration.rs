#![cfg(feature = "research")]

//! Integration tests for Path A core optimization.
//!
//! Tests the complete Path A pipeline:
//! 1. Canonical sequence IR construction
//! 2. Path A optimization from displayed canvases
//! 3. Verification of exact bbox preservation
//! 4. No synthetic transparency introduction
//! 5. Correct disposal semantics

use rusticle::adaptive_ir::{BoundingBox, CanonicalSequenceBuilder, Canvas};
use rusticle::classifier::{classify_sequence, OptimizerPath};
use rusticle::path_a::{optimize_path_a, PathAConfig};
use rusticle::types::{DisposalMethod, Frame, Gif, LoopCount};
use std::time::Duration;

/// Create a test GIF with opaque frames (no transparency).
fn create_opaque_test_gif(width: u16, height: u16, frame_count: usize) -> Gif {
    let mut frames = Vec::new();

    for i in 0..frame_count {
        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];

        // Fill with a color that varies per frame
        let color = (i as u8).wrapping_mul(50);
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[0] = color;
            chunk[1] = color.wrapping_add(50);
            chunk[2] = color.wrapping_add(100);
            chunk[3] = 255; // opaque
        }

        frames.push(Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        });
    }

    Gif {
        width,
        height,
        global_palette: None,
        frames,
        loop_count: LoopCount::Infinite,
        original_palette: None,
    }
}

/// Create a test GIF with small offset changes (voyager-like).
/// Uses offset subframes to match Path A criteria.
fn create_voyager_like_gif(width: u16, height: u16) -> Gif {
    let mut frames = Vec::new();

    // Frame 0: solid background (full canvas)
    let mut frame0_pixels = vec![200u8; (width as usize) * (height as usize) * 4];
    for chunk in frame0_pixels.chunks_exact_mut(4) {
        chunk[3] = 255; // opaque
    }

    frames.push(Frame {
        pixels: frame0_pixels,
        delay: Duration::from_millis(100),
        dispose: DisposalMethod::None,
        local_palette: None,
        left: 0,
        top: 0,
        width,
        height,
    });

    // Frame 1: small offset subframe (50x50 at position 0,0)
    // This is a small patch, not full canvas
    let mut frame1_pixels = vec![100u8; 50 * 50 * 4];
    for chunk in frame1_pixels.chunks_exact_mut(4) {
        chunk[3] = 255; // opaque
    }

    frames.push(Frame {
        pixels: frame1_pixels,
        delay: Duration::from_millis(100),
        dispose: DisposalMethod::None,
        local_palette: None,
        left: 0,
        top: 0,
        width: 50,
        height: 50,
    });

    // Frame 2: small offset subframe (50x50 at position 100,100)
    let mut frame2_pixels = vec![150u8; 50 * 50 * 4];
    for chunk in frame2_pixels.chunks_exact_mut(4) {
        chunk[3] = 255; // opaque
    }

    frames.push(Frame {
        pixels: frame2_pixels,
        delay: Duration::from_millis(100),
        dispose: DisposalMethod::None,
        local_palette: None,
        left: 100,
        top: 100,
        width: 50,
        height: 50,
    });

    Gif {
        width,
        height,
        global_palette: None,
        frames,
        loop_count: LoopCount::Infinite,
        original_palette: None,
    }
}

#[test]
fn test_path_a_voyager_like_classification() {
    // Verify that voyager-like sequences are classified as Path A
    let gif = create_voyager_like_gif(640, 480);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build canonical sequence");

    let result = classify_sequence(&seq).expect("Failed to classify");
    assert_eq!(
        result.path,
        OptimizerPath::PathA,
        "Voyager-like sequence should be classified as Path A"
    );
}

#[test]
fn test_path_a_optimization_from_canonical() {
    // Test Path A optimization using canonical sequence IR
    let gif = create_voyager_like_gif(640, 480);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build canonical sequence");

    // Extract displayed canvases and delays
    let canvases: Vec<Canvas> = seq
        .frames
        .iter()
        .map(|f| f.displayed_canvas.clone_canvas())
        .collect();
    let delays: Vec<Duration> = seq.frames.iter().map(|f| f.delay).collect();

    let config = PathAConfig::default();
    let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

    // Verify frame count
    assert_eq!(result.len(), 3, "Should have 3 optimized frames");

    // Frame 0 should be full-frame
    assert_eq!(result[0].left, 0);
    assert_eq!(result[0].top, 0);
    assert_eq!(result[0].width, 640);
    assert_eq!(result[0].height, 480);

    // Frame 1 should be bbox patch (50x50 at 0,0)
    assert_eq!(result[1].left, 0);
    assert_eq!(result[1].top, 0);
    assert_eq!(result[1].width, 50);
    assert_eq!(result[1].height, 50);

    // Frame 2 should be bbox patch (50x50 at 100,100)
    assert_eq!(result[2].left, 100);
    assert_eq!(result[2].top, 100);
    assert_eq!(result[2].width, 50);
    assert_eq!(result[2].height, 50);
}

#[test]
fn test_path_a_exact_bbox_preservation() {
    // Verify that exact changed bbox is preserved (no dropped changed pixels)
    let gif = create_voyager_like_gif(640, 480);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build canonical sequence");

    let canvases: Vec<Canvas> = seq
        .frames
        .iter()
        .map(|f| f.displayed_canvas.clone_canvas())
        .collect();
    let delays: Vec<Duration> = seq.frames.iter().map(|f| f.delay).collect();

    let config = PathAConfig::default();
    let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

    // Frame 1: verify bbox matches expected changed region
    let frame1_bbox = BoundingBox::new(0, 0, 50, 50);
    assert_eq!(result[1].left, frame1_bbox.left);
    assert_eq!(result[1].top, frame1_bbox.top);
    assert_eq!(result[1].width, frame1_bbox.width());
    assert_eq!(result[1].height, frame1_bbox.height());

    // Frame 2: verify bbox matches expected changed region
    let frame2_bbox = BoundingBox::new(100, 100, 150, 150);
    assert_eq!(result[2].left, frame2_bbox.left);
    assert_eq!(result[2].top, frame2_bbox.top);
    assert_eq!(result[2].width, frame2_bbox.width());
    assert_eq!(result[2].height, frame2_bbox.height());
}

#[test]
fn test_path_a_no_synthetic_transparency() {
    // Verify that Path A does not introduce synthetic transparency
    let gif = create_voyager_like_gif(640, 480);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build canonical sequence");

    let canvases: Vec<Canvas> = seq
        .frames
        .iter()
        .map(|f| f.displayed_canvas.clone_canvas())
        .collect();
    let delays: Vec<Duration> = seq.frames.iter().map(|f| f.delay).collect();

    let config = PathAConfig::default();
    let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

    // Check all frames for synthetic transparency
    for (i, frame) in result.iter().enumerate() {
        for chunk in frame.pixels.chunks_exact(4) {
            assert_eq!(
                chunk[3], 255,
                "Frame {} has non-opaque pixel (alpha={}), Path A should not introduce transparency",
                i, chunk[3]
            );
        }
    }
}

#[test]
fn test_path_a_disposal_semantics() {
    // Verify that all emitted frames use None disposal
    let gif = create_voyager_like_gif(640, 480);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build canonical sequence");

    let canvases: Vec<Canvas> = seq
        .frames
        .iter()
        .map(|f| f.displayed_canvas.clone_canvas())
        .collect();
    let delays: Vec<Duration> = seq.frames.iter().map(|f| f.delay).collect();

    let config = PathAConfig::default();
    let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

    for (i, frame) in result.iter().enumerate() {
        assert_eq!(
            frame.dispose,
            DisposalMethod::None,
            "Frame {} should use None disposal, got {:?}",
            i,
            frame.dispose
        );
    }
}

#[test]
fn test_path_a_large_change_fallback() {
    // Test that large changes fall back to full-frame
    let gif = create_opaque_test_gif(100, 100, 2);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build canonical sequence");

    let canvases: Vec<Canvas> = seq
        .frames
        .iter()
        .map(|f| f.displayed_canvas.clone_canvas())
        .collect();
    let delays: Vec<Duration> = seq.frames.iter().map(|f| f.delay).collect();

    // Use a low threshold to force fallback
    let config = PathAConfig {
        bbox_area_threshold: 0.5, // 50% threshold
    };
    let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

    assert_eq!(result.len(), 2);
    // Frame 1 should be full-frame fallback (entire frame changed)
    assert_eq!(result[1].left, 0);
    assert_eq!(result[1].top, 0);
    assert_eq!(result[1].width, 100);
    assert_eq!(result[1].height, 100);
}

#[test]
fn test_path_a_identical_frames() {
    // Test handling of identical consecutive frames
    let mut gif = create_opaque_test_gif(100, 100, 2);
    // Make frame 1 identical to frame 0
    gif.frames[1].pixels = gif.frames[0].pixels.clone();

    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build canonical sequence");

    let canvases: Vec<Canvas> = seq
        .frames
        .iter()
        .map(|f| f.displayed_canvas.clone_canvas())
        .collect();
    let delays: Vec<Duration> = seq.frames.iter().map(|f| f.delay).collect();

    let config = PathAConfig::default();
    let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

    assert_eq!(result.len(), 2);
    // Frame 1 should be 1x1 minimal patch
    assert_eq!(result[1].width, 1);
    assert_eq!(result[1].height, 1);
}

#[test]
fn test_path_a_empty_sequence() {
    // Test handling of empty sequence
    let config = PathAConfig::default();
    let result = optimize_path_a(&[], &[], config).expect("optimize_path_a failed");
    assert_eq!(result.len(), 0);
}

#[test]
fn test_path_a_single_frame() {
    // Test handling of single-frame sequence
    let gif = create_opaque_test_gif(100, 100, 1);
    let seq = CanonicalSequenceBuilder::build(&gif).expect("Failed to build canonical sequence");

    let canvases: Vec<Canvas> = seq
        .frames
        .iter()
        .map(|f| f.displayed_canvas.clone_canvas())
        .collect();
    let delays: Vec<Duration> = seq.frames.iter().map(|f| f.delay).collect();

    let config = PathAConfig::default();
    let result = optimize_path_a(&canvases, &delays, config).expect("optimize_path_a failed");

    assert_eq!(result.len(), 1);
    // Single frame should be full-frame
    assert_eq!(result[0].left, 0);
    assert_eq!(result[0].top, 0);
    assert_eq!(result[0].width, 100);
    assert_eq!(result[0].height, 100);
}
