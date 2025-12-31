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

# Test GIF sources - mix of types
# Using Giphy's public API (no key needed for trending/random)
# and archive.org for public domain content

TEST_GIFS = [
    # Small, simple animations (good for palette reuse)
    {
        "name": "small_simple_01",
        "url": "https://media.giphy.com/media/3o7TKsQ8MgRDoHl4Oc/giphy.gif",
        "category": "simple",
        "expected_size": "small"
    },
    {
        "name": "small_simple_02", 
        "url": "https://media.giphy.com/media/l0MYt5jPR6QX5pnqM/giphy.gif",
        "category": "simple",
        "expected_size": "small"
    },
    # Cartoon / flat color
    {
        "name": "cartoon_01",
        "url": "https://media.giphy.com/media/JIX9t2j0ZTN9S/giphy.gif",
        "category": "cartoon",
        "expected_size": "medium"
    },
    {
        "name": "cartoon_02",
        "url": "https://media.giphy.com/media/3oEjI6SIIHBdRxXI40/giphy.gif", 
        "category": "cartoon",
        "expected_size": "medium"
    },
    # Photographic / complex gradients (stress test)
    {
        "name": "photo_01",
        "url": "https://media.giphy.com/media/l4FGuhL4U2WyjdkaY/giphy.gif",
        "category": "photographic",
        "expected_size": "large"
    },
    {
        "name": "photo_02",
        "url": "https://media.giphy.com/media/3o7TKSjRrfIPjeiVyM/giphy.gif",
        "category": "photographic", 
        "expected_size": "large"
    },
    # High frame count
    {
        "name": "many_frames_01",
        "url": "https://media.giphy.com/media/xT9IgzoKnwFNmISR8I/giphy.gif",
        "category": "many_frames",
        "expected_size": "large"
    },
    # Transparency heavy
    {
        "name": "transparent_01",
        "url": "https://media.giphy.com/media/3o7btPCcdNniyf0ArS/giphy.gif",
        "category": "transparent",
        "expected_size": "medium"
    },
    # Pixel art (should be perfect for palette reuse)
    {
        "name": "pixel_art_01",
        "url": "https://media.giphy.com/media/l41lUJ1YoZB1lHVPG/giphy.gif",
        "category": "pixel_art",
        "expected_size": "small"
    },
    # Large dimensions
    {
        "name": "large_dims_01",
        "url": "https://media.giphy.com/media/3oz8xZvvOZRmKay4xy/giphy.gif",
        "category": "large",
        "expected_size": "large"
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
        req = urllib.request.Request(url, headers={"User-Agent": "rusticle-test-suite/1.0"})
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
            executor.submit(download_gif, gif, output_dir): gif 
            for gif in TEST_GIFS
        }
        
        for future in as_completed(futures):
            results.append(future.result())
    
    # Summary
    print("-" * 50)
    successful = [r for r in results if r["success"]]
    failed = [r for r in results if not r["success"]]
    
    print(f"\nDownloaded: {len(successful)}/{len(TEST_GIFS)}")
    
    if failed:
        print(f"\nFailed downloads:")
        for r in failed:
            print(f"  - {r['name']}: {r['error']}")
    
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
