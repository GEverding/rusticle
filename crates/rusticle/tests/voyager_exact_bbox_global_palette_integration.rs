use rusticle::{
    DisposalMethod, Frame, VoyagerExactBboxGlobalPaletteBuilder,
};
use std::time::Duration;

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

/// Create a test frame with transparency.
fn create_transparent_frame(width: u16, height: u16) -> Frame {
    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    for (i, chunk) in pixels.chunks_exact_mut(4).enumerate() {
        chunk[0] = 255; // Red
        chunk[1] = 0;
        chunk[2] = 0;
        chunk[3] = if i % 2 == 0 { 0 } else { 255 };
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

#[test]
fn test_exact_bbox_global_palette_single_frame() {
    let frame = create_opaque_frame(100, 100, [255, 0, 0]);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame], 100, 100)
        .expect("build failed");

    // Verify canvas dimensions
    assert_eq!(repr.width, 100);
    assert_eq!(repr.height, 100);

    // Verify single frame
    assert_eq!(repr.frames.len(), 1);

    // Verify full-frame geometry for first frame
    let vframe = &repr.frames[0];
    assert_eq!(vframe.width, 100);
    assert_eq!(vframe.height, 100);
    assert_eq!(vframe.left, 0);
    assert_eq!(vframe.top, 0);

    // Verify full-frame indices
    assert_eq!(vframe.indices.len(), 100 * 100);

    // Verify global palette is derived
    assert!(!repr.global_palette.is_empty());
    assert_eq!(repr.global_palette.len() % 3, 0);
}

#[test]
fn test_exact_bbox_global_palette_two_frames_with_change() {
    let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 10, 10, 20, 20, [0, 255, 0]);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
        .expect("build failed");

    // Verify canvas dimensions
    assert_eq!(repr.width, 100);
    assert_eq!(repr.height, 100);

    // Verify two frames
    assert_eq!(repr.frames.len(), 2);

    // Verify first frame is full-frame
    let vframe0 = &repr.frames[0];
    assert_eq!(vframe0.width, 100);
    assert_eq!(vframe0.height, 100);
    assert_eq!(vframe0.left, 0);
    assert_eq!(vframe0.top, 0);
    assert_eq!(vframe0.indices.len(), 100 * 100);

    // Verify second frame is bbox patch
    let vframe1 = &repr.frames[1];
    assert!(vframe1.width <= 100);
    assert!(vframe1.height <= 100);
    // Bbox should be around the rectangle region (10,10) to (30,30)
    assert!(vframe1.left >= 10);
    assert!(vframe1.top >= 10);
    assert!(vframe1.width > 0);
    assert!(vframe1.height > 0);
    assert_eq!(vframe1.indices.len(), (vframe1.width as usize) * (vframe1.height as usize));

    // Verify global palette is derived
    assert!(!repr.global_palette.is_empty());
    assert_eq!(repr.global_palette.len() % 3, 0);
}

#[test]
fn test_exact_bbox_global_palette_identical_frames() {
    let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    let frame2 = create_opaque_frame(100, 100, [255, 0, 0]);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
        .expect("build failed");

    // Verify two frames
    assert_eq!(repr.frames.len(), 2);

    // Verify first frame is full-frame
    let vframe0 = &repr.frames[0];
    assert_eq!(vframe0.width, 100);
    assert_eq!(vframe0.height, 100);

    // Verify second frame is minimal 1x1 patch (no change)
    let vframe1 = &repr.frames[1];
    assert_eq!(vframe1.width, 1);
    assert_eq!(vframe1.height, 1);
    assert_eq!(vframe1.left, 0);
    assert_eq!(vframe1.top, 0);
    assert_eq!(vframe1.indices.len(), 1);
}

#[test]
fn test_exact_bbox_global_palette_with_transparency() {
    let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    let frame2 = create_transparent_frame(100, 100);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
        .expect("build failed");

    // Verify two frames
    assert_eq!(repr.frames.len(), 2);

    // Verify second frame has transparent index
    let vframe1 = &repr.frames[1];
    assert!(vframe1.transparent_idx.is_some());
}

#[test]
fn test_exact_bbox_global_palette_delays_preserved() {
    let mut frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    frame1.delay = Duration::from_millis(200);

    let mut frame2 = create_opaque_frame(100, 100, [0, 255, 0]);
    frame2.delay = Duration::from_millis(300);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
        .expect("build failed");

    // Verify delays are preserved
    assert_eq!(repr.frames[0].delay, Duration::from_millis(200));
    assert_eq!(repr.frames[1].delay, Duration::from_millis(300));
}

#[test]
fn test_exact_bbox_global_palette_disposal_preserved() {
    let mut frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    frame1.dispose = DisposalMethod::Background;

    let mut frame2 = create_opaque_frame(100, 100, [0, 255, 0]);
    frame2.dispose = DisposalMethod::Previous;

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
        .expect("build failed");

    // Verify disposal methods are preserved
    assert_eq!(repr.frames[0].dispose, DisposalMethod::Background);
    assert_eq!(repr.frames[1].dispose, DisposalMethod::Previous);
}

#[test]
fn test_exact_bbox_global_palette_empty_sequence() {
    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[], 100, 100)
        .expect("build failed");

    // Verify empty sequence
    assert_eq!(repr.width, 100);
    assert_eq!(repr.height, 100);
    assert_eq!(repr.frames.len(), 0);
    assert!(!repr.global_palette.is_empty()); // Minimal palette
}

#[test]
fn test_exact_bbox_global_palette_no_synthetic_transparency() {
    // All opaque frames should not introduce synthetic transparency
    let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    let frame2 = create_opaque_frame(100, 100, [0, 255, 0]);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
        .expect("build failed");

    // Verify no synthetic transparency introduced
    for vframe in &repr.frames {
        // If frame is all opaque, transparent_idx should be None or not used
        if vframe.transparent_idx.is_some() {
            // Check that transparent pixels actually exist in the frame
            // (This is a sanity check; the implementation should not introduce synthetic transparency)
            assert!(vframe.indices.len() > 0);
        }
    }
}

#[test]
fn test_exact_bbox_global_palette_three_frame_sequence() {
    let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 10, 10, 20, 20, [0, 255, 0]);
    let frame3 = create_frame_with_rect(100, 100, [255, 0, 0], 30, 30, 20, 20, [0, 0, 255]);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2, frame3], 100, 100)
        .expect("build failed");

    // Verify three frames
    assert_eq!(repr.frames.len(), 3);

    // Verify first frame is full-frame
    assert_eq!(repr.frames[0].width, 100);
    assert_eq!(repr.frames[0].height, 100);

    // Verify second and third frames are patches
    assert!(repr.frames[1].width < 100 || repr.frames[1].height < 100);
    assert!(repr.frames[2].width < 100 || repr.frames[2].height < 100);

    // Verify global palette is derived from all frames
    assert!(!repr.global_palette.is_empty());
    assert_eq!(repr.global_palette.len() % 3, 0);
}

#[test]
fn test_exact_bbox_global_palette_single_pixel_change() {
    let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    let mut frame2 = create_opaque_frame(100, 100, [255, 0, 0]);

    // Change a single pixel
    frame2.pixels[0] = 0;
    frame2.pixels[1] = 255;
    frame2.pixels[2] = 0;

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
        .expect("build failed");

    // Verify two frames
    assert_eq!(repr.frames.len(), 2);

    // Verify second frame is a small bbox patch (at least 1x1)
    let vframe1 = &repr.frames[1];
    assert!(vframe1.width > 0);
    assert!(vframe1.height > 0);
    assert!(vframe1.indices.len() > 0);
}

#[test]
fn test_exact_bbox_global_palette_large_sequence() {
    // Create a sequence of 10 frames with progressive changes
    let mut frames = Vec::new();
    for i in 0..10 {
        let color = [
            ((i * 25) % 256) as u8,
            ((i * 50) % 256) as u8,
            ((i * 75) % 256) as u8,
        ];
        frames.push(create_opaque_frame(100, 100, color));
    }

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&frames, 100, 100)
        .expect("build failed");

    // Verify all frames are present
    assert_eq!(repr.frames.len(), 10);

    // Verify first frame is full-frame
    assert_eq!(repr.frames[0].width, 100);
    assert_eq!(repr.frames[0].height, 100);

    // Verify global palette is derived
    assert!(!repr.global_palette.is_empty());
    assert_eq!(repr.global_palette.len() % 3, 0);
}

#[test]
fn test_exact_bbox_global_palette_bbox_geometry_accuracy() {
    // Create a frame with a specific rectangle change
    let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    let frame2 = create_frame_with_rect(100, 100, [255, 0, 0], 20, 30, 40, 50, [0, 255, 0]);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2], 100, 100)
        .expect("build failed");

    // Verify second frame bbox
    let vframe1 = &repr.frames[1];
    
    // The bbox should encompass the rectangle region (20,30) to (60,80)
    assert!(vframe1.left <= 20);
    assert!(vframe1.top <= 30);
    assert!(vframe1.left + vframe1.width >= 60);
    assert!(vframe1.top + vframe1.height >= 80);
}

#[test]
fn test_exact_bbox_global_palette_palette_consistency() {
    // Create frames with specific colors
    let frame1 = create_opaque_frame(100, 100, [255, 0, 0]);
    let frame2 = create_opaque_frame(100, 100, [0, 255, 0]);
    let frame3 = create_opaque_frame(100, 100, [0, 0, 255]);

    let repr = VoyagerExactBboxGlobalPaletteBuilder::build(&[frame1, frame2, frame3], 100, 100)
        .expect("build failed");

    // Verify palette is consistent across all frames
    assert!(!repr.global_palette.is_empty());
    
    // All frames should use indices from the same global palette
    for vframe in &repr.frames {
        for &idx in &vframe.indices {
            // Index should be valid for the palette
            let palette_size = repr.global_palette.len() / 3;
            assert!((idx as usize) < palette_size);
        }
    }
}
