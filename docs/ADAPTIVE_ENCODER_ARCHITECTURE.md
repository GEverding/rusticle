# Adaptive Encoder Architecture

**Status**: Canonical design document for rusticle encoding pipeline  
**Version**: 1.0  
**Last Updated**: 2026-04-20

---

## 1. Problem Statement

### Current Divergence

Rusticle's current encoding pipeline applies a fixed sequence of transformations (decode → optimize → lossy → encode) without understanding the semantic structure of the source GIF. This causes two major failure classes:

#### 1a. Disposal-Aware Optimization Bugs

The `optimize()` pass marks pixels transparent by comparing to the previous frame, but ignores disposal semantics:

- **Disposal::Previous** requires the previous frame to remain in memory for correct playback. Marking pixels transparent can break this invariant if the encoder later chooses a different disposal method.
- **Disposal::Background** clears the frame region before compositing the next frame. Optimization that assumes pixel persistence across disposal boundaries produces visually incorrect output.
- **Disposal::None/Keep** have different canvas state implications, but the current code treats all disposal methods identically.

**Example**: A GIF with `Disposal::Previous` frames where optimization marks pixels transparent, then encode chooses `Disposal::Keep` for size reasons → visual corruption.

#### 1b. Over-Transforming Already-Optimized Opaque-Delta GIFs

Voyager-like GIFs (opaque delta frames, global palette, no transparency) are already near-optimal:

- They use full-frame or tight bounding-box patches with opaque pixels.
- A global palette is already chosen and stable.
- Applying aggressive optimization (marking pixels transparent) and then re-quantizing introduces:
  - Synthetic transparency that wasn't in the source.
  - Palette churn as the quantizer adapts to new transparency patterns.
  - Unnecessary CPU cost for analysis that won't improve the output.

**Example**: A 24-frame cartoon with global palette and opaque deltas. Current pipeline: optimize (marks pixels transparent) → lossy (more transparency) → encode (re-quantizes with new palette) → output is larger and slower to decode than the original.

### Root Cause

The pipeline commits to a single frame representation (full-canvas RGBA) and applies transformations in a fixed order without:

1. Understanding the source GIF's structure (opaque vs transparent, delta vs full, disposal semantics).
2. Evaluating multiple candidate representations (full frame, opaque bbox, sparse transparent patch, no-op).
3. Choosing the safest and cheapest candidate for each frame.
4. Respecting disposal-aware invariants during optimization.

---

## 2. Design Goals / Non-Goals

### Goals

1. **Correctness First**: All candidate representations must produce visually identical output when decoded. Disposal semantics are inviolable.
2. **One Truth Model**: A canonical sequence IR (intermediate representation) in display/canvas space that all downstream passes reference.
3. **Adaptive Representation Choice**: For each frame, evaluate multiple GIF-native candidates and select the safest/cheapest.
4. **Respect Source Structure**: Detect and preserve already-optimal structures (opaque deltas, global palettes, no transparency).
5. **SIMD Accelerates Analysis, Not Policy**: SIMD kernels measure properties (diff bounding boxes, color histograms, transparency patterns). Policy decisions (which candidate to choose) are data-driven, not hardcoded.
6. **Unified Engine**: One encoder that handles all GIF types, not separate pipelines for "opaque" vs "transparent" vs "disposal-heavy" GIFs.

### Non-Goals

1. **Forked Permanent Pipelines**: We do not want separate code paths that diverge and require separate testing/maintenance.
2. **Premature Commitment**: We do not commit to a single frame form (e.g., "always use opaque bbox") before evaluating alternatives.
3. **Lossless Perfection**: We accept that some GIFs may not compress as well as specialized tools (e.g., gifsicle with hand-tuned disposal). We optimize for correctness and reasonable compression.
4. **Unbounded Analysis Cost**: Scoring must be fast (cheap proxies preferred). Actual encode-and-measure is justified only for high-uncertainty cases.

---

## 3. Canonical Sequence IR

### Definition

The canonical sequence IR is the ground truth representation of a GIF animation in display/canvas space. All downstream passes (optimization, lossy, encoding) reference this IR and produce candidates that, when decoded, reproduce the same visual output.

### Structure

```rust
pub struct CanonicalSequence {
    // Sequence-level metadata
    pub width: u16,
    pub height: u16,
    pub loop_count: LoopCount,
    
    // Palette strategy (see section 5)
    pub palette_strategy: PaletteStrategy,
    
    // Per-frame canonical state
    pub frames: Vec<CanonicalFrame>,
}

pub struct CanonicalFrame {
    // Source patch metadata
    pub source_patch: SourcePatch,
    
    // Canvas state before drawing this frame
    pub pre_draw_canvas: Canvas,
    
    // Canvas state after drawing this frame (before disposal)
    pub displayed_canvas: Canvas,
    
    // Canvas state after disposal (before next frame's pre_draw)
    pub post_disposal_canvas: Canvas,
    
    // Changed region facts
    pub changed_bbox: BoundingBox,
    pub changed_mask: BitMask,  // Pixel-level change map
    
    // Structural classification
    pub classification: FrameClassification,
    
    // Timing
    pub delay: Duration,
    pub dispose: DisposalMethod,
}

pub struct SourcePatch {
    // The actual pixel data from the source GIF
    pub pixels: Vec<u8>,  // RGBA
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
    
    // Transparency profile
    pub has_transparency: bool,
    pub transparent_pixel_count: usize,
    pub opaque_pixel_count: usize,
    
    // Color profile
    pub unique_colors: usize,
    pub color_histogram: Option<ColorHistogram>,
}

pub struct Canvas {
    pub pixels: Vec<u8>,  // Full canvas RGBA
    pub width: u16,
    pub height: u16,
}

pub struct BoundingBox {
    pub left: u16,
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
}

pub enum FrameClassification {
    OpaqueFullFrame,
    OpaqueDelta { bbox: BoundingBox },
    TransparentSparsePatches { patch_count: usize },
    DisposalHeavyBackground,
    DisposalHeavyPrevious,
    Photographic,
}
```

### Invariants

1. **Disposal Semantics**: `post_disposal_canvas` is computed by applying the disposal method to `displayed_canvas`. This is the ground truth for the next frame's `pre_draw_canvas`.

2. **Canvas Consistency**: `displayed_canvas` is computed by compositing `source_patch` onto `pre_draw_canvas` at the specified position, with alpha blending.

3. **Changed Region Accuracy**: `changed_bbox` and `changed_mask` are computed by comparing `pre_draw_canvas` to `displayed_canvas`. They are pixel-perfect and account for alpha blending.

4. **No Lossy Transformation**: The canonical IR is lossless. It preserves all pixel data from the source GIF, including transparency and color values.

### Why Old Pipeline Violated These

The current pipeline:

1. **Decodes to full-canvas RGBA** but discards the source patch metadata (left, top, width, height, original disposal).
2. **Applies optimize()** which marks pixels transparent without checking disposal semantics. This breaks the disposal invariant.
3. **Applies lossy()** which further marks pixels transparent, compounding the problem.
4. **Encodes** with a quantizer that sees the modified frames, not the source structure.

Result: The encoder has no way to know that a frame was originally opaque-delta with `Disposal::Keep`, so it can't make the right choice about whether to preserve that structure.

---

## 4. Candidate Representations

For each frame, the encoder evaluates multiple GIF-native candidates and selects the one that minimizes cost (bytes + visual risk + temporal stability).

### 4.1 Full Frame

**When it works**: Frames with high change density, photographic content, or when the frame is smaller than the bounding box + overhead.

**When it fails**: Opaque-delta GIFs where full frames are larger than necessary. Introduces unnecessary bytes and palette churn.

**Encoding**: Full-canvas patch at (0, 0) with size (width, height). Disposal is typically `Keep` or `None`.

**Cost factors**:
- Byte cost: High (full canvas pixels).
- Visual risk: Low (no approximation).
- Temporal stability: Medium (palette may change per frame).
- CPU cost: Low (no analysis needed).

### 4.2 Exact Opaque Bounding Box Patch

**When it works**: Opaque-delta GIFs with tight bounding boxes. Preserves the source structure and allows global palette reuse.

**When it fails**: Frames with sparse transparent regions (bbox includes transparent pixels). Frames where the bbox is larger than the full frame (shouldn't happen, but guards against bad analysis).

**Encoding**: Patch at (left, top) with size (bbox_width, bbox_height), all pixels opaque. Disposal is typically `Keep` or `Previous`.

**Cost factors**:
- Byte cost: Low (only changed region).
- Visual risk: Low (exact match to source).
- Temporal stability: High (reuses global palette, no transparency).
- CPU cost: Medium (bounding box detection).

**Invariant**: All pixels in the patch must be opaque (alpha=255). If the source patch has any transparent pixels within the bbox, this candidate is invalid.

### 4.3 Transparent Sparse Patch

**When it works**: Frames with sparse changes and transparency. Allows the encoder to skip storing unchanged pixels.

**When it fails**: Frames where the sparse patch is larger than the full frame. Frames where synthetic transparency introduces visual artifacts (e.g., anti-aliased edges).

**Encoding**: Patch at (left, top) with size (bbox_width, bbox_height), with transparent pixels marking unchanged regions. Disposal is typically `Keep`.

**Cost factors**:
- Byte cost: Medium (sparse pixels + transparency overhead).
- Visual risk: Medium (synthetic transparency may affect anti-aliasing).
- Temporal stability: Medium (palette may change if transparency pattern changes).
- CPU cost: Medium (transparency analysis).

**Invariant**: Transparent pixels must correspond to regions that are unchanged from the previous frame (or post-disposal canvas, depending on disposal method).

### 4.4 Minimal / No-Op Frame

**When it works**: Frames where the displayed canvas is identical to the post-disposal canvas of the previous frame. The frame contributes nothing to the animation.

**When it fails**: Frames with any visual change. Frames where disposal semantics require a frame to exist.

**Encoding**: Empty frame (0x0 patch) or skipped entirely. Disposal is `None`.

**Cost factors**:
- Byte cost: Minimal (just frame header).
- Visual risk: Low if the frame truly contributes nothing.
- Temporal stability: N/A.
- CPU cost: Low (comparison only).

**Invariant**: The displayed canvas must be identical to the post-disposal canvas of the previous frame.

### 4.5 Palette Strategy as Part of Candidate Surface

Each candidate representation can be paired with a palette strategy:

- **Global Palette**: Reuse the sequence-level palette. Fast, stable, but may require dithering.
- **Local Palette**: Use a frame-specific palette. Slower, but may reduce dithering artifacts.
- **Adaptive Global**: Start with global, fall back to local if quality is insufficient.

The chooser evaluates (candidate_form, palette_strategy) pairs and selects the best combination.

---

## 5. Palette Strategy Layer

### Problem

GIFs can use a global palette (shared across all frames) or local palettes (per-frame). The choice affects:

- **Byte cost**: Global palette is stored once; local palettes are stored per frame.
- **Visual quality**: Global palette may require dithering; local palettes can be optimized per frame.
- **Temporal stability**: Global palette is stable across frames; local palettes can cause flicker.
- **Encoding complexity**: Global palette requires all frames to fit in 256 colors; local palettes allow per-frame color selection.

### Voyager-Like Sequences

Voyager GIFs (opaque delta, global palette, no transparency) are already near-optimal because:

1. The source GIF chose a global palette that works well for all frames.
2. Frames are opaque deltas, so the palette is stable across frames.
3. No transparency means no synthetic transparency risk.

**Strategy**: Detect this pattern and prefer global palette reuse. Avoid re-quantizing unless necessary.

### Stable Global Palette vs Local Fallback

**Stable Global Palette Mode**:
- Use the source GIF's global palette (if present) or quantize once for all frames.
- All frames use the same palette.
- Fast, stable, low byte cost.
- Risk: Some frames may not fit well in the global palette, requiring dithering.

**Local Fallback Mode**:
- If a frame doesn't fit well in the global palette (e.g., high color count, poor quality), use a local palette.
- Slower, higher byte cost, but better quality.

**Adaptive Strategy**:
- Start with global palette.
- For each frame, measure quality (PSNR/SSIM) against the source.
- If quality is below a threshold, fall back to local palette for that frame.
- This is the default strategy for mixed GIFs.

### Tradeoffs

| Strategy | Byte Cost | Quality | Stability | CPU Cost |
|----------|-----------|---------|-----------|----------|
| Global Only | Low | Medium | High | Low |
| Local Only | High | High | Low | High |
| Adaptive | Medium | High | Medium | Medium |

**Recommendation**: Use adaptive strategy by default. For Voyager-like sequences, the classifier should detect the pattern and prefer global-only mode.

---

## 6. Scoring / Chooser

### Signals

The chooser evaluates each candidate representation using the following signals:

#### 6.1 Byte Cost

Estimated size of the encoded frame (without actually encoding).

**Proxy**: `bbox_area * bytes_per_pixel + frame_header_overhead`

- Full frame: `width * height * bytes_per_pixel + overhead`
- Opaque bbox: `bbox_width * bbox_height * bytes_per_pixel + overhead`
- Transparent sparse: `changed_pixel_count * bytes_per_pixel + transparency_overhead + overhead`
- No-op: `overhead_only`

**Accuracy**: ±20% (good enough for ranking).

#### 6.2 Visual Risk

Likelihood that the candidate will produce visual artifacts.

**Factors**:
- **Synthetic transparency**: Marking pixels transparent that were opaque in the source. Risk increases with anti-aliased edges, gradients.
- **Palette mismatch**: Using a palette that doesn't match the frame's colors. Risk increases with color count, photographic content.
- **Dithering artifacts**: Global palette may require dithering. Risk increases with color diversity.

**Scoring**:
- Opaque bbox: Risk = 0 (exact match to source).
- Transparent sparse: Risk = transparency_risk_score (see section 6.2a).
- Full frame: Risk = palette_mismatch_score (see section 6.2b).
- No-op: Risk = 0 if truly no-op, else Risk = 1.0 (invalid).

#### 6.2a Transparency Risk Score

```
transparency_risk = (
    (anti_aliased_edge_pixels / changed_pixels) * 0.5 +
    (gradient_pixels / changed_pixels) * 0.3 +
    (synthetic_transparency_pixels / changed_pixels) * 0.2
)
```

**Heuristics**:
- Anti-aliased edges: Pixels with alpha in [1, 254] near opaque regions.
- Gradients: Regions with smooth color transitions.
- Synthetic transparency: Pixels marked transparent by lossy() that were opaque in source.

#### 6.2b Palette Mismatch Score

```
palette_mismatch = (
    (unique_colors_in_frame / 256) * 0.4 +
    (color_distance_to_palette / max_distance) * 0.6
)
```

**Heuristics**:
- Unique colors: Count distinct colors in the frame.
- Color distance: Average distance from frame colors to nearest palette color (in CIELAB space).

#### 6.3 Temporal Instability

Likelihood that the candidate will cause flicker or palette churn across frames.

**Factors**:
- **Palette churn**: If this frame uses a different palette than the previous frame, risk increases.
- **Transparency pattern change**: If this frame's transparency pattern is very different from the previous frame, risk increases.

**Scoring**:
- Global palette: Instability = 0 (stable across frames).
- Local palette: Instability = palette_distance_to_previous_frame.
- Transparent sparse: Instability = transparency_pattern_distance.

#### 6.4 Synthetic Transparency Risk

Specific risk of marking pixels transparent that shouldn't be.

**Factors**:
- **Disposal semantics**: If the frame uses `Disposal::Previous`, marking pixels transparent can break the invariant.
- **Lossy compression**: If lossy() marked pixels transparent, the risk is higher than if optimize() did.

**Scoring**:
```
synthetic_transparency_risk = (
    (disposal_previous_penalty if dispose == Previous else 0) +
    (lossy_penalty if lossy_applied else 0) +
    (transparency_risk_score from 6.2a)
)
```

#### 6.5 CPU Cost

Estimated CPU cost to encode this candidate.

**Factors**:
- Full frame: Low (no analysis).
- Opaque bbox: Medium (bounding box detection).
- Transparent sparse: Medium (transparency analysis).
- No-op: Low (comparison only).

**Scoring**: Relative to full frame (baseline = 1.0).

### 6.6 Concrete First-Pass Scoring Formula

```
score = (
    byte_cost_weight * byte_cost_normalized +
    visual_risk_weight * visual_risk +
    temporal_instability_weight * temporal_instability +
    synthetic_transparency_weight * synthetic_transparency_risk +
    cpu_cost_weight * cpu_cost_normalized
)

// Default weights (tunable per GIF classification)
byte_cost_weight = 0.5
visual_risk_weight = 0.3
temporal_instability_weight = 0.1
synthetic_transparency_weight = 0.05
cpu_cost_weight = 0.05

// Normalization: scale all factors to [0, 1]
byte_cost_normalized = min(byte_cost / max_byte_cost, 1.0)
cpu_cost_normalized = min(cpu_cost / max_cpu_cost, 1.0)
```

**Selection**: Choose the candidate with the lowest score.

### 6.7 When to Encode-and-Measure

Cheap proxies are used for ranking. Actual encode-and-measure is justified when:

1. **High uncertainty**: Multiple candidates have similar scores (within 10%).
2. **Visual risk is high**: Synthetic transparency or palette mismatch risk > 0.3.
3. **Byte cost is critical**: The GIF is very large or the output size is a hard constraint.

**Encode-and-measure process**:
1. Encode the candidate using the chosen palette strategy.
2. Measure PSNR/SSIM against the source.
3. Measure actual byte cost.
4. Update the score with real data.
5. Compare against other candidates and select the best.

---

## 7. GIF Structure Taxonomy

### 7.1 Opaque-Delta / Global-Palette

**Characteristics**:
- Frames are opaque deltas (no transparency).
- Global palette is used and stable across frames.
- Disposal is typically `Keep` or `None`.
- Examples: Voyager GIFs, cartoons, simple animations.

**Priors**:
- Prefer opaque bbox candidate (exact match to source).
- Prefer global palette (stable, low byte cost).
- Avoid synthetic transparency (risk = 0).
- Avoid re-quantization (reuse source palette).

**Scoring adjustments**:
- Byte cost weight: 0.6 (size matters).
- Visual risk weight: 0.2 (low risk).
- Temporal instability weight: 0.05 (stable).
- Synthetic transparency weight: 0.0 (not applicable).

### 7.2 Transparency-Heavy Sparse Delta

**Characteristics**:
- Frames have significant transparency.
- Changes are sparse (small bounding boxes).
- Disposal is typically `Keep`.
- Examples: UI animations, screen recordings, overlays.

**Priors**:
- Prefer transparent sparse patch candidate (minimizes byte cost).
- Prefer local palette (better quality for sparse regions).
- Transparency risk is inherent; manage it carefully.
- Synthetic transparency is acceptable if it reduces byte cost.

**Scoring adjustments**:
- Byte cost weight: 0.7 (size is critical).
- Visual risk weight: 0.15 (transparency is expected).
- Temporal instability weight: 0.1 (palette may change).
- Synthetic transparency weight: 0.05 (acceptable if byte cost is low).

### 7.3 Disposal-Heavy Background / Previous

**Characteristics**:
- Frames use `Disposal::Background` or `Disposal::Previous`.
- Frames may be sparse or full.
- Disposal semantics are critical for correctness.
- Examples: Animated backgrounds, frame-by-frame animations.

**Priors**:
- Respect disposal semantics strictly (no synthetic transparency that breaks invariants).
- Prefer candidates that preserve disposal semantics.
- Avoid marking pixels transparent if disposal is `Previous` (breaks invariant).
- Measure visual correctness carefully (disposal semantics are easy to get wrong).

**Scoring adjustments**:
- Byte cost weight: 0.4 (correctness > size).
- Visual risk weight: 0.4 (disposal semantics are risky).
- Temporal instability weight: 0.1 (disposal changes canvas state).
- Synthetic transparency weight: 0.1 (high risk if disposal is `Previous`).

### 7.4 Photographic / Noisy

**Characteristics**:
- Frames have high color count and smooth gradients.
- Changes are dense (large bounding boxes or full frames).
- Disposal is typically `Keep` or `None`.
- Examples: Video-like GIFs, photographs, complex scenes.

**Priors**:
- Prefer full frame candidate (sparse patches are inefficient).
- Prefer local palette (better quality for high color count).
- Visual quality is critical (photographic content is sensitive to compression).
- Synthetic transparency is risky (may affect gradients).

**Scoring adjustments**:
- Byte cost weight: 0.3 (quality > size).
- Visual risk weight: 0.5 (quality is critical).
- Temporal instability weight: 0.1 (palette may change).
- Synthetic transparency weight: 0.1 (risky for gradients).

---

## 8. SIMD Boundaries

SIMD kernels accelerate analysis, not policy. The following kernels are good SIMD targets:

### 8.1 Bounding Box Detection

**Kernel**: `find_diff_bounding_box(pre_draw, displayed, width, height) -> BoundingBox`

**Why SIMD**: Pixel-by-pixel comparison across the entire canvas. SIMD can compare 16 pixels in parallel (SSE2) or 32 pixels (AVX2).

**Implementation**: Scan rows and columns in parallel, find first/last changed pixel.

**Output**: `(left, top, right, bottom)` of the changed region.

### 8.2 Transparency Analysis

**Kernel**: `analyze_transparency(pixels, width, height) -> TransparencyProfile`

**Why SIMD**: Scan all pixels for alpha values. SIMD can process 16 pixels in parallel.

**Implementation**: Count opaque, transparent, and semi-transparent pixels. Identify anti-aliased edges.

**Output**: `(opaque_count, transparent_count, semi_transparent_count, edge_pixels)`

### 8.3 Color Histogram

**Kernel**: `build_color_histogram(pixels, width, height) -> Histogram`

**Why SIMD**: Count occurrences of each color. SIMD can process multiple pixels in parallel.

**Implementation**: Scan pixels and update histogram buckets (256 colors or 256^3 for full RGB).

**Output**: `Histogram { color_counts: [u32; 256], unique_colors: usize }`

### 8.4 Palette Matching

**Kernel**: `match_pixels_to_palette(pixels, palette) -> (indices, error)`

**Why SIMD**: For each pixel, find the nearest palette color. SIMD can process multiple pixels in parallel.

**Implementation**: For each pixel, compute distance to all palette colors and find minimum.

**Output**: `(indices: Vec<u8>, error: f32)` where error is the total color distance.

### 8.5 Synthetic Transparency Detection

**Kernel**: `detect_synthetic_transparency(source, optimized, bbox) -> SyntheticTransparencyMap`

**Why SIMD**: Compare source and optimized frames pixel-by-pixel to find newly transparent pixels.

**Implementation**: Scan pixels and mark those that were opaque in source but transparent in optimized.

**Output**: `BitMask` of synthetic transparency pixels.

### Important: SIMD is Analysis, Not Policy

All SIMD kernels produce **measurements**, not decisions. The policy layer (chooser) uses these measurements to make decisions. For example:

- `find_diff_bounding_box` measures the changed region; the chooser decides whether to use opaque bbox or full frame.
- `analyze_transparency` measures transparency patterns; the chooser decides whether synthetic transparency is acceptable.
- `build_color_histogram` measures color diversity; the chooser decides whether to use global or local palette.

This separation ensures that policy decisions are data-driven and can be tuned without changing SIMD kernels.

---

## 9. Failure Modes / Guardrails

### 9.1 Catastrophic Divergence

**Failure**: The output GIF is visually very different from the source (e.g., wrong colors, missing frames, flicker).

**Causes**:
- Disposal semantics violated (e.g., marking pixels transparent when disposal is `Previous`).
- Palette churn (e.g., each frame uses a different palette, causing flicker).
- Synthetic transparency breaks anti-aliasing (e.g., edges become jagged).

**Guardrails**:
1. **Disposal Invariant Check**: Before marking pixels transparent, verify that the disposal method allows it.
   - `Disposal::Previous`: Do not mark pixels transparent (breaks invariant).
   - `Disposal::Background`: Safe to mark pixels transparent (background is cleared anyway).
   - `Disposal::Keep` / `Disposal::None`: Safe to mark pixels transparent (canvas persists).

2. **Palette Stability Check**: If a frame uses a local palette, measure the distance to the previous frame's palette. If distance > threshold, flag as high instability.

3. **Synthetic Transparency Check**: Measure the number of pixels marked transparent by lossy() that were opaque in the source. If > threshold, flag as high risk.

### 9.2 Invalid Quality Comparisons

**Failure**: Comparing PSNR/SSIM of candidates that use different palettes or transparency patterns. The comparison is invalid because the candidates are not directly comparable.

**Causes**:
- Comparing full frame (global palette) to opaque bbox (local palette).
- Comparing opaque bbox to transparent sparse (different transparency patterns).

**Guardrails**:
1. **Palette Normalization**: When comparing candidates with different palettes, convert both to the same palette before measuring quality.
2. **Transparency Normalization**: When comparing candidates with different transparency patterns, measure quality only on opaque pixels.
3. **Canonical Reference**: Always measure quality against the canonical sequence IR (displayed_canvas), not against other candidates.

### 9.3 Palette Churn

**Failure**: Each frame uses a different palette, causing flicker or color shifts.

**Causes**:
- Using local palette for every frame without checking stability.
- Quantizing each frame independently without considering the previous frame's palette.

**Guardrails**:
1. **Palette Distance Threshold**: If the distance between consecutive frame palettes > threshold, flag as high instability.
2. **Palette Reuse**: Prefer reusing the previous frame's palette if quality is acceptable.
3. **Global Palette Preference**: Start with global palette and fall back to local only if necessary.

### 9.4 Transparent-Index Issues

**Failure**: The transparent index (color index used to represent transparency) is not consistent across frames, causing visual artifacts.

**Causes**:
- Using different transparent indices in different frames.
- Reusing a transparent index for an opaque color in another frame.

**Guardrails**:
1. **Transparent Index Reservation**: Reserve a specific color index (e.g., 255) for transparency across all frames.
2. **Transparent Index Validation**: Before encoding, verify that the transparent index is not used for opaque colors.
3. **Transparent Index Consistency**: If a frame uses a local palette, ensure the transparent index is consistent with the global palette.

### 9.5 Safe Fallback Behavior

**Fallback Strategy**: If the chooser encounters an error or high uncertainty, fall back to the safest candidate.

**Safe Candidate Priority**:
1. **Full Frame with Global Palette**: Always correct, but may be larger.
2. **Full Frame with Local Palette**: Always correct, but higher byte cost.
3. **Opaque Bbox with Global Palette**: Correct if the bbox is accurate and all pixels are opaque.
4. **Transparent Sparse with Global Palette**: Correct if transparency is accurate and disposal semantics are respected.

**Fallback Trigger**:
- If the chooser's top candidate has visual risk > 0.5, fall back to full frame.
- If the chooser encounters an error during encode-and-measure, fall back to full frame.
- If the chooser detects disposal semantic violations, fall back to full frame.

---

## 10. Phased Implementation Plan

### Phase 1: Canonical Sequence IR (`rusticle-usz`)

**Goal**: Build the canonical sequence IR and populate it during decode.

**Tasks**:
- Define `CanonicalSequence`, `CanonicalFrame`, `SourcePatch`, `Canvas` types.
- Modify `decode.rs` to populate the canonical IR during decoding.
- Implement canvas state tracking (pre_draw, displayed, post_disposal).
- Implement bounding box detection (SIMD kernel).
- Add tests to verify invariants (disposal semantics, canvas consistency).

**Output**: Canonical IR is available after decode; all downstream passes reference it.

### Phase 2: Frame Classification (`524`)

**Goal**: Classify each frame into one of the four GIF structure types.

**Tasks**:
- Implement `FrameClassification` enum and classifier function.
- Analyze frame properties: transparency, color count, bbox size, disposal method.
- Assign classification based on heuristics.
- Add tests to verify classification accuracy on known GIFs.

**Output**: Each frame has a classification that biases candidate selection.

### Phase 3: Candidate Representation Layer (`4si`)

**Goal**: Implement candidate representation generation for each frame.

**Tasks**:
- Implement candidate generators for: full frame, opaque bbox, transparent sparse, no-op.
- For each candidate, compute estimated byte cost and visual risk.
- Add validation to ensure candidates are correct (e.g., opaque bbox has no transparent pixels).
- Add tests to verify candidate correctness.

**Output**: For each frame, generate all valid candidates.

### Phase 4: Palette Strategy Layer (`jjj`)

**Goal**: Implement palette strategy selection and quantization.

**Tasks**:
- Implement `PaletteStrategy` enum (global, local, adaptive).
- Implement global palette quantization (once for all frames).
- Implement local palette quantization (per-frame).
- Implement adaptive strategy (global with local fallback).
- Add tests to verify palette quality and stability.

**Output**: Palette strategy is selected per frame based on classification and quality.

### Phase 5: Scoring / Chooser (`b1h`)

**Goal**: Implement the scoring formula and chooser logic.

**Tasks**:
- Implement scoring formula with configurable weights.
- Implement chooser logic to select the best candidate.
- Implement encode-and-measure for high-uncertainty cases.
- Add tests to verify chooser selects optimal candidates.

**Output**: For each frame, the chooser selects the best (candidate, palette_strategy) pair.

### Phase 6: SIMD Kernels (`nyg`)

**Goal**: Implement SIMD-accelerated analysis kernels.

**Tasks**:
- Implement `find_diff_bounding_box` (SIMD).
- Implement `analyze_transparency` (SIMD).
- Implement `build_color_histogram` (SIMD).
- Implement `match_pixels_to_palette` (SIMD).
- Implement `detect_synthetic_transparency` (SIMD).
- Benchmark against scalar implementations.

**Output**: Analysis kernels are SIMD-accelerated; policy layer uses measurements.

### Phase 7: Failure Mode Guardrails (`6dx`)

**Goal**: Implement guardrails to detect and prevent failure modes.

**Tasks**:
- Implement disposal invariant checks.
- Implement palette stability checks.
- Implement synthetic transparency checks.
- Implement transparent index validation.
- Implement safe fallback behavior.
- Add tests to verify guardrails catch errors.

**Output**: Encoder detects and prevents catastrophic divergence, palette churn, etc.

### Phase 8: Optimization Pass Refactor (`51d`)

**Goal**: Refactor `optimize()` and `lossy()` to respect disposal semantics.

**Tasks**:
- Modify `optimize()` to check disposal semantics before marking pixels transparent.
- Modify `lossy()` to respect disposal invariants.
- Add tests to verify optimization respects disposal semantics.
- Benchmark to ensure no performance regression.

**Output**: Optimization passes are disposal-aware and correct.

### Phase 9: Encoder Integration (`ue1`)

**Goal**: Integrate the adaptive encoder into the main encode pipeline.

**Tasks**:
- Modify `encode.rs` to use the canonical IR and chooser.
- Implement candidate encoding (convert candidate to GIF frame).
- Integrate palette strategy into quantization.
- Benchmark against current encoder.
- Add integration tests to verify correctness on diverse GIFs.

**Output**: Adaptive encoder is the default; old encoder is deprecated.

---

## 11. Open Questions

### 11.1 Palette Quantization Strategy

**Question**: Should we quantize the global palette once at the beginning, or should we quantize per-frame and then find a common palette?

**Options**:
1. **Quantize once**: Fast, stable, but may not fit all frames well.
2. **Quantize per-frame, then find common**: Slower, but better quality.
3. **Quantize once, then refine**: Quantize once, then for each frame, measure quality and refine if needed.

**Impact**: Affects byte cost, visual quality, and CPU cost.

**Decision needed**: Product/engineering decision on quality vs speed tradeoff.

### 11.2 Synthetic Transparency Threshold

**Question**: How much synthetic transparency is acceptable before we fall back to a different candidate?

**Options**:
1. **Conservative**: < 5% of pixels can be marked transparent.
2. **Moderate**: < 10% of pixels can be marked transparent.
3. **Aggressive**: < 20% of pixels can be marked transparent.

**Impact**: Affects byte cost and visual quality.

**Decision needed**: Tuning decision based on user feedback and benchmarks.

### 11.3 Disposal Semantics Strictness

**Question**: Should we strictly enforce disposal semantics (never mark pixels transparent if disposal is `Previous`), or should we allow it with a high penalty?

**Options**:
1. **Strict**: Never mark pixels transparent if disposal is `Previous`.
2. **Penalized**: Allow it, but with a high visual risk penalty.

**Impact**: Affects correctness and byte cost.

**Decision needed**: Correctness decision; recommend strict enforcement.

### 11.4 Encode-and-Measure Threshold

**Question**: When should we actually encode and measure candidates, vs using cheap proxies?

**Options**:
1. **Always**: Encode all candidates and measure. Slow but accurate.
2. **High uncertainty**: Encode only if top candidates have similar scores.
3. **High risk**: Encode only if visual risk is high.
4. **Never**: Use cheap proxies only. Fast but may miss optimal candidates.

**Impact**: Affects CPU cost and output quality.

**Decision needed**: Performance/quality tradeoff decision.

### 11.5 Scoring Weights

**Question**: What are the optimal weights for the scoring formula?

**Options**:
1. **Byte-cost-heavy**: Optimize for size (byte_cost_weight = 0.7).
2. **Quality-heavy**: Optimize for visual quality (visual_risk_weight = 0.5).
3. **Balanced**: Balance size and quality (current defaults).

**Impact**: Affects output size and visual quality.

**Decision needed**: Product decision on optimization goals. Recommend starting with balanced weights and tuning based on benchmarks.

### 11.6 Classification Heuristics

**Question**: What heuristics should we use to classify frames?

**Options**:
1. **Simple**: Use transparency, color count, bbox size.
2. **Complex**: Use entropy, gradient detection, disposal method, temporal patterns.

**Impact**: Affects classification accuracy and CPU cost.

**Decision needed**: Tuning decision based on classification accuracy on diverse GIFs.

### 11.7 Fallback Behavior for Disposal::Previous

**Question**: For frames with `Disposal::Previous`, should we:
1. Never mark pixels transparent (strict).
2. Allow marking transparent, but require encode-and-measure to verify correctness.
3. Use a heuristic to detect safe transparency patterns.

**Options**: See above.

**Impact**: Affects correctness and byte cost.

**Decision needed**: Correctness decision; recommend strict enforcement initially, then relax if safe patterns are identified.

---

## Appendix: Glossary

- **Canonical Sequence IR**: Ground truth representation of a GIF animation in display/canvas space.
- **Candidate Representation**: A GIF-native encoding of a frame (full frame, opaque bbox, transparent sparse, no-op).
- **Disposal Method**: How to dispose of a frame before showing the next (None, Keep, Background, Previous).
- **Bounding Box**: The smallest rectangle that contains all changed pixels.
- **Synthetic Transparency**: Pixels marked transparent by optimization/lossy that were opaque in the source.
- **Palette Strategy**: How to assign colors to frames (global, local, adaptive).
- **Temporal Instability**: Likelihood that a candidate will cause flicker or palette churn.
- **Visual Risk**: Likelihood that a candidate will produce visual artifacts.
- **Encode-and-Measure**: Actually encoding a candidate and measuring its byte cost and visual quality.
- **Cheap Proxy**: A fast heuristic estimate of byte cost or visual quality, without actual encoding.

---

## References

- **Disposal Semantics**: GIF89a specification, section on Graphic Control Extension.
- **Canvas Compositing**: Alpha blending formulas in decode.rs.
- **SIMD Kernels**: fast_image_resize crate for image processing primitives.
- **Quantization**: imagequant crate for color quantization.
- **Quality Metrics**: PSNR/SSIM implementations in quality.rs.
