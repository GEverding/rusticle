use rusticle::{Filter, Gif};

fn main() {
    let files = [
        "test_gifs/benchmark_suite/cartoon_01.gif",
        "test_gifs/benchmark_suite/photo_01.gif",
        "test_gifs/benchmark_suite/pixel_art_01.gif",
    ];

    for path in &files {
        let data = std::fs::read(path).expect("read");
        let gif = Gif::from_bytes(&data).expect("decode");

        println!("\n=== {} ===", path);
        println!(
            "Original: {}x{}, {} frames, palette: {:?}",
            gif.width,
            gif.height,
            gif.frames.len(),
            gif.original_palette.as_ref().map(|p| p.len())
        );

        // Encode without resize (should definitely hit fast path)
        let (_, stats) = gif.to_bytes_with_stats().expect("encode");
        println!(
            "No resize - fast: {}, imagequant: {}",
            stats.quantize_fast_path_count, stats.quantize_imagequant_count
        );

        // Encode with resize
        let resized = gif
            .clone()
            .resize(320, 240, Filter::Lanczos3)
            .expect("resize");
        let (_, stats2) = resized.to_bytes_with_stats().expect("encode");
        println!(
            "With resize - fast: {}, imagequant: {}",
            stats2.quantize_fast_path_count, stats2.quantize_imagequant_count
        );
    }
}
