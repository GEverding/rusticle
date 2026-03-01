# Rusticle Benchmarks

Performance comparisons against gifsicle 1.96 on Apple Silicon (M-series).

## Test Environment

- macOS (darwin)
- Rust 1.x with `--release` profile
- jemalloc allocator
- SIMD via fast_image_resize

## Resize Performance vs gifsicle

### With Fast Path (global palette GIFs)

| Test File | Frames | rusticle | gifsicle | Speedup |
|-----------|--------|----------|----------|---------|
| cartoon_01.gif (1.8MB) | 24 | 104ms | 479ms | **4.6x** |
| photo_01.gif (2.7MB) | 56 | 199ms | 982ms | **4.9x** |

### Fallback Path (local palette GIFs)

| Test File | Frames | rusticle | gifsicle | Speedup |
|-----------|--------|----------|----------|---------|
| test3.gif (9MB) | 197 | 894ms | 3144ms | **3.5x** |
| test2.gif (34MB) | 367 | 2.4s | 9.2s | **3.8x** |

### Full Pipeline: resize + optimize O3 + lossy 80

| Test File | Frames | rusticle | gifsicle | Speedup | Output Size |
|-----------|--------|----------|----------|---------|-------------|
| test3.gif (9MB) | 197 | 2.1s | 9.9s | **4.7x** | 5.9MB vs 6.5MB |
| test2.gif (34MB) | 367 | 3.7s | 22.9s | **6.2x** | 6.0MB vs 5.1MB |

## Fast Path Internal Optimization

Building PaletteLUT once per GIF vs per frame:

| Metric | Per-frame LUT | Single LUT | Improvement |
|--------|---------------|------------|-------------|
| Encode time | 223ms | 46ms | **4.8x** |
| Total time | 278ms | 100ms | **2.8x** |

This 4.8x internal improvement is on top of already being faster than gifsicle.

## Diff-Based Bounding Box

SIMD-accelerated detection of changed regions between frames for optimized encoding.

### Performance

| Frame Size | Scenario | Time |
|------------|----------|------|
| 100x100 | Center diff (10x10) | 5.4 µs |
| 320x240 | Corner diff (40x40) | 52.9 µs |
| 640x480 | Bottom-right diff (worst case) | 65.3 µs |
| 320x240 | Identical (early exit) | 8.9 µs |

### Key Observations

- **Small diffs**: ~5.4 µs for 100x100 frame with localized change
- **Medium diffs**: ~53 µs for 320x240 frame with corner region change
- **Large diffs (worst case)**: ~65 µs for 640x480 frame requiring full scan
- **Identical frames**: Early exit optimization provides ~9 µs baseline

The diff bounding box detection enables:
1. Cropping frames to only changed regions (reduces encoding work)
2. Skipping unchanged frames entirely
3. Optimized disposal methods based on actual changes

## Compression Quality

### Output Size Comparison

| Operation | rusticle | gifsicle | Winner |
|-----------|----------|----------|--------|
| resize 320x240 | 7.8MB | 8.0MB | rusticle |
| optimize O3 | 8.3MB | 9.0MB | rusticle |
| full pipeline | 5.9MB | 6.5MB | **rusticle** |

### Fast Path Quality Thresholds

Triggers fallback to imagequant if exceeded:
- Average distance²: > 150
- Outlier ratio: > 5%
- Palette utilization: < 30%

Typical fast path stats on cartoon GIFs:
- Average distance²: ~29
- Outlier ratio: 0%
- Palette utilization: 88%

## Architecture

### Why rusticle is fast

1. **SIMD resize** - fast_image_resize with AVX2/NEON
2. **Parallel frames** - rayon for multi-core processing
3. **Fast path encoding** - skip quantization, reuse original palette
4. **32KB palette LUT** - O(1) nearest-neighbor color mapping
5. **jemalloc** - optimized allocation patterns

### When fast path activates

- GIF has global palette (captured during decode)
- Quality metrics pass thresholds
- Automatic fallback to imagequant if quality insufficient

### Quantization Strategy

- **Fast path**: Palette LUT nearest-neighbor (O(1) per pixel)
- **Fallback**: imagequant (same engine as pngquant/gifski)
- **Dithering**: Floyd-Steinberg via imagequant

## Reproducing Benchmarks

```bash
# Download test suite
python3 scripts/download_test_gifs.py

# Build CLI release binary
cargo build --release -p rusticle-cli

# Resize benchmark
./target/release/rusticle resize test_gifs/benchmark_suite/photo_01.gif /tmp/out.gif
time gifsicle --resize 320x240 test_gifs/benchmark_suite/photo_01.gif -o /tmp/g.gif

# Run full regression benchmark suite
cargo run --release -p rusticle-bench
```

## Stable Regression Workflow

Use a curated baseline in git and keep raw run logs out of git.

```bash
# Run side-by-side benchmark suite (rusticle vs gifsicle)
cargo run --release -p rusticle-bench

# Validate latest run against curated baseline tolerances
python3 scripts/check_benchmark_baseline.py
```

Files:
- Baseline: `docs/bench_baseline.json`
- Raw run log: `outputs/bench_results.jsonl` (ignored by git)

## Summary

| Scenario | Speedup vs gifsicle |
|----------|---------------------|
| Resize (fast path) | **4.6-4.9x** |
| Resize (fallback) | **3.5-3.8x** |
| Full pipeline | **4.7-6.2x** |

Plus: output files are often **smaller** than gifsicle.

## Quality Metrics Comparison

Comparing resize quality using PSNR (Peak Signal-to-Noise Ratio) and SSIM (Structural Similarity Index).

### Methodology

1. Resize original to 320x240 as reference
2. Compare reference to tool's output
3. Higher PSNR/SSIM = closer to reference resize

### Results

| Test File | Tool | Avg PSNR | Avg SSIM | Output Size | Verdict |
|-----------|------|----------|----------|-------------|---------|
| cartoon_01 (fast path) | rusticle | 31.9 dB | 0.983 | 1.16 MB | GOOD |
| cartoon_01 | gifsicle | **40.5 dB** | **0.999** | 1.35 MB | EXCELLENT |
| photo_01 (fast path) | rusticle | 36.9 dB | 0.998 | 1.88 MB | GOOD |
| photo_01 | gifsicle | **38.8 dB** | **0.998** | 2.46 MB | GOOD |
| test3 (imagequant) | rusticle | 34.7 dB | 0.996 | 8.92 MB | GOOD |
| test3 | gifsicle | **37.2 dB** | **0.997** | 8.38 MB | GOOD |

### Interpretation

**Quality thresholds:**
- EXCELLENT: PSNR ≥ 40 dB, SSIM ≥ 0.95
- GOOD: PSNR ≥ 30 dB, SSIM ≥ 0.90
- ACCEPTABLE: PSNR ≥ 25 dB, SSIM ≥ 0.80

**Key findings:**
- gifsicle produces ~2-8 dB higher PSNR
- Both tools achieve "GOOD" quality on all tests
- rusticle's fast path trades ~5 dB PSNR for **4.9x speed**
- rusticle often produces **smaller files** despite lower PSNR

### The Tradeoff

| Metric | rusticle (fast path) | gifsicle |
|--------|---------------------|----------|
| Speed | **4.9x faster** | baseline |
| PSNR | ~5 dB lower | higher |
| File size | often smaller | larger |
| Quality rating | GOOD | EXCELLENT |

For most use cases (web thumbnails, previews, caching), GOOD quality at 4.9x speed is the right tradeoff.
