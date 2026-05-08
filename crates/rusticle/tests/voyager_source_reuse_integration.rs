#![cfg(feature = "research")]

use rusticle::{
    DisposalMethod, Frame, Gif, LoopCount, Palette, SourceReuseViability, VoyagerSourceReuseBuilder,
};
use std::time::Duration;

mod common;

/// Create a simple test frame with opaque pixels.
fn create_opaque_frame(width: u16, height: u16, color: [u8; 3]) -> Frame {
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    for chunk in pixels.chunks_exact_mut(4) {
        chunk[0] = color[0];
        chunk[1] = color[1];
        chunk[2] = color[2];
        chunk[3] = 255; // Opaque
    }

    Frame {
        pixels,
        delay: Duration::from_millis(100),
        dispose: DisposalMethod::Keep,
        local_palette: None,
        left: 0,
        top: 0,
        width,
        height,
    }
}

/// Create a test frame with a colored rectangle on a background.
fn create_frame_with_rect(
    width: u16,
    height: u16,
    bg_color: [u8; 3],
    rect_left: u16,
    rect_top: u16,
    rect_width: u16,
    rect_height: u16,
    rect_color: [u8; 3],
) -> Frame {
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];

    // Fill background
    for chunk in pixels.chunks_exact_mut(4) {
        chunk[0] = bg_color[0];
        chunk[1] = bg_color[1];
        chunk[2] = bg_color[2];
        chunk[3] = 255;
    }

    // Draw rectangle
    for y in 0..rect_height {
        for x in 0..rect_width {
            let canvas_x = rect_left as usize + x as usize;
            let canvas_y = rect_top as usize + y as usize;
            if canvas_x < width as usize && canvas_y < height as usize {
                let idx = (canvas_y * (width as usize) + canvas_x) * 4;
                pixels[idx] = rect_color[0];
                pixels[idx + 1] = rect_color[1];
                pixels[idx + 2] = rect_color[2];
                pixels[idx + 3] = 255;
            }
        }
    }

    Frame {
        pixels,
        delay: Duration::from_millis(100),
        dispose: DisposalMethod::Keep,
        local_palette: None,
        left: 0,
        top: 0,
        width,
        height,
    }
}

/// Create a test GIF with a source global palette.
fn create_test_gif_with_palette(palette_colors: Vec<[u8; 3]>) -> Gif {
    Gif {
        width: 100,
        height: 100,
        global_palette: Some(Palette {
            colors: palette_colors,
        }),
        frames: vec![],
        loop_count: LoopCount::Infinite,
        original_palette: None,
    }
}

/// Create a test GIF without global palette.
fn create_test_gif_no_palette() -> Gif {
    Gif {
        width: 100,
        height: 100,
        global_palette: None,
        frames: vec![],
        loop_count: LoopCount::Infinite,
        original_palette: None,
    }
}

#[test]
fn test_source_reuse_viable_single_frame() {
    let frame = create_opaque_frame(100, 100, [255, 0, 0]);
    let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let source_gif = create_test_gif_with_palette(palette);

    let repr =
        VoyagerSourceReuseBuilder::build(&[frame], 100, 100, &source_gif).expect("build failed");

    // Verify viability
    assert_eq!(repr.viability, SourceReuseViability::Viable);

    // Verify canvas dimensions
    assert_eq!(repr.width, 100);
    assert_eq!(repr.height, 100);

    // Verify single frame
    assert_eq!(repr.frames.len(), 1);

    // Verify full-frame geometry
    let vframe = &repr.frames[0];
    assert_eq!(vframe.width, 100);
    assert_eq!(vframe.height, 100);
    assert_eq!(vframe.left, 0);
    assert_eq!(vframe.top, 0);

    // Verify full-frame indices
    assert_eq!(vframe.indices.len(), 100 * 100);

    // Verify global palette is reused from source
    assert!(!repr.global_palette.is_empty());
    assert_eq!(repr.global_palette.len(), 3 * 3); // 3 colors * 3 bytes each
}

#[test]
fn test_source_reuse_no_source_palette() {
    let frame = create_opaque_frame(100, 100, [255, 0, 0]);
    let source_gif = create_test_gif_no_palette();

    let repr =
        VoyagerSourceReuseBuilder::build(&[frame], 100, 100, &source_gif).expect("build failed");

    // Verify not viable
    assert_eq!(repr.viability, SourceReuseViability::NoSourceGlobalPalette);

    // Should have no frames
    assert_eq!(repr.frames.len(), 0);
}

#[test]
fn test_source_reuse_exact_bbox_patches() {
    // Create a voyager-like sequence with small changes
    let frame0 = create_frame_with_rect(100, 100, [200, 200, 200], 0, 0, 100, 100, [200, 200, 200]);
    let frame1 = create_frame_with_rect(100, 100, [200, 200, 200], 0, 0, 100, 100, [200, 200, 200]);

    // Add a small change in frame1 at (10, 10) with size 20x20
    let mut frame1_pixels = frame1.pixels.clone();
    for y in 10..30 {
        for x in 10..30 {
            let idx = (y * 100 + x) * 4;
            frame1_pixels[idx] = 100;
            frame1_pixels[idx + 1] = 100;
            frame1_pixels[idx + 2] = 100;
        }
    }
    let frame1_modified = Frame {
        pixels: frame1_pixels,
        ..frame1
    };

    let palette = vec![[200, 200, 200], [100, 100, 100]];
    let source_gif = create_test_gif_with_palette(palette);

    let repr = VoyagerSourceReuseBuilder::build(&[frame0, frame1_modified], 100, 100, &source_gif)
        .expect("build failed");

    assert_eq!(repr.viability, SourceReuseViability::Viable);
    assert_eq!(repr.frames.len(), 2);

    // Frame 0 should be full-frame
    assert_eq!(repr.frames[0].left, 0);
    assert_eq!(repr.frames[0].top, 0);
    assert_eq!(repr.frames[0].width, 100);
    assert_eq!(repr.frames[0].height, 100);

    // Frame 1 should be bbox patch (20x20 at 10,10)
    assert_eq!(repr.frames[1].left, 10);
    assert_eq!(repr.frames[1].top, 10);
    assert_eq!(repr.frames[1].width, 20);
    assert_eq!(repr.frames[1].height, 20);
}

#[test]
fn test_source_reuse_identical_frames_minimal_patch() {
    let frame0 = create_opaque_frame(50, 50, [255, 0, 0]);
    let frame1 = frame0.clone();

    let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let source_gif = create_test_gif_with_palette(palette);

    let repr = VoyagerSourceReuseBuilder::build(&[frame0, frame1], 50, 50, &source_gif)
        .expect("build failed");

    assert_eq!(repr.viability, SourceReuseViability::Viable);
    assert_eq!(repr.frames.len(), 2);

    // Frame 1 should be 1x1 minimal patch
    assert_eq!(repr.frames[1].width, 1);
    assert_eq!(repr.frames[1].height, 1);
    assert_eq!(repr.frames[1].left, 0);
    assert_eq!(repr.frames[1].top, 0);
}

#[test]
fn test_source_reuse_no_local_palette_churn() {
    // Source-reuse candidate should never produce local palettes
    let frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
    let frame2 = create_opaque_frame(50, 50, [0, 255, 0]);

    let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let source_gif = create_test_gif_with_palette(palette);

    let repr = VoyagerSourceReuseBuilder::build(&[frame1, frame2], 50, 50, &source_gif)
        .expect("build failed");

    assert_eq!(repr.viability, SourceReuseViability::Viable);

    // Verify global palette is used (no local_palette field in VoyagerSourceReuseFrame)
    assert_eq!(repr.frames.len(), 2);
    assert!(!repr.global_palette.is_empty());
}

#[test]
fn test_source_reuse_preserves_timing_and_disposal() {
    let mut frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
    frame1.delay = Duration::from_millis(200);
    frame1.dispose = DisposalMethod::Background;

    let mut frame2 = create_opaque_frame(50, 50, [0, 255, 0]);
    frame2.delay = Duration::from_millis(300);
    frame2.dispose = DisposalMethod::Previous;

    let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let source_gif = create_test_gif_with_palette(palette);

    let repr = VoyagerSourceReuseBuilder::build(&[frame1, frame2], 50, 50, &source_gif)
        .expect("build failed");

    assert_eq!(repr.viability, SourceReuseViability::Viable);

    // Verify delays are preserved
    assert_eq!(repr.frames[0].delay, Duration::from_millis(200));
    assert_eq!(repr.frames[1].delay, Duration::from_millis(300));

    // Verify disposal methods are preserved
    assert_eq!(repr.frames[0].dispose, DisposalMethod::Background);
    assert_eq!(repr.frames[1].dispose, DisposalMethod::Previous);
}

#[test]
fn test_source_reuse_empty_sequence() {
    let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let source_gif = create_test_gif_with_palette(palette);

    let repr = VoyagerSourceReuseBuilder::build(&[], 100, 100, &source_gif).expect("build failed");

    assert_eq!(repr.viability, SourceReuseViability::Viable);
    assert_eq!(repr.frames.len(), 0);
}

#[test]
fn test_source_reuse_output_decodable() {
    // Create a sequence that should be decodable
    let frame1 = create_opaque_frame(32, 32, [255, 0, 0]);
    let frame2 = create_opaque_frame(32, 32, [0, 255, 0]);
    let frame3 = create_opaque_frame(32, 32, [0, 0, 255]);

    let palette = vec![[255, 0, 0], [0, 255, 0], [0, 0, 255]];
    let source_gif = create_test_gif_with_palette(palette);

    let repr = VoyagerSourceReuseBuilder::build(&[frame1, frame2, frame3], 32, 32, &source_gif)
        .expect("build failed");

    assert_eq!(repr.viability, SourceReuseViability::Viable);

    // Verify frame count matches input
    assert_eq!(repr.frames.len(), 3);

    // Verify each frame has valid indices
    let palette_colors = repr.global_palette.len() / 3;
    for (frame_idx, vframe) in repr.frames.iter().enumerate() {
        assert!(
            !vframe.indices.is_empty(),
            "frame {} should have indices",
            frame_idx
        );

        // All indices should be valid
        for &idx in &vframe.indices {
            assert!(
                (idx as usize) < palette_colors,
                "frame {} has invalid index {}",
                frame_idx,
                idx
            );
        }
    }

    // Verify canvas dimensions are consistent
    assert_eq!(repr.width, 32);
    assert_eq!(repr.height, 32);
}

#[test]
fn test_source_reuse_multi_frame_voyager_like() {
    // Simulate a voyager-like sequence with small changes
    let frame1 = create_opaque_frame(64, 64, [200, 200, 200]);
    let frame2 = create_frame_with_rect(64, 64, [200, 200, 200], 10, 10, 20, 20, [100, 100, 100]);
    let frame3 = create_frame_with_rect(64, 64, [200, 200, 200], 30, 30, 15, 15, [150, 150, 150]);
    let frame4 = create_opaque_frame(64, 64, [200, 200, 200]);

    let palette = vec![[200, 200, 200], [100, 100, 100], [150, 150, 150]];
    let source_gif = create_test_gif_with_palette(palette);

    let repr =
        VoyagerSourceReuseBuilder::build(&[frame1, frame2, frame3, frame4], 64, 64, &source_gif)
            .expect("build failed");

    assert_eq!(repr.viability, SourceReuseViability::Viable);

    // Verify all frames present
    assert_eq!(repr.frames.len(), 4);

    // Frame 0 should be full-frame
    assert_eq!(repr.frames[0].width, 64);
    assert_eq!(repr.frames[0].height, 64);

    // Frames 1-3 should be bbox patches (except frame 3 which is identical to frame 0)
    assert!(repr.frames[1].width < 64 || repr.frames[1].height < 64);
    assert!(repr.frames[2].width < 64 || repr.frames[2].height < 64);

    // Verify all indices are valid
    let palette_colors = repr.global_palette.len() / 3;
    for vframe in &repr.frames {
        for &idx in &vframe.indices {
            assert!((idx as usize) < palette_colors);
        }
    }
}

#[test]
fn test_source_reuse_large_palette() {
    // Test with a larger palette (up to 256 colors)
    let mut palette = Vec::new();
    for i in 0..256 {
        palette.push([
            (i % 256) as u8,
            ((i * 2) % 256) as u8,
            ((i * 3) % 256) as u8,
        ]);
    }

    let frame = create_opaque_frame(50, 50, [255, 0, 0]);
    let source_gif = create_test_gif_with_palette(palette);

    let repr =
        VoyagerSourceReuseBuilder::build(&[frame], 50, 50, &source_gif).expect("build failed");

    assert_eq!(repr.viability, SourceReuseViability::Viable);
    assert_eq!(repr.global_palette.len(), 256 * 3); // 256 colors * 3 bytes each
}
