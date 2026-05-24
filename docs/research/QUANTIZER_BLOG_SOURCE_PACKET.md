# Quantizer Blog Post — Source Packet

> **Purpose:** Standalone source material for a future blog post about the Rust quantizer optimization work in rusticle. Hand this document to a writing model or use it as a structured outline. All benchmark numbers are exact. Do not alter them.

---

## 1. What the Post Should Be (and Should Not Be)

### Best framing

This post is about:

- Optimizing under real constraints
- Building a practical, permissive (MIT-friendly) Rust quantizer path
- Measuring honestly across a broad corpus
- Keeping only the wins that survive broader benchmarks

### It should **not** be

- "Rust is fast"
- "Look at all these tricks"
- "We made everything better"

### The actual story

> We replaced a constrained fallback quantizer path, found several real wins, and then reverted three plausible optimizations because the broader corpus said they were not worth keeping.

That is the interesting post.

---

## 2. Core Thesis

Primary:

> The valuable outcome wasn't just speed.
> It was building a benchmark-driven process that let us keep the real wins, reject the fake ones, and leave the encoder in a simpler, more defensible state.

Secondary:

> The fastest quantizer is often the one you don't run.

The secondary thesis is backed directly by the exact-`<=256` color bypass and the general direction of the work.

---

## 3. Suggested Titles

| # | Title |
|---|-------|
| 1 | **Building a MIT-only GIF quantizer in Rust: wins, regressions, reversions** |
| 2 | **How I optimized a Rust GIF quantizer without lying to myself** |
| 3 | **What survived a real optimization campaign in Rust** |
| 4 | **Benchmark-driven quantizer work in Rust: what we kept and what we reverted** |
| 5 | **Replacing a GIF quantizer in Rust under real constraints** |

---

## 4. Audience

**Primary:**
- Rust performance engineers
- Systems/perf-minded developers
- Library authors
- People who care about "how to optimize honestly"

**Secondary:**
- Image processing / codec-adjacent developers
- People interested in SIMD and profiling workflows

---

## 5. Tone Guidance

Write it:

- Direct
- Concrete
- Mildly skeptical
- Low-hype
- Numbers first
- Clear about what was reverted

Avoid:

- Chest-thumping
- Vague claims like "significantly faster" unless followed immediately by numbers
- Treating negative results like embarrassment

**The reversions are a strength, not a weakness.**

---

## 6. Public-Facing Narrative Structure

### Act 1 — Constraint

- Wanted a better no-imagequant path
- Needed a practical permissive/MIT-friendly fallback
- Existing fallback path was not where it needed to be

### Act 2 — Real Wins

- Custom Wu fallback quantizer
- Source-palette seeding
- Exact `<=256` color bypass
- AVX2 nearest-color
- Weighted-unique refinement

### Act 3 — Tempting Ideas That Didn't Survive

- Seeded zero-refine shortcut
- Sampled dither gate
- Temporal palette reuse

### Act 4 — Outcome

- Stable retained stack is good
- Repo is simpler than the "everything stays" version would have been
- ARM validation remains the real outstanding item

---

## 7. What Actually Landed and Stayed

### Retained stack (benchmark-supported)

- Custom Wu no-imagequant fallback
- Source-palette seeding
- Exact `<=256` color bypass
- AVX2 nearest-color on x86
- Weighted-unique refinement
- Simple dither policy:
  - Ordered dither for `quality <= 70`
  - Floyd-Steinberg otherwise

### Landed but not fully validated

- ARM64 NEON nearest-color path
- Needs validation/benchmark on Mac/ARM hardware (see §13)

---

## 8. What Was Tried and Then Reverted

These belong in the post. They are the honest part.

### `rusticle-zkvm.1` — Seeded Zero-Refine Shortcut

**Idea:** If a seeded palette is already strong enough, skip k-means refinement entirely.

**Outcome:**
- Loose gate got speed wins but introduced quality regressions
- Tightened gate preserved quality but became too marginal to justify default complexity
- Reverted as default behavior

### `rusticle-zkvm.2` — Sampled Dither Dispatch

**Idea:** Choose no-dither / ordered / Floyd-Steinberg from sampled error instead of a simple fixed quality split.

**Outcome:**
- Trio of test files looked okay
- Broader suite exposed real regressions, especially on pixel-art and many-frame content
- Reverted

### `rusticle-zkvm.3` — Temporal Palette Reuse

**Idea:** Reuse frame N-1's final palette/LUT for frame N when the frames are close enough.

**Outcome:**
- Required sequentializing the no-imagequant path
- Broad x86 results were very bad
- Reverted immediately

---

## 9. Commit / Story Timeline

Use these as internal anchors. Not all hashes need to appear in final prose, but the sequence should be preserved.

### Kept stack

| Commit | Description |
|--------|-------------|
| `cdcb2ff` | Replace exoquant fallback with custom Wu quantizer |
| `df81dfa` | Source-palette seeding |
| `9939ffd` | Exact `<=256` bypass |
| `6a55113` | Cleanup / remove temp override |
| `1284282` | AVX2 nearest-color fast path |
| `7d76c5f` | Weighted unique-color refinement |

### Reverted experiments / documentation

| Commit | Description |
|--------|-------------|
| `d83e3da` | Revert sampled dither dispatch after corpus regressions |
| `4edc4a7` | Record reverted temporal reuse experiment in docs |
| `b38b53c` | Revert seeded zero-refine default path |

A temporary "wip" commit existed while the experiments were stacked together, but the enduring result is the reverted, cleaner state.

---

## 10. Benchmark Data

> **Caveat:** All numbers are from local runs, not a published lab setup. Say that explicitly in the post.

---

### A. Retained Stack — Trio Results After `7d76c5f`

These are the "what actually stayed" numbers.

| File | Timing | Quality |
|------|-------:|---------|
| `cartoon_02` | `65.66ms → 62.68ms` | PSNR `40.36`, SSIM `0.9991` |
| `photo_01` | `213.41ms → 188.96ms` | PSNR `40.35`, SSIM `0.9990` |
| `voyager` | `94.18ms → 96.98ms` | PSNR `41.53`, SSIM `0.9992` |

**Interpretation:**
- cartoon and photo improved
- voyager's story is not micro-tuning; it's mostly the exact-color bypass and indexed-structure handling

---

### B. Seeded Zero-Refine (`zkvm.1`)

#### Loose gate — fast but wrong

Policy: sample limit `256`, mean SSE `<= 32`

| File | Encode | Quality delta |
|------|-------:|---------------|
| `cartoon_02` | `97ms → 76ms` | PSNR `40.60 → 40.05`, BA `2.47 → 2.58` |
| `photo_01` | `211ms → 133ms` | PSNR `40.32 → 39.19`, BA `2.57 → 2.80` |
| `voyager` | `54ms → 49ms` | unchanged quality |

**Interpretation:** Attractive speed. Unacceptable quality drop on real content.

---

#### Tight gate — safe but too small

Policy: high quality only (`>= 71`), non-empty seeded only, near-cap seeds (`max_colors - 16`), sample limit `1024`, mean SSE `<= 8`, max SSE `<= 64`

| File | Encode | Quality delta |
|------|-------:|---------------|
| `cartoon_02` | `97ms → 93ms` | unchanged |
| `photo_01` | `211ms → 222ms` | unchanged |
| `voyager` | `54ms → 54ms` | unchanged |

---

#### Broader x86 suite recheck — 30 files, `.1` vs `7d76c5f`

| Metric | Value |
|--------|------:|
| Total wall time delta | **+40.96 ms** over suite |
| Current total | **12,375.31 ms** |
| Baseline total | **12,334.35 ms** |
| Delta % | **+0.33%** |
| Avg per-file wall delta | **+1.37 ms** |
| Avg CLI total delta | **+3.46 ms** |
| Avg BA delta | **+0.001** |
| Avg PSNR delta | **−0.0117 dB** |
| Avg SSIM delta | **~0** |

**Category breakdown:**

| Category | Runtime delta | Notes |
|----------|-------------:|-------|
| photographic | **−28.61 ms** avg | faster |
| large | **+31.48 ms** | slower |
| many_frames | **+10.16 ms** | slower |
| pixel_art | **+8.79 ms** | slower |
| transparent | **+3.30 ms** | slower |
| cartoon | **+2.30 ms** | slower |
| simple | **−2.19 ms** | slightly faster |

**Outliers:**
- Biggest regression: `large_dims_02` at **+85.74 ms**
- Biggest improvement: `photo_04` at **−141.13 ms**

**Interpretation:** Effectively neutral. Maybe a slight niche win on photographic content. Not enough to justify default complexity. Reverted.

---

### C. Sampled Dither Dispatch (`zkvm.2`)

#### Broader x86 suite vs `7d76c5f`

| Metric | Value |
|--------|------:|
| Runtime delta | **+0.10%** |
| Avg BA delta | **+0.072** |
| Avg PSNR delta | **−0.426 dB** |
| Avg SSIM delta | **−0.000097** |

**Category pain:**

| Category | BA delta | PSNR delta | SSIM delta |
|----------|:--------:|:----------:|:----------:|
| pixel_art avg | **+0.37** | **−2.34 dB** | — |
| many_frames avg | **+0.16** | **−0.72 dB** | **−0.000625** |

**Worst file regressions:**

| File | BA delta | PSNR delta |
|------|:--------:|:----------:|
| `pixel_art_02` | **+1.48** | **−9.36 dB** |
| `many_frames_01` | **+0.64** | **−2.89 dB** |

**Interpretation:** The trio was misleading. Broader corpus exposed real quality regressions. Reverted.

---

### D. Temporal Palette Reuse (`zkvm.3`)

Baseline: compared against pre-`zkvm.3` state (`d83e3da`).

#### 30-file suite result

| Metric | Value |
|--------|------:|
| Avg wall runtime delta | **+636.81 ms/file** |
| Avg BA delta | **+0.3787** |
| Avg PSNR delta | **−1.0333 dB** |
| Avg SSIM delta | **−0.0001233** |
| Avg output bytes delta | **−24,089 bytes** |

**Category breakdown:**

| Category | Runtime delta | BA delta |
|----------|-------------:|:--------:|
| many_frames | **+871.89 ms** | **+0.5675** |
| cartoon | **+657.51 ms** | **+0.1960** |
| photographic | **−325.54 ms** (one strong outlier) | **+0.3040** |
| transparent | **+803.43 ms** | **+0.2250** |
| pixel_art | **+1,594.40 ms** | **+0.56** |

**Top runtime regressions:**

| File | Runtime delta |
|------|-------------:|
| `pixel_art_03` | **+3,520.39 ms** |
| `pixel_art_04` | **+2,032.86 ms** |
| `small_simple_02` | **+2,026.26 ms** |
| `many_frames_04` | **+2,013.30 ms** |
| `photo_05` | **+1,204.92 ms** |

**Interpretation:** The sequentialization cost was brutal. The reuse signal did not pay for itself. Reverted immediately.

---

## 11. Profiling and Tooling Details

### Linux perf

Blocked by kernel policy: `perf_event_paranoid = 4`.

This is a real-world detail worth including in the post — it's a common constraint on hardened Linux systems.

### gprofng

Used as fallback. Useful, but later release binaries were stripped enough that symbol-level hotspot inspection was limited.

### Hotspot data — `gprofng` after AVX2 on `photo_01`

| Symbol | Exclusive CPU |
|--------|-------------:|
| `rusticle::quantize::kmeans::nearest_color_avx2` | **77.36%** |

Caller split:

| Caller | Share |
|--------|------:|
| `refine_palette` | **68.29%** |
| final quantization/remap path | **31.71%** |

**Interpretation:** AVX2 was aimed at the right hotspot. After obvious waste is removed, nearest-color dominates.

---

### Stage timing proxy (release builds, weaker signal but still useful)

| File | Decode | Process | Encode |
|------|-------:|--------:|-------:|
| `cartoon_02` | `28ms` | `7ms` | `26ms` |
| `photo_01` | `111ms` | `16ms` | `55ms` |
| `pixel_art_02` | `25ms` | `11ms` | `71ms` |

**Interpretation:** Encode stage still matters a lot, especially on pixel-art style content.

---

## 12. Secondary Details Worth Including

### Native build experiment

- `RUSTFLAGS="-C target-cpu=native"` gave only modest local gains
- Roughly **4–6%** on cartoon/photo
- No meaningful voyager win
- Not kept as repo policy

### Ideas explicitly rejected earlier

These are good "we tried it, it lost" material:

- **Palette-space resize prototype** — too slow, lower quality
- **Final remap LUT experiment** — slower, changed output

Including these makes the post more credible. They show the filter was working before the formal benchmark suite was even involved.

---

## 13. Final Outcome

### Landed and trusted

The conservative, benchmark-supported stack through `7d76c5f`:

1. Custom Wu fallback
2. Source-palette seeding
3. Exact `<=256` color bypass
4. AVX2 nearest-color
5. Weighted-unique refinement

### Landed but still awaiting ARM validation

- NEON nearest-color path
- `rusticle-vj7y` remains the real outstanding validation task
- Needs benchmark on Mac/ARM hardware

### Evaluated and reverted

- `.1` seeded zero-refine
- `.2` sampled dither gate
- `.3` temporal palette reuse

---

## 14. How to Write the Conclusion

Best conclusion shape:

> The satisfying part wasn't that every optimization idea worked.
> It was that the process let us keep the few that clearly did, and confidently delete the ones that didn't.

Then explicitly name the **kept wins**:
- Exact-color bypass
- AVX2 nearest-color
- Weighted-unique refinement
- Source-palette seeding
- Custom Wu fallback

Then name the **deletions**:
- Zero-refine
- Dither dispatch
- Temporal reuse

That contrast is the whole post.

---

## 15. What Not to Claim

Do **not** claim:

- That the later ladder experiments were wins
- That the final stack is universally faster than imagequant
- That temporal reuse is promising (unless heavily caveated)
- That the broader suite proved huge gains

Do say:

- The big, durable wins were the earlier practical ones
- The later "clever" experiments mostly lost
- That is exactly why broad-suite measurement mattered

---

## 16. Suggested Section Headings

A strong outline for the actual post:

1. **The constraint: I needed a permissive fallback quantizer**
2. **The first real wins came from avoiding work**
3. **SIMD helped, but only after the algorithm stopped wasting effort**
4. **Three plausible optimizations that failed broader benchmarks**
5. **Why the boring final stack is the right one**
6. **What still needs ARM validation**
7. **What this changed about how I optimize Rust code**

---

## 17. Source Files to Cite or Mine

### Primary source files

- `crates/rusticle/src/quantize/mod.rs`
- `crates/rusticle/src/quantize/wu.rs`
- `crates/rusticle/src/quantize/kmeans.rs`
- `crates/rusticle/src/quantize/dither.rs`
- `crates/rusticle/src/encode.rs`

### Docs / experiment sources

- `docs/research/CUSTOM_QUANTIZER_EXPERIMENT_LOG.md`
- `docs/research/QUANTIZER_DECISION_LADDER.md`

### Benchmark inputs / scripts

- `test_gifs/benchmark_suite/manifest.json`
- `scripts/download_test_gifs.py`

### Commit history (optional historical references)

| Commit | Role |
|--------|------|
| `cdcb2ff` | Start of kept stack |
| `df81dfa` | Source-palette seeding |
| `9939ffd` | Exact bypass |
| `1284282` | AVX2 |
| `7d76c5f` | End of kept stack |
| `d83e3da` | Revert dither dispatch |
| `4edc4a7` | Document temporal reuse revert |
| `b38b53c` | Revert zero-refine |

---

## 18. Suggested Prompt for Another Writing Model

Use something like this verbatim:

---

> Write a blog post in Garrett Everding's voice from the following source packet.
> Tone: direct, skeptical, concrete, low-hype, systems-minded.
> Audience: Rust performance and library engineering readers.
>
> The post should not be a victory lap. The central story is that several optimization ideas were benchmarked honestly and then reverted. The final message should be that the process mattered as much as the speedups.
>
> Preserve all benchmark numbers exactly as given.
> Do not overclaim.
> Do not imply that reverted experiments were wins.
> Emphasize the kept durable wins: custom Wu fallback, source-palette seeding, exact <=256 bypass, AVX2 nearest-color, weighted-unique refinement.
> Emphasize that zero-refine, sampled dither dispatch, and temporal palette reuse were all evaluated and then removed from default behavior.
>
> Structure it as an optimization story under constraints, not as generic Rust advocacy.
>
> Include enough technical detail to satisfy experienced readers, but keep the narrative moving.

---

*End of source packet.*
