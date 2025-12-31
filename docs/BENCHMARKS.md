# Rusticle Benchmarks

Performance comparisons against gifsicle 1.96 on Apple Silicon (M-series).

## Test Environment

- macOS (darwin)
- Rust 1.x with `--release` profile
- jemalloc allocator
- SIMD via fast_image_resize

## Resize Performance

### Full Pipeline: resize 320x240 + optimize O3 + lossy 80

| Test File | Frames | rusticle | gifsicle | Speedup | Output Size |
|-----------|--------|----------|----------|---------|-------------|
| test3.gif (9MB) | 197 | 2.1s | 9.9s | **4.7x** | 5.9MB vs 6.5MB |
| test2.gif (34MB) | 367 | 3.7s | 22.9s | **6.2x** | 6.0MB vs 5.1MB |

### Resize-only with Fast Path (palette reuse)

| Test File | Frames | Encode Time | Notes |
|-----------|--------|-------------|-------|
| cartoon_01.gif | 24 | 46ms | Global palette, fast path |
| cartoon_02.gif | 12 | 10ms | Global palette, fast path |
| photo_01.gif | 56 | 88ms | Global palette, fast path |
| photo_02.gif | 43 | 73ms | Global palette, fast path |
| pixel_art_01.gif | 31 | 456ms | Local palettes, imagequant fallback |
| small_simple_02.gif | 111 | 1.6s | Local palettes, imagequant fallback |

### Fast Path Optimization Impact

Before (LUT per frame):
- Encode: 223ms
- Total: 278ms

After (LUT once per GIF):
- Encode: 46ms (**4.8x faster**)
- Total: 100ms (**2.8x faster**)

## Compression Quality

### Output Size Comparison

| Operation | rusticle | gifsicle | Winner |
|-----------|----------|----------|--------|
| resize 320x240 | 7.8MB | 8.0MB | rusticle |
| optimize O3 | 8.3MB | 9.0MB | rusticle |
| full pipeline | 5.9MB | 6.5MB | rusticle |

### Quality Metrics

Fast path quality thresholds (triggers fallback if exceeded):
- Average distance²: < 150
- Outlier ratio: < 5%
- Palette utilization: > 30%

Typical fast path stats on cartoon GIFs:
- Average distance²: ~29
- Outlier ratio: 0%
- Palette utilization: 88%

## Architecture

### Why rusticle is fast

1. **SIMD resize** via fast_image_resize (AVX2/NEON)
2. **Parallel frame processing** via rayon
3. **Fast path encoding** - skip quantization, reuse original palette
4. **32KB palette LUT** - O(1) nearest-neighbor color mapping
5. **jemalloc** - better allocation patterns for image processing

### When fast path is used

- GIF has global palette (stored during decode)
- Quality metrics pass thresholds
- Automatic fallback to imagequant if quality too low

### Quantization

- Primary: imagequant (same as pngquant/gifski)
- Fast path: palette LUT nearest-neighbor
- Dithering: Floyd-Steinberg via imagequant

## Reproducing Benchmarks

```bash
# Download test suite
python3 scripts/download_test_gifs.py

# Build release
cargo build --release

# Run benchmarks
./target/release/rusticle all test_gifs/test3.gif /tmp/out.gif
time gifsicle --resize 320x240 -O3 --lossy=80 test_gifs/test3.gif -o /tmp/gifsicle.gif

# Compare sizes
ls -lh /tmp/out.gif /tmp/gifsicle.gif
```

## Version History

- **v0.1.0**: Initial release
  - 3-6x faster than gifsicle
  - Competitive compression (sometimes smaller)
  - Fast path with palette reuse (4.8x encode speedup)
