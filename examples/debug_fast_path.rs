use rusticle::{Filter, Gif, OptLevel};

fn main() {
    let data = std::fs::read("test_gifs/benchmark_suite/cartoon_01.gif").unwrap();
    let gif = Gif::from_bytes(&data).unwrap();

    println!("=== Resize only ===");
    let resized = gif.clone().resize(240, 240, Filter::Lanczos3).unwrap();
    println!(
        "Palette preserved: {:?}",
        resized.original_palette.as_ref().map(|p| p.len())
    );

    println!("\n=== After O3 ===");
    let optimized = resized.optimize(OptLevel::O3);
    println!(
        "Palette after O3: {:?}",
        optimized.original_palette.as_ref().map(|p| p.len())
    );

    // The issue: does optimize() clear the palette?
    println!("\n=== Frame analysis after O3 ===");
    for (i, frame) in optimized.frames.iter().take(5).enumerate() {
        let transparent = frame.pixels.chunks_exact(4).filter(|p| p[3] < 128).count();
        let total = frame.pixels.len() / 4;
        println!(
            "Frame {}: {}x{} at ({},{}), {}/{} transparent ({:.1}%)",
            i,
            frame.width,
            frame.height,
            frame.left,
            frame.top,
            transparent,
            total,
            100.0 * transparent as f64 / total as f64
        );
    }

    // Now encode with debug
    println!("\n=== Encoding with debug ===");
    std::env::set_var("RUSTICLE_DEBUG", "1");
    let (_, stats) = optimized.to_bytes_with_stats().unwrap();
    println!(
        "\nFast: {}, imagequant: {}",
        stats.quantize_fast_path_count, stats.quantize_imagequant_count
    );
}
