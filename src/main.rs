use rusticle::{Filter, Gif, OptLevel};
use std::env;
use std::fs;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!("Usage: rusticle <operation> <input.gif> [output.gif]");
        eprintln!("Operations: resize, optimize, lossy, all");
        std::process::exit(1);
    }

    let op = &args[1];
    let input = &args[2];
    let output = args.get(3).map(|s| s.as_str());

    // Read file
    let start = Instant::now();
    let data = fs::read(input)?;
    let read_time = start.elapsed();

    // Decode
    let start = Instant::now();
    let gif = Gif::from_bytes(&data)?;
    let decode_time = start.elapsed();

    println!(
        "Input: {}x{}, {} frames, {:.2} MB",
        gif.width,
        gif.height,
        gif.frames.len(),
        data.len() as f64 / 1_000_000.0
    );
    println!("Read: {:?}", read_time);
    println!("Decode: {:?}", decode_time);

    // Process based on operation
    let start = Instant::now();
    let processed = match op.as_str() {
        "resize" => gif.resize(320, 240, Filter::Lanczos3)?,
        "optimize" => gif.optimize(OptLevel::O3),
        "lossy" => gif.lossy(80),
        "all" => gif
            .resize(320, 240, Filter::Lanczos3)?
            .optimize(OptLevel::O3)
            .lossy(80),
        _ => {
            eprintln!("Unknown operation: {}", op);
            std::process::exit(1);
        }
    };
    let process_time = start.elapsed();
    println!("Process ({}): {:?}", op, process_time);

    // Encode
    let start = Instant::now();
    let encoded = processed.to_bytes()?;
    let encode_time = start.elapsed();
    println!("Encode: {:?}", encode_time);
    println!(
        "Output: {:.2} MB ({:.1}% of original)",
        encoded.len() as f64 / 1_000_000.0,
        encoded.len() as f64 / data.len() as f64 * 100.0
    );

    // Write if output specified
    if let Some(out_path) = output {
        let start = Instant::now();
        fs::write(out_path, &encoded)?;
        println!("Write: {:?}", start.elapsed());
    }

    println!(
        "Total: {:?}",
        read_time + decode_time + process_time + encode_time
    );

    Ok(())
}
