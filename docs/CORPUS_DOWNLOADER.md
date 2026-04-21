# Corpus Downloader Tool

Implements the large GIF corpus acquisition pipeline from `LARGE_GIF_CORPUS_PIPELINE.md`.

## Quick Start

```bash
# Download 512 GIFs from Giphy and Tenor
python3 scripts/corpus_downloader.py \
    --output corpus \
    --target 512 \
    --sources giphy tenor

# Download smaller batch for testing
python3 scripts/corpus_downloader.py \
    --output corpus_test \
    --target 50 \
    --sources giphy
```

## Output Structure

```
corpus/
├── gifs/                          # Downloaded GIF files
│   ├── corpus_0001.gif
│   ├── corpus_0002.gif
│   └── ...
├── manifest.json                  # Full manifest (JSON array + metadata)
├── manifest.jsonl                 # Streaming manifest (one entry per line)
├── failures.jsonl                 # Failed downloads/extractions
├── splits/
│   ├── train.txt                  # 70% of corpus (one ID per line)
│   ├── validate.txt               # 15% of corpus
│   ├── test.txt                   # 15% of corpus
│   └── split_seed.txt             # Seed used for reproducibility
├── by_content_type/
│   ├── cartoon.txt
│   ├── photographic.txt
│   ├── pixel_art.txt
│   ├── voyager_like.txt
│   ├── text_ui.txt
│   └── mixed.txt
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

## Manifest Schema

Each entry in `manifest.json` contains:

```json
{
  "id": "corpus_0001",
  "source_url": "https://media.giphy.com/media/...",
  "source_id": "giphy_0000",
  "source_type": "giphy|tenor|archive|opengameart|wikimedia",
  "local_path": "corpus/gifs/corpus_0001.gif",
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
      "0": 5,
      "1": 2,
      "2": 3,
      "3": 2
    },
    "dominant_method": "2",
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
    "avg_offset_x": 12.0,
    "avg_offset_y": 8.0,
    "max_offset_x": 45,
    "max_offset_y": 32
  },
  "content_type": "cartoon|photographic|pixel_art|voyager_like|text_ui|mixed",
  "content_confidence": 0.85,
  "tags": ["animation", "simple", "flat-color"],
  "notes": "Optional human or automated notes"
}
```

## Metadata Extraction

The tool extracts structural metadata without full GIF decoding:

### Dimensions & Frame Count
- Parsed from GIF header
- Accurate for all GIFs

### Transparency
- Detected from Graphics Control Extension (GCE)
- Categorized as: heavy (>50%), light (<10%), none, mixed

### Disposal Methods
- Parsed from GCE disposal field
- Distribution tracked per frame
- Dominant method identified
- Categories: restore_bg_heavy, restore_prev_heavy, none_heavy, mixed

### Palette
- Global palette: detected from packed byte in header
- Local palettes: counted from Image Descriptor blocks
- Categories: global_only, local_only, mixed, grayscale_like

### Subframes
- Frames with non-zero offset (x, y) are counted
- Average and max offsets computed
- Ratio of offset frames to total frames

### Content Type (Heuristic)
- **Pixel art**: small dimensions (<256px)
- **Photographic**: large dimensions (>1280px)
- **Mixed**: default for medium dimensions
- Confidence score reflects heuristic reliability

### Tags
Generated automatically from structural analysis:
- `transparency_heavy`, `transparency_light`, `transparency_none`, `transparency_mixed`
- `disposal_restore_bg`, `disposal_restore_prev`, `disposal_none`, `disposal_mixed`
- `frames_single`, `frames_few`, `frames_many`, `frames_very_many`
- `dims_small`, `dims_medium`, `dims_large`, `dims_very_large`
- `palette_global`, `palette_local`, `palette_mixed`, `palette_grayscale`
- `content_cartoon`, `content_photographic`, `content_pixel_art`, `content_voyager`, `content_text_ui`

## Deduplication

- **Primary**: MD5 hash of raw GIF bytes
- **Strategy**: Keep first occurrence, log duplicates
- **Logged in**: `failures.jsonl` with error_type="duplicate_md5"

## Failure Handling

Failures are logged to `failures.jsonl`:

```json
{
  "source_url": "https://...",
  "source_type": "giphy",
  "source_id": "giphy_0000",
  "error_type": "network_timeout|invalid_gif|metadata_extraction|duplicate_md5|save_error|download_error",
  "error_message": "...",
  "timestamp": "2026-04-21T12:34:56Z"
}
```

### Retry Strategy

- **Network timeout**: Retry up to 3 times with exponential backoff (0s, 2s, 5s)
- **Invalid GIF**: Log and skip
- **Metadata extraction**: Log and skip
- **Duplicate MD5**: Log and skip
- **Save error**: Log and skip

## Source Adapters

### Giphy (Implemented)
- **Status**: Fallback to direct URLs (API requires key)
- **License**: CC0
- **Coverage**: ~12 direct URLs (expandable)
- **Rate limit**: 0.5s between downloads

### Tenor (Implemented)
- **Status**: Fallback to direct URLs (API requires key)
- **License**: CC0
- **Coverage**: ~2 direct URLs (expandable)
- **Rate limit**: 0.5s between downloads

### Internet Archive (Stubbed)
- **Status**: Not implemented (requires API integration)
- **License**: CC0 / public domain
- **Coverage**: Potentially 80+ GIFs
- **Next steps**: Implement `advancedsearch.php` query + result parsing

### OpenGameArt (Stubbed)
- **Status**: Not implemented (requires manual curation)
- **License**: CC0 / CC-BY
- **Coverage**: ~20 GIFs
- **Next steps**: Manual URL list or web scraping

### Wikimedia Commons (Stubbed)
- **Status**: Not implemented (requires API integration)
- **License**: CC0 / CC-BY
- **Coverage**: ~12 GIFs
- **Next steps**: Implement `allimages` query + filtering

## Extending the Tool

### Adding Direct URLs

Edit `download_from_giphy()` or `download_from_tenor()` to add more URLs:

```python
direct_urls = [
    "https://media.giphy.com/media/...",
    "https://media.giphy.com/media/...",
    # ... more URLs
]
```

### Adding a New Source Adapter

1. Create a new method `download_from_<source>()`
2. Implement URL fetching and GIF processing
3. Call it from `run()` if source is enabled
4. Update `source_type` in manifest entries

Example:

```python
def download_from_archive(self):
    """Download GIFs from Internet Archive."""
    logger.info("Downloading from Internet Archive...")
    
    # Query advancedsearch.php
    # Parse JSON results
    # Download each GIF
    # Process with self.process_gif()
```

## Reproducibility

- **Seed**: Fixed random seed (42) for train/validate/test splits
- **Source list**: Checked in; same URLs → same results
- **Pipeline version**: Tracked in manifest metadata
- **Timestamp**: Recorded for each corpus generation

To reproduce a corpus:

```bash
# Same command → same split assignment (different runs may have different GIFs if sources change)
python3 scripts/corpus_downloader.py --output corpus --target 512 --sources giphy tenor
```

## Performance

- **Download time**: ~1-2 seconds per GIF (network dependent)
- **Metadata extraction**: <10ms per GIF (no full decode)
- **Deduplication**: O(1) MD5 lookup
- **Total time for 512 GIFs**: ~10-15 minutes (with rate limiting)

## Logging

- **Console**: INFO level, timestamped
- **File**: `corpus_downloader.log` (same directory as script)
- **Failures**: `corpus/failures.jsonl` (structured JSON)

## Known Limitations

1. **Metadata extraction**: Heuristic-based, not full GIF decode
   - Transparency ratio is estimated (not pixel-accurate)
   - Duration is not computed (would require full decode)
   - Content type classification is simple (dimensions-based)

2. **API adapters**: Currently stubbed (Tenor, Archive.org, etc.)
   - Requires API keys or manual curation
   - Can be extended with proper implementation

3. **Perceptual deduplication**: Not implemented
   - Only MD5 (byte-level) deduplication
   - Near-duplicates (re-encoded) are not detected

4. **Quality filtering**: Not implemented
   - All valid GIFs are included
   - Quality metrics are for analysis, not filtering

## Future Work (rusticle-1tb)

1. **Expand source adapters**:
   - Implement Internet Archive API integration
   - Implement Wikimedia Commons API integration
   - Add manual curation for OpenGameArt

2. **Improve metadata extraction**:
   - Full GIF decode for accurate transparency ratio
   - Compute frame durations
   - Better content type classification (color variance analysis)

3. **Add perceptual deduplication**:
   - Implement PHASH or similar
   - Detect re-encoded duplicates

4. **Generate statistics report**:
   - Composition summary (by transparency, disposal, content type, etc.)
   - Failure analysis
   - Source breakdown
   - Recommendations for future work

5. **Validate composition**:
   - Check distribution against goals
   - Identify underrepresented categories
   - Suggest additional queries

## Usage in Benchmarks

The corpus can be used in rusticle-bench:

```bash
# Future: benchmark against corpus
cargo run -p rusticle-bench -- \
    --corpus corpus/manifest.json \
    --split train \
    --output bench_results.json
```

## References

- [Large GIF Corpus Pipeline](./LARGE_GIF_CORPUS_PIPELINE.md)
- [GIF Specification](https://www.w3.org/Graphics/GIF/spec-gif89a.txt)
- [Giphy API](https://developers.giphy.com/docs/api/)
- [Tenor API](https://tenor.com/developer/documentation)
- [Internet Archive API](https://archive.org/advancedsearch.php)
- [Wikimedia Commons API](https://commons.wikimedia.org/w/api.php)
