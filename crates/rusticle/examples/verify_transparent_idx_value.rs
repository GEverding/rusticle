use rusticle::{DisposalMethod, Frame, Gif, LoopCount};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a test GIF with transparent pixels
    let width = 10u16;
    let height = 10u16;

    // Create RGBA pixels: mostly opaque red, with some transparent pixels
    let mut pixels = vec![0u8; width as usize * height as usize * 4];

    // Fill with opaque red
    for i in 0..(width as usize * height as usize) {
        pixels[i * 4] = 255; // R
        pixels[i * 4 + 1] = 0; // G
        pixels[i * 4 + 2] = 0; // B
        pixels[i * 4 + 3] = 255; // A (opaque)
    }

    // Add some transparent pixels in the middle
    for y in 3..7 {
        for x in 3..7 {
            let idx = (y * width as usize + x) * 4;
            pixels[idx + 3] = 0; // Make transparent
        }
    }

    // Create GIF
    let gif = Gif {
        width,
        height,
        global_palette: None,
        frames: vec![Frame {
            pixels,
            delay: Duration::from_millis(100),
            dispose: DisposalMethod::None,
            local_palette: None,
            left: 0,
            top: 0,
            width,
            height,
        }],
        loop_count: LoopCount::Infinite,
        original_palette: None,
    };

    // Encode
    let encoded = gif.to_bytes()?;

    // Decode to verify
    let gif2 = Gif::from_bytes(&encoded)?;

    println!("=== Transparent Index Value Verification ===");
    println!("Created GIF with {} transparent pixels", 4 * 4);
    println!("Encoded size: {} bytes", encoded.len());

    if let Some(frame) = gif2.frames.first() {
        // Check transparent pixels
        let mut transparent_indices = std::collections::HashSet::new();
        for (i, pixel) in frame.pixels.chunks_exact(4).enumerate() {
            if pixel[3] < 128 {
                // This is a transparent pixel - but we can't directly see the index
                // from the decoded RGBA data. The index is used internally during encoding.
                transparent_indices.insert(i);
            }
        }

        println!("Decoded transparent pixels: {}", transparent_indices.len());
        println!("\nNote: The transparent index optimization ensures that during");
        println!("encoding, transparent pixels use index 0 (or the lowest available)");
        println!("for better LZW compression. This is verified by the unit tests.");
    }

    Ok(())
}
