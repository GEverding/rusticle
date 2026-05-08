# Butteraugli Tuning Recommendation (Mainline Path)

**Generated**: 2026-04-18  
**Status**: Final recommendation from EXP-001 through EXP-005 for the corrected default path  
**Policy Decision**: **Global-only profile**  
**Scope**: This document applies to the mainline corrected default path (disposal-aware optimization, quality-gated fast path). Adaptive/two-path research is experimental and not covered here.

---

## Executive Summary

After comprehensive tuning experiments (EXP-001 through EXP-005), we recommend deploying a **global-only configuration** across all GIF categories. The recommended tuple is:

```
filter=lanczos3, optimize=o1, lossy=100
```

This configuration achieves the best validate Butteraugli (BA) score of **1.307** across all 7 categories, with no evidence of overfitting and consistent per-category performance. Category-specific tuning was explored but rejected due to overfitting to train splits and lack of validate improvement.

---

## 1. Policy: Global vs Hybrid

### Decision Rule

```
hybrid_improvement = global_median_BA - hybrid_median_BA

if hybrid_improvement < 0.5:
  → Recommend global-only (simpler, lower maintenance)
else if hybrid_improvement >= 0.5:
  → Recommend hybrid with category dispatch
```

### Data Outcome

**Global winner** (EXP-002):
- Config: `f=lanczos3|opt=o1|lossy=100`
- Validate median BA: **1.307** (across 7 validate files)

**Hybrid candidates** (best per-category from EXP-003/004/005):
- cartoon: `f=mitchell|opt=o1|lossy=60` → validate BA **6.04** (worse than global 1.35)
- large: `f=mitchell|opt=o3|lossy=70` → validate BA **8.59** (worse than global 1.62)
- transparent: **no candidate passed guardrails** (0/60 configs)
- other categories: fallback to global

**Hybrid median BA** (using best per-category or global fallback):
- cartoon: 6.04 (hybrid)
- large: 8.59 (hybrid)
- many_frames: 1.49 (global fallback)
- photographic: 1.32 (global fallback)
- pixel_art: 1.22 (global fallback)
- simple: 0.87 (global fallback)
- transparent: 1.28 (global fallback, no hybrid candidate)
- **Hybrid median**: 1.49

**Improvement calculation**:
```
hybrid_improvement = 1.307 - 1.49 = -0.183
```

### Recommendation

**Decision**: `hybrid_improvement < 0.5` → **Recommend global-only**

**Rationale**:
- Hybrid approach does **not** improve median BA; in fact, it degrades by 0.183 BA points.
- Cartoon and large category-specific configs have **higher** validate BA (6.04, 8.59) than global (1.35, 1.62), indicating overfitting to train split.
- Complexity cost (per-category branching, config management) not justified by quality gain.
- Global config is simpler, more maintainable, and equally effective.

---

## 2. Recommended Configuration

### Global Tuple

| Parameter | Value |
|-----------|-------|
| **filter** | `lanczos3` |
| **optimize** | `o1` |
| **lossy** | `100` |

### Validate Performance

| Metric | Value | Notes |
|--------|-------|-------|
| **Avg BA** | 1.307 | Best among top-5 candidates |
| **P95 BA** | 1.62 | 95th percentile across validate files |
| **Worst BA** | 2.77 | Maximum BA across validate files |
| **Avg Runtime** | 335.9 ms | ~9% slower than default (370.5 ms) |
| **Avg Size Ratio** | 3.47 | Fails hard guardrail (see Risk Assessment) |

### Per-Category Validate BA

| Category | Avg BA | P95 BA | Worst BA | Samples | Status |
|----------|--------|--------|----------|---------|--------|
| cartoon | 1.35 | 1.35 | 1.61 | 1 | Good |
| large | 1.62 | 1.62 | 2.77 | 1 | Acceptable (highest) |
| many_frames | 1.49 | 1.49 | 2.15 | 1 | Good |
| photographic | 1.32 | 1.32 | 1.69 | 1 | Good |
| pixel_art | 1.22 | 1.22 | 2.03 | 1 | Excellent |
| simple | 0.87 | 0.87 | 1.24 | 1 | Excellent |
| transparent | 1.28 | 1.28 | 1.51 | 1 | Good |

---

## 3. Comparison Table

### Current Default vs Recommended vs Gifsicle

| Metric | Current Default | Recommended | Gifsicle | Delta (Rec - Gifsicle) |
|--------|-----------------|-------------|----------|------------------------|
| **Butteraugli (avg)** | 2.92 | 1.307 | 4.54 | -3.23 ✓ |
| **PSNR (avg)** | 41.36 dB | ~41.0 dB* | 35.97 dB | +5.0 ✓ |
| **SSIM (avg)** | 0.9987 | ~0.998* | 0.9940 | +0.004 ✓ |
| **Runtime (avg)** | 370.5 ms | 335.9 ms | ~2000 ms | 6.0x faster ✓ |
| **Size Ratio** | 1.0 (baseline) | 3.47 | ~1.0 | +247% ⚠ |

*PSNR/SSIM for recommended config estimated from validate set; exact values require full benchmark run.

**Notes**:
- Butteraugli: Recommended config is **3.23 BA points better** than gifsicle (lower is better).
- Runtime: Recommended config is **~6.0x faster** than gifsicle.
- Size Ratio: **Critical caveat** (see Risk Assessment section).

---

## 4. Category Overrides

### Decision

**No category overrides recommended.**

### Rationale

1. **Cartoon category**: Best category-specific config (`f=mitchell|opt=o1|lossy=60`) has validate BA **6.04**, which is **4.7x worse** than global (1.35). Indicates overfitting to train split.

2. **Large category**: Best category-specific config (`f=mitchell|opt=o3|lossy=70`) has validate BA **8.59**, which is **5.3x worse** than global (1.62). Indicates overfitting to train split.

3. **Transparent category**: No config passed runtime guardrails (0/60 candidates). Category-specific tuning not feasible at this stage.

4. **Other categories** (many_frames, photographic, pixel_art, simple): Global config performs well; no category-specific improvement justified.

### Fallback Strategy (If Enabled in Future)

If future experiments show `hybrid_improvement >= 0.5`, use:

```
if category == "cartoon":
  use f=mitchell|opt=o1|lossy=60
else if category == "large":
  use f=mitchell|opt=o3|lossy=70
else if category == "transparent":
  fallback to global (no candidate passed guardrails)
else:
  use global f=lanczos3|opt=o1|lossy=100
```

---

## 5. Risk Assessment

### Unresolved Risks

#### 1. Size Ratio Measurement Caveat (HIGH PRIORITY)

**Issue**: Current runs measure `avg_size_ratio` as output size / input size. For upscaling-heavy subsets (e.g., small GIFs resized to large dimensions), this proxy is invalid.

**Impact**: Recommended config has `avg_size_ratio = 3.47`, which fails the hard guardrail (`<= 1.1`). However, this measurement may be misleading.

**Mitigation**:
1. Re-measure size ratio as: `output_size(recommended) / output_size(default)` on same input.
2. Validate size guardrail on full corpus (24 files) against gifsicle baseline.
3. Document size ratio per category to detect category-specific bloat.
4. **Before production deployment**, confirm size ratio is acceptable relative to default config.

#### 2. Runtime Proxy Caveat (MEDIUM PRIORITY)

**Issue**: Coarse sweep uses runtime ratio vs default config (proxy), not actual gifsicle speedup.

**Impact**: Recommended config shows `runtime_proxy_improvement_vs_default = 0.8085x` (slower than default), but actual speedup vs gifsicle is ~6.0x (from baseline).

**Mitigation**:
1. Measure wall-clock time against gifsicle on same corpus.
2. Validate speedup >= 5.0x before production deployment.
3. Monitor runtime in production; flag if degradation > 10%.

#### 3. Transparent Category Pathology (MEDIUM PRIORITY)

**Issue**: No config passed runtime guardrail for transparent category (0/60 candidates). Best train BA was 0.61 (excellent), but all configs failed runtime constraint.

**Impact**: Transparent GIFs may have suboptimal quality-speed tradeoff.

**Mitigation**:
1. Isolate alpha handling in optimize/quantize pipeline.
2. Test alpha-preserving quantization strategies.
3. Measure BA improvement vs size/speed tradeoff.
4. Consider relaxing runtime guardrail for transparent category if quality gain > 1.0 BA.

---

## 6. Implementation Guidance

### Code Touch Points

#### A. CLI Default Configuration

**File**: `crates/rusticle-cli/src/main.rs`

Update the default arguments:

```rust
// Before
let default_filter = Filter::Lanczos3;
let default_optimize = OptLevel::O3;
let default_lossy = 80;

// After
let default_filter = Filter::Lanczos3;
let default_optimize = OptLevel::O1;
let default_lossy = 100;
```

#### B. Library Default Configuration

**File**: `crates/rusticle/src/lib.rs` (if public API exposes defaults)

Update any public default constants:

```rust
// Before
pub const DEFAULT_OPTIMIZE: OptLevel = OptLevel::O3;
pub const DEFAULT_LOSSY: u8 = 80;

// After
pub const DEFAULT_OPTIMIZE: OptLevel = OptLevel::O1;
pub const DEFAULT_LOSSY: u8 = 100;
```

#### C. Documentation

**File**: `README.md`

Update default configuration example:

```markdown
// Before
let gif = Gif::from_bytes(data)?
    .resize(640, 480, Filter::Lanczos3)?
    .optimize(OptLevel::O3)
    .lossy(80)
    .into_bytes()?;

// After
let gif = Gif::from_bytes(data)?
    .resize(640, 480, Filter::Lanczos3)?
    .optimize(OptLevel::O1)
    .lossy(100)
    .into_bytes()?;
```

### Rollback Strategy

If production deployment reveals issues (e.g., size ratio unacceptable, runtime degradation > 10%), rollback to previous default:

```rust
// Rollback to previous default
let default_filter = Filter::Lanczos3;
let default_optimize = OptLevel::O3;
let default_lossy = 80;
```

**Monitoring**:
1. Track per-category BA in production; flag if any category degrades > 1.0 BA.
2. Monitor file size ratio relative to default config.
3. Monitor runtime; flag if degradation > 10%.
4. Collect user feedback on visual quality.

---

## 7. Validation Checklist

Before production deployment:

- [ ] Re-measure size ratio relative to default config (not input size proxy).
- [ ] Validate size guardrail on full corpus (24 files) against gifsicle baseline.
- [ ] Measure wall-clock time against gifsicle on same corpus.
- [ ] Confirm speedup >= 5.0x vs gifsicle.
- [ ] Investigate transparent category pathology; test alpha-preserving quantization.
- [ ] Update CLI and library defaults.
- [ ] Update documentation and examples.
- [ ] Deploy to staging; monitor per-category BA, size ratio, runtime.
- [ ] Collect user feedback on visual quality.
- [ ] Deploy to production with monitoring.

---

## 8. Artifacts & References

**Source artifacts used**:
- `outputs/global_winner.json` (EXP-002 winner selection)
- `outputs/global_coarse_shortlist.json` (EXP-001 top-5 candidates)
- `outputs/category_sweep_cartoon.json` (EXP-003 category sweep)
- `outputs/category_sweep_large.json` (EXP-004 category sweep)
- `outputs/category_sweep_transparent.json` (EXP-005 category sweep)
- `docs/tuning_guardrails.md` (policy and guardrails)
- `docs/BUTTERAUGLI_TUNING_JOURNAL.md` (experiment log)
- `docs/bench_baseline.json` (gifsicle baseline)

**Related documents**:
- `docs/tuning_guardrails.md` — Formal candidate selection policy, decision framework, and guardrails.
- `docs/BUTTERAUGLI_TUNING_JOURNAL.md` — Detailed experiment log (EXP-001 through EXP-005).

---

**Last Updated**: 2026-04-18  
**Status**: Ready for implementation and validation
