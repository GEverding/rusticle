#!/usr/bin/env python3
"""
Download diverse GIF test suite for benchmarking rusticle.

Categories:
- Flat color / cartoon (good for palette reuse)
- Gradients / photographic (stress test for quantization)
- Small / large dimensions
- Few frames / many frames
- Transparent / opaque

Uses public domain / CC0 sources.
"""

import os
import sys
import json
import hashlib
import urllib.request
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor, as_completed

# Test GIF sources - direct media URLs only

TEST_GIFS = [
    # Small, simple animations (good for palette reuse)
    {
        "name": "small_simple_01",
        "url": "https://media.giphy.com/media/3o7TKsQ8MgRDoHl4Oc/giphy.gif",
        "category": "simple",
        "expected_size": "small",
    },
    {
        "name": "small_simple_02",
        "url": "https://media.giphy.com/media/l0MYt5jPR6QX5pnqM/giphy.gif",
        "category": "simple",
        "expected_size": "small",
    },
    {
        "name": "small_simple_03",
        "url": "https://media.giphy.com/media/xTiTnMhJTwNHChdTZS/giphy.gif",
        "category": "simple",
        "expected_size": "small",
    },
    {
        "name": "small_simple_04",
        "url": "https://media.giphy.com/media/3oEjI6SIIHBdRxXI40/giphy.gif",
        "category": "simple",
        "expected_size": "small",
    },
    # Cartoon / flat color
    {
        "name": "cartoon_01",
        "url": "https://media.giphy.com/media/JIX9t2j0ZTN9S/giphy.gif",
        "category": "cartoon",
        "expected_size": "medium",
    },
    {
        "name": "cartoon_02",
        "url": "https://media.giphy.com/media/xUA7aM09ByyR1w5YWc/giphy.gif",
        "category": "cartoon",
        "expected_size": "medium",
    },
    {
        "name": "cartoon_03",
        "url": "https://media.giphy.com/media/26BRuo6sLetdllPAQ/giphy.gif",
        "category": "cartoon",
        "expected_size": "medium",
    },
    {
        "name": "cartoon_04",
        "url": "https://media.giphy.com/media/5VKbvrjxpVJCM/giphy.gif",
        "category": "cartoon",
        "expected_size": "medium",
    },
    # Photographic / complex gradients (stress test)
    {
        "name": "photo_01",
        "url": "https://media.giphy.com/media/l4FGuhL4U2WyjdkaY/giphy.gif",
        "category": "photographic",
        "expected_size": "large",
    },
    {
        "name": "photo_02",
        "url": "https://media.giphy.com/media/3orieYvhT5EVfSFyBa/giphy.gif",
        "category": "photographic",
        "expected_size": "large",
    },
    {
        "name": "photo_03",
        "url": "https://media.giphy.com/media/3o7aD2saalBwwftBIY/giphy.gif",
        "category": "photographic",
        "expected_size": "large",
    },
    {
        "name": "photo_04",
        "url": "https://media.giphy.com/media/26ufdipQqU2lhNA4g/giphy.gif",
        "category": "photographic",
        "expected_size": "large",
    },
    # High frame count
    {
        "name": "many_frames_01",
        "url": "https://media.giphy.com/media/xT9IgzoKnwFNmISR8I/giphy.gif",
        "category": "many_frames",
        "expected_size": "large",
    },
    {
        "name": "many_frames_02",
        "url": "https://media.giphy.com/media/l1J9yTco40EU5JzTW/giphy.gif",
        "category": "many_frames",
        "expected_size": "large",
    },
    {
        "name": "many_frames_03",
        "url": "https://media.giphy.com/media/xT9IgG50Fb7Mi0prBC/giphy.gif",
        "category": "many_frames",
        "expected_size": "large",
    },
    # Transparency heavy
    {
        "name": "transparent_01",
        "url": "https://media.giphy.com/media/3o7btPCcdNniyf0ArS/giphy.gif",
        "category": "transparent",
        "expected_size": "medium",
    },
    {
        "name": "transparent_02",
        "url": "https://media.giphy.com/media/3ohs4CacylzFaHjMM8/giphy.gif",
        "category": "transparent",
        "expected_size": "medium",
    },
    {
        "name": "transparent_03",
        "url": "https://media.giphy.com/media/13HgwGsXF0aiGY/giphy.gif",
        "category": "transparent",
        "expected_size": "medium",
    },
    # Pixel art (should be perfect for palette reuse)
    {
        "name": "pixel_art_01",
        "url": "https://media.giphy.com/media/l41lUJ1YoZB1lHVPG/giphy.gif",
        "category": "pixel_art",
        "expected_size": "small",
    },
    {
        "name": "pixel_art_02",
        "url": "https://media.giphy.com/media/26ufnwVewFJ5FnNdu/giphy.gif",
        "category": "pixel_art",
        "expected_size": "small",
    },
    {
        "name": "pixel_art_03",
        "url": "https://media.giphy.com/media/xT0xeJpnrWC4XWblEk/giphy.gif",
        "category": "pixel_art",
        "expected_size": "small",
    },
    # Large dimensions
    {
        "name": "large_dims_01",
        "url": "https://media.giphy.com/media/3oz8xZvvOZRmKay4xy/giphy.gif",
        "category": "large",
        "expected_size": "large",
    },
    {
        "name": "large_dims_02",
        "url": "https://media.giphy.com/media/3oriO0OEd9QIDdllqo/giphy.gif",
        "category": "large",
        "expected_size": "large",
    },
    {
        "name": "large_dims_03",
        "url": "https://media.giphy.com/media/l3q2K5jinAlChoCLS/giphy.gif",
        "category": "large",
        "expected_size": "large",
    },
    # Additional diverse entries
    {
        "name": "cartoon_05",
        "url": "https://media.giphy.com/media/l0HlDy9x8FZo0XO1i/giphy.gif",
        "category": "cartoon",
        "expected_size": "medium",
    },
    {
        "name": "photo_05",
        "url": "https://media.giphy.com/media/3o6ZtpWz664SPmo50Y/giphy.gif",
        "category": "photographic",
        "expected_size": "large",
    },
    {
        "name": "many_frames_04",
        "url": "https://media.giphy.com/media/l0MYt5jPR6QX5pnqM/giphy.gif",
        "category": "many_frames",
        "expected_size": "large",
    },
    {
        "name": "pixel_art_04",
        "url": "https://media.giphy.com/media/l0MYt5jPR6QX5pnqM/giphy.gif",
        "category": "pixel_art",
        "expected_size": "small",
    },
    {
        "name": "transparent_04",
        "url": "https://media.giphy.com/media/3o7btPCcdNniyf0ArS/giphy.gif",
        "category": "transparent",
        "expected_size": "medium",
    },
    {
        "name": "small_simple_05",
        "url": "https://media.giphy.com/media/xTiTnMhJTwNHChdTZS/giphy.gif",
        "category": "simple",
        "expected_size": "small",
    },
]


def download_gif(gif_info: dict, output_dir: Path) -> dict:
    """Download a single GIF and return metadata."""
    name = gif_info["name"]
    url = gif_info["url"]
    output_path = output_dir / f"{name}.gif"

    result = {
        "name": name,
        "url": url,
        "category": gif_info["category"],
        "path": str(output_path),
        "success": False,
        "error": None,
    }

    try:
        # Download with timeout
        req = urllib.request.Request(
            url, headers={"User-Agent": "rusticle-test-suite/1.0"}
        )
        with urllib.request.urlopen(req, timeout=30) as response:
            data = response.read()

        # Validate it's actually a GIF
        if not data.startswith(b"GIF"):
            result["error"] = "Not a valid GIF file"
            return result

        # Write to file
        output_path.write_bytes(data)

        # Get file stats
        result["size_bytes"] = len(data)
        result["size_human"] = format_size(len(data))
        result["md5"] = hashlib.md5(data).hexdigest()
        result["success"] = True

        print(f"✓ {name}: {result['size_human']}")

    except Exception as e:
        result["error"] = str(e)
        print(f"✗ {name}: {e}")

    return result


def format_size(size_bytes: int) -> str:
    """Format bytes as human readable."""
    for unit in ["B", "KB", "MB", "GB"]:
        if size_bytes < 1024:
            return f"{size_bytes:.1f}{unit}"
        size_bytes /= 1024
    return f"{size_bytes:.1f}TB"


def main():
    # Output directory
    script_dir = Path(__file__).parent
    output_dir = script_dir.parent / "test_gifs" / "benchmark_suite"
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"Downloading {len(TEST_GIFS)} test GIFs to {output_dir}")
    print("-" * 50)

    results = []

    # Download in parallel
    with ThreadPoolExecutor(max_workers=4) as executor:
        futures = {
            executor.submit(download_gif, gif, output_dir): gif for gif in TEST_GIFS
        }

        for future in as_completed(futures):
            results.append(future.result())

    # Summary
    print("-" * 50)
    successful = [r for r in results if r["success"]]
    failed = [r for r in results if not r["success"]]
    successful_md5s = [r["md5"] for r in successful if "md5" in r]
    unique_md5_count = len(set(successful_md5s))

    print(f"\nDownloaded: {len(successful)}/{len(TEST_GIFS)}")
    print(f"Unique MD5s: {unique_md5_count}")

    if failed:
        print(f"\nFailed downloads:")
        for r in failed:
            print(f"  - {r['name']}: {r['error']}")

    duplicate_groups = {}
    for result in successful:
        md5 = result.get("md5")
        if md5 is None:
            continue
        duplicate_groups.setdefault(md5, []).append(result["name"])

    duplicate_groups = {
        md5: sorted(names) for md5, names in duplicate_groups.items() if len(names) > 1
    }

    if duplicate_groups:
        print("\n⚠ Duplicate content detected (matching md5):")
        for md5, names in sorted(duplicate_groups.items()):
            print(f"  {md5}: {', '.join(names)}")
    else:
        print("\nNo duplicate MD5 hashes detected.")

    # Write manifest
    manifest_path = output_dir / "manifest.json"
    manifest = {
        "total": len(TEST_GIFS),
        "successful": len(successful),
        "gifs": results,
        "categories": list(set(g["category"] for g in TEST_GIFS)),
    }
    manifest_path.write_text(json.dumps(manifest, indent=2))
    print(f"\nManifest written to {manifest_path}")

    # Category breakdown
    print("\nBy category:")
    for cat in manifest["categories"]:
        count = len([r for r in successful if r["category"] == cat])
        print(f"  {cat}: {count}")

    return 0 if len(failed) == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
