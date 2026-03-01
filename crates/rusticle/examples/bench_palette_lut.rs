use rusticle::PaletteLut;
use std::time::Instant;

fn main() {
    // Create a test palette with 256 colors
    let mut palette = vec![[0u8; 3]; 256];
    for i in 0..256 {
        palette[i] = [i as u8, (i >> 1) as u8, (i >> 2) as u8];
    }

    // Warm up
    let _ = PaletteLut::new(&palette);

    // Time the LUT construction (multiple runs)
    let mut times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let _lut = PaletteLut::new(&palette);
        let elapsed = start.elapsed();
        times.push(elapsed.as_secs_f64() * 1000.0);
    }

    let avg = times.iter().sum::<f64>() / times.len() as f64;
    let min = times.iter().copied().fold(f64::INFINITY, f64::min);
    let max = times.iter().copied().fold(0.0, f64::max);

    println!("PaletteLut construction (256 colors):");
    println!("  Min: {:.2}ms", min);
    println!("  Avg: {:.2}ms", avg);
    println!("  Max: {:.2}ms", max);
}
