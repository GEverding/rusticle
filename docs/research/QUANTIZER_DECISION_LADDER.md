# Quantizer Decision Ladder — Encoder-Style GIF Optimization Design

## Status
Design proposal. Tiers are tagged inline as [COMMITTED], [NEW], [EXPERIMENT], or TODO; the experiment log remains the [EXPERIMENT] record.

## Design principles
1. The fastest path is the one we skip — earliest possible bypass.
2. Profile-driven tier selection — each tier exists because perf data justified it.
3. Cheap gates, expensive payloads — spend nanoseconds to save milliseconds.
4. Different content needs different work — pixel art ≠ voyager ≠ photo.
5. Algorithm beats micro-optimization — reducing calls beats SIMDing the same calls.

## Flow diagram
```text
┌─────────────────────────────────────────────────────────────────┐
│                          DECODE                                  │
│  • RGBA frames                                                   │
│  • global_palette, local_palette per frame                       │
│  • disposal, offsets, transparent_idx                            │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│                      CLASSIFY (cheap)                            │
│  Per frame, gather signals:                                      │
│  • has source palette? (global or local)                         │
│  • unique source palette colors (≤64? ≤192? full 256?)           │
│  • has transparency?                                             │
│  • is keyframe vs diff subframe?                                 │
│  • size, aspect, expected resize ratio                           │
│  • content class hint: pixel-art / cartoon / photo / voyager     │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│                       RESIZE                                     │
│  • Lanczos3 / Bilinear / Nearest (SIMD via fast_image_resize)    │
│  • PRESERVE source palette metadata downstream                   │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│                  OPTIMIZE (O1/O2/O3)                             │
│  • Inter-frame transparency                                      │
│  • Subframe bbox extraction                                      │
│  • Disposal handling                                             │
│  • NOTE: keep original_palette + local_palette references alive  │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼
              ┌───────────────────────┐
              │   ENCODE: per frame   │  ← parallel via rayon
              └───────────┬───────────┘
                          │
                          ▼
        ╔═════════════════════════════════════╗
        ║   QUANTIZATION DECISION LADDER       ║
        ║   (cheapest tier that fits, wins)    ║
        ╚═════════════════╤═══════════════════╝
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ TIER 0  ─  LUT FAST PATH                          [COMMITTED]   │
│ ─────────────────────────────────────────                       │
│  IF  original_palette is present                                 │
│  AND PaletteLut quality gate passes                              │
│        (avg_dist² < 150, outlier < 5%, util > 30%)              │
│  THEN map via 64³ LUT, O(1) per pixel                            │
│  COST: ~free per pixel after LUT build                           │
│  HITS: most CLI re-encodes with stable palettes                  │
└─────────────────────────┬───────────────────────────────────────┘
                  miss / not eligible
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ TIER 1  ─  EXACT ≤256 BYPASS                     [COMMITTED]    │
│ ─────────────────────────────────────────                       │
│  Scan unique opaque RGBs with early-exit at >max_colors          │
│  IF unique opaque ≤ max_colors                                   │
│  THEN emit exact palette + indices, NO QUANTIZATION              │
│  COST: one O(pixels) hash pass, bounded                          │
│  HITS: pixel art, simple cartoons, voyager-class, already-q'd    │
│  SEE: 9939ffd                                                    │
└─────────────────────────┬───────────────────────────────────────┘
                          │ miss
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ TIER 2  ─  TEMPORAL PALETTE REUSE               [EXPERIMENT]    │
│ ─────────────────────────────────────────                       │
│  IF prior frame's final palette is available                     │
│  AND sampled nearest-color error against it is low               │
│  THEN reuse that palette + LUT, no rebuild                       │
│  COST: small sample scan, then O(1) per pixel via LUT            │
│  HITS: long stable sequences (looping anims, talking heads)      │
│  CAVEAT: if ever landed, the custom / no-imagequant path would   │
│          need to serialize the chain it forms                    │
└─────────────────────────┬───────────────────────────────────────┘
                          │ miss / unavailable
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ TIER 3  ─  SEEDED + ZERO REFINE                  [EXPERIMENT]   │
│ ─────────────────────────────────────────                       │
│  IF source palette has strong coverage                           │
│      (deduped seed count ≥ 240)                                  │
│  AND sampled distortion against seeded palette is low            │
│  THEN skip k-means entirely, go straight to remap                │
│  COST: dedup + sampled error check, then final remap             │
│  HITS: cartoons / indexed sources with full palettes             │
│  STATUS: evaluated + reverted; not landed by default             │
│  SEE: df81dfa                                                    │
└─────────────────────────┬───────────────────────────────────────┘
                          │ miss
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ TIER 4  ─  SEEDED + LIGHT REFINE                  [COMMITTED]   │
│ ─────────────────────────────────────────                       │
│  Source palette seeds + weighted unique-color k-means            │
│  Iteration count from heuristic:                                 │
│      seed ≤192  →  1 pass                                        │
│      seed >192  →  3 passes                                      │
│  COST: refine on unique colors only, AVX2 nearest-color          │
│  HITS: photo-like / drifted-palette content                      │
│  SEE: 7d76c5f                                                    │
└─────────────────────────┬───────────────────────────────────────┘
                          │ no seeds available
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│ TIER 5  ─  FULL WU + FILL + REFINE                [COMMITTED]   │
│ ─────────────────────────────────────────                       │
│  Wu histogram → split → maybe farthest-point fill → k-means      │
│  COST: highest                                                   │
│  HITS: rare; mostly degenerate input or worst-case content       │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼
        ╔═════════════════════════════════════╗
        ║       DITHER DECISION GATE           ║
        ╚═════════════════╤═══════════════════╝
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│  Cheap sample: quantize N random pixels with chosen palette      │
│  Measure sampled distortion + variance                           │
│                                                                  │
│  • distortion ≈ 0          → no dither                           │
│  • low distortion          → ordered Bayer (cheap, LZW-friendly) │
│  • high distortion         → Floyd-Steinberg serpentine          │
│  • quality knob can also  ─→ force one of the above              │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│                  FINAL REMAP / INDEX EMIT                        │
│  • AVX2 nearest-color on x86_64                  [COMMITTED]    │
│  • NEON nearest-color on aarch64                 [TODO  vj7y]    │
│  • scalar fallback elsewhere                                     │
│  • respect tier's output if it already produced indices          │
└─────────────────────────┬───────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│        TRANSPARENT INDEX REMAP + WRITE FRAME                     │
│  • Prefer index 0 (LZW friendlier)                               │
│  • Avoid sacrificing a live opaque color when palette full       │
└─────────────────────────────────────────────────────────────────┘
```

## Tier cost table

| tier | when it wins | typical CPU shape |
|---|---|---|
| 0 LUT | LUT-stable inputs, default CLI pipeline | tiny per frame |
| 1 exact256 | indexed sources, pixel art, voyager-class | ~one hash pass |
| 2 temporal reuse | long stable animations | sample scan + LUT (proposed) |
| 3 seeded no-refine | cartoon / strong indexed palette | dedup + sample (evaluated/reverted) |
| 4 seeded light refine | photo-like, drifted palette | small k-means on unique colors |
| 5 full Wu+fill+refine | rare worst case | largest |

## Content-class mapping

| content | likely tier |
|---|---|
| Pixel art / UI | 1 |
| Simple cartoon | 1 or 3 |
| Voyager-style indexed | 1 (after resize) |
| Animated cartoon (256-color palette) | 3 or 4 |
| Photo (true continuous tone) | 4 |
| Adversarial / extreme | 5 |

## Roadmap (priority order)
1. Tier 3 — seeded zero-refine shortcut (evaluated/reverted)
2. Dither decision gate — sampled error-based
3. Tier 2 — temporal palette reuse (design / experiment only)
4. Region-aware work allocation (speculative)
5. NEON nearest-color on aarch64

## Implementation note

Tier 2 remains a proposal. The current encoder keeps the custom / no-imagequant path parallel; only a future landed Tier 2 would introduce serialization.

## What we explicitly DO NOT plan to do
- big new quantizer algorithms
- transform-domain tricks
- aggressive bigger LUTs (already tried and rejected)
- threading model changes (rayon stays; only Tier 2 affects this)

## Summary
For every frame, ask "what is the cheapest tier that produces acceptable output?" and stop there. That is the encoder mindset translated into GIF land.

## Cross-links
- Experiment log: [docs/research/CUSTOM_QUANTIZER_EXPERIMENT_LOG.md](CUSTOM_QUANTIZER_EXPERIMENT_LOG.md)
- Commit refs: `9939ffd`, `df81dfa`, `7d76c5f`, `1284282`
