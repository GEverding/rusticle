#!/usr/bin/env python3
"""Run parameter sweeps for rusticle tuning and emit JSONL artifacts."""

from __future__ import annotations

import argparse
import json
import math
import os
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

DEFAULT_FILTERS = ["nearest", "bilinear", "mitchell", "lanczos3"]
DEFAULT_OPTIMIZES = ["o1", "o2", "o3"]
DEFAULT_LOSSY = [60, 70, 80, 90, 100]


def parse_csv(value: str | None) -> list[str]:
    if value is None:
        return []
    return [part.strip() for part in value.split(",") if part.strip()]


def parse_int_csv(value: str | None) -> list[int]:
    values: list[int] = []
    for raw in parse_csv(value):
        try:
            values.append(int(raw))
        except ValueError as exc:
            raise argparse.ArgumentTypeError(
                f"invalid integer value in list: {raw}"
            ) from exc
    return values


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        default="test_gifs/benchmark_suite/manifest.json",
        help="Input corpus manifest JSON (default: test_gifs/benchmark_suite/manifest.json)",
    )
    parser.add_argument(
        "--rusticle-bin",
        default="target/release/rusticle",
        help="Path to rusticle CLI binary (default: target/release/rusticle)",
    )
    parser.add_argument(
        "--filters",
        default=None,
        help="Comma-separated filters (nearest,bilinear,mitchell,lanczos3)",
    )
    parser.add_argument(
        "--optimizes",
        default=None,
        help="Comma-separated optimize levels (o1,o2,o3)",
    )
    parser.add_argument(
        "--lossy",
        default=None,
        help="Comma-separated lossy values 0-100 (default: 60,70,80,90,100)",
    )
    parser.add_argument(
        "--categories",
        default=None,
        help="Comma-separated manifest categories to include",
    )
    parser.add_argument(
        "--files",
        default=None,
        help="Comma-separated file names to include (with or without .gif)",
    )
    parser.add_argument(
        "--max-configs",
        type=int,
        default=None,
        help="Maximum number of configs to execute",
    )
    parser.add_argument(
        "--repeats",
        type=int,
        default=1,
        help="Repeats per config (default: 1)",
    )
    parser.add_argument(
        "--output",
        default="outputs/sweep_global.jsonl",
        help="JSONL output path (default: outputs/sweep_global.jsonl)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print planned configs/files without executing",
    )
    return parser.parse_args()


def require_binary(path: Path) -> None:
    if not path.exists():
        raise FileNotFoundError(
            f"rusticle binary not found at '{path}'. Build it first: cargo build --release -p rusticle-cli"
        )
    if not path.is_file():
        raise FileNotFoundError(f"rusticle binary path is not a file: '{path}'")
    if not os.access(path, os.X_OK):
        raise PermissionError(f"rusticle binary is not executable: '{path}'")


def parse_manifest(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        raise FileNotFoundError(f"manifest not found: {path}")
    data = json.loads(path.read_text(encoding="utf-8"))
    gifs = data.get("gifs")
    if not isinstance(gifs, list):
        raise ValueError("manifest JSON missing 'gifs' list")
    entries: list[dict[str, Any]] = []
    for item in gifs:
        if not isinstance(item, dict):
            continue
        if not item.get("success", False):
            continue
        file_path = item.get("path")
        name = item.get("name")
        category = item.get("category")
        if (
            not isinstance(file_path, str)
            or not isinstance(name, str)
            or not isinstance(category, str)
        ):
            continue
        entries.append(
            {
                "name": name,
                "category": category,
                "path": file_path,
            }
        )
    entries.sort(key=lambda e: e["name"])
    return entries


def choose_filters(raw: str | None) -> list[str]:
    if raw is None:
        return DEFAULT_FILTERS.copy()
    requested = parse_csv(raw)
    unknown = [value for value in requested if value not in DEFAULT_FILTERS]
    if unknown:
        raise ValueError(f"invalid --filters values: {unknown}")
    return requested


def choose_optimizes(raw: str | None) -> list[str]:
    if raw is None:
        return DEFAULT_OPTIMIZES.copy()
    requested = parse_csv(raw)
    unknown = [value for value in requested if value not in DEFAULT_OPTIMIZES]
    if unknown:
        raise ValueError(f"invalid --optimizes values: {unknown}")
    return requested


def choose_lossy(raw: str | None) -> list[int]:
    if raw is None:
        return DEFAULT_LOSSY.copy()
    requested = parse_int_csv(raw)
    for value in requested:
        if value < 0 or value > 100:
            raise ValueError(f"invalid --lossy value: {value} (expected 0-100)")
    return requested


def normalize_file_token(raw: str) -> str:
    token = raw.strip()
    if token.lower().endswith(".gif"):
        return token[:-4]
    return token


def filter_entries(
    entries: list[dict[str, Any]],
    categories_raw: str | None,
    files_raw: str | None,
) -> list[dict[str, Any]]:
    categories = set(parse_csv(categories_raw))
    files = {normalize_file_token(token) for token in parse_csv(files_raw)}

    selected: list[dict[str, Any]] = []
    for entry in entries:
        if categories and entry["category"] not in categories:
            continue
        if files and entry["name"] not in files:
            continue
        selected.append(entry)
    return selected


def make_configs(
    filters: list[str], optimizes: list[str], lossy_values: list[int]
) -> list[dict[str, Any]]:
    configs: list[dict[str, Any]] = []
    for filter_name in filters:
        for optimize in optimizes:
            for lossy in lossy_values:
                config_id = f"f={filter_name}|opt={optimize}|lossy={lossy}"
                configs.append(
                    {
                        "config_id": config_id,
                        "filter": filter_name,
                        "optimize": optimize,
                        "lossy": lossy,
                    }
                )
    return configs


def percentile(values: list[float], pct: float) -> float:
    if not values:
        raise ValueError("cannot compute percentile of empty list")
    if len(values) == 1:
        return values[0]
    sorted_values = sorted(values)
    rank = (len(sorted_values) - 1) * pct
    low = math.floor(rank)
    high = math.ceil(rank)
    if low == high:
        return sorted_values[low]
    weight = rank - low
    return sorted_values[low] * (1.0 - weight) + sorted_values[high] * weight


def run_command(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, capture_output=True, text=True, check=False)


def parse_quality_summary(output_text: str) -> tuple[float | None, float | None]:
    avg_ba: float | None = None
    worst_ba: float | None = None

    for line in output_text.splitlines():
        stripped = line.strip()
        if stripped.startswith("Avg BA:"):
            parts = stripped.split()
            if len(parts) >= 3:
                try:
                    avg_ba = float(parts[2])
                except ValueError:
                    avg_ba = None
        elif stripped.startswith("Worst BA:"):
            parts = stripped.split()
            if len(parts) >= 3:
                try:
                    worst_ba = float(parts[2])
                except ValueError:
                    worst_ba = None

    return avg_ba, worst_ba


def round_or_none(value: float | None, digits: int = 6) -> float | None:
    if value is None:
        return None
    return round(value, digits)


def run_config(
    rusticle_bin: Path,
    config: dict[str, Any],
    entries: list[dict[str, Any]],
    repeats: int,
) -> dict[str, Any]:
    runtime_ms_samples: list[float] = []
    size_ratio_samples: list[float] = []
    ba_avg_samples: list[float] = []
    ba_worst_samples: list[float] = []

    failed_attempts = 0
    failed_names: set[str] = set()

    category_ba_values: dict[str, list[float]] = {}
    category_ba_worst: dict[str, list[float]] = {}

    with tempfile.TemporaryDirectory(prefix="rusticle_sweep_") as tmpdir:
        tmp_root = Path(tmpdir)
        for repeat_idx in range(repeats):
            for entry in entries:
                src = Path(entry["path"])
                name = entry["name"]
                category = entry["category"]
                out = (
                    tmp_root
                    / f"{name}__{config['filter']}__{config['optimize']}__{config['lossy']}__r{repeat_idx}.gif"
                )

                resize_cmd = [
                    str(rusticle_bin),
                    "resize",
                    str(src),
                    str(out),
                    "--width",
                    "320",
                    "--height",
                    "240",
                    "--filter",
                    str(config["filter"]),
                    "--optimize",
                    str(config["optimize"]),
                    "--lossy",
                    str(config["lossy"]),
                ]

                start = time.perf_counter()
                resize_result = run_command(resize_cmd)
                runtime_ms = (time.perf_counter() - start) * 1000.0

                if resize_result.returncode != 0:
                    failed_attempts += 1
                    failed_names.add(name)
                    sys.stderr.write(
                        f"[warn] resize failed config={config['config_id']} file={name} repeat={repeat_idx + 1}/{repeats}: "
                        f"exit={resize_result.returncode}\n"
                    )
                    if resize_result.stderr:
                        sys.stderr.write(resize_result.stderr)
                        if not resize_result.stderr.endswith("\n"):
                            sys.stderr.write("\n")
                    continue

                runtime_ms_samples.append(runtime_ms)

                try:
                    input_size = src.stat().st_size
                    output_size = out.stat().st_size
                except OSError as exc:
                    failed_attempts += 1
                    failed_names.add(name)
                    sys.stderr.write(
                        f"[warn] size stat failed config={config['config_id']} file={name}: {exc}\n"
                    )
                    continue

                if input_size > 0:
                    size_ratio_samples.append(output_size / input_size)

                quality_cmd = [str(rusticle_bin), "quality", str(src), str(out)]
                quality_result = run_command(quality_cmd)
                if quality_result.returncode != 0:
                    failed_attempts += 1
                    failed_names.add(name)
                    sys.stderr.write(
                        f"[warn] quality failed config={config['config_id']} file={name} repeat={repeat_idx + 1}/{repeats}: "
                        f"exit={quality_result.returncode}\n"
                    )
                    if quality_result.stderr:
                        sys.stderr.write(quality_result.stderr)
                        if not quality_result.stderr.endswith("\n"):
                            sys.stderr.write("\n")
                    continue

                quality_output = f"{quality_result.stdout}\n{quality_result.stderr}"
                avg_ba, worst_ba = parse_quality_summary(quality_output)

                if avg_ba is not None:
                    ba_avg_samples.append(avg_ba)
                    category_ba_values.setdefault(category, []).append(avg_ba)
                if worst_ba is not None:
                    ba_worst_samples.append(worst_ba)
                    category_ba_worst.setdefault(category, []).append(worst_ba)

    runtime_avg = statistics.fmean(runtime_ms_samples) if runtime_ms_samples else None
    runtime_median = (
        statistics.median(runtime_ms_samples) if runtime_ms_samples else None
    )
    size_ratio_avg = (
        statistics.fmean(size_ratio_samples) if size_ratio_samples else None
    )

    avg_ba = statistics.fmean(ba_avg_samples) if ba_avg_samples else None
    p95_ba = percentile(ba_avg_samples, 0.95) if ba_avg_samples else None
    worst_ba = max(ba_worst_samples) if ba_worst_samples else None

    per_category_ba: dict[str, dict[str, Any]] = {}
    for category in sorted({entry["category"] for entry in entries}):
        values = category_ba_values.get(category, [])
        worst_values = category_ba_worst.get(category, [])
        per_category_ba[category] = {
            "samples": len(values),
            "avg_ba": round_or_none(statistics.fmean(values) if values else None),
            "p95_ba": round_or_none(percentile(values, 0.95) if values else None),
            "worst_ba": round_or_none(max(worst_values) if worst_values else None),
        }

    return {
        "config_id": config["config_id"],
        "params": {
            "filter": config["filter"],
            "optimize": config["optimize"],
            "lossy": config["lossy"],
        },
        "files_evaluated_count": len(entries),
        "samples_evaluated_count": len(entries) * repeats,
        "failed_files": len(failed_names),
        "failed_attempts": failed_attempts,
        "runtime_ms": {
            "avg": round_or_none(runtime_avg),
            "median": round_or_none(runtime_median),
        },
        "size_ratio": {
            "avg": round_or_none(size_ratio_avg),
        },
        "ba": {
            "avg": round_or_none(avg_ba),
            "p95": round_or_none(p95_ba),
            "worst": round_or_none(worst_ba),
            "samples": len(ba_avg_samples),
        },
        "per_category_ba": per_category_ba,
    }


def git_commit_hash() -> str:
    result = run_command(["git", "rev-parse", "HEAD"])
    if result.returncode != 0:
        return "unknown"
    value = result.stdout.strip()
    return value if value else "unknown"


def print_dry_run(
    configs: list[dict[str, Any]], entries: list[dict[str, Any]], repeats: int
) -> None:
    print("dry-run: no commands executed")
    print(f"configs: {len(configs)}")
    print(f"files:   {len(entries)}")
    print(f"repeats: {repeats}")
    print()
    print("planned files:")
    for entry in entries:
        print(f"  - {entry['name']}.gif [{entry['category']}] -> {entry['path']}")
    print()
    print("planned configs:")
    for idx, cfg in enumerate(configs, start=1):
        print(
            f"  {idx:03d}. {cfg['config_id']} "
            f"(filter={cfg['filter']}, optimize={cfg['optimize']}, lossy={cfg['lossy']})"
        )


def main() -> int:
    args = parse_args()

    try:
        filters = choose_filters(args.filters)
        optimizes = choose_optimizes(args.optimizes)
        lossy_values = choose_lossy(args.lossy)
    except (ValueError, argparse.ArgumentTypeError) as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1

    if not filters:
        print("Error: empty filter set", file=sys.stderr)
        return 1
    if not optimizes:
        print("Error: empty optimize set", file=sys.stderr)
        return 1
    if not lossy_values:
        print("Error: empty lossy set", file=sys.stderr)
        return 1
    if args.repeats < 1:
        print("Error: --repeats must be >= 1", file=sys.stderr)
        return 1

    rusticle_bin = Path(args.rusticle_bin)
    if not args.dry_run:
        try:
            require_binary(rusticle_bin)
        except (FileNotFoundError, PermissionError) as exc:
            print(f"Error: {exc}", file=sys.stderr)
            return 1

    try:
        entries = parse_manifest(Path(args.manifest))
    except (FileNotFoundError, OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1

    entries = filter_entries(entries, args.categories, args.files)
    if not entries:
        print("Error: no files selected after applying filters", file=sys.stderr)
        return 1

    configs = make_configs(filters, optimizes, lossy_values)
    if args.max_configs is not None:
        if args.max_configs < 0:
            print("Error: --max-configs must be >= 0", file=sys.stderr)
            return 1
        configs = configs[: args.max_configs]

    if not configs:
        print("Error: no configs to run", file=sys.stderr)
        return 1

    if args.dry_run:
        print_dry_run(configs, entries, args.repeats)
        return 0

    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)

    commit = git_commit_hash()

    with output_path.open("a", encoding="utf-8") as out_file:
        for idx, config in enumerate(configs, start=1):
            print(f"[{idx}/{len(configs)}] running {config['config_id']}")
            record = run_config(rusticle_bin, config, entries, args.repeats)
            record["timestamp"] = datetime.now(timezone.utc).isoformat()
            record["git_commit"] = commit
            out_file.write(json.dumps(record, sort_keys=True) + "\n")
            out_file.flush()

            print(
                f"  files={record['files_evaluated_count']} failed={record['failed_files']} "
                f"runtime_avg_ms={record['runtime_ms']['avg']} ba_avg={record['ba']['avg']}"
            )

    print(f"wrote results to {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
