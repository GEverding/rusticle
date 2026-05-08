# Quality Investigation Summary

## Why Butteraugli

- Added to catch perceptual regressions PSNR/SSIM missed.
- Used to measure whether the corrected default path stayed competitive against gifsicle on real corpora.

## What Went Wrong Initially

- **Disposal-aware optimization bug**: wrong reference state for disposal-heavy frames caused bad diffs and catastrophic quality loss.
- **Subframe reference-state bug**: lossy optimization used the wrong canvas state for cropped subframes.
- **O3 semantic bug**: lossiness was effectively hidden inside optimize(), instead of being a separate perceptual step.
- **Quality fallback bug**: invalid measurement states could silently mask failures instead of reporting them.

## What We Tried / Learned

- **Adaptive/tiered optimizer prototype**: interesting, but too complex and not rollout-ready.
- **Two-path architecture**: clearer and safer, but the current implementation was not better than the corrected default path overall.
- **Voyager-class corrected study**: the big win was representation, not heuristics; opaque bbox patches are the right fit for that narrow class.

## Corpus-Quality Result

| Metric | Result |
|---|---:|
| Multi-frame files evaluated | 149 |
| Worst rusticle BA | 7.60 |
| Max rusticle-worse-than-gifsicle delta | +1.09 |

- Many of the worst files still had rusticle beating gifsicle.
- Two very large gifsicle failures were much worse than rusticle.
- Net: the corrected default path is competitive on the evaluated corpus.

## Final Recommendation

- **Corrected default path is the current product direction.**
- **Adaptive/two-path remains research.**
- **Voyager-specific path remains a narrow future option.**
- Future work should prioritize **data quality** and a **larger corpus**, not optimizer complexity.
