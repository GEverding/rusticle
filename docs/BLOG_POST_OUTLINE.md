# Blog Post Outline

## 1) Working title ideas
- Butteraugli Caught What PSNR Missed
- Why We Stopped Tuning and Fixed the Representation
- The Corrected Default Path Wins for the Right Reason
- Voyager Was a Narrow Win, Not a New Default
- From Heuristics to Guardrails: What the GIF Investigation Changed

## 2) Thesis
Butteraugli exposed quality regressions that PSNR/SSIM underweighted, and the real breakthrough was not more tuning but fixing correctness and representation. The corrected default path is now the product direction; adaptive/two-path remains a research track, and the voyager-class win is real but narrow and representation-specific.

## 3) Recommended narrative arc
1. Start with the misleading comfort of PSNR/SSIM.
2. Show Butteraugli surfacing the regressions.
3. Explain the correctness bugs that mattered more than tuning.
4. Contrast the corrected default path with adaptive/two-path and voyager-class experiments.
5. Close on the corpus-quality pass: corrected default is broadly competitive, not just locally good.

## 4) Section-by-section outline
1. **The metrics that lied by omission** — why BA changed the conversation.
2. **The bugfix wave** — disposal, subframe, O3 semantics, and quality-state fixes.
3. **What improved after the fixes** — holdout offenders and corpus-quality results.
4. **Voyager-class study** — opaque bbox patches beat transparent sparse patches for that class.
5. **Why adaptive/two-path stays research** — useful, but not yet the best mainline tradeoff.
6. **Product decision** — corrected default path ships; future work stays bounded.

## 5) Key evidence/examples to cite
- Butteraugli findings: regressions PSNR/SSIM underweighted; transparent/category outliers.
- Correctness fixes: disposal-aware optimization, subframe reference-state, O3 semantic fix, quality fallback fix.
- Voyager corrected study:
  - `rusticle_default` vs `gifsicle_baseline` vs `opaque_bbox_global/local`
  - transparent bbox local is non-viable.
- Corpus-quality outliers: 149 multi-frame files evaluated; worst rusticle BA 7.60; max rusticle-worse-than-gifsicle delta +1.09.
- Adaptive telemetry: 87.1% opaque-delta/global-palette; 100% global palette reuse; current-path fallback still default.

## 6) Claims we can safely make
- Butteraugli found perceptual issues that PSNR/SSIM missed.
- Semantic/correctness bugs were the primary blockers early on.
- The corrected default path is the current product direction.
- Adaptive/two-path remains research, not mainline.
- Voyager-class wins are narrow and representation-specific.
- The corrected default path is broadly competitive on the corpus-quality run.

## 7) Claims to avoid / not overstate
- Do not claim the adaptive path is ready for default rollout.
- Do not generalize voyager results to all GIFs.
- Do not say the corpus proves global optimality; it shows competitiveness.
- Do not frame BA improvements as universal if size/runtime tradeoffs differ.
- Do not imply transparency is always bad; it is bad for the voyager class studied.

## 8) Suggested GIF embeds with why each matters
- `test_gifs/benchmark_suite/cartoon_02.gif` + `outputs/cartoon_02_resized.gif` — representative general-corpus example for resize quality.
- `test_gifs/benchmark_suite/photo_01.gif` + `outputs/photo_01_resized.gif` — photographic case to show non-cartoon behavior.
- `test_gifs/holdout_suite/790106_0203_voyager_58m_to_31m_reduced.gif` — the voyager-class anchor file.
- `outputs/voyager_rusticle_default.gif` — current default path, the baseline under critique.
- `outputs/voyager_gifsicle_baseline.gif` — external baseline for comparison.
- `outputs/voyager_opaque_bbox_global.gif` — best-bytes voyager candidate.
- `outputs/voyager_opaque_bbox_local.gif` — best-quality voyager candidate.

## 9) Ending options
- **Technical:** corrected default ships; adaptive/two-path stays behind the research gate.
- **Product:** quality guardrails changed the roadmap more than raw tuning did.
- **Reflective:** the lesson was to respect representation and semantics before chasing heuristics.
