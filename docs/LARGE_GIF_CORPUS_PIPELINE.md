# Large GIF Corpus Pipeline (≥512 GIFs)

## Overview

This document specifies the reproducible pipeline for acquiring, deduplicating, extracting metadata, and structurally classifying a large corpus of ≥512 diverse GIFs to drive future optimizer decisions in rusticle.

**Status**: Design specification. Implementation in `rusticle-961`.

---

## 1. Target Corpus Size & Composition

### Primary Goal
- **Minimum 512 successfully acquired and validated GIFs**
- Stratified by structural characteristics to represent real-world diversity
- Reproducible: same seed + source list → same corpus

### Composition Goals

The corpus should include balanced representation across:

| Dimension | Categories | Target Distribution |
|-----------|-----------|---------------------|
| **Transparency** | Heavy (>50% transparent pixels) | 20–25% |
| | Light (<10% transparent) | 20–25% |
| | None (fully opaque) | 20–25% |
| | Mixed (10–50%) | 20–25% |
| **Disposal** | Restore-to-background heavy (>50% frames) | 15–20% |
| | Restore-to-previous heavy (>50% frames) | 15–20% |
| | None/Unspecified (>50% frames) | 15–20% |
| | Mixed disposal methods | 15–20% |
| **Frame Count** | Single frame | 10–15% |
| | 2–5 frames | 20–25% |
| | 6–20 frames | 25–30% |
| | 21+ frames | 20–25% |
| **Dimensions** | Small (<256px in either dimension) | 15–20% |
| | Medium (256–640px) | 30–35% |
| | Large (641–1280px) | 25–30% |
| | Very large (>1280px) | 10–15% |
| **Palette Type** | Global palette only | 30–35% |
| | Local palettes (per-frame) | 20–25% |
| | Mixed global + local | 20–25% |
| | Grayscale-like | 10–15% |
| **Content Type** | Cartoon/flat-color (low color variance) | 20–25% |
| | Photographic/noisy (high color variance) | 20–25% |
| | Pixel art (structured, small palette) | 15–20% |
| | Voyager-like (opaque delta, minimal disposal) | 15–20% |
| | Text/UI (sharp edges, limited colors) | 10–15% |

**Rationale**: These dimensions capture the structural variety that affects encoding efficiency, quantization difficulty, and optimization strategy selection.

---

## 2. Source Selection & Licensing

### Approved Sources

1. **Giphy (CC0 / Public Domain)**
   - API: `https://api.giphy.com/v1/gifs/search?q=<query>&api_key=<key>&limit=50`
   - License: Most content is CC0 or public domain
   - Advantage: Diverse, large-scale, well-categorized
   - Limitation: Rate-limited; requires API key (free tier: 43 requests/hour)

2. **Tenor (CC0 / Public Domain)**
   - API: `https://api.tenor.com/v2/search?q=<query>&key=<key>&limit=50`
   - License: Most content is CC0
   - Advantage: Similar to Giphy, good coverage
   - Limitation: Rate-limited; requires API key

3. **Internet Archive (CC0 / Public Domain)**
   - Source: `https://archive.org/advancedsearch.php?q=filetype:gif&output=json`
   - License: Explicitly CC0 or public domain
   - Advantage: Stable, no rate limits, archival quality
   - Limitation: Slower, less curated

4. **OpenGameArt.org (CC0 / CC-BY)**
   - Source: `https://opengameart.org/` (manual curation)
   - License: Explicitly CC0 or CC-BY
   - Advantage: High-quality pixel art and game assets
   - Limitation: Smaller corpus, manual curation required

5. **Wikimedia Commons (CC0 / CC-BY)**
   - Source: `https://commons.wikimedia.org/w/api.php?action=query&list=allimages&aisort=timestamp&aidir=descending&ailimit=50&aiprop=url&format=json`
   - License: Explicitly CC0 or CC-BY
   - Advantage: Diverse, stable, well-documented
   - Limitation: Smaller GIF population

### Licensing Compliance

- **All GIFs must be CC0 or CC-BY licensed** (or explicitly public domain)
- Store source URL and license metadata in manifest
- Document any attribution requirements in corpus README
- Exclude any proprietary or unclear-license content

### Query Strategy

Use diverse search terms to maximize structural variety:

**Cartoon/Flat-color**: "cartoon", "animation", "simple", "flat", "logo"
**Photographic**: "photo", "nature", "landscape", "weather", "space"
**Pixel Art**: "pixel art", "retro", "8-bit", "sprite", "game"
**Transparency**: "transparent", "alpha", "overlay", "effect"
**Text/UI**: "text", "loading", "progress", "button", "ui"
**Voyager-like**: "minimal", "simple animation", "delta", "efficient"
**High Frame Count**: "long animation", "sequence", "movie", "clip"
**Large Dimensions**: "high resolution", "4k", "1080p", "large"

---

## 3. Deduplication Rule

### Primary: MD5 Hash

- Compute MD5 hash of raw GIF file bytes
- Store in manifest as `md5` field
- Deduplicate: keep first occurrence, discard subsequent matches
- Rationale: Fast, deterministic, sufficient for byte-level dedup

### Secondary: Perceptual Hash (Optional, Future)

- For future work: implement PHASH or similar for near-duplicate detection
- Not required for initial corpus
- Would catch re-encoded versions of same content

### Deduplication Process

1. Download GIF
2. Compute MD5 immediately
3. Check against existing manifest
4. If match found: log as duplicate, skip file write
5. If new: write file, add to manifest

---

## 4. Manifest Schema

### File Format
- **Location**: `corpus/manifest.jsonl` (JSON Lines, one entry per line)
- **Also**: `corpus/manifest.json` (full array for convenience)
- **Rationale**: JSONL allows streaming processing; JSON for tooling

### Per-GIF Entry Schema

```json
{
  "id": "corpus_001",
  "source_url": "https://media.giphy.com/media/...",
  "source_id": "giphy_abc123",
  "source_type": "giphy|tenor|archive|opengameart|wikimedia",
  "local_path": "corpus/gifs/corpus_001.gif",
  "md5": "a1b2c3d4e5f6...",
  "license": "CC0|CC-BY|public-domain",
  "license_url": "https://...",
  "acquired_at": "2026-04-21T12:34:56Z",
  "file_size_bytes": 12345,
  "download_time_ms": 234,
  "success": true,
  "error": null,
  "dimensions": {
    "width": 640,
    "height": 480
  },
  "frame_count": 12,
  "duration_ms": 1200,
  "transparency": {
    "has_transparency": true,
    "transparent_pixel_ratio": 0.35,
    "category": "heavy|light|none|mixed"
  },
  "disposal": {
    "distribution": {
      "none": 5,
      "do_not_dispose": 2,
      "restore_to_background": 3,
      "restore_to_previous": 2
    },
    "dominant_method": "restore_to_background",
    "dominant_ratio": 0.25,
    "category": "restore_bg_heavy|restore_prev_heavy|none_heavy|mixed"
  },
  "palette": {
    "has_global_palette": true,
    "global_palette_size": 256,
    "has_local_palettes": false,
    "local_palette_count": 0,
    "unique_colors_across_frames": 512,
    "category": "global_only|local_only|mixed|grayscale_like"
  },
  "subframe": {
    "offset_subframe_ratio": 0.45,
    "offset_subframe_count": 5,
    "full_frame_count": 7,
    "avg_offset_x": 12,
    "avg_offset_y": 8,
    "max_offset_x": 45,
    "max_offset_y": 32
  },
  "content_type": "cartoon|photographic|pixel_art|voyager_like|text_ui|mixed",
  "content_confidence": 0.85,
  "tags": ["animation", "simple", "flat-color"],
  "quality_metrics": {
    "avg_psnr": 28.5,
    "avg_ssim": 0.92,
    "max_frame_size_bytes": 8192,
    "avg_frame_size_bytes": 4096
  },
  "notes": "Optional human or automated notes"
}
```

### Manifest Metadata

```json
{
  "corpus_version": "1.0",
  "generated_at": "2026-04-21T12:34:56Z",
  "pipeline_version": "rusticle-961",
  "total_requested": 600,
  "total_acquired": 512,
  "total_unique": 510,
  "total_duplicates": 2,
  "total_failed": 88,
  "composition": {
    "by_transparency": {
      "heavy": 65,
      "light": 64,
      "none": 63,
      "mixed": 64
    },
    "by_disposal": {
      "restore_bg_heavy": 60,
      "restore_prev_heavy": 61,
      "none_heavy": 62,
      "mixed": 63
    },
    "by_frame_count": {
      "single": 50,
      "2_5": 120,
      "6_20": 150,
      "21_plus": 92
    },
    "by_dimensions": {
      "small": 70,
      "medium": 170,
      "large": 150,
      "very_large": 50
    },
    "by_palette_type": {
      "global_only": 170,
      "local_only": 100,
      "mixed": 100,
      "grayscale_like": 42
    },
    "by_content_type": {
      "cartoon": 120,
      "photographic": 110,
      "pixel_art": 80,
      "voyager_like": 80,
      "text_ui": 60,
      "mixed": 62
    }
  },
  "sources": {
    "giphy": 250,
    "tenor": 150,
    "archive": 80,
    "opengameart": 20,
    "wikimedia": 12
  },
  "failure_summary": {
    "network_timeout": 30,
    "invalid_gif": 25,
    "license_unclear": 15,
    "other": 18
  }
}
```

---

## 5. Structural Metadata Extraction

### Per-GIF Extraction Process

1. **Decode GIF** using `gif` crate
2. **Extract frame-level metadata**:
   - Frame dimensions, offset (x, y)
   - Disposal method
   - Delay (duration)
   - Local palette (if present)
3. **Compute transparency metrics**:
   - Count transparent pixels across all frames
   - Compute ratio relative to total pixels
   - Classify as heavy/light/none/mixed
4. **Analyze disposal distribution**:
   - Count frames by disposal method
   - Identify dominant method
   - Classify as restore_bg_heavy, restore_prev_heavy, none_heavy, or mixed
5. **Analyze palette**:
   - Check for global palette
   - Count local palettes
   - Estimate unique colors across all frames
   - Classify as global_only, local_only, mixed, or grayscale_like
6. **Compute subframe metrics**:
   - Count frames with non-zero offset
   - Compute offset subframe ratio
   - Track average/max offsets
7. **Classify content type** (heuristic):
   - Photographic: high unique color count, high variance
   - Cartoon: low unique color count, flat regions
   - Pixel art: small dimensions, structured patterns
   - Voyager-like: opaque, minimal disposal, small offsets
   - Text/UI: sharp edges, limited colors, specific aspect ratios
8. **Compute quality baseline** (optional):
   - Decode to RGBA
   - Compute PSNR/SSIM between consecutive frames
   - Store avg/max metrics

### Implementation Notes

- Use `gif` crate for decoding (already in dependencies)
- Parallelize with rayon where possible
- Cache intermediate results to avoid re-decoding
- Handle corrupted/malformed GIFs gracefully (log error, continue)

---

## 6. Category Tagging & Bucketing

### Automatic Tags

Generated from structural analysis:

- `transparency_heavy`, `transparency_light`, `transparency_none`, `transparency_mixed`
- `disposal_restore_bg`, `disposal_restore_prev`, `disposal_none`, `disposal_mixed`
- `frames_single`, `frames_few`, `frames_many`, `frames_very_many`
- `dims_small`, `dims_medium`, `dims_large`, `dims_very_large`
- `palette_global`, `palette_local`, `palette_mixed`, `palette_grayscale`
- `content_cartoon`, `content_photographic`, `content_pixel_art`, `content_voyager`, `content_text_ui`
- `subframe_heavy`, `subframe_light`, `subframe_none`

### Manual Tags (Optional)

For future curation:

- `quality_excellent`, `quality_good`, `quality_poor`
- `interesting`, `edge_case`, `stress_test`
- `real_world`, `synthetic`, `test_pattern`

### Bucketing Strategy

For future optimizer training:

```
corpus/
├── gifs/
│   ├── corpus_001.gif
│   ├── corpus_002.gif
│   └── ...
├── manifest.jsonl
├── manifest.json
├── splits/
│   ├── train.txt          # 70% of corpus (random stratified)
│   ├── validate.txt       # 15% of corpus
│   ├── test.txt           # 15% of corpus
│   └── split_seed.txt     # Seed used for reproducibility
├── by_content_type/
│   ├── cartoon.txt
│   ├── photographic.txt
│   ├── pixel_art.txt
│   ├── voyager_like.txt
│   └── text_ui.txt
├── by_transparency/
│   ├── heavy.txt
│   ├── light.txt
│   ├── none.txt
│   └── mixed.txt
└── by_disposal/
    ├── restore_bg_heavy.txt
    ├── restore_prev_heavy.txt
    ├── none_heavy.txt
    └── mixed.txt
```

Each `.txt` file contains one corpus ID per line.

---

## 7. Failure Handling & Retries

### Failure Categories

| Category | Cause | Action |
|----------|-------|--------|
| **Network timeout** | Download took >60s | Retry up to 3 times with exponential backoff |
| **Invalid GIF** | File doesn't start with `GIF` magic bytes | Log, skip, continue |
| **License unclear** | Source metadata missing or ambiguous | Log, skip, continue |
| **Decode error** | GIF crate fails to parse | Log error details, skip, continue |
| **Duplicate MD5** | File already in corpus | Log, skip, continue |
| **Metadata extraction failure** | Error computing structural metrics | Log, skip, continue |

### Retry Strategy

```
Attempt 1: timeout=60s, backoff=0s
Attempt 2: timeout=90s, backoff=2s
Attempt 3: timeout=120s, backoff=5s
Failure: Log and continue
```

### Logging

- Log all failures to `corpus/failures.jsonl`
- Include: source_url, error_type, error_message, timestamp, attempt_count
- Periodically review failures to identify systematic issues

---

## 8. Pipeline Outputs & Future Benchmarks

### Outputs

1. **Corpus directory** (`corpus/`)
   - `gifs/`: 512+ GIF files
   - `manifest.jsonl`: Streaming manifest
   - `manifest.json`: Full manifest (for tooling)
   - `failures.jsonl`: Failed downloads/extractions
   - `splits/`: Train/validate/test splits
   - `by_*/`: Category-based bucketing

2. **Statistics report** (`corpus/CORPUS_STATS.md`)
   - Summary of composition
   - Failure analysis
   - Source breakdown
   - Recommendations for future work

### Feeding Future Benchmarks

The corpus enables:

1. **Optimizer training**: Use train split to tune quantization, disposal handling, subframe cropping
2. **Validation**: Use validate split to measure generalization
3. **Regression testing**: Use test split for stable benchmarks
4. **Content-specific optimization**: Analyze performance by content_type, transparency, disposal
5. **Scaling studies**: Measure performance vs frame count, dimensions, palette size

### Benchmark Integration

```bash
# Future: rusticle-bench can use corpus
cargo run -p rusticle-bench -- \
  --corpus corpus/manifest.json \
  --split train \
  --output bench_results.json
```

---

## 9. Non-Goals

This pipeline is **not**:

- **Manual curation forever**: Automated classification is good enough; manual review is optional
- **Optimizer logic**: Pipeline extracts metadata; optimizer decisions are separate
- **Real-time updates**: Corpus is static once generated; updates are explicit new runs
- **Exhaustive coverage**: 512 GIFs is representative, not comprehensive
- **Streaming ingestion**: Corpus is built once, then used for multiple experiments
- **Deduplication beyond MD5**: Perceptual hashing is future work
- **Quality filtering**: All valid GIFs are included; quality metrics are for analysis, not filtering

---

## 10. Implementation Roadmap

### Phase 1: Pipeline Infrastructure (rusticle-961)

- [ ] Implement corpus downloader (Giphy + Tenor APIs)
- [ ] Implement GIF decoder + metadata extractor
- [ ] Implement deduplication (MD5)
- [ ] Implement manifest writer (JSONL + JSON)
- [ ] Implement bucketing/splitting logic
- [ ] Implement failure handling + logging

### Phase 2: Corpus Acquisition

- [ ] Run downloader with diverse queries
- [ ] Monitor for failures, adjust queries
- [ ] Reach ≥512 unique GIFs
- [ ] Validate composition against goals
- [ ] Generate final manifest + statistics

### Phase 3: Integration

- [ ] Check corpus into git (or external storage)
- [ ] Update benchmark suite to use corpus
- [ ] Document corpus usage in README
- [ ] Archive baseline metrics

---

## 11. Reproducibility & Versioning

### Reproducibility

- **Seed**: Fixed random seed for train/validate/test splits
- **Source list**: Checked in; same queries → same results (modulo API changes)
- **Pipeline version**: Tracked in manifest metadata
- **Timestamp**: Recorded for each corpus generation

### Versioning

- **Corpus version**: Incremented on major changes (e.g., new sources, new metadata fields)
- **Pipeline version**: Tied to rusticle release (e.g., `rusticle-961`)
- **Manifest schema**: Versioned; backward compatibility maintained

### Archival

- Corpus is immutable once generated
- Manifest is the source of truth
- GIF files are stored locally; URLs are for reference only
- Failures are logged for analysis

---

## 12. Example Queries & Expected Results

### Giphy Queries

```
cartoon, animation, simple, flat, logo
photo, nature, landscape, weather, space
pixel art, retro, 8-bit, sprite, game
transparent, alpha, overlay, effect
text, loading, progress, button, ui
minimal, simple animation, delta, efficient
long animation, sequence, movie, clip
high resolution, 4k, 1080p, large
```

**Expected**: ~250 GIFs from Giphy (50 per query, accounting for duplicates)

### Tenor Queries

Same as Giphy, expect ~150 GIFs

### Archive.org Query

```
filetype:gif AND (animation OR cartoon OR pixel)
```

**Expected**: ~80 GIFs

### OpenGameArt + Wikimedia

Manual curation, ~30 GIFs total

---

## 13. Success Criteria

- [x] Specification document exists and is detailed
- [ ] ≥512 unique GIFs acquired
- [ ] Manifest contains all required fields
- [ ] Composition matches goals (within ±5%)
- [ ] All GIFs are CC0 or CC-BY licensed
- [ ] Deduplication is working (no MD5 collisions in final corpus)
- [ ] Metadata extraction is accurate (spot-check 10 GIFs)
- [ ] Bucketing/splitting is reproducible (same seed → same split)
- [ ] Failure rate is <20% (≥600 attempted → ≥512 successful)
- [ ] Pipeline is documented and runnable by others

---

## 14. References

- GIF Specification: https://www.w3.org/Graphics/GIF/spec-gif89a.txt
- Giphy API: https://developers.giphy.com/docs/api/
- Tenor API: https://tenor.com/developer/documentation
- Internet Archive API: https://archive.org/advancedsearch.php
- Wikimedia Commons API: https://commons.wikimedia.org/w/api.php
- rusticle library: `/home/garrett/code/geverding/rusticle`
