#!/usr/bin/env python3
"""Benchmark gifsicle/rusticle profiles on holdout corpus."""

from __future__ import annotations

import argparse
import json
import re
import statistics
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest",
        default="test_gifs/holdout_suite/manifest.json",
        help="Holdout manifest path",
    )
    parser.add_argument("--width", type=int, default=160)
    parser.add_argument("--height", type=int, default=120)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument(
        "--results-output",
        default="outputs/holdout_profile_results.json",
        help="Per-file detailed output JSON",
    )
    parser.add_argument(
        "--summary-output",
        default="outputs/holdout_profile_summary.json",
        help="Aggregate output JSON",
    )
    parser.add_argument(
        "--rusticle-bin",
        default="target/release/rusticle",
        help="rusticle binary path",
    )
    parser.add_argument(
        "--gifsicle-bin", default="gifsicle", help="gifsicle binary path"
    )
    return parser.parse_args()


def parse_manifest(manifest_path: Path) -> list[dict[str, Any]]:
    payload = json.loads(manifest_path.read_text(encoding="utf-8"))
    gifs = payload.get("gifs")
    if not isinstance(gifs, list):
        raise ValueError("manifest missing gifs list")
    entries: list[dict[str, Any]] = []
    for item in gifs:
        if not isinstance(item, dict) or not item.get("success"):
            continue
        path = item.get("path")
        name = item.get("name")
        width = item.get("width")
        height = item.get("height")
        md5 = item.get("md5")
        if not isinstance(path, str) or not isinstance(name, str):
            continue
        entries.append(
            {
                "name": name,
                "path": path,
                "width": width,
                "height": height,
                "md5": md5,
            }
        )
    entries.sort(key=lambda e: e["name"])
    return entries


def run_cmd(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, capture_output=True, text=True, check=False)


def parse_quality(text: str) -> dict[str, float | None]:
    patterns = {
        "avg_psnr": r"Avg PSNR:\s+([0-9.]+)",
        "avg_ssim": r"Avg SSIM:\s+([0-9.]+)",
        "avg_ba": r"Avg BA:\s+([0-9.]+)",
        "worst_ba": r"Worst BA:\s+([0-9.]+)",
    }
    metrics: dict[str, float | None] = {}
    for key, pattern in patterns.items():
        match = re.search(pattern, text)
        metrics[key] = float(match.group(1)) if match else None
    return metrics


def median_index(values: list[float]) -> int:
    ordered = sorted(enumerate(values), key=lambda item: item[1])
    return ordered[len(ordered) // 2][0]


def profile_commands(
    profile: str,
    src: Path,
    out: Path,
    width: int,
    height: int,
    rusticle_bin: str,
    gifsicle_bin: str,
) -> list[str]:
    if profile == "gifsicle_baseline":
        return [
            gifsicle_bin,
            str(src),
            "--resize",
            f"{width}x{height}",
            "-O3",
            "--lossy=80",
            "-o",
            str(out),
        ]
    if profile == "rusticle_default":
        return [
            rusticle_bin,
            "resize",
            str(src),
            str(out),
            "--width",
            str(width),
            "--height",
            str(height),
            "--filter",
            "lanczos3",
            "--optimize",
            "o3",
            "--lossy",
            "80",
        ]
    if profile == "rusticle_optimized_global":
        return [
            rusticle_bin,
            "resize",
            str(src),
            str(out),
            "--width",
            str(width),
            "--height",
            str(height),
            "--filter",
            "lanczos3",
            "--optimize",
            "o1",
            "--lossy",
            "100",
        ]
    raise ValueError(f"unknown profile: {profile}")


def run_profile(
    entry: dict[str, Any],
    profile: str,
    width: int,
    height: int,
    repeats: int,
    rusticle_bin: str,
    gifsicle_bin: str,
) -> dict[str, Any]:
    src = Path(entry["path"])
    runtimes_ms: list[float] = []
    sizes: list[int] = []

    with tempfile.TemporaryDirectory(prefix="holdout_profile_") as tmpdir:
        tmp = Path(tmpdir)
        outputs: list[Path] = []
        for repeat in range(repeats):
            out = tmp / f"{entry['name']}__{profile}__r{repeat}.gif"
            cmd = profile_commands(
                profile,
                src,
                out,
                width,
                height,
                rusticle_bin,
                gifsicle_bin,
            )
            start = time.perf_counter()
            result = run_cmd(cmd)
            elapsed_ms = (time.perf_counter() - start) * 1000.0
            if result.returncode != 0:
                raise RuntimeError(
                    f"{profile} failed for {entry['name']}: exit={result.returncode} stderr={result.stderr.strip()}"
                )
            runtimes_ms.append(elapsed_ms)
            sizes.append(out.stat().st_size)
            outputs.append(out)

        median_run_idx = median_index(runtimes_ms)
        representative_out = outputs[median_run_idx]

        quality_cmd = [rusticle_bin, "quality", str(src), str(representative_out)]
        quality_result = run_cmd(quality_cmd)
        quality_error: str | None = None
        if quality_result.returncode != 0:
            quality = {
                "avg_psnr": None,
                "avg_ssim": None,
                "avg_ba": None,
                "worst_ba": None,
            }
            quality_error = f"exit={quality_result.returncode} stderr={quality_result.stderr.strip()}"
        else:
            quality = parse_quality(f"{quality_result.stdout}\n{quality_result.stderr}")

    return {
        "median_runtime_ms": statistics.median(runtimes_ms),
        "median_output_bytes": int(statistics.median(sizes)),
        "quality": quality,
        "quality_error": quality_error,
        "runtime_samples_ms": runtimes_ms,
        "output_size_samples": sizes,
    }


def mean(values: list[float]) -> float | None:
    if not values:
        return None
    return sum(values) / len(values)


def aggregate(profile_rows: list[dict[str, Any]]) -> dict[str, float | int | None]:
    psnr = [
        row["quality"]["avg_psnr"]
        for row in profile_rows
        if row["quality"]["avg_psnr"] is not None
    ]
    ssim = [
        row["quality"]["avg_ssim"]
        for row in profile_rows
        if row["quality"]["avg_ssim"] is not None
    ]
    avg_ba_values = [
        row["quality"]["avg_ba"]
        for row in profile_rows
        if row["quality"]["avg_ba"] is not None
    ]
    worst_ba_values = [
        row["quality"]["worst_ba"]
        for row in profile_rows
        if row["quality"]["worst_ba"] is not None
    ]
    runtime = [float(row["median_runtime_ms"]) for row in profile_rows]
    output_bytes = [float(row["median_output_bytes"]) for row in profile_rows]

    return {
        "count": len(profile_rows),
        "quality_samples": len(avg_ba_values),
        "quality_failures": len(profile_rows) - len(avg_ba_values),
        "avg_psnr": mean(psnr),
        "avg_ssim": mean(ssim),
        "avg_ba": mean(avg_ba_values),
        "worst_ba": max(worst_ba_values) if worst_ba_values else None,
        "avg_runtime_ms": mean(runtime),
        "avg_output_bytes": mean(output_bytes),
    }


def main() -> int:
    args = parse_args()
    manifest_path = Path(args.manifest)
    entries = parse_manifest(manifest_path)
    if not entries:
        raise ValueError("no successful entries found in holdout manifest")

    profiles = [
        "gifsicle_baseline",
        "rusticle_default",
        "rusticle_optimized_global",
    ]

    per_file_results: list[dict[str, Any]] = []
    for idx, entry in enumerate(entries, start=1):
        print(f"[{idx}/{len(entries)}] {entry['name']}")
        row = {
            "name": entry["name"],
            "path": entry["path"],
            "width": entry.get("width"),
            "height": entry.get("height"),
            "profiles": {},
        }
        for profile in profiles:
            row["profiles"][profile] = run_profile(
                entry,
                profile,
                args.width,
                args.height,
                args.repeats,
                args.rusticle_bin,
                args.gifsicle_bin,
            )
        per_file_results.append(row)

    summary: dict[str, Any] = {
        "manifest": str(manifest_path.resolve()),
        "width": args.width,
        "height": args.height,
        "repeats": args.repeats,
        "profiles": {},
    }
    for profile in profiles:
        profile_rows = [row["profiles"][profile] for row in per_file_results]
        summary["profiles"][profile] = aggregate(profile_rows)

    results_output = Path(args.results_output)
    results_output.parent.mkdir(parents=True, exist_ok=True)
    results_output.write_text(json.dumps(per_file_results, indent=2), encoding="utf-8")

    summary_output = Path(args.summary_output)
    summary_output.parent.mkdir(parents=True, exist_ok=True)
    summary_output.write_text(json.dumps(summary, indent=2), encoding="utf-8")

    ranking = sorted(
        summary["profiles"].items(),
        key=lambda item: (
            float("inf") if item[1]["avg_ba"] is None else item[1]["avg_ba"],
            float("inf")
            if item[1]["avg_runtime_ms"] is None
            else item[1]["avg_runtime_ms"],
        ),
    )

    print("\nProfile ranking (lower BA/runtime is better)")
    print("profile                        avg_ba  avg_runtime_ms  worst_ba  avg_bytes")
    print("-" * 76)
    for profile, metrics in ranking:
        avg_ba = metrics["avg_ba"]
        avg_runtime = metrics["avg_runtime_ms"]
        worst_ba = metrics["worst_ba"]
        avg_bytes = metrics["avg_output_bytes"]
        print(
            f"{profile:30} "
            f"{(avg_ba if avg_ba is not None else float('nan')):7.3f} "
            f"{(avg_runtime if avg_runtime is not None else float('nan')):14.2f} "
            f"{(worst_ba if worst_ba is not None else float('nan')):8.3f} "
            f"{int(avg_bytes) if avg_bytes is not None else 0:10d}"
        )

    print(f"\nWrote results: {results_output}")
    print(f"Wrote summary: {summary_output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
