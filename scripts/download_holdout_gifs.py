#!/usr/bin/env python3
"""Download a holdout GIF suite disjoint from benchmark corpus MD5s."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Iterable

DEFAULT_BENCHMARK_MANIFEST = "test_gifs/benchmark_suite/manifest.json"
DEFAULT_OUTPUT_DIR = "test_gifs/holdout_suite"
DEFAULT_TARGET = 39
DEFAULT_MIN_WIDTH = 160
DEFAULT_MIN_HEIGHT = 120

WIKIMEDIA_API = "https://commons.wikimedia.org/w/api.php"
WIKIMEDIA_QUERIES = [
    "filetype:bitmap gif animation",
    "filetype:bitmap gif cartoon",
    "filetype:bitmap gif nature",
    "filetype:bitmap gif sports",
    "filetype:bitmap gif dance",
    "filetype:bitmap gif science",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=int, default=DEFAULT_TARGET)
    parser.add_argument("--min-width", type=int, default=DEFAULT_MIN_WIDTH)
    parser.add_argument("--min-height", type=int, default=DEFAULT_MIN_HEIGHT)
    parser.add_argument(
        "--benchmark-manifest",
        default=DEFAULT_BENCHMARK_MANIFEST,
        help="Path to benchmark suite manifest for overlap exclusion",
    )
    parser.add_argument(
        "--output-dir",
        default=DEFAULT_OUTPUT_DIR,
        help="Directory to write holdout GIFs + manifest",
    )
    parser.add_argument(
        "--max-attempts",
        type=int,
        default=1600,
        help="Stop after this many candidate downloads",
    )
    return parser.parse_args()


def parse_gif_dimensions(data: bytes) -> tuple[int, int] | None:
    if len(data) < 10:
        return None
    if not (data.startswith(b"GIF87a") or data.startswith(b"GIF89a")):
        return None
    width = int.from_bytes(data[6:8], "little")
    height = int.from_bytes(data[8:10], "little")
    return width, height


def load_excluded_md5s(manifest_path: Path) -> set[str]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    gifs = manifest.get("gifs")
    if not isinstance(gifs, list):
        raise ValueError("benchmark manifest missing 'gifs' list")
    excluded: set[str] = set()
    for item in gifs:
        if not isinstance(item, dict) or not item.get("success"):
            continue
        md5 = item.get("md5")
        if isinstance(md5, str) and md5:
            excluded.add(md5)
    return excluded


def sanitize_name(raw: str) -> str:
    name = re.sub(r"^File:", "", raw, flags=re.IGNORECASE)
    name = re.sub(r"\.gif$", "", name, flags=re.IGNORECASE)
    name = re.sub(r"[^a-zA-Z0-9]+", "_", name).strip("_").lower()
    return name or "holdout_gif"


def json_get(url: str, timeout: int = 30) -> dict[str, Any]:
    req = urllib.request.Request(url, headers={"User-Agent": "rusticle-holdout/1.0"})
    with urllib.request.urlopen(req, timeout=timeout) as response:
        return json.loads(response.read().decode("utf-8"))


def iter_wikimedia_candidates() -> Iterable[dict[str, str]]:
    seen_urls: set[str] = set()
    for query in WIKIMEDIA_QUERIES:
        cursor: str | None = None
        while True:
            params: dict[str, str] = {
                "action": "query",
                "format": "json",
                "generator": "search",
                "gsrsearch": query,
                "gsrnamespace": "6",
                "gsrlimit": "50",
                "gsrinfo": "",
                "prop": "imageinfo",
                "iiprop": "url|mime",
                "iiurlwidth": "1",
            }
            if cursor:
                params["gsroffset"] = cursor

            url = f"{WIKIMEDIA_API}?{urllib.parse.urlencode(params)}"
            payload = json_get(url)
            pages = payload.get("query", {}).get("pages", {})

            for page in pages.values():
                if not isinstance(page, dict):
                    continue
                title = page.get("title")
                image_info = page.get("imageinfo")
                if not isinstance(title, str) or not isinstance(image_info, list):
                    continue
                if not image_info:
                    continue
                info0 = image_info[0]
                if not isinstance(info0, dict):
                    continue
                url_value = info0.get("url")
                mime = info0.get("mime")
                if mime != "image/gif" or not isinstance(url_value, str):
                    continue
                if url_value in seen_urls:
                    continue
                seen_urls.add(url_value)
                yield {
                    "title": title,
                    "url": url_value,
                    "category": "wikimedia",
                }

            continuation = payload.get("continue", {})
            next_offset = continuation.get("gsroffset")
            if not isinstance(next_offset, int):
                break
            cursor = str(next_offset)


def download_bytes(url: str, timeout: int = 45) -> bytes:
    req = urllib.request.Request(url, headers={"User-Agent": "rusticle-holdout/1.0"})
    with urllib.request.urlopen(req, timeout=timeout) as response:
        return response.read()


def main() -> int:
    args = parse_args()

    benchmark_manifest = Path(args.benchmark_manifest)
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    if not benchmark_manifest.exists():
        print(f"benchmark manifest not found: {benchmark_manifest}", file=sys.stderr)
        return 1

    excluded_md5s = load_excluded_md5s(benchmark_manifest)
    accepted_md5s: set[str] = set()
    accepted_files = 0
    overlap_count = 0
    attempts = 0
    min_width_seen: int | None = None
    min_height_seen: int | None = None
    results: list[dict[str, Any]] = []
    existing_names: set[str] = set()

    for candidate in iter_wikimedia_candidates():
        if accepted_files >= args.target or attempts >= args.max_attempts:
            break
        attempts += 1

        base_name = sanitize_name(candidate["title"])
        final_name = base_name
        suffix = 2
        while final_name in existing_names:
            final_name = f"{base_name}_{suffix}"
            suffix += 1
        existing_names.add(final_name)

        out_path = output_dir / f"{final_name}.gif"
        result: dict[str, Any] = {
            "name": final_name,
            "url": candidate["url"],
            "path": str(out_path.resolve()),
            "category": candidate.get("category"),
            "success": False,
            "size": None,
            "md5": None,
            "width": None,
            "height": None,
            "exclusion_reason": None,
        }

        try:
            data = download_bytes(candidate["url"])
            dims = parse_gif_dimensions(data)
            if dims is None:
                result["exclusion_reason"] = "invalid_gif_header"
                results.append(result)
                continue

            width, height = dims
            md5 = hashlib.md5(data).hexdigest()
            result["size"] = len(data)
            result["md5"] = md5
            result["width"] = width
            result["height"] = height

            if md5 in excluded_md5s:
                overlap_count += 1
                result["exclusion_reason"] = "md5_overlap_with_benchmark_suite"
            elif md5 in accepted_md5s:
                result["exclusion_reason"] = "duplicate_md5_in_holdout"
            elif width <= args.min_width or height <= args.min_height:
                result["exclusion_reason"] = (
                    f"dimensions_not_gt_{args.min_width}x{args.min_height}"
                )
            else:
                out_path.write_bytes(data)
                result["success"] = True
                accepted_md5s.add(md5)
                accepted_files += 1
                min_width_seen = (
                    width if min_width_seen is None else min(min_width_seen, width)
                )
                min_height_seen = (
                    height if min_height_seen is None else min(min_height_seen, height)
                )
                print(
                    f"✓ {accepted_files:02d}/{args.target} {final_name} "
                    f"{width}x{height} {len(data)} bytes"
                )
            results.append(result)
        except Exception as exc:  # noqa: BLE001
            result["exclusion_reason"] = f"download_error:{exc}"
            results.append(result)

    manifest = {
        "target": args.target,
        "successful": accepted_files,
        "gifs": results,
    }
    manifest_path = output_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2), encoding="utf-8")

    min_dims = (
        f"{min_width_seen}x{min_height_seen}"
        if min_width_seen is not None and min_height_seen is not None
        else "n/a"
    )
    print("-" * 72)
    print(f"downloaded count: {accepted_files}")
    print(f"unique md5: {len(accepted_md5s)}")
    print(f"overlap count: {overlap_count}")
    print(f"min dimensions: {min_dims}")
    print(f"manifest: {manifest_path}")

    if accepted_files < args.target:
        print(
            f"warning: collected {accepted_files}/{args.target}; increase --max-attempts or add sources",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
