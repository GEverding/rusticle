use rusticle::{Filter, Gif};

fn main() {
    // Test with a real GIF file
    let test_data =
        std::fs::read("test_gifs/benchmark_suite/cartoon_01.gif").expect("Failed to read test GIF");

    match Gif::from_bytes(&test_data) {
        Ok(gif) => {
            println!("✓ Decoded GIF: {}x{}", gif.width, gif.height);
            println!("  Frames: {}", gif.frames.len());
            println!(
                "  Original palette: {:?}",
                gif.original_palette.as_ref().map(|p| p.len())
            );

            // Resize and encode - should use fast path if palette is acceptable
            match gif.resize(320, 240, Filter::Lanczos3) {
                Ok(resized) => {
                    println!("✓ Resized to: {}x{}", resized.width, resized.height);
                    println!(
                        "  Original palette preserved: {:?}",
                        resized.original_palette.as_ref().map(|p| p.len())
                    );

                    match resized.to_bytes() {
                        Ok(bytes) => {
                            println!("✓ Encoded successfully: {} bytes", bytes.len());
                        }
                        Err(e) => {
                            println!("✗ Encode failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    println!("✗ Resize failed: {}", e);
                }
            }
        }
        Err(e) => {
            println!("✗ Decode failed: {}", e);
        }
    }
}
