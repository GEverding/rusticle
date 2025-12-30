/// Test that lossy compression doesn't cause progressive corruption
/// across multiple encode/decode cycles.
///
/// This test verifies the fix for the bug where lossy compression
/// compared frames to a maintained canvas that got out of sync with
/// actual encode/decode pixel values, causing error accumulation.
mod common;

use common::create_gradient_gif;
use rusticle::Gif;

#[test]
fn test_lossy_no_progressive_corruption() {
    // Create a test GIF with multiple frames
    let original = create_gradient_gif(100, 100, 5);

    // Apply lossy compression
    let lossy1 = original.clone().lossy(80);

    // Encode and decode (simulates saving and loading)
    let bytes1 = lossy1.to_bytes().expect("Failed to encode");
    let decoded1 = Gif::from_bytes(&bytes1).expect("Failed to decode");

    // Apply lossy again to the decoded result
    let lossy2 = decoded1.clone().lossy(80);
    let bytes2 = lossy2.to_bytes().expect("Failed to encode");
    let decoded2 = Gif::from_bytes(&bytes2).expect("Failed to decode");

    // Apply lossy a third time
    let lossy3 = decoded2.clone().lossy(80);
    let bytes3 = lossy3.to_bytes().expect("Failed to encode");
    let decoded3 = Gif::from_bytes(&bytes3).expect("Failed to decode");

    // Verify dimensions are preserved
    assert_eq!(decoded3.width, original.width);
    assert_eq!(decoded3.height, original.height);
    assert_eq!(decoded3.frames.len(), original.frames.len());

    // Verify file size doesn't keep growing (sign of corruption)
    // After the first lossy pass, subsequent passes should produce similar sizes
    let size_diff = (bytes3.len() as i64 - bytes2.len() as i64).abs();
    let size_ratio = size_diff as f64 / bytes2.len() as f64;

    assert!(
        size_ratio < 0.1,
        "File size changed by {:.1}% between passes 2 and 3, indicating corruption. \
         Pass 2: {} bytes, Pass 3: {} bytes",
        size_ratio * 100.0,
        bytes2.len(),
        bytes3.len()
    );
}

#[test]
fn test_lossy_vs_optimize_consistency() {
    // Both lossy and optimize should produce stable results
    let original = create_gradient_gif(100, 100, 5);

    // Test optimize stability
    let opt1 = original.clone().optimize(rusticle::OptLevel::O3);
    let opt_bytes1 = opt1.to_bytes().expect("Failed to encode");
    let opt_decoded1 = Gif::from_bytes(&opt_bytes1).expect("Failed to decode");

    let opt2 = opt_decoded1.clone().optimize(rusticle::OptLevel::O3);
    let opt_bytes2 = opt2.to_bytes().expect("Failed to encode");

    // Test lossy stability
    let lossy1 = original.clone().lossy(80);
    let lossy_bytes1 = lossy1.to_bytes().expect("Failed to encode");
    let lossy_decoded1 = Gif::from_bytes(&lossy_bytes1).expect("Failed to decode");

    let lossy2 = lossy_decoded1.clone().lossy(80);
    let lossy_bytes2 = lossy2.to_bytes().expect("Failed to encode");

    // Both should be stable (second pass produces similar size)
    let opt_diff = (opt_bytes2.len() as i64 - opt_bytes1.len() as i64).abs();
    let opt_ratio = opt_diff as f64 / opt_bytes1.len() as f64;

    let lossy_diff = (lossy_bytes2.len() as i64 - lossy_bytes1.len() as i64).abs();
    let lossy_ratio = lossy_diff as f64 / lossy_bytes1.len() as f64;

    assert!(
        opt_ratio < 0.05,
        "Optimize not stable: {:.1}% change",
        opt_ratio * 100.0
    );

    assert!(
        lossy_ratio < 0.1,
        "Lossy not stable: {:.1}% change",
        lossy_ratio * 100.0
    );
}

#[test]
fn test_lossy_frame_comparison() {
    // Verify that lossy compares frames correctly
    let original = create_gradient_gif(50, 50, 3);

    // Apply lossy
    let lossy = original.clone().lossy(80);

    // Encode and decode
    let bytes = lossy.to_bytes().expect("Failed to encode");
    let decoded = Gif::from_bytes(&bytes).expect("Failed to decode");

    // All frames should have valid pixel data
    for (i, frame) in decoded.frames.iter().enumerate() {
        assert_eq!(
            frame.pixels.len(),
            frame.width as usize * frame.height as usize * 4,
            "Frame {} has invalid pixel data size",
            i
        );

        // Check that not all pixels are transparent (would indicate corruption)
        let opaque_count = frame
            .pixels
            .chunks_exact(4)
            .filter(|chunk| chunk[3] > 0)
            .count();

        assert!(
            opaque_count > 0,
            "Frame {} has no opaque pixels (corrupted)",
            i
        );
    }
}
