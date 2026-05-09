# Writer Handoff Note

## Recommended angle / thesis
We did not get here by tuning harder. The real story is that Butteraugli exposed quality problems PSNR/SSIM hid, and the meaningful win came from fixing correctness and representation. The corrected default path is the product answer; adaptive/two-path is still research. The Voyager result is real, but it only applies to a narrow class.

## Strongest evidence to lean on
- Butteraugli found regressions that PSNR/SSIM underweighted.
- Disposal/subframe/quality-state fixes removed the worst failures.
- Corpus results show the corrected default path is broadly competitive.
- Voyager-class wins favor opaque bbox/global palette reuse, but only for that class.

## What not to overclaim
- Don’t say adaptive/two-path is ready for default rollout.
- Don’t generalize Voyager results to all GIFs.
- Don’t claim global optimality from the corpus.
- Don’t imply transparency is always bad; it was bad in the studied Voyager-like case.

## Suggested reading order
1. `docs/QUALITY_INVESTIGATION_SUMMARY.md`
2. `docs/BLOG_POST_OUTLINE.md`
3. `docs/research/TUNING_RECOMMENDATION.md`
4. `docs/research/BUTTERAUGLI_TUNING_JOURNAL.md`
5. `docs/research/ADAPTIVE_ENCODER_RESULTS.md`
6. `docs/research/VOYAGER_STUDY_CORRECTED_SPEC.md`
7. `docs/research/TWO_PATH_OPTIMIZER_DECISION.md`

## Suggested GIF comparison order
1. `test_gifs/benchmark_suite/cartoon_02.gif`
2. `test_gifs/benchmark_suite/photo_01.gif`
3. `test_gifs/holdout_suite/790106_0203_voyager_58m_to_31m_reduced.gif`
4. `outputs/voyager_rusticle_default.gif`
5. `outputs/voyager_gifsicle_baseline.gif`
6. `outputs/voyager_opaque_bbox_global.gif`
7. `outputs/voyager_opaque_bbox_local.gif`

## If you only remember one thing
The lesson is not “better tuning wins.” It’s that semantic correctness and representation choices mattered more than the optimizer knobs, and once those were fixed, the corrected default path became the right story to tell. Butteraugli was the tool that made that visible. The blog should sound like an engineering postmortem with measured conclusions, not a victory lap.

## Tone guidance
Engineering-first, honest, and no hype.
