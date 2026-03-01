use rusticle::Gif;

#[test]
fn test_decode_empty_bytes() {
    // Empty bytes should fail gracefully
    let result = Gif::from_bytes(&[]);
    assert!(result.is_err());
}

#[test]
fn test_decode_from_read() {
    // Test that from_read is available
    let data: &[u8] = &[];
    let result = Gif::from_read(data);
    assert!(result.is_err());
}

#[test]
fn test_compositing_produces_full_frames() {
    // Load a real GIF and verify frames are composited to full canvas size
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let data = std::fs::read(workspace_root.join("outputs/original_test3.gif"))
        .expect("test file should exist");
    let gif = Gif::from_bytes(&data).expect("should decode");

    // All frames should now be full canvas size (composited)
    for (i, frame) in gif.frames.iter().enumerate() {
        assert_eq!(
            frame.width, gif.width,
            "Frame {} width should match canvas width after compositing",
            i
        );
        assert_eq!(
            frame.height, gif.height,
            "Frame {} height should match canvas height after compositing",
            i
        );
        assert_eq!(
            frame.left, 0,
            "Frame {} should be at left=0 after compositing",
            i
        );
        assert_eq!(
            frame.top, 0,
            "Frame {} should be at top=0 after compositing",
            i
        );
        assert_eq!(
            frame.pixels.len(),
            (gif.width as usize) * (gif.height as usize) * 4,
            "Frame {} should have full canvas pixel data",
            i
        );
    }
}
