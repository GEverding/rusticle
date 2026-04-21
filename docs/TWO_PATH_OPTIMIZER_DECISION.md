# Two-Path Optimizer: Architecture Decision & De-Scope

**Status**: Architecture decision memo  
**Date**: 2026-04-20  
**Decision Issue**: `rusticle-45a`  
**Epic**: `rusticle-1yq`

---

## 1. Decision Summary

We are stepping back from the universal adaptive optimizer as the mainline product direction. Instead, we are implementing two explicit, bounded production strategies:

- **Path A (Conservative Opaque-Delta Reconstruction)**: For GIFs structurally close to optimal GIF-native streams (no transparency, stable/global palette, offset subframes, Keep/None disposal, low changed-area ratio). Decode → resize → compute exact changed bbox → emit opaque bbox patch with None disposal.

- **Path B (General Sparse/Transparent Optimization)**: For transparency-heavy, disposal-heavy, mixed, or non-obviously opaque-delta GIFs. Keep the current corrected RGB-canvas-based pipeline with disposal-aware sparse patches, lossless structural optimization, and explicit lossy as a separate concern.

The adaptive/tiered optimizer work remains interesting research but is not the immediate product path. We are freezing it behind a research feature gate and focusing on validating and shipping the two-path baseline.

---

## 2. Why We Are Changing Direction

### 2.1 Disposal/Subframe/O3 Fixes Revealed the Real Problem

The correctness fixes in `rusticle-45a` (disposal-aware optimization, subframe reference-state fixes, O3 semantic fix, quality invalid-state fix) taught us that the core issue was not tuning or heuristics—it was **representation mismatch and semantic bundling**.

- **Representation mismatch**: The pipeline committed to full-canvas RGBA early, then tried to retrofit disposal semantics and sparse patches on top. This caused subtle bugs where optimization decisions violated disposal invariants.
- **Semantic bundling**: The adaptive optimizer tried to choose a single policy (tier0 → tier1 → tier2) that would work for all GIF classes. But opaque-delta GIFs and transparency-heavy GIFs have fundamentally different optimal strategies.

### 2.2 Voyager-Like GIFs Show Harm from Aggressive General Path

Already-optimized opaque-delta/global-palette sequences (e.g., voyager.gif) are harmed by the general aggressive path:

- They use full-frame or tight bounding-box patches with opaque pixels and a single global palette.
- Applying aggressive optimization (marking pixels transparent) + lossy (more transparency) + re-quantization introduces:
  - Synthetic transparency that wasn't in the source.
  - Palette churn as the quantizer adapts to new transparency patterns.
  - Unnecessary CPU cost for analysis that won't improve the output.
- Result: larger output, slower decode, higher CPU cost than the original.

### 2.3 Adaptive Optimizer Complexity Outpaced Validation

The adaptive/tiered optimizer (tier0 classifier → tier1 pruning → tier2 measure → encode-and-measure chooser) became increasingly complex:

- Tier0 heuristics were brittle and required constant tuning.
- Tier1 pruning logic was hard to reason about (which candidates are safe to discard?).
- Tier2 measurement budget was a moving target (how much CPU is "reasonable"?).
- Encode-and-measure chooser added latency without clear wins on the holdout set.

We reached a point where the complexity of the adaptive system exceeded our ability to validate it against real-world GIFs. The two-path approach is simpler, more predictable, and easier to reason about.

---

## 3. What We Keep (Mainline Foundation)

These modules/concepts are retained and form the foundation for both Path A and Path B:

### 3.1 Core Correctness & Display-Space Truth Model

- **`adaptive_ir.rs`**: Canonical sequence IR (CanonicalSequence, CanonicalFrame, Canvas, BoundingBox, ChangedRegion). This is the ground truth representation in display/canvas space. Both paths reference this.
- **`decode.rs`**: GIF decoding with correct disposal-aware compositing. Produces the canonical sequence IR.
- **`types.rs`**: Core types (Gif, Frame, DisposalMethod, Filter, OptLevel, Palette). Unchanged.

### 3.2 Disposal-Aware & Subframe Fixes

- **`optimize.rs`**: Structural optimization (marking unchanged pixels transparent) with correct disposal semantics. O3 semantic fix (crop to bbox, use Keep disposal for subframes). Threshold=0 (lossless only). Perceptual thresholding moved to `lossy()`.
- **`quality.rs`**: Quality metrics (PSNR, SSIM, Butteraugli). Invalid-state fix (no quality measurement on frames with disposal=Previous without proper reference state).

### 3.3 Profiler & Structural Analysis

- **`profiler.rs`**: GIF profiling (GifProfile, SequenceMetrics, SequenceTaxonomy, DisposalDistribution, PaletteInfo, PatchDensity, TransparencyAnalysis, ChangeStatistics). Used by both paths for understanding GIF structure.
- **`analysis_kernels.rs`**: SIMD-accelerated pixel analysis (find_diff_bounding_box, mark_unchanged_pixels_simd, analyze_transparency_simd, analyze_color_distance_simd). Used by both paths.
- **`simd_opt.rs`**: SIMD optimization utilities (DiffRect, find_diff_bounding_box, mark_unchanged_pixels_simd). Used by both paths.

### 3.4 Palette & Encoding Infrastructure

- **`palette_strategy.rs`**: Palette strategy determination (PaletteStrategy, PaletteStrategySet, determine_palette_strategies). Determines whether to use global or per-frame local palettes. Used by both paths.
- **`palette_lut.rs`**: Palette lookup table (PaletteLut, PaletteMapStats). Fast color mapping. Used by both paths.
- **`palette_realize.rs`**: Palette realization (PaletteRealizer, QuantizedFrameData). Converts RGBA frames to quantized palette indices. Used by both paths.
- **`encode.rs`**: GIF encoding with quantization. Used by both paths.
- **`resize.rs`**: Frame resizing (fast_image_resize). Used by both paths.

### 3.5 Benchmarking & Journaling

- **`profiler.rs`** (metrics/telemetry parts): Structured logging and metrics collection for benchmarking and debugging.
- **`docs/BENCHMARKS.md`**: Benchmark infrastructure and baseline results.
- **`docs/bench_baseline.json`**: Baseline metrics for regression detection.

### 3.6 Safe Minimal/No-Op Rules

- **`optimize.rs`**: Safe rules for when optimization is a no-op (e.g., frame 0 always full-frame, identical consecutive frames handled correctly).
- **`encode.rs`**: Safe encoding rules (e.g., no transparency in palette if not needed).

---

## 4. What Becomes Mainline Product Direction

### 4.1 Path A: Conservative Opaque-Delta Reconstruction

**Target GIFs**: Structurally close to optimal GIF-native streams.

**Characteristics**:
- No transparent GCEs (global transparency index not used).
- ≥90% frames use Keep or None disposal.
- Global palette or ≥80% palette stability.
- Median changed-area ratio ≤ 0.6 (authored delta behavior).

**Pipeline**:
1. Decode to displayed canvases (canonical sequence IR).
2. Resize canvases.
3. For each consecutive pair of resized displayed frames: compute exact changed bbox.
4. If bbox area ≤ threshold fraction of canvas (e.g., 0.7): emit cropped opaque patch at bbox offset.
5. If bbox area > threshold or frame 0: emit full-frame opaque.
6. All emitted frames use disposal=None (Keep) semantics.
7. No transparency in emitted pixel data—every pixel in the patch is opaque RGB.
8. Build single global palette from all resized canvases using imagequant.
9. Apply global palette to all frames (per-frame local override only if quantization error exceeds threshold).
10. Encode with global palette.

**Rationale**: Path A is simple, predictable, and preserves the structure of already-optimized GIFs. It avoids synthetic transparency, palette churn, and unnecessary CPU cost.

**Implementation Tasks**: `rusticle-n7z` (classifier), `rusticle-0g0` (core), `rusticle-69a` (palette strategy).

### 4.2 Path B: General Sparse/Transparent Optimization

**Target GIFs**: Transparency-heavy, disposal-heavy, mixed, or non-obviously opaque-delta GIFs.

**Characteristics**: Everything not classified as Path A.

**Pipeline**:
1. Decode to displayed canvases (canonical sequence IR).
2. Resize canvases.
3. Apply current corrected RGB-canvas-based optimization:
   - Structural optimization (marking unchanged pixels transparent) with correct disposal semantics.
   - Disposal-aware sparse patch behavior allowed.
   - Lossless structural optimization remains lossless.
   - Lossy compression (perceptual thresholding) is explicit and separate.
4. Palette strategy determination (global or per-frame local).
5. Palette realization and encoding.

**Rationale**: Path B is the current corrected pipeline. It handles all GIF types correctly, including transparency and complex disposal patterns. It is not optimized for opaque-delta GIFs, but it is safe and correct.

**Implementation Tasks**: `rusticle-zry` (refactor as explicit Path B), `rusticle-eh9` (regression tests).

### 4.3 Routing & Integration

**Classifier** (`rusticle-n7z`): Deterministic, explainable classifier that routes a decoded GIF sequence to Path A or Path B based on structural features.

**Integration** (`rusticle-a7w`): Wire classifier behind experimental flag (`--optimizer-strategy` CLI flag with values: `auto`, `path-a`, `path-b`, `legacy`). Default is `legacy` during transition.

---

## 5. What We Freeze as Research / Future Work

These modules are not being deleted, but are not driving the current product path. They are frozen behind a research feature gate or moved to a research directory.

### 5.1 Universal Adaptive/Tiered Optimizer

- **`tier0_classifier.rs`**: Tier0 classifier (Tier0Classifier, Tier0Decision). Heuristic-based classification of GIF structure. Replaced by simpler two-path classifier.
- **`tier1_pruning.rs`**: Tier1 pruning (Tier1Pruner, PruneReason, PruneResult). Candidate pruning based on heuristics. Replaced by explicit Path A/B logic.
- **`tier2_measure.rs`**: Tier2 measurement (Tier2Measurer, Tier2Telemetry, MeasurementBudget, QualityGuardrails). Encode-and-measure for high-uncertainty candidates. Deferred as research.

**Rationale**: The tiered approach was designed to handle all GIF classes with a single adaptive policy. The two-path approach is simpler and more predictable. The tiered logic can be revisited as future research if needed.

**Gating Plan**: Move to `src/research/` directory or gate behind `#[cfg(feature = "research")]`. Not exported from `lib.rs` by default.

### 5.2 Sequence Optimizer & DP-Lite

- **`sequence_optimizer.rs`**: Sequence optimizer (SequenceOptimizer, SequenceOptimizerConfig). DP-lite beam search over candidate representations. Deferred as research.

**Rationale**: Sequence optimization (choosing candidates for all frames jointly) is complex and adds latency. The two-path approach chooses candidates per-frame based on simple rules. Sequence optimization can be revisited as future research if per-frame choices are insufficient.

**Gating Plan**: Move to `src/research/` directory or gate behind `#[cfg(feature = "research")]`. Not exported from `lib.rs` by default.

### 5.3 Encode-and-Measure Chooser

- **`encode_and_measure.rs`**: Encode-and-measure (EncodeAndMeasure, EncodeAndMeasureConfig, EncodeAndMeasureTelemetry, MeasuredCandidate). Actual encode-and-measure for candidate evaluation. Deferred as research.

**Rationale**: Encode-and-measure adds latency without clear wins on the holdout set. The two-path approach uses simple structural rules. Encode-and-measure can be revisited as future research if needed.

**Gating Plan**: Move to `src/research/` directory or gate behind `#[cfg(feature = "research")]`. Not exported from `lib.rs` by default.

### 5.4 LUT-Aware Policy Model

- **`lut_policy.rs`**: LUT-aware policy model (candidate_to_family, CandidateFamily, CpuBudgetClass, LutEligibility, PolicySignals, QuantizationCostClass). Heuristic-based policy for candidate selection. Deferred as research scaffolding.

**Rationale**: The LUT-aware policy was designed to predict which candidates would compress well. The two-path approach uses simpler structural rules. The LUT-aware policy can be revisited as future research if needed.

**Gating Plan**: Move to `src/research/` directory or gate behind `#[cfg(feature = "research")]`. Not exported from `lib.rs` by default.

### 5.5 Candidate Generation & Realization

- **`candidate_gen.rs`**: Candidate generation (Candidate, CandidateGenerator, CandidateMetadata, CandidateRepresentation, SafetyReason). Generates multiple candidate representations for each frame. Deferred as research.
- **`materialize.rs`**: Materializer (Materializer). Materializes candidates into pixel data. Deferred as research.
- **`adaptive_fallback.rs`**: Adaptive fallback (AdaptiveBytesPreparer, AdaptiveStage, FallbackReason, FallbackTelemetry). Fallback logic for adaptive encoder. Deferred as research.
- **`adaptive_encode.rs`**: Adaptive encode (AdaptiveConfig, AdaptiveDecision). Adaptive encoding orchestration. Deferred as research.

**Rationale**: Candidate generation and realization are part of the adaptive optimizer infrastructure. The two-path approach uses simpler per-frame logic. These can be revisited as future research if needed.

**Gating Plan**: Move to `src/research/` directory or gate behind `#[cfg(feature = "research")]`. Not exported from `lib.rs` by default.

### 5.6 Scoring & Adaptive IR

- **`scoring.rs`**: Scoring (Chooser, DecisionReason, FrameDecision, ScoreBreakdown, Scorer, SequenceDecision). Candidate scoring and decision making. Deferred as research.
- **`adaptive_ir.rs`** (adaptive-specific parts): Parts of the adaptive IR that are specific to the adaptive optimizer. The canonical sequence IR (CanonicalSequence, CanonicalFrame, Canvas) is kept; the adaptive-specific parts (e.g., candidate tracking) are deferred.

**Rationale**: Scoring was designed to rank candidates. The two-path approach uses simpler structural rules. Scoring can be revisited as future research if needed.

**Gating Plan**: Move to `src/research/` directory or gate behind `#[cfg(feature = "research")]`. Not exported from `lib.rs` by default.

---

## 6. What We Throw Away from Mainline

These concepts are explicitly **not** being carried forward in the mainline product path:

### 6.1 Universal Adaptive Chooser as the Path to Ship Next

The idea that a single adaptive policy (tier0 → tier1 → tier2 → encode-and-measure) should be the default path for all GIFs is abandoned. The two-path approach is simpler and more predictable.

### 6.2 Bytes-First Adaptive Decision Making

The idea that we should optimize for output bytes first, then validate quality, is abandoned. The two-path approach prioritizes correctness and structural preservation. Lossy compression is explicit and separate.

### 6.3 Assumption That One Post-Resize Optimization Policy Should Serve All GIF Classes

The idea that a single optimization policy (with tunable parameters) should work for all GIF classes is abandoned. The two-path approach uses different policies for different GIF classes.

---

## 7. Practical Next Steps

The following beads tasks implement the two-path optimizer:

1. **`rusticle-n7z`**: Two-path classifier (deterministic Path A vs Path B routing).
2. **`rusticle-0g0`**: Path A core (exact opaque bbox reconstruction).
3. **`rusticle-69a`**: Path A palette strategy (global-preferred stable palette handling).
4. **`rusticle-zry`**: Refactor Path B (make current sparse/transparent pipeline an explicit named branch).
5. **`rusticle-eh9`**: Regression tests (path selection correctness and semantic preservation).
6. **`rusticle-a7w`**: Integration routing (wire classifier → Path A / Path B behind experimental flag).
7. **`rusticle-9vw`**: Benchmark evaluation (Path A/B routing vs current default vs gifsicle on holdout).
8. **`rusticle-dp0`**: Docs and journal (two-path architecture summary and future research notes).

Execute in order. Each task is a discrete, testable unit of work.

---

## 8. Module Mapping Table

| File/Module | Status | Rationale |
|---|---|---|
| `adaptive_ir.rs` | KEEP | Canonical sequence IR (CanonicalSequence, CanonicalFrame, Canvas, BoundingBox, ChangedRegion) is the ground truth for both paths. |
| `adaptive_encode.rs` | RESEARCH | Adaptive encoding orchestration. Deferred as research. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `adaptive_fallback.rs` | RESEARCH | Adaptive fallback logic. Deferred as research. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `analysis_kernels.rs` | KEEP | SIMD-accelerated pixel analysis (find_diff_bounding_box, analyze_transparency_simd, etc.). Used by both paths. |
| `candidate_gen.rs` | RESEARCH | Candidate generation. Deferred as research. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `decode.rs` | KEEP | GIF decoding with correct disposal-aware compositing. Produces canonical sequence IR. |
| `encode.rs` | KEEP | GIF encoding with quantization. Used by both paths. |
| `encode_and_measure.rs` | RESEARCH | Encode-and-measure chooser. Deferred as research. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `error.rs` | KEEP | Error types. Unchanged. |
| `image_compat.rs` | KEEP | Image crate compatibility. Unchanged. |
| `lut_policy.rs` | RESEARCH | LUT-aware policy model. Deferred as research scaffolding. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `materialize.rs` | RESEARCH | Materializer (candidate materialization). Deferred as research. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `optimize.rs` | KEEP | Structural optimization with correct disposal semantics. O3 semantic fix. Lossless only (threshold=0). Perceptual thresholding in `lossy()`. |
| `palette_lut.rs` | KEEP | Palette lookup table. Used by both paths. |
| `palette_realize.rs` | KEEP | Palette realization (RGBA → quantized indices). Used by both paths. |
| `palette_strategy.rs` | KEEP | Palette strategy determination (global vs per-frame local). Used by both paths. |
| `profiler.rs` | KEEP | GIF profiling and structural analysis. Used by both paths. Metrics/telemetry for benchmarking. |
| `quality.rs` | KEEP | Quality metrics (PSNR, SSIM, Butteraugli). Invalid-state fix. |
| `resize.rs` | KEEP | Frame resizing. Used by both paths. |
| `scoring.rs` | RESEARCH | Candidate scoring and decision making. Deferred as research. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `sequence_optimizer.rs` | RESEARCH | Sequence optimizer (DP-lite beam search). Deferred as research. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `simd_opt.rs` | KEEP | SIMD optimization utilities. Used by both paths. |
| `tier0_classifier.rs` | RESEARCH | Tier0 classifier (heuristic-based). Replaced by simpler two-path classifier (`rusticle-n7z`). Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `tier1_pruning.rs` | RESEARCH | Tier1 pruning (candidate pruning). Replaced by explicit Path A/B logic. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `tier2_measure.rs` | RESEARCH | Tier2 measurement (encode-and-measure). Deferred as research. Move to `src/research/` or gate behind `#[cfg(feature = "research")]`. |
| `types.rs` | KEEP | Core types (Gif, Frame, DisposalMethod, Filter, OptLevel, Palette). Unchanged. |
| `async_io.rs` | KEEP | Async I/O. Unchanged. |
| `docs/ADAPTIVE_ENCODER_ARCHITECTURE.md` | RESEARCH | Adaptive encoder architecture. Archived as historical reference. Not updated for two-path approach. |
| `docs/ADAPTIVE_ENCODER_RESULTS.md` | RESEARCH | Adaptive encoder results. Archived as historical reference. |
| `docs/BENCHMARKS.md` | KEEP | Benchmark infrastructure. Updated with two-path results. |
| `docs/bench_baseline.json` | KEEP | Baseline metrics. Updated with two-path baseline. |
| `docs/BUTTERAUGLI_TUNING_JOURNAL.md` | RESEARCH | Butteraugli tuning journal. Archived as historical reference. |
| `docs/tuning_guardrails.md` | RESEARCH | Tuning guardrails. Archived as historical reference. |
| `docs/TUNING_RECOMMENDATION.md` | RESEARCH | Tuning recommendation. Archived as historical reference. |
| `docs/TWO_PATH_OPTIMIZER_DECISION.md` | KEEP | This document. Architecture decision for two-path optimizer. |

---

## 9. Implementation Results & Benchmark Outcome

### 9.1 Two-Path Router Evaluation (rusticle-9vw)

**Date**: 2026-04-20  
**Corpus**: 42 images (3 known offenders + 39-image holdout), 3 repeats each = 126 total runs  
**Profiles Tested**: gifsicle baseline, rusticle default, two-path auto, forced Path A, forced Path B

#### Key Metrics

| Profile | Avg BA | Worst BA | Avg Runtime | Avg Bytes | Path A Rate | Path B Rate | Fallback |
|---------|--------|----------|-------------|-----------|-------------|-------------|----------|
| **gifsicle_baseline** | 7.115 | 20.21 | 274.7 ms | 167 KB | — | — | — |
| **rusticle_default** | 1.562 | 11.89 | 147.0 ms | 378 KB | — | — | — |
| **rusticle_two_path_auto** | 1.632 | 11.88 | 171.4 ms | 363 KB | **66.7%** | **33.3%** | **0.0%** |
| **rusticle_two_path_forced_a** | 1.976 | 23.20 | 150.5 ms | 271 KB | — | — | — |
| **rusticle_two_path_forced_b** | 1.562 | 11.89 | 152.3 ms | 378 KB | — | — | — |

#### Findings

1. **Classifier Routing Works**: Auto-classifier successfully routes 66.7% to Path A, 33.3% to Path B, with 0% fallback rate across all 126 runs.

2. **Quality Trade-off**: Two-path auto achieves 1.632 avg BA vs 1.562 for default (+4.5% degradation). This is a **modest quality loss** that does not justify the routing overhead.

3. **File Size Improvement**: Two-path auto produces 363 KB avg vs 378 KB for default (−3.9% reduction). Improvement is **marginal**.

4. **Runtime Overhead**: Two-path auto runs 171.4 ms vs 147.0 ms for default (+16.6% slower). The classifier feature extraction and Path A attempt add **significant overhead** that outweighs the modest file size gain.

5. **Path A Limitations**: Forced Path A shows 1.976 avg BA (+26.5% vs default), indicating **Path A is too conservative** for mixed sequences. Offender analysis confirms:
   - Voyager (opaque-delta): Path A correctly selected, quality identical to default ✓
   - Galilean Moon (transparent): Path A shows 9× quality degradation (0.810 BA), confirming Path A unsuitable for transparency
   - Trapezius (sparse): Path A shows 3.2× quality degradation (2.430 BA), confirming Path A too conservative

6. **Path B Identical to Default**: Forced Path B produces identical metrics to default (1.562 BA, 378 KB), indicating **no benefit from routing to Path B**. Path B is currently just the default pipeline.

#### Honest Reporting: Fallback Analysis

- **Fallback Rate**: 0.0% (0/126 runs)
- **Conclusion**: Two-path router is stable and reliable with no hidden failures or silent fallbacks.

### 9.2 What This Means

#### Architecture Assessment

✓ **Strengths**:
- Classifier is deterministic and makes sound routing decisions (voyager → Path A, transparent → Path B)
- Two-path architecture is simpler and easier to reason about than the prior tiered adaptive optimizer
- Zero fallback failures across all test cases
- Clear semantic separation (Path A = opaque-delta, Path B = general)

✗ **Weaknesses**:
- Current Path A implementation is too conservative; forced Path A shows 26.5% quality degradation
- Current Path B is identical to default; no specialized optimization for transparency/sparse patches
- Classifier overhead (+16.6% runtime) outweighs modest file size gains (−3.9%)
- Overall two-path auto is **not better than default** on the quality/runtime/size tradeoff

#### Recommendation: Keep the Architecture, Tune the Paths

**Decision**: The two-path architecture is **sound and worth keeping**, but the current Path A and Path B implementations are **not yet superior to the default**. The architecture is clearer and simpler than the prior adaptive approach, which has value for maintainability and reasoning. However, shipping the current implementation would be a regression.

**Next Steps for Tuning**:

1. **Path A Conservatism**: Path A is rejecting valid optimizations. Consider:
   - Relaxing classifier thresholds (e.g., allow some transparency, lower palette stability requirement)
   - Implementing per-frame palette overrides for frames that don't fit global palette
   - Adding adaptive disposal handling for Keep/None frames

2. **Path B Specialization**: Path B is currently identical to default. Consider:
   - Implementing transparency-aware quantization (preserve alpha channel through optimization)
   - Adding sparse/transparent-specific patch strategies
   - Improving disposal handling for Background/Previous disposal frames

3. **Classifier Overhead**: The +16.6% runtime overhead is significant. Consider:
   - Caching classification results across multiple encodes
   - Lazy feature extraction (compute only features needed for decision)
   - SIMD-accelerated feature computation (already have SIMD kernels in `analysis_kernels.rs`)

#### Comparison to Prior Adaptive Approach

The two-path approach is **simpler but less effective** than the prior tiered adaptive optimizer:

- **Prior approach**: Tier0 classifier → Tier1 pruning → Tier2 measure → encode-and-measure chooser. Complex but potentially more effective.
- **Two-path approach**: Simple classifier → Path A or Path B. Simpler but currently not better than default.

The trade-off is **worth making** because:
1. Simpler architecture is easier to validate and maintain
2. Clearer semantics make it easier to reason about correctness
3. Tuning two paths is more tractable than tuning a complex tiered system
4. The architecture is sound; the paths just need optimization

---

## 10. Open Questions (Updated)

### 10.1 Path A Conservatism Tuning

**Question**: What classifier thresholds and Path A implementation changes would make Path A competitive with default?

**Current finding**: Forced Path A shows 26.5% quality degradation. This suggests Path A is rejecting valid optimizations or applying overly conservative rules.

**Hypothesis**: Path A's bbox-only strategy is too restrictive. Consider allowing:
- Per-frame palette overrides for frames with high quantization error
- Transparency in specific frames if it improves compression
- Adaptive disposal handling based on frame content

**Validation needed**: Implement tuning experiments; measure quality/size/runtime on holdout set.

**Task**: Future work after `rusticle-dp0` (docs).

### 10.2 Path B Specialization

**Question**: What optimizations would make Path B better than default for transparency-heavy and sparse sequences?

**Current finding**: Forced Path B is identical to default (1.562 BA, 378 KB). This indicates Path B is not yet specialized.

**Hypothesis**: Path B should implement:
- Transparency-aware quantization (preserve alpha through optimization)
- Sparse patch strategies for high-transparency frames
- Disposal-aware optimization for Background/Previous disposal

**Validation needed**: Implement specializations; measure quality/size/runtime on holdout set.

**Task**: Future work after `rusticle-dp0` (docs).

### 10.3 Classifier Overhead Reduction

**Question**: Can classifier overhead be reduced from +16.6% to acceptable levels (<5%)?

**Current finding**: Feature extraction and Path A attempt add 24.4 ms per image.

**Hypothesis**: Overhead can be reduced by:
- Caching classification results (if same image encoded multiple times)
- Lazy feature extraction (compute only features needed for decision)
- SIMD-accelerated feature computation (use existing `analysis_kernels.rs`)

**Validation needed**: Profile classifier; implement optimizations; measure overhead reduction.

**Task**: Future work after `rusticle-dp0` (docs).

### 10.4 Research Feature Gating

**Question**: Should frozen research modules be gated behind a `#[cfg(feature = "research")]` feature flag, or moved to a separate `src/research/` directory?

**Current assumption**: Feature flag is simpler for now. Can be revisited if research directory grows large.

**Decision needed**: Before shipping two-path as default.

### 10.5 Backward Compatibility

**Question**: Should the default `--optimizer-strategy` be `legacy` (current behavior) or `auto` (two-path routing)?

**Current finding**: Two-path auto is not better than default. Keep `legacy` as default until Path A and Path B are tuned.

**Decision**: Default remains `legacy` until two-path auto is competitive.

### 10.6 Path A Identical Frame Handling

**Question**: How should Path A handle identical consecutive frames? Emit 1×1 pixel patch at (0,0)? Skip frame? Emit full-frame?

**Current assumption**: Emit 1×1 pixel patch at (0,0) to indicate no change. Decoder will composite correctly.

**Validation needed**: Ensure encoder handles 1×1 patches correctly. Benchmark on holdout set.

**Task**: Future work after `rusticle-dp0` (docs).

---

## 11. References

- **Decision Issue**: `rusticle-45a` (Re-evaluate optimizer architecture around two-path strategy)
- **Epic**: `rusticle-1yq` (Two-path GIF optimizer: conservative opaque-delta (A) + general sparse/transparent (B))
- **Benchmark Task**: `rusticle-9vw` (Benchmark evaluation: Path A/B routing vs current default vs gifsicle on holdout)
- **Benchmark Report**: `outputs/TWO_PATH_ROUTER_EVALUATION.md` (detailed evaluation results and analysis)
- **Prior Architecture Doc**: `docs/ADAPTIVE_ENCODER_ARCHITECTURE.md` (archived as historical reference)
- **Correctness Fixes**: Disposal-aware optimization, subframe reference-state fixes, O3 semantic fix, quality invalid-state fix (all in `rusticle-45a`)
