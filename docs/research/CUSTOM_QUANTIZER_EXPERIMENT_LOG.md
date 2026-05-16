# Custom Quantizer Experiment Log

## 1. Title + short summary

Replacing exoquant with an MIT-compatible custom quantizer path.

The work is a speed/quality tradeoff exercise: keep the stack permissive, get closer to imagequant quality where possible, and avoid paying for quantization when the input is already effectively encoded.

Current stack state:
- default path: imagequant-backed
- no-default-features path: custom Wu-based quantizer
- exact `<=256` opaque-color bypass is committed (`9939ffd`)
- AVX2 nearest-color fast path is committed (`1284282`)
- weighted-unique k-means is still provisional / uncommitted

## 2. Goals / constraints

- MIT-only fallback path
- competitive quality with imagequant where possible
- faster than exoquant
- no unsafe unless justified and benchmarked
- targeted intrinsics are fine when the numbers support them

## 3. Baselines

Original benchmark trio: `cartoon_02`, `photo_01`, `voyager`.

Notes we have:
- custom Wu is comfortably faster than historical exoquant, but still slower than imagequant and below the target quality threshold (`rusticle-2p7k.7`)
- the benchmark numbers were taken from local runs on the original repo assets
- exact trio values are not recopied here; preserve them from the benchmark logs if we want a publishable table later

Other known baseline anchors:
- `photo_01` resize benchmark in `docs/BENCHMARKS.md`: 199ms vs 982ms for gifsicle on the resize path
- corpus-quality summary: 149 multi-frame files evaluated, worst rusticle BA 7.60, max rusticle-worse-than-gifsicle delta +1.09 (`docs/QUALITY_INVESTIGATION_SUMMARY.md`)

## 4. Experiment timeline

- Wu histogram + splitting + k-means + dithering initial replacement
  - idea: MIT-safe replacement for exoquant
  - result: worked as a first pass, but quality lagged imagequant
  - decision: keep as the fallback foundation, not the final answer

- first-pass fixes (FS signed error, rounded centroids, transparency reserve, deterministic ordering)
  - idea: remove obvious quantization mistakes and non-determinism
  - result: reduced sharp edges in the implementation
  - decision: keep

- 6-bit histogram experiment (`cdcb2ff`)
  - idea: shrink histogram cost
  - result: helped `voyager`, hurt `cartoon`
  - decision: revert

- source-palette seeding (`df81dfa`)
  - idea: seed from the source palette instead of starting cold
  - result: helped `voyager` and some indexed cases
  - decision: keep as a seed, not a hard reuse policy

- palette-space resize prototype
  - idea: resize in palette space instead of RGBA first
  - result: too slow and lower quality
  - decision: drop

- exact `<=256`-color bypass (`9939ffd`)
  - idea: skip quantization entirely when the frame already fits
  - result: huge win on voyager-like indexed cases
  - decision: keep

- AVX2 nearest-color fast path (`1284282`)
  - idea: accelerate the hot nearest-color search
  - result: ~1.3x on `cartoon`/`photo`, flat on `voyager`
  - decision: keep

- weighted unique-color k-means experiment (current, uncommitted)
  - idea: bias clustering by frequency instead of treating all colors equally
  - result: speed win so far; quality/retain-vs-revert decision still provisional
  - decision: not final yet

- dither frontier (ordered / no-dither)
  - idea: see if simpler dither policies improve speed without visible harm
  - result: ordered dither regressed badly; no-dither was not a clean win
  - decision: not kept

## 5. What profiling taught us

- pre-AVX2: `photo_01` was dominated by `refine_palette` and FS dither
- post-AVX2: nearest-color itself dominates; roughly ~68% from `refine_palette` and ~32% from the final remap path
- `voyager` mostly exits through exact `<=256` now

## 6. Current takeaways

- the fastest quantizer is the one you don’t run
- indexed GIF structure matters
- source palette is useful as a seed, not necessarily as a hard reuse policy
- algorithmic pass count mattered more than generic micro-optimizations until the obvious waste was removed

## 7. Open experiments

- final remap LUT
- whether weighted unique-color k-means should be kept
- ARM64/NEON nearest-color path

## 8. Appendix / benchmark tables

| Stack / note | Result |
|---|---|
| historical exoquant baseline | reference only; exact trio values live in local benchmark logs |
| custom Wu (`rusticle-2p7k.7`) | faster than exoquant, slower than imagequant, below target quality |
| `9939ffd` exact `<=256` | major win on voyager-like indexed cases |
| `1284282` AVX2 nearest-color | ~1.3x on cartoon/photo; flat on voyager |
| `docs/QUALITY_INVESTIGATION_SUMMARY.md` | 149 multi-frame files; worst rusticle BA 7.60; max delta +1.09 |
| `docs/BENCHMARKS.md` resize anchor | `photo_01`: 199ms vs 982ms gifsicle on resize path |

Notes:
- some figures are approximate or run-specific; label them that way in the eventual blog post
- weighted-unique k-means landed; final-LUT experiment was rejected and removed from the code path
