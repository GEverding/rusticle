#!/usr/bin/env python3
"""Generate a deterministic stratified train/validate split for GIF tuning."""

from __future__ import annotations

import argparse
import json
import random
from collections import defaultdict
from datetime import UTC, datetime
from pathlib import Path


DEFAULT_SEED = 20260418
DEFAULT_MANIFEST = "test_gifs/benchmark_suite/manifest.json"
DEFAULT_OUTPUT = "scripts/corpus_split.json"
TARGET_VALIDATE_RATIO = 0.25


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate deterministic corpus train/validate split"
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=DEFAULT_SEED,
        help=f"Deterministic RNG seed (default: {DEFAULT_SEED})",
    )
    parser.add_argument(
        "--manifest",
        default=DEFAULT_MANIFEST,
        help=f"Path to manifest JSON (default: {DEFAULT_MANIFEST})",
    )
    parser.add_argument(
        "--output",
        default=DEFAULT_OUTPUT,
        help=f"Output split JSON path (default: {DEFAULT_OUTPUT})",
    )
    return parser.parse_args()


def normalize_entry_path(path_text: str, repo_root: Path) -> str:
    path = Path(path_text)
    if path.is_absolute():
        try:
            return path.relative_to(repo_root).as_posix()
        except ValueError:
            return path.as_posix()
    return path.as_posix()


def load_successful_entries(
    manifest_path: Path, repo_root: Path
) -> list[dict[str, str]]:
    with manifest_path.open("r", encoding="utf-8") as f:
        manifest = json.load(f)

    entries: list[dict[str, str]] = []
    for item in manifest.get("gifs", []):
        if not item.get("success", False):
            continue

        category = item.get("category")
        file_path = item.get("path")
        if not category or not file_path:
            continue

        entries.append(
            {
                "category": str(category),
                "path": normalize_entry_path(str(file_path), repo_root),
            }
        )

    entries.sort(key=lambda item: (item["category"], item["path"]))
    return entries


def compute_validate_targets(
    category_to_files: dict[str, list[str]],
    target_validate_total: int,
) -> dict[str, int]:
    """Allocate validate counts by category.

    Rules:
    - Category size <= 1: 0 validate samples.
    - Category size > 1: at least 1 validate sample.
    - Then add additional validate samples to approach target total.
    """
    targets: dict[str, int] = {}

    for category, files in category_to_files.items():
        n = len(files)
        targets[category] = 0 if n <= 1 else 1

    current = sum(targets.values())
    if current >= target_validate_total:
        return targets

    remaining = target_validate_total - current

    # Allocate extra validation samples by largest fractional remainder.
    candidates: list[tuple[float, str]] = []
    for category, files in category_to_files.items():
        n = len(files)
        max_extra = n - 1 - targets[category]
        if max_extra <= 0:
            continue

        desired = n * TARGET_VALIDATE_RATIO
        remainder = desired - int(desired)
        candidates.append((remainder, category))

    # Deterministic tie break by category name.
    candidates.sort(key=lambda item: (-item[0], item[1]))

    while remaining > 0 and candidates:
        progressed = False
        for _, category in candidates:
            n = len(category_to_files[category])
            if targets[category] < n - 1:
                targets[category] += 1
                remaining -= 1
                progressed = True
                if remaining == 0:
                    break
        if not progressed:
            break

    return targets


def build_stats(
    split_files: list[dict[str, str]], categories: list[str]
) -> dict[str, object]:
    counts = {category: 0 for category in categories}
    for item in split_files:
        counts[item["category"]] += 1

    return {
        "total": len(split_files),
        "categories": counts,
    }


def load_existing_generated_at(output_path: Path) -> str | None:
    if not output_path.exists():
        return None

    try:
        with output_path.open("r", encoding="utf-8") as f:
            existing = json.load(f)
        value = existing.get("generated_at")
        return value if isinstance(value, str) and value else None
    except (OSError, json.JSONDecodeError):
        return None


def now_utc_iso() -> str:
    return datetime.now(UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def main() -> int:
    args = parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    manifest_path = Path(args.manifest)
    if not manifest_path.is_absolute():
        manifest_path = (repo_root / manifest_path).resolve()

    output_path = Path(args.output)
    if not output_path.is_absolute():
        output_path = (repo_root / output_path).resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)

    successful_entries = load_successful_entries(manifest_path, repo_root)
    total_files = len(successful_entries)

    category_to_files: dict[str, list[str]] = defaultdict(list)
    for item in successful_entries:
        category_to_files[item["category"]].append(item["path"])

    categories = sorted(category_to_files.keys())

    target_validate_total = round(total_files * TARGET_VALIDATE_RATIO)
    validate_targets = compute_validate_targets(
        category_to_files, target_validate_total
    )

    rng = random.Random(args.seed)
    train_items: list[dict[str, str]] = []
    validate_items: list[dict[str, str]] = []
    categories_without_validate: list[str] = []

    for category in categories:
        files = sorted(category_to_files[category])
        shuffled = list(files)
        rng.shuffle(shuffled)

        validate_count = validate_targets.get(category, 0)
        if validate_count == 0:
            categories_without_validate.append(category)

        validate_set = set(shuffled[:validate_count])
        for file_path in files:
            item = {"category": category, "path": file_path}
            if file_path in validate_set:
                validate_items.append(item)
            else:
                train_items.append(item)

    train_items.sort(key=lambda item: item["path"])
    validate_items.sort(key=lambda item: item["path"])

    split_payload_without_generated_at = {
        "method": "stratified_by_category_v1",
        "seed": args.seed,
        "source_manifest": manifest_path.relative_to(repo_root).as_posix(),
        "total_files": total_files,
        "train": [item["path"] for item in train_items],
        "validate": [item["path"] for item in validate_items],
        "stats": {
            "target_validate_ratio": TARGET_VALIDATE_RATIO,
            "train": build_stats(train_items, categories),
            "validate": build_stats(validate_items, categories),
            "categories_without_validate": categories_without_validate,
        },
    }

    generated_at = load_existing_generated_at(output_path) or now_utc_iso()

    split_payload = {
        "generated_at": generated_at,
        **split_payload_without_generated_at,
    }

    with output_path.open("w", encoding="utf-8") as f:
        json.dump(split_payload, f, indent=2)
        f.write("\n")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
