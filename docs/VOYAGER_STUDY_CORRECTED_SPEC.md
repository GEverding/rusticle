# Corrected Voyager-Class Representation Study Spec

## Study Goal

After resizing a voyager-class GIF to a fixed target dimension, what representation strategy best balances encoded output size, visual quality, and runtime?

Specifically: does the choice of **patch semantics** (opaque-only vs. transparent-aware) and **palette policy** (global vs. local) materially affect the encoded byte count and quality tradeoff for this narrow corpus class?

---

## Why Prior Study Was Invalid

The earlier voyager representation study (`VOYAGER_REPR_IMPLEMENTATION.md`) produced invalid evidence for decision-making:

### Flaw 1: Proxy Bytes, Not Encoded Bytes
- **What was measured**: Palette size + index buffer size (raw palette + index array in memory)
- **What should be measured**: Actual GIF-encoded byte count after LZW compression
- **Why it matters**: LZW compression is highly sensitive to frame structure, palette locality, and pixel patterns. A candidate with fewer raw bytes may encode to *more* bytes, and vice versa.
- **Consequence**: Byte comparisons from that study cannot be used as evidence for choosing a representation.

### Flaw 2: Gifsicle Baseline Dimension Mismatch
- **What happened**: At least one key comparison used gifsicle output at a different target size than the rusticle candidates.
- **Why it matters**: Dimension changes affect both visual quality and encoded size. Comparing a 640×480 candidate against a 600×450 gifsicle output is not apples-to-apples.
- **Consequence**: The baseline comparison is invalid; we cannot claim rusticle outperforms or underperforms gifsicle without matching dimensions exactly.

### Decision
**Do not use prior study byte comparisons as encoded-byte evidence.** This corrected study starts fresh with actual GIF bytes and strict dimension matching.

---

## Voyager-Class Inclusion Criteria

The voyager-class is a narrow, structural subset of GIFs. Inclusion requires **all** of the following:

1. **No Transparent GCEs**
   - No frame has a Graphic Control Extension with transparency enabled
   - Simplifies palette and disposal semantics
   - Reduces edge cases in patch reconstruction

2. **Mostly None/Keep Disposal**
   - Disposal method is `None` or `Keep` for ≥90% of frames
   - Indicates stable, compositing-friendly structure
   - Excludes restore-to-background and restore-to-previous patterns

3. **Many Offset Subframes**
   - ≥50% of frames have non-zero left/top offset (not full-canvas)
   - Indicates spatial locality and potential for bounding-box optimization
   - Excludes full-frame-only sequences

4. **Stable/Global Palette Tendencies**
   - Color distribution is relatively consistent across frames
   - Suggests a single global palette may be viable
   - Excludes sequences with wildly different color sets per frame

5. **Narrow Corpus Only**
   - Study corpus is small and explicit (3–6 files)
   - Each file's inclusion rationale is documented
   - No generic adaptive chooser in the loop; no broad holdout set

---

## Fixed Target Geometry Rule

**All candidates and baselines must use the exact same target size.**

- Measure target dimensions before study begins
- Apply the same resize operation to all candidates
- No aspect-ratio-preserving shortcuts unless applied identically to all tools
- If a tool cannot match the exact target, mark it non-viable and document why
- Record target dimensions in the study results

---

## Candidate Matrix

### Candidate 1: Current Rusticle Default
- **Representation**: Full-frame, sequence-global palette, full-frame quantization
- **Rationale**: Establishes current baseline; control path from `VOYAGER_REPR_IMPLEMENTATION.md`
- **Expected viability**: High (already implemented and tested)

### Candidate 2: Gifsicle Baseline
- **Representation**: Gifsicle's default encoding at the exact target dimensions
- **Rationale**: Industry standard; apples-to-apples comparison with strict dimension matching
- **Expected viability**: High (external tool, well-tested)
- **Note**: Must verify gifsicle output dimensions match target exactly

### Candidate 3: Exact Opaque Bbox + Derived Global Palette
- **Representation**: Reconstruct opaque bounding box per frame; derive single global palette from opaque pixels only
- **Rationale**: Isolates the effect of sparse patch semantics (opaque-only) with global palette policy
- **Expected viability**: Medium (requires bbox reconstruction; may fail on frames with no opaque pixels)

### Candidate 4: Exact Opaque Bbox + Local Palettes
- **Representation**: Reconstruct opaque bounding box per frame; derive a local palette per frame from opaque pixels only
- **Rationale**: Isolates the effect of sparse patch semantics (opaque-only) with local palette policy
- **Expected viability**: Medium (requires per-frame palette derivation; may increase encoded size due to palette overhead)

### Candidate 5: Exact Bbox + Transparent Unchanged Pixels + Local Palettes
- **Representation**: Reconstruct exact bounding box (opaque + transparent unchanged); derive local palette per frame; mark unchanged transparent pixels as transparent in GCE
- **Rationale**: Isolates the effect of transparent-aware patch semantics with local palette policy
- **Expected viability**: Low–Medium (most complex; requires tracking unchanged pixels; may fail if transparent pixels dominate)

### Candidate 6 (Diagnostic, Not Presumed Winner): Exact Opaque Bbox + Source Global Palette Reuse
- **Representation**: Reconstruct opaque bounding box per frame; reuse the original GIF's global palette if present, or derive one if absent
- **Rationale**: Diagnostic only—tests whether source palette reuse is viable; not a presumed winner
- **Expected viability**: Low (source palette may not cover opaque pixels well; included only to rule it in or out)

---

## Metrics

For each candidate on each file in the corpus:

| Metric | Unit | Purpose |
|--------|------|---------|
| **Avg Butteraugli (BA)** | BA units | Visual quality loss vs. original resized frame |
| **Worst BA** | BA units | Worst single-frame quality (identifies outliers) |
| **Output Bytes** | bytes | Actual GIF file size after encoding |
| **Runtime** | ms | Wall-clock time to resize + encode |
| **Viability** | pass/fail + reason | Did the candidate encode successfully? If not, why? |

---

## Study Rules

1. **Actual Bytes Only**
   - Measure GIF file size on disk, not proxy metrics
   - Use the same encoder (rusticle's GIF encoder) for all rusticle candidates
   - Use gifsicle's encoder for the gifsicle baseline

2. **Same Target Size for All Outputs**
   - Verify all outputs are exactly the same dimensions
   - If a candidate cannot match the target, mark it non-viable

3. **No Generic Adaptive Chooser in the Loop**
   - Each candidate is evaluated independently
   - No fallback logic that switches between candidates mid-study
   - No per-file tuning; same parameters for all files

4. **No Broad Holdout Corpus in This Phase**
   - Study the full narrow voyager-class corpus
   - No train/test split; this is a bounded decision study, not a generalization study

---

## Stop/Go Criteria

### Go: Promote to Implementation Wave
If one candidate **clearly dominates** on the quality/bytes/runtime tradeoff:
- Consistently lower encoded bytes across the corpus (≥10% improvement)
- Comparable or better visual quality (avg BA within 5% of best)
- Acceptable runtime (no more than 2× slowest candidate)
- Viability ≥90% (fails on ≤1 file, with documented reason)

**Action**: Promote the dominant candidate to the next implementation wave (e.g., integrate into the two-path router).

### Stop: Revisit Assumptions
If no candidate dominates:
- Byte counts are within ±5% of each other
- Quality tradeoffs are unclear or contradictory
- Multiple candidates are viable but none is clearly better

**Action**: Do not code more optimizer logic. Instead:
1. Revisit the representation assumptions (e.g., is global palette viable for this class?)
2. Check whether the voyager-class definition is too broad or too narrow
3. Consider whether the corpus is representative
4. Decide whether to refine the candidate matrix or abandon the representation approach

---

## Open Questions

Keep these bounded and specific:

1. **Does sparse patch semantics (opaque-only bbox) reduce encoded bytes vs. full-frame?**
   - Hypothesis: Yes, by reducing LZW input size
   - Measured by: Candidate 3 vs. Candidate 1

2. **Does local palette policy outweigh global palette policy for this class?**
   - Hypothesis: No; global palette is sufficient for voyager-class
   - Measured by: Candidate 3 vs. Candidate 4

3. **Is transparent-aware patch reconstruction viable?**
   - Hypothesis: No; complexity outweighs benefit
   - Measured by: Candidate 5 viability and byte count vs. Candidate 4

4. **How does rusticle's best candidate compare to gifsicle?**
   - Hypothesis: Within ±10% on encoded bytes
   - Measured by: Best rusticle candidate vs. Candidate 2

5. **Is source palette reuse a viable shortcut?**
   - Hypothesis: No; source palette is too coarse
   - Measured by: Candidate 6 viability and byte count vs. Candidate 3

---

## Study Phases

### Phase 1: Corpus Selection (rusticle-047)
- Identify 3–6 voyager-class files
- Document inclusion rationale for each
- Record target dimensions
- **Output**: `docs/voyager_class_corpus.json` (see below)

### Phase 2: Candidate Implementation (rusticle-wtn)
- Implement candidates 1–6
- Mark viability (pass/fail + reason) for each candidate on each file

### Phase 3: Encoding & Measurement (rusticle-k4c)
- Encode all viable candidates to actual GIF bytes
- Collect metrics: BA, bytes, runtime
- Aggregate results by candidate

### Phase 4: Decision (this spec)
- Apply stop/go criteria
- Document recommendation
- File follow-up work if needed

---

## Guardrails

- **No hand-tuning per file**: Same parameters for all files
- **No post-hoc candidate selection**: Candidates are fixed before measurement
- **No cherry-picking metrics**: Report all metrics, even unfavorable ones
- **Explicit failure reporting**: If a candidate fails, document why and count it against viability
- **Dimension verification**: Spot-check output dimensions before analysis

---

## Corpus Selection (Phase 1 Output)

The voyager-class corpus is defined in `docs/voyager_class_corpus.json`. Key findings:

- **Corpus size**: 4 files (smaller than hoped due to strict criteria)
- **Files**: 790106_0203_voyager_58m_to_31m_reduced, pangea_animation_03, apparent_retrograde_motion_of_mars_in_2003, 8_cell_simple
- **Caveat**: No files in the available corpus have ≥50% offset subframes. All selected files are full-frame animations (0% offset). This is a significant deviation from the voyager-class definition.
- **Recommendation**: Proceed with Phase 2 using these 4 files. Document the offset subframes caveat in the final study results.

---

## Success Criteria

This spec is complete when:
1. ✓ File exists at `docs/VOYAGER_STUDY_CORRECTED_SPEC.md`
2. ✓ Candidate matrix is specific enough to implement
3. ✓ Stop/go criteria are clear and measurable
4. ✓ Voyager-class inclusion criteria are explicit
5. ✓ Target geometry rule is locked in
6. ✓ Metrics are defined and unambiguous
7. ✓ Corpus selection complete: `docs/voyager_class_corpus.json` exists with explicit rationale for each file
