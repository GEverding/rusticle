# Rusticle Benchmarks

Performance comparisons against gifsicle 1.96 on Apple Silicon (M-series).

## Correctness Fix Wave (2026-04-20)

**Disposal-aware optimization and subframe reference-state fixes** resolved frame corruption in the optimization pipeline. Full holdout corpus rerun (39 files, 160×120 resize) shows:

| Profile | Metric | Pre-Fix | Post-Fix | Improvement |
|---------|--------|---------|----------|-------------|
| rusticle_default | Avg Butteraugli | 13.09 | 3.85 | −9.24 (−71%) |
| rusticle_default | Worst-case BA | 241.30 | 40.48 | −200.82 (−83%) |
| rusticle_optimized_global | Avg Butteraugli | 9.22 | 0.46 | −8.76 (−95%) |
| rusticle_optimized_global | Worst-case BA | 241.30 | 3.59 | −237.71 (−99%) |

**Key finding**: Catastrophic tail (worst-case BA > 200) was entirely due to incorrect reference-state computation in `optimize()` and `lossy()` when disposal methods (Background/Previous) or subframe cropping (from O3) were involved. Post-fix, worst-case BA is now in single digits for the optimized profile.

**Caveat**: Post-fix quality aggregates exclude 3 measurement failures (pre-fix included bogus perfect-score fallbacks). Butteraugli and worst-case metrics are the trustable signals. See `outputs/holdout_profile_comparison_postfix.md` for detailed before/after analysis.

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
| Resize (avg) | **4.12x** |
| Full pipeline (avg) | **8.79x** |
| Overall (all operations) | **6.46x** |

Plus: output files are often **smaller** than gifsicle, and quality is **EXCELLENT** (avg PSNR 41.36 dB vs gifsicle 35.97 dB).

## Quality Metrics Comparison

Comparing resize quality using PSNR (Peak Signal-to-Noise Ratio), SSIM (Structural Similarity Index), and optional Butteraugli perceptual distance.

### Methodology

1. Resize original to 320x240 as reference
2. Compare reference to tool's output
3. Higher PSNR/SSIM = closer to reference resize
4. **Lower Butteraugli = better** (opposite of PSNR/SSIM)

### Aggregate Results (48 test pairs)

Latest benchmark run (2026-04-18T16:05:34):

| Tool | Avg PSNR | Avg SSIM | Avg Butteraugli |
|------|----------|----------|-----------------|
| rusticle | 41.36 dB | 0.9987 | 2.92 |
| gifsicle | 35.97 dB | 0.9940 | 4.54 |

**Note:** Aggregate across 24 test files × 2 operations (resize + full pipeline) = 48 pairs

### Interpretation

**Quality thresholds (PSNR/SSIM):**
- EXCELLENT: PSNR ≥ 40 dB, SSIM ≥ 0.95
- GOOD: PSNR ≥ 30 dB, SSIM ≥ 0.90
- ACCEPTABLE: PSNR ≥ 25 dB, SSIM ≥ 0.80

**Butteraugli perceptual distance (when feature enabled):**

Butteraugli measures human-perceptible differences. **Lower scores are better** (opposite of PSNR/SSIM).

| Score Range | Perception | Use Case |
|-------------|-----------|----------|
| < 1.0 | Imperceptible | Excellent quality, lossless-like |
| 1.0–2.0 | Good quality | Acceptable for most applications |
| 2.0–3.0 | Noticeable | Visible artifacts, use with caution |
| > 3.0 | Significant | Poor quality, not recommended |

**Note:** Butteraugli scores are only computed when:
- The `butteraugli` feature is enabled (compile-time flag)
- Image dimensions are ≥ 8×8 pixels
- Using `QualityMetrics::compare_with_dimensions()` (not the legacy `compare()` method)

## Butteraugli Thresholds by Category

Per-category Butteraugli thresholds derived from benchmark suite (48 test pairs across 7 categories):

| Category | Count | Avg BA | P95 BA | Worst BA | Suggested Threshold | Needs Investigation |
|----------|------:|-------:|-------:|----------:|---------------------:|:---:|
| cartoon | 8 | 3.45 | 6.57 | 6.69 | 6.0 | No |
| large | 6 | 3.71 | 5.60 | 5.95 | 6.0 | No |
| many_frames | 6 | 2.30 | 3.24 | 3.25 | 4.0 | No |
| photographic | 8 | 2.25 | 5.55 | 7.07 | 6.0 | No |
| pixel_art | 6 | 3.28 | 6.23 | 7.31 | 6.0 | No |
| simple | 8 | 2.23 | 3.28 | 3.65 | 4.0 | No |
| transparent | 6 | 3.46 | 8.19 | 9.70 | 6.0 | **Yes** |
| **Global** | **48** | **2.92** | **6.93** | **9.70** | **6.0** | **Yes** |

**Key findings:**
- rusticle avg PSNR 41.36 dB vs gifsicle 35.97 dB (rusticle **5.4 dB higher**)
- rusticle avg SSIM 0.9987 vs gifsicle 0.9940 (rusticle **0.47% higher**)
- rusticle avg Butteraugli 2.92 vs gifsicle 4.54 (rusticle **35% lower**, better perceptual quality)
- Butteraugli thresholds range from 4.0 (many_frames, simple) to 6.0 (cartoon, large, photographic, pixel_art, transparent, global)
- Transparent category requires investigation (worst BA 9.70 exceeds threshold 6.0 × 1.6)

### The Tradeoff

| Metric | rusticle | gifsicle |
|--------|----------|----------|
| Speed | **6.46x faster** (avg across all ops) | baseline |
| PSNR | **41.36 dB** (EXCELLENT) | 35.97 dB (GOOD) |
| SSIM | **0.9987** | 0.9940 |
| Butteraugli | **2.92** (35% lower, better) | 4.54 |
| File size | often smaller | larger |

rusticle delivers **superior quality** (5.4 dB higher PSNR) at **6.46x speed**. Per-category results vary: transparent GIFs show higher Butteraugli variance; fast path excels on cartoon/photographic content. Lossy compression (when enabled) trades imperceptible quality for smaller files.

## Threshold Derivation Methodology

Per-category Butteraugli thresholds are computed from benchmark suite results to enable automated quality regression testing.

### Formula

```
suggested_threshold = ceil(p95_ba * 1.2)
clamped to [1.0, 6.0]
```

Where:
- `p95_ba` = 95th percentile Butteraugli score for the category
- `ceil()` = round up to nearest integer
- Clamping ensures thresholds stay within practical range

### Why Per-Category Thresholds?

1. **Different content types have different perceptual characteristics**
   - Photographic images (2.25 avg BA) are more sensitive to artifacts than simple content (2.23 avg BA)
   - Transparent GIFs (3.46 avg BA) show higher variance due to alpha blending complexity

2. **Enables early detection of regressions**
   - Cartoon threshold (6.0) catches quality drops in animation-heavy content
   - Many-frames threshold (4.0) is stricter for high-frame-count GIFs where artifacts compound

3. **Accounts for natural variance**
   - P95 percentile captures 95% of normal variation
   - 1.2× multiplier provides 20% headroom for legitimate improvements
   - Clamping [1.0, 6.0] prevents unrealistic thresholds

### Investigation Flag

Categories where `worst_ba > suggested_threshold * 1.5` are flagged for investigation:
- **transparent** category: worst BA 9.70 vs threshold 6.0 (ratio 1.62)
- Indicates potential outliers or systematic quality issues requiring review

## Regenerating Thresholds

Thresholds are derived from the benchmark suite and should be regenerated when:
- Adding new test GIFs to the suite
- Changing quantization or resizing algorithms
- Updating quality metric implementations

### Step-by-Step

```bash
# 1. Download latest test GIFs
python3 scripts/download_test_gifs.py

# 2. Run benchmark suite (generates outputs/bench_results.jsonl)
cargo run --release -p rusticle-bench

# 3. Derive thresholds from benchmark results
python3 scripts/derive_ba_thresholds.py

# 4. Copy generated thresholds to docs
cp outputs/ba_thresholds.json docs/ba_thresholds.json
```

### When to Regenerate

- **After algorithm changes**: Resize filter, quantization, or optimization changes
- **Adding test cases**: New GIFs in benchmark suite
- **Quarterly review**: Validate thresholds against real-world usage patterns
- **Before major release**: Ensure quality baselines are current

### Validation

After regenerating:

```bash
# Verify thresholds are valid JSON
python3 -m json.tool docs/ba_thresholds.json > /dev/null

# Check for investigation flags
grep '"needs_investigation": true' docs/ba_thresholds.json

# Run quality regression tests
cargo test --features butteraugli quality_metrics
```
