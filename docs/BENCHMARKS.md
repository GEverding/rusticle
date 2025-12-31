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

# Build release
cargo build --release

# Resize benchmark (fast path)
./target/release/rusticle resize test_gifs/benchmark_suite/photo_01.gif /tmp/out.gif
time gifsicle --resize 320x240 test_gifs/benchmark_suite/photo_01.gif -o /tmp/g.gif

# Full pipeline benchmark  
./target/release/rusticle all test_gifs/test3.gif /tmp/out.gif
time gifsicle --resize 320x240 -O3 --lossy=80 test_gifs/test3.gif -o /tmp/g.gif

# Compare sizes
ls -lh /tmp/out.gif /tmp/g.gif
```

## Summary

| Scenario | Speedup vs gifsicle |
|----------|---------------------|
| Resize (fast path) | **4.6-4.9x** |
| Resize (fallback) | **3.5-3.8x** |
| Full pipeline | **4.7-6.2x** |

Plus: output files are often **smaller** than gifsicle.
