# Voyager Representation Study & Two-Path Router: Research & Future Opt-In

**Status**: Research / Future Opt-In (not current mainline product path)

**Last Updated**: 2026-04-21

---

## Overview

This document describes the **voyager-class representation study** and the **two-path optimizer routing** system. Both are preserved research directions that may become opt-in features in the future, but are **not** the current mainline product path.

The current mainline product path is the **corrected default path** (documented in `rusticle-187`), which uses the standard encode/optimize pipeline without the voyager or two-path routing logic.

---

## Voyager Representation Study (Epic `rusticle-502`)

### What It Was Trying to Solve

The voyager study explored whether **alternative representation strategies** (different ways to structure GIF frames and palettes) could improve compression or quality compared to the standard approach.

The study tested four candidates:

1. **Control Path** (`voyager_repr.rs`): Full-frame output with sequence-global palette
2. **Source-Reuse** (`voyager_source_reuse.rs`): Exact bbox patches + reused source palette
3. **Exact Bbox + Derived Global** (`voyager_exact_bbox_global_palette.rs`): Exact bbox patches + fresh sequence-global palette
4. **Exact Bbox + Derived Global + Fallback** (`voyager_exact_bbox_global_palette_with_fallback.rs`): Exact bbox patches + fresh global palette + full-frame fallback threshold

### Structural Assumptions

Each voyager candidate assumes:
- Input is **resized displayed canvases** (RGBA, already composited correctly)
- Output is **quantized frame data** (palette indices) ready for GIF encoding
- **No synthetic transparency** is introduced (transparency only where it already exists structurally)
- **Frame timing and disposal are preserved** exactly

The candidates differ in:
- **Geometry**: Full-frame vs. exact bbox patches
- **Palette strategy**: Sequence-global vs. source-reused vs. per-frame local
- **Fallback logic**: None vs. full-frame threshold

### What the Latest Evidence Says

**Voyager wins are real on specific GIF classes** (e.g., animation-heavy, opaque-delta sequences), but **generality is not established**:

- The exact bbox + derived global palette approach (`voyager_exact_bbox_global_palette.rs`) shows promise for certain GIF types
- Source palette reuse (`voyager_source_reuse.rs`) is too strict (fails when source has no global palette) and doesn't generalize well
- The control path (`voyager_repr.rs`) is a clean baseline but doesn't offer compression wins
- The fallback variant (`voyager_exact_bbox_global_palette_with_fallback.rs`) adds practical production logic but requires larger-corpus validation

**Blocker**: Validation on a larger, more diverse GIF corpus is needed before promoting any voyager candidate to mainline.

### Current Status

- **Not integrated into mainline product path**
- **Preserved in code** as self-contained modules for future research
- **No runtime integration** — these modules are not called by default
- **Available for opt-in experimentation** if needed

---

## Two-Path Router & Classifier (Experimental Routing)

### What It Was Trying to Solve

The two-path system explored whether **routing different GIF classes to different optimization strategies** could improve overall compression without requiring expensive measurement/classification overhead.

The hypothesis: opaque-delta GIFs (already-optimized, no transparency, stable palette) and transparency-heavy GIFs have fundamentally different optimal strategies. A simple, deterministic classifier could route them appropriately.

### Structural Assumptions

The two-path system assumes:
- **Deterministic classification** based on structural features (no randomness or measurement)
- **Two bounded paths**: Path A (conservative opaque-delta) and Path B (general sparse/transparent)
- **Fast classification** (no encode-and-measure overhead)
- **Safe fallback** (if Path A fails, fall back to Path B or legacy)

### Path A: Conservative Opaque-Delta Reconstruction

**Module**: `path_a.rs`, `path_a_palette.rs`

**Assumptions**:
- Input GIF is structurally close to optimal GIF-native streams
- No transparent GCEs
- Stable/global palette
- Many offset subframes
- Mostly Keep/None disposal
- Low changed-area ratio

**Strategy**:
1. Compute exact changed bbox between consecutive displayed canvases
2. Emit opaque bbox patches (preferred) or full-frame fallback
3. Use stable global palette (source-reused or derived)
4. **Never introduce synthetic transparency**

**Thresholds**:
- Bbox area threshold: 0.7 (70% of canvas area) — if exceeded, emit full frame instead of patch
- Palette quality threshold: 30.0 dB (conservative, rarely triggers local palette fallback)

### Path B: General Sparse/Transparent Optimization

**Module**: `path_b.rs`

**Assumptions**:
- Input GIF may have transparency, mixed disposal, or non-obvious opaque-delta structure
- Requires more general optimization strategy

**Strategy**:
- (Details in `path_b.rs` module documentation)

### Classifier Logic

**Module**: `classifier.rs`

**Classification Features** (computed from decoded canonical sequence):
- Transparent GCE usage
- Palette stability (global vs. local)
- Disposal mix (Keep/None vs. Background/Previous)
- Offset subframe prevalence
- Changed-area ratio (median bbox-of-change / canvas-area)

**Path A Criteria** (all must hold):
- No transparent GCEs
- ≥90% frames use Keep or None disposal
- Global palette or ≥80% palette stability
- Median changed-area ratio ≤ 0.6

**Everything else → Path B**

### Router Integration

**Module**: `two_path_router.rs`

**Routing Modes**:
- `Legacy`: Current default behavior (no routing, no classifier)
- `Auto`: Classifier decides (Path A for opaque-delta, Path B for mixed/transparent)
- `PathA`: Force Path A regardless of input
- `PathB`: Force Path B regardless of input

**Telemetry**:
- Logs path selection, classifier features, and fallback reasons
- Emits to stderr for debugging

### What the Latest Evidence Says

**Two-path routing shows promise** for certain GIF classes, but **generality is not established**:

- Path A works well for already-optimized opaque-delta sequences
- Path B is a reasonable fallback for mixed/transparency-heavy GIFs
- The classifier is simple and deterministic, but may be too conservative (routes too many GIFs to Path B)
- No large-corpus validation yet on whether two-path routing beats the corrected default path overall

**Blocker**: Validation on a larger, more diverse GIF corpus is needed before promoting two-path routing to mainline.

### Current Status

- **Not integrated into mainline product path**
- **Preserved in code** as self-contained modules for future research
- **Available via `OptimizerStrategy::Auto` flag** if explicitly enabled
- **Safe fallback to legacy path** always available
- **No runtime integration by default** — the corrected default path is the explicit mainline

---

## Code Organization

### Voyager Modules (Research)

```
crates/rusticle/src/
├── voyager_repr.rs                                    # Control path (full-frame + global palette)
├── voyager_source_reuse.rs                            # Candidate #2 (bbox + source palette)
├── voyager_exact_bbox_global_palette.rs               # Candidate #3 (bbox + derived global palette)
└── voyager_exact_bbox_global_palette_with_fallback.rs # Candidate #4 (bbox + derived global + fallback)
```

### Two-Path Modules (Experimental Routing)

```
crates/rusticle/src/
├── classifier.rs                # Deterministic classification logic
├── two_path_router.rs           # Routing integration point
├── path_a.rs                    # Conservative opaque-delta reconstruction
└── path_a_palette.rs            # Palette strategy for Path A
```

### Related Modules (Supporting Infrastructure)

```
crates/rusticle/src/
├── path_b.rs                    # General sparse/transparent optimization
├── adaptive_ir.rs               # Canonical sequence representation
├── profiler.rs                  # GIF profiling for classification features
└── ... (other modules)
```

---

## How to Use (If Experimenting)

### Voyager Candidates

To use a voyager candidate directly:

```rust
use rusticle::voyager_exact_bbox_global_palette::VoyagerExactBboxGlobalPaletteBuilder;

let repr = VoyagerExactBboxGlobalPaletteBuilder::build(
    &resized_frames,
    canvas_width,
    canvas_height,
)?;
```

### Two-Path Router

To enable two-path routing:

```rust
use rusticle::two_path_router::{route_optimize, OptimizerStrategy, TwoPathConfig};

let config = TwoPathConfig {
    strategy: OptimizerStrategy::Auto, // or PathA, PathB, Legacy
    ..Default::default()
};

let result = route_optimize(&gif, &config)?;
```

---

## Future Work

1. **Larger-corpus validation**: Test voyager candidates and two-path routing on a diverse GIF corpus (>10k images)
2. **Classifier refinement**: Adjust thresholds and features based on validation results
3. **Path A optimization**: Explore further optimizations for opaque-delta sequences
4. **Integration decision**: If validation is positive, promote to mainline; otherwise, archive as historical research

---

## References

- Epic `rusticle-502`: Bounded representation study
- Task `rusticle-187`: Clean up mainline around corrected default path
- Task `rusticle-7o5`: Document voyager-specific path as research in code
