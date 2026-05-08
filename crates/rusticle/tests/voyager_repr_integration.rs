use rusticle::{DisposalMethod, Frame, VoyagerBuilder};
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

#[test]
fn test_voyager_control_path_single_frame() {
    let frame = create_opaque_frame(100, 100, [255, 0, 0]);
    let repr = VoyagerBuilder::build(&[frame], 100, 100).expect("build failed");

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

    // Verify global palette exists
    assert!(!repr.global_palette.is_empty());
    // Palette should be RGB (divisible by 3)
    assert_eq!(repr.global_palette.len() % 3, 0);
}

#[test]
fn test_voyager_control_path_multi_frame_voyager_like() {
    // Simulate a voyager-like sequence: multiple frames with different colors
    let frame1 = create_opaque_frame(64, 64, [255, 0, 0]);     // Red
    let frame2 = create_opaque_frame(64, 64, [0, 255, 0]);     // Green
    let frame3 = create_opaque_frame(64, 64, [0, 0, 255]);     // Blue
    let frame4 = create_opaque_frame(64, 64, [255, 255, 0]);   // Yellow

    let repr = VoyagerBuilder::build(&[frame1, frame2, frame3, frame4], 64, 64)
        .expect("build failed");

    // Verify canvas dimensions
    assert_eq!(repr.width, 64);
    assert_eq!(repr.height, 64);

    // Verify all frames present
    assert_eq!(repr.frames.len(), 4);

    // Verify each frame has full-canvas geometry
    for (i, vframe) in repr.frames.iter().enumerate() {
        assert_eq!(vframe.width, 64, "frame {} width mismatch", i);
        assert_eq!(vframe.height, 64, "frame {} height mismatch", i);
        assert_eq!(vframe.left, 0, "frame {} left mismatch", i);
        assert_eq!(vframe.top, 0, "frame {} top mismatch", i);
        assert_eq!(vframe.indices.len(), 64 * 64, "frame {} indices length mismatch", i);
    }

    // Verify single global palette (not per-frame)
    assert!(!repr.global_palette.is_empty());
    let palette_colors = repr.global_palette.len() / 3;
    assert!(palette_colors <= 256, "palette should have at most 256 colors");

    // Verify all indices are valid
    for vframe in &repr.frames {
        for &idx in &vframe.indices {
            assert!(
                (idx as usize) < palette_colors,
                "index {} out of range for palette with {} colors",
                idx,
                palette_colors
            );
        }
    }
}

#[test]
fn test_voyager_control_path_preserves_timing() {
    let mut frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
    frame1.delay = Duration::from_millis(150);
    frame1.dispose = DisposalMethod::Background;

    let mut frame2 = create_opaque_frame(50, 50, [0, 255, 0]);
    frame2.delay = Duration::from_millis(250);
    frame2.dispose = DisposalMethod::Previous;

    let mut frame3 = create_opaque_frame(50, 50, [0, 0, 255]);
    frame3.delay = Duration::from_millis(350);
    frame3.dispose = DisposalMethod::None;

    let repr = VoyagerBuilder::build(&[frame1, frame2, frame3], 50, 50)
        .expect("build failed");

    // Verify delays are preserved
    assert_eq!(repr.frames[0].delay, Duration::from_millis(150));
    assert_eq!(repr.frames[1].delay, Duration::from_millis(250));
    assert_eq!(repr.frames[2].delay, Duration::from_millis(350));

    // Verify disposal methods are preserved
    assert_eq!(repr.frames[0].dispose, DisposalMethod::Background);
    assert_eq!(repr.frames[1].dispose, DisposalMethod::Previous);
    assert_eq!(repr.frames[2].dispose, DisposalMethod::None);
}

#[test]
fn test_voyager_control_path_no_local_palettes() {
    // The control path should never produce local palettes
    let frame1 = create_opaque_frame(50, 50, [255, 0, 0]);
    let frame2 = create_opaque_frame(50, 50, [0, 255, 0]);

    let repr = VoyagerBuilder::build(&[frame1, frame2], 50, 50).expect("build failed");

    // Verify global palette is used (no local palettes in VoyagerFrame)
    assert!(!repr.global_palette.is_empty());

    // Verify frame count matches
    assert_eq!(repr.frames.len(), 2);
}

#[test]
fn test_voyager_control_path_output_decodable() {
    // Create a sequence that should be decodable
    let frame1 = create_opaque_frame(32, 32, [255, 0, 0]);
    let frame2 = create_opaque_frame(32, 32, [0, 255, 0]);
    let frame3 = create_opaque_frame(32, 32, [0, 0, 255]);

    let repr = VoyagerBuilder::build(&[frame1, frame2, frame3], 32, 32)
        .expect("build failed");

    // Verify frame count matches input
    assert_eq!(repr.frames.len(), 3);

    // Verify each frame has valid indices
    let palette_colors = repr.global_palette.len() / 3;
    for (frame_idx, vframe) in repr.frames.iter().enumerate() {
        assert_eq!(
            vframe.indices.len(),
            32 * 32,
            "frame {} should have 32*32 indices",
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
fn test_voyager_control_path_empty_sequence() {
    let repr = VoyagerBuilder::build(&[], 100, 100).expect("build failed");

    assert_eq!(repr.width, 100);
    assert_eq!(repr.height, 100);
    assert_eq!(repr.frames.len(), 0);
    // Should still have a minimal palette
    assert!(!repr.global_palette.is_empty());
}

#[test]
fn test_voyager_control_path_large_sequence() {
    // Test with a larger sequence to verify scalability
    let mut frames = Vec::new();
    for i in 0..10 {
        let color = [
            ((i * 25) % 256) as u8,
            ((i * 50) % 256) as u8,
            ((i * 75) % 256) as u8,
        ];
        frames.push(create_opaque_frame(64, 64, color));
    }

    let repr = VoyagerBuilder::build(&frames, 64, 64).expect("build failed");

    // Verify all frames are present
    assert_eq!(repr.frames.len(), 10);

    // Verify each frame has correct geometry
    for vframe in &repr.frames {
        assert_eq!(vframe.width, 64);
        assert_eq!(vframe.height, 64);
        assert_eq!(vframe.indices.len(), 64 * 64);
    }

    // Verify palette is reasonable size
    let palette_colors = repr.global_palette.len() / 3;
    assert!(palette_colors > 0);
    assert!(palette_colors <= 256);
}
