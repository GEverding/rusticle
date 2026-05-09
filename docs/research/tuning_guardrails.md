# Butteraugli Tuning: Candidate Selection Policy & Guardrails

**Status**: Policy established from EXP-001 through EXP-005 (2026-04-18)  
**Decision**: **Global-only profile recommended at this stage** (hybrid improvement insufficient)

---

## 1. Decision Framework

### Rule: Global vs Hybrid Selection

```
hybrid_improvement = global_median_BA - hybrid_median_BA

if hybrid_improvement < 0.5:
  → Recommend global-only (simpler, lower maintenance)
else if hybrid_improvement >= 0.5:
  → Recommend hybrid with category dispatch
```

### Current Data Outcome

**Global winner** (EXP-002):
- Config: `f=lanczos3|opt=o1|lossy=100`
- Validate median BA: **1.307** (across 7 files)
- Per-category validate BA:
  - cartoon: 1.35
  - large: 1.62
  - many_frames: 1.49
  - photographic: 1.32
  - pixel_art: 1.22
  - simple: 0.87
  - transparent: 1.28

**Hybrid candidates** (best per-category from EXP-003/004/005):
- cartoon: `f=mitchell|opt=o1|lossy=60` → validate BA **6.04**
- large: `f=mitchell|opt=o3|lossy=70` → validate BA **8.59**
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

**Decision**: `hybrid_improvement < 0.5` → **Recommend global-only**

**Rationale**:
- Hybrid approach does **not** improve median BA; in fact, it degrades by 0.183 BA points.
- Cartoon and large category-specific configs have **higher** validate BA (6.04, 8.59) than global (1.35, 1.62).
- This indicates overfitting to train split or category-specific pathologies not generalizable to validate.
- Complexity cost (per-category branching, config management) not justified by quality gain.

---

## 2. Guardrails

### Hard Guardrails (Must Pass)

1. **Size Ratio**: `avg_size_ratio <= 1.1` vs input-size proxy
   - **Rationale**: Prevent output bloat; acceptable 10% overhead for quality gain.
   - **Caveat** (measurement): Current runs use input file size as proxy. For upscaling-heavy subsets (e.g., small GIFs resized to large), this proxy can be invalid. **Fair evaluation requires**:
     - Measure size ratio relative to default config (`f=lanczos3|opt=o3|lossy=80`) on same input, not input-size proxy.
     - Validate size guardrail on full corpus against gifsicle baseline in later stage (e.g., before production deployment).
   - **Current status**: All global top-5 candidates **failed** this guardrail (size ratios 1.67–3.47).

2. **Per-Category Butteraugli**: `avg_ba <= 8.0` per category
   - **Rationale**: Prevent pathological quality degradation in any single category.
   - **Current status**: All global top-5 candidates **passed** (max per-category BA 1.62).

3. **Runtime Proxy** (train-time only): `runtime_improvement_vs_default >= 1.0x`
   - **Rationale**: Ensure candidate is not slower than default config.
   - **Note**: This is a **proxy** because coarse sweep does not include gifsicle timing. Final validation must compare against gifsicle wall-clock time.
   - **Current status**: All global top-5 candidates **failed** (proxy ratios 0.81–0.86x, i.e., slower than default).

### Soft Guardrails (Preferred)

1. **Overfitting**: `train_vs_validate avg_ba_delta <= 1.0`
   - **Rationale**: Detect configs that overfit to train split.
   - **Current status**: All global top-5 candidates **passed** (deltas -0.045 to +0.142).

2. **Speedup vs Gifsicle**: `speedup >= 5.0x` (final validation only)
   - **Rationale**: Maintain performance advantage over gifsicle.
   - **Current status**: Not yet measured in coarse sweep; requires gifsicle baseline.

---

## 3. Current-Data Policy Outcome

### Global-Only Recommendation

**Selected config**: `f=lanczos3|opt=o1|lossy=100`

**Validate performance**:
- Avg BA: 1.307 (best among top-5)
- P95 BA: 1.62
- Worst BA: 2.77
- Runtime: 335.9 ms (slower than default by ~9%)
- Size ratio: 3.47 (fails hard guardrail)

**Why this config despite guardrail failures**:
- No config in top-5 passed all hard guardrails (size ratio <= 1.1).
- Fallback selection used lowest validate avg BA with tie-breakers (p95 BA, then runtime).
- This config has best validate BA (1.3071) and no overfitting signal.

**Category-specific validate BA**:
- Cartoon: 1.35 (good)
- Large: 1.62 (acceptable, but higher than other categories)
- Many_frames: 1.49 (good)
- Photographic: 1.32 (good)
- Pixel_art: 1.22 (excellent)
- Simple: 0.87 (excellent)
- Transparent: 1.28 (good, but category-specific sweep found no better candidate)

**Validation evidence**:
- Cartoon category sweep (EXP-003): 20/60 configs passed guardrails; best validate BA was 6.04 (mitchell/o1/60), **worse than global 1.35**.
- Large category sweep (EXP-004): 5/60 configs passed guardrails; best validate BA was 8.59 (mitchell/o3/70), **worse than global 1.62**.
- Transparent category sweep (EXP-005): 0/60 configs passed guardrails; no candidate available.

**Conclusion**: Category-specific tuning does not improve validate quality; global config is simpler and equally effective.

---

## 4. Hybrid Dispatch Rules (If/When Enabled)

**Not recommended at this stage**, but if future data shows `hybrid_improvement >= 0.5`, use:

### Category Overrides

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

### Fallback Rules

1. **Categories with < 3 validate samples**: Use global config.
   - Rationale: Insufficient data for reliable category-specific tuning.
   - Current corpus: all categories have >= 1 validate sample; cartoon, large, transparent have exactly 1.

2. **Categories with no guardrail-passing candidate**: Use global config.
   - Current: transparent (0/60 passed).

3. **Validate BA worse than global**: Use global config.
   - Current: cartoon (6.04 > 1.35), large (8.59 > 1.62).

---

## 5. Decision Tree

```
┌─ Input GIF
│
├─ Detect category (from metadata or heuristics)
│
├─ [GLOBAL-ONLY STAGE] Use f=lanczos3|opt=o1|lossy=100
│  └─ Validate BA: 1.307 (median across 7 files)
│
└─ [FUTURE: Hybrid dispatch if hybrid_improvement >= 0.5]
   ├─ if category in {cartoon, large, ...}:
   │  └─ Use category-specific config
   └─ else:
      └─ Use global config
```

---

## 6. Measurement Caveats & Future Work

### Size Ratio Guardrail Caveat

**Issue**: Current runs measure `avg_size_ratio` as output size / input size. For upscaling-heavy subsets (e.g., small GIFs resized to large dimensions), this proxy is invalid because:
- Input size is not a fair baseline for upscaled output.
- Comparison should be relative to default config on the same input.

**Fair evaluation**:
1. Measure size ratio as: `output_size(candidate) / output_size(default)` on same input.
2. Validate size guardrail on full corpus (24 files) against gifsicle baseline in later stage.
3. Document size ratio per category to detect category-specific bloat.

### Runtime Proxy Caveat

**Issue**: Coarse sweep uses runtime ratio vs default config (proxy), not actual gifsicle speedup.

**Fair evaluation**:
1. Measure wall-clock time against gifsicle on same corpus.
2. Validate speedup >= 5.0x before production deployment.

### Transparent Category Pathology

**Issue**: No config passed runtime guardrail for transparent category (0/60).

**Investigation needed**:
1. Isolate alpha handling in optimize/quantize pipeline.
2. Test alpha-preserving quantization strategies.
3. Measure BA improvement vs size/speed tradeoff.

---

## 7. Next Steps

1. **Validate size guardrail fairly**: Re-measure size ratio relative to default config, not input size.
2. **Measure gifsicle speedup**: Compare wall-clock time against gifsicle baseline on full corpus.
3. **Investigate transparent pathology**: Test alpha-preserving quantization; measure BA vs size/speed.
4. **Implement global config in CLI**: Deploy `f=lanczos3|opt=o1|lossy=100` as default.
5. **Monitor category-specific BA**: Track per-category BA in production; flag if any category degrades > 1.0 BA.
6. **Revisit hybrid dispatch**: If future experiments show `hybrid_improvement >= 0.5`, implement category-aware branching.

---

**Last Updated**: 2026-04-18  
**Experiment Artifacts**: EXP-001 through EXP-005 (see `research/BUTTERAUGLI_TUNING_JOURNAL.md`)
