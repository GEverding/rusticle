use rusticle::Gif;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Test with a transparent sample GIF
    let data = fs::read("samples/transparent_1_resize_50pct.gif")?;
    let gif = Gif::from_bytes(&data)?;

    println!("=== Transparent Index Optimization Test ===");
    println!("Original file size: {} bytes", data.len());
    println!("Number of frames: {}", gif.frames.len());

    // Re-encode to see if transparent index is optimized
    let encoded = gif.to_bytes()?;
    println!("Re-encoded size: {} bytes", encoded.len());

    // Decode the re-encoded version to check transparent indices
    let gif2 = Gif::from_bytes(&encoded)?;
    println!("Re-decoded frames: {}", gif2.frames.len());

    // Check if transparent pixels are using index 0
    if let Some(frame) = gif2.frames.first() {
        println!("\nFirst frame analysis:");
        println!("  Dimensions: {}x{}", frame.width, frame.height);
        println!("  Pixel count: {}", frame.pixels.len() / 4);

        // Count transparent pixels
        let transparent_count = frame.pixels.chunks_exact(4).filter(|p| p[3] < 128).count();
        println!("  Transparent pixels: {}", transparent_count);
    }

    Ok(())
}
