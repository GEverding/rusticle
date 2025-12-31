use rusticle::{Filter, Gif, OptLevel};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = fs::read("test_gifs/benchmark_suite/cartoon_01.gif")?;
    let gif = Gif::from_bytes(&data)?;

    // Resize + O2
    let optimized = gif
        .clone()
        .resize(240, 240, Filter::Lanczos3)?
        .optimize(OptLevel::O2);

    // Check frame 1's transparent pixels
    let f1 = &optimized.frames[1];
    let pixel_count = f1.width as usize * f1.height as usize;

    // Count transparent pixels and their RGB values
    let mut trans_colors: std::collections::HashMap<(u8, u8, u8), usize> =
        std::collections::HashMap::new();
    for i in 0..pixel_count {
        let idx = i * 4;
        if f1.pixels[idx + 3] < 128 {
            let key = (f1.pixels[idx], f1.pixels[idx + 1], f1.pixels[idx + 2]);
            *trans_colors.entry(key).or_insert(0) += 1;
        }
    }

    println!("Transparent pixel colors in frame 1:");
    for (color, count) in &trans_colors {
        println!("  RGB{:?}: {} pixels", color, count);
    }

    // Now encode just frame 1 and see what index is used
    let bytes = optimized.to_bytes()?;

    // Parse the GIF to find frame 1's transparent index
    let mut i = 0;
    let mut frame = 0;
    while i < bytes.len() - 8 {
        if bytes[i] == 0x21 && bytes[i + 1] == 0xF9 {
            let packed = bytes[i + 3];
            let trans_flag = packed & 0x01;
            let trans_idx = bytes[i + 6];
            if frame == 1 {
                println!(
                    "\nFrame 1 GCE: trans_flag={}, trans_idx={}",
                    trans_flag, trans_idx
                );
                break;
            }
            frame += 1;
        }
        i += 1;
    }

    Ok(())
}
