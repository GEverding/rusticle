# rusticle

High-performance GIF resize, optimize, and encode library.

> **Alpha software.** API may change. Not validated at scale or across diverse GIF inputs. Use at your own risk.

## What it does

Rust library and CLI for decoding, resizing, optimizing, and encoding GIF images. 3–6x faster than gifsicle on tested inputs. Produces comparable or smaller output files.

## Requirements

- **Nightly Rust** — uses `#![feature(portable_simd)]` for SIMD pixel operations
- `gifsicle` — optional, required only for benchmark comparisons

## Quick start

**Library:**

```rust
use rusticle::{Gif, Filter, OptLevel};

let data = std::fs::read("input.gif")?;
let bytes = Gif::from_bytes(&data)?
    .resize(640, 480, Filter::Lanczos3)?
    .optimize(OptLevel::O2)
    .lossy(80)
    .to_bytes()?;
std::fs::write("output.gif", bytes)?;
```

**CLI:**

```bash
cargo install rusticle-cli

# Resize to 320x240
rusticle resize input.gif -W 320 -H 240

# Resize preserving aspect ratio + optimize + lossy
rusticle resize input.gif --fit -W 640 -H 480 --optimize o3 --lossy 80

# Compare quality between two GIFs
rusticle quality original.gif processed.gif
```

## Performance

| Operation | Speedup vs gifsicle |
|-----------|---------------------|
| Resize (fast path) | 4.6–4.9× |
| Resize (fallback) | 3.5–3.8× |
| Full pipeline | 4.7–6.2× |

See [docs/BENCHMARKS.md](docs/BENCHMARKS.md) for full details.

## How it's fast

**Palette LUT fast path** — For GIFs with a global palette, builds a 262KB lookup table (64³ entries, 6 bits per channel) for O(1) nearest-neighbor color mapping. Skips expensive imagequant quantization entirely. Quality-gated: automatically falls back to imagequant if avg distance² ≥ 150, outlier ratio ≥ 5%, or palette utilization ≤ 30%.

**Parallel frame processing** — Rayon `par_iter` for quantization and frame optimization. Scales with core count.

**SIMD pixel operations** — Portable SIMD (`std::simd`, u8x16) for marking unchanged pixels and computing diff bounding boxes between frames. Requires nightly Rust.

**Diff bounding box cropping** — At `OptLevel::O3`, frames are cropped to only the changed region vs the previous frame, reducing encoding work.

**SIMD-accelerated resize** — Uses the `fast_image_resize` crate (AVX2/NEON internally).

**imagequant fallback** — Same quantization engine as pngquant/gifski. Used when the fast path would produce insufficient quality.

**Transparent index optimization** — Prefers index 0 for transparency to improve LZW compression ratio.

**jemalloc** — CLI binary uses jemalloc on non-MSVC targets.

## Tradeoffs

- Fast path trades ~5 dB PSNR for ~5× speed on palette-matching GIFs (GOOD vs EXCELLENT quality)
- Requires nightly Rust (`portable_simd`)
- Lossy compression is conservative — max threshold 20 even at `quality=0`
- Pipeline ordering matters: calling `optimize()` before `lossy()` can cause lossy to no-op on cropped subframes

## API

| Type | Description |
|------|-------------|
| `Gif` | Core type. Decode, resize, optimize, encode. |
| `Frame` | Single GIF frame with RGBA pixels and metadata. |
| `Filter` | Resize algorithm: `Nearest`, `Bilinear`, `Mitchell`, `Lanczos3`. |
| `OptLevel` | Optimization level: `O1` (basic), `O2` (standard), `O3` (aggressive with diff cropping). |
| `QualityMetrics` | PSNR/SSIM/Butteraugli comparison between frames. |
| `PaletteLut` | O(1) palette color lookup table (internal, but public). |
| `EncodeStats` | Encoding statistics (fast path vs fallback, timing). |

## Quality Metrics

The `QualityMetrics` type compares two images and returns:

- **PSNR** (Peak Signal-to-Noise Ratio): Higher is better. Typical range 30–50 dB.
- **SSIM** (Structural Similarity Index): Higher is better. Range 0–1, with > 0.95 excellent.
- **Butteraugli** (perceptual distance): **Lower is better** (opposite of PSNR/SSIM). Requires `butteraugli` feature and image dimensions ≥ 8×8. Typical range: < 1.0 imperceptible, 1.0–2.0 good, > 3.0 noticeable.

Enable Butteraugli support:

```toml
[dependencies]
rusticle = { version = "0.1", features = ["butteraugli"] }
```

Then use `QualityMetrics::compare_with_dimensions()` to compute Butteraugli scores:

```rust
use rusticle::QualityMetrics;

let metrics = QualityMetrics::compare_with_dimensions(&original, &processed, 640, 480);
if let Some(ba) = metrics.butteraugli {
    println!("Butteraugli: {:.2} (lower is better)", ba);
}
```

## Project structure

```
crates/
├── rusticle/          # Library crate
├── rusticle-cli/      # CLI binary (clap + jemalloc)
└── rusticle-bench/    # Internal benchmarks vs gifsicle
```

## Features

```toml
[features]
async = ["tokio"]           # Async I/O support
serde = ["dep:serde"]       # Serialize types (Filter, OptLevel, QualityMetrics, etc.)
image = ["dep:image"]       # image crate conversions (Frame ↔ RgbaImage)
butteraugli = ["dep:butteraugli"]  # Butteraugli perceptual distance metrics
```

With the `image` feature enabled:

```rust
use rusticle::{Frame, Gif};
use image::RgbaImage;

// Frame → RgbaImage (zero-copy, consuming)
let img: RgbaImage = frame.try_into()?;

// RgbaImage → Frame
let frame: Frame = img.try_into()?;

// Build Gif from Vec<RgbaImage>
let gif = Gif::from_rgba_images(images, Duration::from_millis(100))?;

// Extract frames as Vec<RgbaImage>
let images = gif.into_rgba_images()?;
```

## Building

```bash
# Library
cargo check -p rusticle

# CLI (release, for accurate perf)
cargo build --release -p rusticle-cli

# Tests
cargo test -p rusticle

# Lint
cargo clippy --workspace -- -D warnings
```

## License

MIT
