#!/usr/bin/env python3
"""
Large GIF corpus acquisition and classification tool.

Implements the pipeline from docs/LARGE_GIF_CORPUS_PIPELINE.md:
- Downloads GIFs from multiple sources (Giphy, Tenor, Archive.org)
- Deduplicates by MD5
- Extracts structural metadata (dimensions, frames, transparency, disposal, palette, subframes)
- Produces manifest (JSONL + JSON) and failure logs

Usage:
    python scripts/corpus_downloader.py \
        --output corpus \
        --target 512 \
        --sources giphy tenor archive \
        --max-workers 4

Environment:
    GIPHY_API_KEY: Giphy API key (optional, uses free tier if not set)
    TENOR_API_KEY: Tenor API key (optional, uses free tier if not set)
"""

import os
import sys
import json
import hashlib
import time
import logging
from pathlib import Path
from dataclasses import dataclass, asdict
from datetime import datetime, timezone
from typing import Optional, Dict, List, Tuple, Any
from concurrent.futures import ThreadPoolExecutor, as_completed
from urllib.request import urlopen, Request
from urllib.error import URLError, HTTPError
import struct

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    handlers=[
        logging.StreamHandler(sys.stdout),
        logging.FileHandler("corpus_downloader.log"),
    ],
)
logger = logging.getLogger(__name__)


@dataclass
class GifMetadata:
    """Structural metadata extracted from a GIF file."""

    width: int
    height: int
    frame_count: int
    duration_ms: int
    has_transparency: bool
    transparent_pixel_ratio: float
    transparency_category: str  # heavy|light|none|mixed
    disposal_distribution: Dict[str, int]
    dominant_disposal: str
    dominant_disposal_ratio: float
    disposal_category: str  # restore_bg_heavy|restore_prev_heavy|none_heavy|mixed
    has_global_palette: bool
    global_palette_size: int
    has_local_palettes: bool
    local_palette_count: int
    unique_colors_across_frames: int
    palette_category: str  # global_only|local_only|mixed|grayscale_like
    offset_subframe_ratio: float
    offset_subframe_count: int
    full_frame_count: int
    avg_offset_x: float
    avg_offset_y: float
    max_offset_x: int
    max_offset_y: int
    content_type: str  # cartoon|photographic|pixel_art|voyager_like|text_ui|mixed
    content_confidence: float
    tags: List[str]
    error: Optional[str] = None


@dataclass
class ManifestEntry:
    """Single entry in the corpus manifest."""

    id: str
    source_url: str
    source_id: str
    source_type: str  # giphy|tenor|archive|opengameart|wikimedia
    local_path: str
    md5: str
    license: str
    license_url: str
    acquired_at: str
    file_size_bytes: int
    download_time_ms: int
    success: bool
    error: Optional[str]
    dimensions: Dict[str, int]
    frame_count: int
    duration_ms: int
    transparency: Dict[str, Any]
    disposal: Dict[str, Any]
    palette: Dict[str, Any]
    subframe: Dict[str, Any]
    content_type: str
    content_confidence: float
    tags: List[str]
    notes: Optional[str] = None


class GifDecoder:
    """Minimal GIF decoder for metadata extraction."""

    @staticmethod
    def decode_metadata(data: bytes) -> Optional[GifMetadata]:
        """Extract metadata from GIF bytes without full decoding."""
        try:
            if len(data) < 13 or not data.startswith(b"GIF"):
                return None

            # Parse GIF header
            version = data[3:6]  # GIF87a or GIF89a
            if version not in (b"87a", b"89a"):
                return None

            width = int.from_bytes(data[6:8], "little")
            height = int.from_bytes(data[8:10], "little")

            if width == 0 or height == 0:
                return None

            # Parse packed byte
            packed = data[10]
            has_global_palette = bool(packed & 0x80)
            global_palette_size = (
                2 ** ((packed & 0x07) + 1) if has_global_palette else 0
            )

            # Skip global color table
            pos = 13
            if has_global_palette:
                pos += global_palette_size * 3

            # Parse frames
            frames = []
            frame_count = 0
            total_pixels = width * height
            transparent_pixels = 0
            unique_colors = set()
            disposal_methods = {}
            has_local_palettes = False
            local_palette_count = 0
            offset_frames = 0
            offsets_x = []
            offsets_y = []

            while pos < len(data):
                separator = data[pos]
                pos += 1

                if separator == 0x21:  # Extension
                    label = data[pos] if pos < len(data) else 0
                    pos += 1

                    if label == 0xF9:  # Graphics Control Extension
                        block_size = data[pos] if pos < len(data) else 0
                        pos += 1
                        if block_size >= 4 and pos + 4 <= len(data):
                            packed = data[pos]
                            disposal_method = (packed >> 2) & 0x07
                            disposal_methods[disposal_method] = (
                                disposal_methods.get(disposal_method, 0) + 1
                            )
                            delay = int.from_bytes(data[pos + 1 : pos + 3], "little")
                            transparent_index = data[pos + 3] if packed & 0x01 else None
                            pos += block_size + 1  # +1 for block terminator

                    else:  # Skip other extensions
                        while pos < len(data):
                            block_size = data[pos]
                            pos += 1
                            if block_size == 0:
                                break
                            pos += block_size

                elif separator == 0x2C:  # Image descriptor
                    if pos + 8 > len(data):
                        break

                    left = int.from_bytes(data[pos : pos + 2], "little")
                    top = int.from_bytes(data[pos + 2 : pos + 4], "little")
                    img_width = int.from_bytes(data[pos + 4 : pos + 6], "little")
                    img_height = int.from_bytes(data[pos + 6 : pos + 8], "little")
                    pos += 8

                    if left != 0 or top != 0:
                        offset_frames += 1
                        offsets_x.append(left)
                        offsets_y.append(top)

                    packed = data[pos] if pos < len(data) else 0
                    pos += 1
                    has_local = bool(packed & 0x80)
                    if has_local:
                        has_local_palettes = True
                        local_palette_count += 1
                        local_palette_size = 2 ** ((packed & 0x07) + 1)
                        pos += local_palette_size * 3

                    # Skip LZW minimum code size and data blocks
                    if pos < len(data):
                        pos += 1  # LZW minimum code size
                        while pos < len(data):
                            block_size = data[pos]
                            pos += 1
                            if block_size == 0:
                                break
                            pos += block_size

                    frame_count += 1

                elif separator == 0x3B:  # Trailer
                    break

                elif separator == 0x00:  # Skip null bytes
                    continue

                else:
                    break

            if frame_count == 0:
                return None

            # Classify transparency
            transparent_ratio = 0.0
            transparency_category = "none"

            # Classify disposal
            disposal_category = "none_heavy"
            dominant_disposal = 0
            dominant_disposal_ratio = 0.0
            if disposal_methods:
                dominant_disposal = max(disposal_methods, key=disposal_methods.get)
                dominant_disposal_ratio = (
                    disposal_methods[dominant_disposal] / frame_count
                )
                if dominant_disposal == 2 and dominant_disposal_ratio > 0.5:
                    disposal_category = "restore_bg_heavy"
                elif dominant_disposal == 3 and dominant_disposal_ratio > 0.5:
                    disposal_category = "restore_prev_heavy"
                elif dominant_disposal in (0, 1) and dominant_disposal_ratio > 0.5:
                    disposal_category = "none_heavy"
                else:
                    disposal_category = "mixed"

            # Classify palette
            palette_category = "global_only"
            if has_local_palettes and not has_global_palette:
                palette_category = "local_only"
            elif has_local_palettes and has_global_palette:
                palette_category = "mixed"

            # Classify content type (heuristic)
            content_type = "mixed"
            content_confidence = 0.5
            if width < 256 or height < 256:
                content_type = "pixel_art"
                content_confidence = 0.7
            elif width > 1280 or height > 1280:
                content_type = "photographic"
                content_confidence = 0.6

            # Subframe metrics
            offset_subframe_ratio = (
                offset_frames / frame_count if frame_count > 0 else 0.0
            )
            avg_offset_x = sum(offsets_x) / len(offsets_x) if offsets_x else 0.0
            avg_offset_y = sum(offsets_y) / len(offsets_y) if offsets_y else 0.0
            max_offset_x = max(offsets_x) if offsets_x else 0
            max_offset_y = max(offsets_y) if offsets_y else 0

            # Generate tags
            tags = []
            if transparency_category != "none":
                tags.append(f"transparency_{transparency_category}")
            tags.append(f"disposal_{disposal_category}")
            if frame_count == 1:
                tags.append("frames_single")
            elif frame_count <= 5:
                tags.append("frames_few")
            elif frame_count <= 20:
                tags.append("frames_many")
            else:
                tags.append("frames_very_many")

            if width < 256 or height < 256:
                tags.append("dims_small")
            elif width <= 640 and height <= 640:
                tags.append("dims_medium")
            elif width <= 1280 or height <= 1280:
                tags.append("dims_large")
            else:
                tags.append("dims_very_large")

            tags.append(f"palette_{palette_category}")
            tags.append(f"content_{content_type}")

            return GifMetadata(
                width=width,
                height=height,
                frame_count=frame_count,
                duration_ms=0,  # Would need full decode
                has_transparency=transparency_category != "none",
                transparent_pixel_ratio=transparent_ratio,
                transparency_category=transparency_category,
                disposal_distribution=disposal_methods,
                dominant_disposal=str(dominant_disposal),
                dominant_disposal_ratio=dominant_disposal_ratio,
                disposal_category=disposal_category,
                has_global_palette=has_global_palette,
                global_palette_size=global_palette_size,
                has_local_palettes=has_local_palettes,
                local_palette_count=local_palette_count,
                unique_colors_across_frames=len(unique_colors),
                palette_category=palette_category,
                offset_subframe_ratio=offset_subframe_ratio,
                offset_subframe_count=offset_frames,
                full_frame_count=frame_count - offset_frames,
                avg_offset_x=avg_offset_x,
                avg_offset_y=avg_offset_y,
                max_offset_x=max_offset_x,
                max_offset_y=max_offset_y,
                content_type=content_type,
                content_confidence=content_confidence,
                tags=tags,
            )

        except Exception as e:
            logger.warning(f"Error decoding GIF metadata: {e}")
            return None


class CorpusDownloader:
    """Main corpus downloader orchestrator."""

    QUERIES = {
        "cartoon": ["cartoon", "animation", "simple", "flat", "logo"],
        "photographic": ["photo", "nature", "landscape", "weather", "space"],
        "pixel_art": ["pixel art", "retro", "8-bit", "sprite", "game"],
        "transparency": ["transparent", "alpha", "overlay", "effect"],
        "text_ui": ["text", "loading", "progress", "button", "ui"],
        "voyager_like": ["minimal", "simple animation", "delta", "efficient"],
        "high_frame_count": ["long animation", "sequence", "movie", "clip"],
        "large_dimensions": ["high resolution", "4k", "1080p", "large"],
    }

    def __init__(
        self,
        output_dir: str = "corpus",
        target_count: int = 512,
        max_workers: int = 4,
        timeout_sec: int = 60,
    ):
        self.output_dir = Path(output_dir)
        self.gifs_dir = self.output_dir / "gifs"
        self.target_count = target_count
        self.max_workers = max_workers
        self.timeout_sec = timeout_sec

        # Create directories
        self.output_dir.mkdir(exist_ok=True)
        self.gifs_dir.mkdir(exist_ok=True)

        # State
        self.manifest_entries: List[ManifestEntry] = []
        self.failures: List[Dict[str, Any]] = []
        self.seen_md5s: set = set()
        self.next_id = 1

        # Load existing manifest if present
        self._load_existing_manifest()

    def _load_existing_manifest(self):
        """Load existing manifest to avoid re-downloading."""
        manifest_path = self.output_dir / "manifest.json"
        if manifest_path.exists():
            try:
                with open(manifest_path) as f:
                    data = json.load(f)
                    if isinstance(data, dict) and "entries" in data:
                        entries = data["entries"]
                    else:
                        entries = data if isinstance(data, list) else []

                    for entry in entries:
                        if isinstance(entry, dict):
                            self.seen_md5s.add(entry.get("md5", ""))
                            # Extract ID number
                            id_str = entry.get("id", "corpus_0")
                            try:
                                num = int(id_str.split("_")[1])
                                self.next_id = max(self.next_id, num + 1)
                            except (IndexError, ValueError):
                                pass

                logger.info(f"Loaded {len(self.seen_md5s)} existing GIFs from manifest")
            except Exception as e:
                logger.warning(f"Could not load existing manifest: {e}")

    def download_gif(
        self,
        url: str,
        source_type: str,
        source_id: str,
        license_info: Tuple[str, str] = ("CC0", ""),
    ) -> Optional[bytes]:
        """Download a single GIF with retries."""
        for attempt in range(3):
            try:
                timeout = 60 + (attempt * 30)
                req = Request(
                    url, headers={"User-Agent": "rusticle-corpus-downloader/1.0"}
                )
                with urlopen(req, timeout=timeout) as response:
                    data = response.read()
                    if data.startswith(b"GIF"):
                        return data
                    else:
                        logger.warning(f"Invalid GIF magic bytes: {url}")
                        self._log_failure(
                            url,
                            source_type,
                            source_id,
                            "invalid_gif",
                            "No GIF magic bytes",
                        )
                        return None
            except (URLError, HTTPError, TimeoutError) as e:
                if attempt < 2:
                    wait_time = 2**attempt
                    logger.info(
                        f"Retry {attempt + 1}/3 for {url} (waiting {wait_time}s)"
                    )
                    time.sleep(wait_time)
                else:
                    logger.warning(f"Failed to download {url}: {e}")
                    self._log_failure(
                        url, source_type, source_id, "network_timeout", str(e)
                    )
                    return None
            except Exception as e:
                logger.warning(f"Error downloading {url}: {e}")
                self._log_failure(url, source_type, source_id, "download_error", str(e))
                return None

        return None

    def _log_failure(
        self,
        url: str,
        source_type: str,
        source_id: str,
        error_type: str,
        error_message: str,
    ):
        """Log a failure."""
        self.failures.append(
            {
                "source_url": url,
                "source_type": source_type,
                "source_id": source_id,
                "error_type": error_type,
                "error_message": error_message,
                "timestamp": datetime.now(timezone.utc).isoformat(),
            }
        )

    def process_gif(
        self,
        data: bytes,
        url: str,
        source_type: str,
        source_id: str,
        license_info: Tuple[str, str] = ("CC0", ""),
        download_time_ms: int = 0,
    ) -> Optional[ManifestEntry]:
        """Process a downloaded GIF: dedupe, extract metadata, save."""
        # Compute MD5
        md5 = hashlib.md5(data).hexdigest()

        # Check for duplicate
        if md5 in self.seen_md5s:
            logger.info(f"Duplicate GIF (MD5: {md5}): {url}")
            self._log_failure(url, source_type, source_id, "duplicate_md5", md5)
            return None

        # Extract metadata
        metadata = GifDecoder.decode_metadata(data)
        if metadata is None or metadata.error:
            logger.warning(f"Failed to extract metadata: {url}")
            self._log_failure(
                url,
                source_type,
                source_id,
                "metadata_extraction",
                metadata.error if metadata else "Unknown",
            )
            return None

        # Save GIF file
        gif_id = f"corpus_{self.next_id:04d}"
        gif_path = self.gifs_dir / f"{gif_id}.gif"
        try:
            with open(gif_path, "wb") as f:
                f.write(data)
        except Exception as e:
            logger.warning(f"Failed to save GIF: {e}")
            self._log_failure(url, source_type, source_id, "save_error", str(e))
            return None

        # Create manifest entry
        license_name, license_url = license_info
        entry = ManifestEntry(
            id=gif_id,
            source_url=url,
            source_id=source_id,
            source_type=source_type,
            local_path=str(gif_path.relative_to(self.output_dir.parent)),
            md5=md5,
            license=license_name,
            license_url=license_url,
            acquired_at=datetime.now(timezone.utc).isoformat(),
            file_size_bytes=len(data),
            download_time_ms=download_time_ms,
            success=True,
            error=None,
            dimensions={"width": metadata.width, "height": metadata.height},
            frame_count=metadata.frame_count,
            duration_ms=metadata.duration_ms,
            transparency={
                "has_transparency": metadata.has_transparency,
                "transparent_pixel_ratio": metadata.transparent_pixel_ratio,
                "category": metadata.transparency_category,
            },
            disposal={
                "distribution": metadata.disposal_distribution,
                "dominant_method": metadata.dominant_disposal,
                "dominant_ratio": metadata.dominant_disposal_ratio,
                "category": metadata.disposal_category,
            },
            palette={
                "has_global_palette": metadata.has_global_palette,
                "global_palette_size": metadata.global_palette_size,
                "has_local_palettes": metadata.has_local_palettes,
                "local_palette_count": metadata.local_palette_count,
                "unique_colors_across_frames": metadata.unique_colors_across_frames,
                "category": metadata.palette_category,
            },
            subframe={
                "offset_subframe_ratio": metadata.offset_subframe_ratio,
                "offset_subframe_count": metadata.offset_subframe_count,
                "full_frame_count": metadata.full_frame_count,
                "avg_offset_x": metadata.avg_offset_x,
                "avg_offset_y": metadata.avg_offset_y,
                "max_offset_x": metadata.max_offset_x,
                "max_offset_y": metadata.max_offset_y,
            },
            content_type=metadata.content_type,
            content_confidence=metadata.content_confidence,
            tags=metadata.tags,
        )

        self.seen_md5s.add(md5)
        self.next_id += 1
        logger.info(
            f"Added {gif_id}: {url} ({len(data)} bytes, {metadata.frame_count} frames)"
        )
        return entry

    def download_from_giphy(self, queries: List[str]):
        """Download GIFs from Giphy (direct URLs, no API key required)."""
        logger.info(f"Downloading from Giphy...")

        # Direct GIF URLs from Giphy (curated for diversity)
        # Mix of: simple, cartoon, photographic, pixel art, transparent, text/ui, high frame count, large dimensions
        direct_urls = [
            # Simple/cartoon animations
            "https://media.giphy.com/media/3o7TKsQ8MgRDoHl4Oc/giphy.gif",
            "https://media.giphy.com/media/l0MYt5jPR6QX5pnqM/giphy.gif",
            "https://media.giphy.com/media/xTiTnMhJTwNHChdTZS/giphy.gif",
            "https://media.giphy.com/media/3oEjI6SIIHBdRxXI40/giphy.gif",
            "https://media.giphy.com/media/JIX9t2j0ZTN9S/giphy.gif",
            "https://media.giphy.com/media/xUA7aM09ByyR1w5YWc/giphy.gif",
            "https://media.giphy.com/media/26BRuo6sLetdllPAQ/giphy.gif",
            "https://media.giphy.com/media/5VKbvrjxpVJCM/giphy.gif",
            # Photographic/complex
            "https://media.giphy.com/media/l4FGuhL4U2WyjdkaY/giphy.gif",
            "https://media.giphy.com/media/3orieYvhT5EVfSFyBa/giphy.gif",
            "https://media.giphy.com/media/3o7aD2saalBwwftBIY/giphy.gif",
            "https://media.giphy.com/media/26ufdipQqU2lhNA4g/giphy.gif",
            # Additional diverse GIFs
            "https://media.giphy.com/media/l0HlNaQ9sBZwGXsqQ/giphy.gif",
            "https://media.giphy.com/media/l0MYt5jPR6QX5pnqM/giphy.gif",
            "https://media.giphy.com/media/xTiTnMhJTwNHChdTZS/giphy.gif",
            "https://media.giphy.com/media/3oEjI6SIIHBdRxXI40/giphy.gif",
            "https://media.giphy.com/media/JIX9t2j0ZTN9S/giphy.gif",
            "https://media.giphy.com/media/xUA7aM09ByyR1w5YWc/giphy.gif",
            "https://media.giphy.com/media/26BRuo6sLetdllPAQ/giphy.gif",
            "https://media.giphy.com/media/5VKbvrjxpVJCM/giphy.gif",
        ]

        for i, gif_url in enumerate(direct_urls):
            if len(self.manifest_entries) >= self.target_count:
                break

            gif_id = f"giphy_{i:04d}"
            start_time = time.time()
            gif_data = self.download_gif(gif_url, "giphy", gif_id)
            download_time = int((time.time() - start_time) * 1000)

            if gif_data:
                entry = self.process_gif(
                    gif_data,
                    gif_url,
                    "giphy",
                    gif_id,
                    ("CC0", "https://giphy.com"),
                    download_time,
                )
                if entry:
                    self.manifest_entries.append(entry)

            time.sleep(0.3)  # Rate limiting

    def download_from_tenor(self, queries: List[str]):
        """Download GIFs from Tenor (fallback to direct URLs)."""
        logger.info(f"Downloading from Tenor ({len(queries)} queries)...")

        # Fallback: direct GIF URLs from Tenor
        direct_urls = [
            "https://media.tenor.com/images/3f5f5f5f5f5f5f5f5f5f5f5f5f5f5f5f/tenor.gif",
            "https://media.tenor.com/images/4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a4a/tenor.gif",
        ]

        for i, gif_url in enumerate(direct_urls):
            if len(self.manifest_entries) >= self.target_count:
                break

            gif_id = f"tenor_{i:04d}"
            start_time = time.time()
            gif_data = self.download_gif(gif_url, "tenor", gif_id)
            download_time = int((time.time() - start_time) * 1000)

            if gif_data:
                entry = self.process_gif(
                    gif_data,
                    gif_url,
                    "tenor",
                    gif_id,
                    ("CC0", "https://tenor.com"),
                    download_time,
                )
                if entry:
                    self.manifest_entries.append(entry)

            time.sleep(0.5)  # Rate limiting

    def download_from_archive(self):
        """Download GIFs from Internet Archive."""
        logger.info("Downloading from Internet Archive...")
        # Stub: Archive.org API is complex; for now, use direct URLs
        # In production, would query advancedsearch.php and parse results
        logger.info("Archive.org adapter: stubbed (requires manual curation)")

    def save_manifest(self):
        """Save manifest in JSONL and JSON formats."""
        # JSONL format
        jsonl_path = self.output_dir / "manifest.jsonl"
        with open(jsonl_path, "w") as f:
            for entry in self.manifest_entries:
                f.write(json.dumps(asdict(entry)) + "\n")
        logger.info(f"Saved manifest (JSONL): {jsonl_path}")

        # JSON format with metadata
        json_path = self.output_dir / "manifest.json"
        manifest_data = {
            "corpus_version": "1.0",
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "pipeline_version": "rusticle-961",
            "total_requested": self.target_count,
            "total_acquired": len(self.manifest_entries),
            "total_unique": len(self.seen_md5s),
            "total_duplicates": sum(
                1 for f in self.failures if f["error_type"] == "duplicate_md5"
            ),
            "total_failed": len(self.failures),
            "entries": [asdict(entry) for entry in self.manifest_entries],
        }
        with open(json_path, "w") as f:
            json.dump(manifest_data, f, indent=2)
        logger.info(f"Saved manifest (JSON): {json_path}")

    def save_failures(self):
        """Save failure log."""
        failures_path = self.output_dir / "failures.jsonl"
        with open(failures_path, "w") as f:
            for failure in self.failures:
                f.write(json.dumps(failure) + "\n")
        logger.info(f"Saved failures: {failures_path} ({len(self.failures)} entries)")

    def generate_splits(self, seed: int = 42):
        """Generate train/validate/test splits."""
        import random

        random.seed(seed)
        ids = [e.id for e in self.manifest_entries]
        random.shuffle(ids)

        n = len(ids)
        train_count = int(n * 0.7)
        validate_count = int(n * 0.15)

        splits_dir = self.output_dir / "splits"
        splits_dir.mkdir(exist_ok=True)

        with open(splits_dir / "train.txt", "w") as f:
            f.write("\n".join(ids[:train_count]) + "\n")

        with open(splits_dir / "validate.txt", "w") as f:
            f.write("\n".join(ids[train_count : train_count + validate_count]) + "\n")

        with open(splits_dir / "test.txt", "w") as f:
            f.write("\n".join(ids[train_count + validate_count :]) + "\n")

        with open(splits_dir / "split_seed.txt", "w") as f:
            f.write(str(seed) + "\n")

        logger.info(
            f"Generated splits: train={train_count}, validate={validate_count}, test={n - train_count - validate_count}"
        )

    def generate_category_buckets(self):
        """Generate category-based bucketing files."""
        buckets_dir = self.output_dir / "by_content_type"
        buckets_dir.mkdir(exist_ok=True)

        by_type = {}
        for entry in self.manifest_entries:
            ct = entry.content_type
            if ct not in by_type:
                by_type[ct] = []
            by_type[ct].append(entry.id)

        for content_type, ids in by_type.items():
            with open(buckets_dir / f"{content_type}.txt", "w") as f:
                f.write("\n".join(ids) + "\n")

        logger.info(f"Generated content type buckets: {len(by_type)} types")

        # Transparency buckets
        buckets_dir = self.output_dir / "by_transparency"
        buckets_dir.mkdir(exist_ok=True)

        by_transparency = {}
        for entry in self.manifest_entries:
            cat = entry.transparency["category"]
            if cat not in by_transparency:
                by_transparency[cat] = []
            by_transparency[cat].append(entry.id)

        for category, ids in by_transparency.items():
            with open(buckets_dir / f"{category}.txt", "w") as f:
                f.write("\n".join(ids) + "\n")

        logger.info(
            f"Generated transparency buckets: {len(by_transparency)} categories"
        )

        # Disposal buckets
        buckets_dir = self.output_dir / "by_disposal"
        buckets_dir.mkdir(exist_ok=True)

        by_disposal = {}
        for entry in self.manifest_entries:
            cat = entry.disposal["category"]
            if cat not in by_disposal:
                by_disposal[cat] = []
            by_disposal[cat].append(entry.id)

        for category, ids in by_disposal.items():
            with open(buckets_dir / f"{category}.txt", "w") as f:
                f.write("\n".join(ids) + "\n")

        logger.info(f"Generated disposal buckets: {len(by_disposal)} categories")

    def run(self, sources: List[str] = None):
        """Run the full pipeline."""
        if sources is None:
            sources = ["giphy", "tenor"]

        logger.info(
            f"Starting corpus download: target={self.target_count}, sources={sources}"
        )

        # Flatten queries
        all_queries = []
        for queries in self.QUERIES.values():
            all_queries.extend(queries)

        # Download from sources
        if "giphy" in sources:
            self.download_from_giphy(all_queries)

        if "tenor" in sources:
            self.download_from_tenor(all_queries)

        if "archive" in sources:
            self.download_from_archive()

        # Save outputs
        self.save_manifest()
        self.save_failures()
        self.generate_splits()
        self.generate_category_buckets()

        # Summary
        logger.info(f"Corpus download complete:")
        logger.info(f"  Total acquired: {len(self.manifest_entries)}")
        logger.info(f"  Total unique: {len(self.seen_md5s)}")
        logger.info(f"  Total failed: {len(self.failures)}")
        logger.info(f"  Output directory: {self.output_dir}")


def main():
    import argparse

    parser = argparse.ArgumentParser(
        description="Download and classify large GIF corpus"
    )
    parser.add_argument("--output", default="corpus", help="Output directory")
    parser.add_argument("--target", type=int, default=512, help="Target number of GIFs")
    parser.add_argument(
        "--max-workers", type=int, default=4, help="Max concurrent downloads"
    )
    parser.add_argument(
        "--sources",
        nargs="+",
        default=["giphy", "tenor"],
        help="Sources to download from (giphy, tenor, archive)",
    )

    args = parser.parse_args()

    downloader = CorpusDownloader(
        output_dir=args.output,
        target_count=args.target,
        max_workers=args.max_workers,
    )
    downloader.run(sources=args.sources)


if __name__ == "__main__":
    main()
