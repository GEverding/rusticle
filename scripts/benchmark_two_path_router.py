#!/usr/bin/env python3
"""Benchmark two-path router against default and gifsicle on offenders + holdout."""

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
        "--offenders-manifest",
        default="test_gifs/holdout_suite/offenders_manifest.json",
        help="Offenders manifest path",
    )
    parser.add_argument(
        "--holdout-manifest",
        default="test_gifs/holdout_suite/manifest.json",
        help="Holdout manifest path",
    )
    parser.add_argument("--width", type=int, default=160)
    parser.add_argument("--height", type=int, default=120)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument(
        "--results-output",
        default="outputs/two_path_router_results.json",
        help="Per-file detailed output JSON",
    )
    parser.add_argument(
        "--summary-output",
        default="outputs/two_path_router_summary.json",
        help="Aggregate output JSON",
    )
    parser.add_argument(
        "--markdown-output",
        default="outputs/two_path_router_report.md",
        help="Human-readable markdown report",
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


def parse_telemetry(stderr: str) -> dict[str, Any]:
    """Parse two-path router telemetry from stderr."""
    telemetry: dict[str, Any] = {
        "selected_path": None,
        "fallback_used": False,
        "fallback_reason": None,
        "classification_features": {},
        "reasons": [],
    }

    # Extract selected path
    path_match = re.search(r"selected_path=([^\n]+)", stderr)
    if path_match:
        path_str = path_match.group(1).strip()
        if "Path A" in path_str:
            telemetry["selected_path"] = "Path A"
        elif "Path B" in path_str:
            telemetry["selected_path"] = "Path B"

    # Extract classification features
    features_match = re.search(r"classification_features: (.+?)(?:\n|$)", stderr)
    if features_match:
        features_str = features_match.group(1)
        # Parse individual features
        for feature in [
            "has_transparent_gce",
            "keep_none_disposal_ratio",
            "palette_stability",
            "offset_patch_ratio",
            "median_changed_area_ratio",
        ]:
            pattern = f"{feature}=([^,\\s]+)"
            match = re.search(pattern, features_str)
            if match:
                val = match.group(1)
                try:
                    telemetry["classification_features"][feature] = (
                        float(val) if "." in val else val == "true"
                    )
                except ValueError:
                    telemetry["classification_features"][feature] = val

    # Extract reasons
    for line in stderr.split("\n"):
        if "[two-path-router] reason:" in line:
            reason = line.split("reason:", 1)[1].strip()
            telemetry["reasons"].append(reason)

    # Check for fallback
    if "fallback" in stderr.lower():
        telemetry["fallback_used"] = True
        fallback_match = re.search(r"fallback.*?:\s*(.+?)(?:\n|$)", stderr)
        if fallback_match:
            telemetry["fallback_reason"] = fallback_match.group(1).strip()

    return telemetry


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
    if profile == "rusticle_two_path_auto":
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
            "--optimizer-strategy",
            "auto",
            "--optimizer-telemetry",
        ]
    if profile == "rusticle_two_path_forced_a":
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
            "--optimizer-strategy",
            "path-a",
        ]
    if profile == "rusticle_two_path_forced_b":
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
            "--optimizer-strategy",
            "path-b",
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
    telemetries: list[dict[str, Any]] = []

    with tempfile.TemporaryDirectory(prefix="two_path_bench_") as tmpdir:
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

            # Parse telemetry if available
            if "two_path" in profile:
                telemetries.append(parse_telemetry(result.stderr))

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

    # Aggregate telemetry
    aggregated_telemetry = None
    if telemetries:
        # Count path selections
        path_a_count = sum(1 for t in telemetries if t.get("selected_path") == "Path A")
        path_b_count = sum(1 for t in telemetries if t.get("selected_path") == "Path B")
        fallback_count = sum(1 for t in telemetries if t.get("fallback_used"))

        aggregated_telemetry = {
            "path_a_count": path_a_count,
            "path_b_count": path_b_count,
            "path_a_rate": path_a_count / len(telemetries) if telemetries else 0,
            "path_b_rate": path_b_count / len(telemetries) if telemetries else 0,
            "fallback_count": fallback_count,
            "fallback_rate": fallback_count / len(telemetries) if telemetries else 0,
            "sample_telemetry": telemetries[median_run_idx] if telemetries else None,
        }

    return {
        "median_runtime_ms": statistics.median(runtimes_ms),
        "median_output_bytes": int(statistics.median(sizes)),
        "quality": quality,
        "quality_error": quality_error,
        "runtime_samples_ms": runtimes_ms,
        "output_size_samples": sizes,
        "telemetry": aggregated_telemetry,
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

    # Aggregate telemetry
    total_path_a = sum(
        row["telemetry"]["path_a_count"]
        for row in profile_rows
        if row["telemetry"] is not None
    )
    total_path_b = sum(
        row["telemetry"]["path_b_count"]
        for row in profile_rows
        if row["telemetry"] is not None
    )
    total_fallback = sum(
        row["telemetry"]["fallback_count"]
        for row in profile_rows
        if row["telemetry"] is not None
    )
    total_repeats = sum(len(row["runtime_samples_ms"]) for row in profile_rows)

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
        "path_a_count": total_path_a,
        "path_b_count": total_path_b,
        "fallback_count": total_fallback,
        "path_a_rate": total_path_a / total_repeats if total_repeats > 0 else 0,
        "path_b_rate": total_path_b / total_repeats if total_repeats > 0 else 0,
        "fallback_rate": total_fallback / total_repeats if total_repeats > 0 else 0,
    }


def write_markdown_report(
    summary: dict[str, Any],
    per_file: list[dict[str, Any]],
    output_path: Path,
) -> None:
    """Write human-readable markdown report."""
    lines = [
        "# Two-Path Router Benchmark Report\n",
        f"Generated: {time.strftime('%Y-%m-%d %H:%M:%S')}\n",
        "\n## Summary\n",
        "This report evaluates the two-path optimizer routing strategy against the current default\n",
        "and gifsicle baseline on known offenders and the 39-image holdout corpus.\n",
        "\n### Profiles Tested\n",
        "1. **gifsicle_baseline**: gifsicle -O3 --lossy=80\n",
        "2. **rusticle_default**: Current default (no routing)\n",
        "3. **rusticle_two_path_auto**: Classifier-driven router (Path A/B)\n",
        "4. **rusticle_two_path_forced_a**: Forced Path A (conservative opaque-delta)\n",
        "5. **rusticle_two_path_forced_b**: Forced Path B (general sparse/transparent)\n",
        "\n## Aggregate Metrics\n",
        "\n| Profile | Avg BA | Worst BA | Avg PSNR | Avg SSIM | Avg Runtime (ms) | Avg Bytes |\n",
        "|---------|--------|----------|----------|----------|------------------|----------|\n",
    ]

    profiles = [
        "gifsicle_baseline",
        "rusticle_default",
        "rusticle_two_path_auto",
        "rusticle_two_path_forced_a",
        "rusticle_two_path_forced_b",
    ]

    for profile in profiles:
        if profile not in summary["profiles"]:
            continue
        metrics = summary["profiles"][profile]
        avg_ba = metrics.get("avg_ba")
        worst_ba = metrics.get("worst_ba")
        avg_psnr = metrics.get("avg_psnr")
        avg_ssim = metrics.get("avg_ssim")
        avg_runtime = metrics.get("avg_runtime_ms")
        avg_bytes = metrics.get("avg_output_bytes")

        lines.append(
            f"| {profile:30} | "
            f"{(avg_ba if avg_ba is not None else float('nan')):7.3f} | "
            f"{(worst_ba if worst_ba is not None else float('nan')):8.3f} | "
            f"{(avg_psnr if avg_psnr is not None else float('nan')):8.2f} | "
            f"{(avg_ssim if avg_ssim is not None else float('nan')):8.4f} | "
            f"{(avg_runtime if avg_runtime is not None else float('nan')):16.2f} | "
            f"{int(avg_bytes) if avg_bytes is not None else 0:10d} |\n"
        )

    lines.append("\n## Path Selection Analysis\n")
    lines.append(
        "| Profile | Path A Count | Path A Rate | Path B Count | Path B Rate | Fallback Count | Fallback Rate |\n"
    )
    lines.append(
        "|---------|--------------|-------------|--------------|-------------|----------------|---------------|\n"
    )

    for profile in profiles:
        if profile not in summary["profiles"]:
            continue
        metrics = summary["profiles"][profile]
        if "path_a_count" not in metrics:
            continue

        path_a_count = metrics.get("path_a_count", 0)
        path_a_rate = metrics.get("path_a_rate", 0)
        path_b_count = metrics.get("path_b_count", 0)
        path_b_rate = metrics.get("path_b_rate", 0)
        fallback_count = metrics.get("fallback_count", 0)
        fallback_rate = metrics.get("fallback_rate", 0)

        lines.append(
            f"| {profile:30} | "
            f"{path_a_count:12d} | {path_a_rate:10.1%} | "
            f"{path_b_count:12d} | {path_b_rate:10.1%} | "
            f"{fallback_count:14d} | {fallback_rate:13.1%} |\n"
        )

    lines.append("\n## Per-File Results\n")
    lines.append("### Offenders\n")

    offender_names = {
        "790106_0203_voyager_58m_to_31m_reduced",
        "galilean_moon_laplace_resonance_animation_2",
        "trapezius_animation_small2",
    }

    for file_result in per_file:
        if file_result["name"] not in offender_names:
            continue
        lines.append(f"\n#### {file_result['name']}\n")
        lines.append(
            "| Profile | Avg BA | Worst BA | Avg Runtime (ms) | Output Bytes |\n"
        )
        lines.append(
            "|---------|--------|----------|------------------|---------------|\n"
        )

        for profile in profiles:
            if profile not in file_result["profiles"]:
                continue
            prof_data = file_result["profiles"][profile]
            avg_ba = prof_data["quality"].get("avg_ba")
            worst_ba = prof_data["quality"].get("worst_ba")
            runtime = prof_data.get("median_runtime_ms")
            output_bytes = prof_data.get("median_output_bytes")

            lines.append(
                f"| {profile:30} | "
                f"{(avg_ba if avg_ba is not None else float('nan')):7.3f} | "
                f"{(worst_ba if worst_ba is not None else float('nan')):8.3f} | "
                f"{(runtime if runtime is not None else float('nan')):16.2f} | "
                f"{output_bytes if output_bytes is not None else 0:13d} |\n"
            )

            # Add telemetry if available
            if prof_data.get("telemetry"):
                telem = prof_data["telemetry"]
                lines.append(
                    f"  - Path A: {telem['path_a_count']} ({telem['path_a_rate']:.1%}), "
                    f"Path B: {telem['path_b_count']} ({telem['path_b_rate']:.1%}), "
                    f"Fallback: {telem['fallback_count']} ({telem['fallback_rate']:.1%})\n"
                )

    lines.append("\n## Conclusions\n")
    lines.append(
        "- **Path A Selection Rate**: Indicates how often the classifier routes to the conservative opaque-delta path\n"
    )
    lines.append(
        "- **Path B Selection Rate**: Indicates how often the classifier routes to the general sparse/transparent path\n"
    )
    lines.append(
        "- **Fallback Rate**: Indicates how often Path A failed and fell back to Path B or legacy\n"
    )
    lines.append(
        "- **Quality Metrics**: PSNR/SSIM/BA measure perceptual quality; lower BA is better\n"
    )
    lines.append(
        "- **Honest Reporting**: Fallback counts are separated from true two-path routed outputs\n"
    )

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text("".join(lines), encoding="utf-8")


def main() -> int:
    args = parse_args()

    # Load both manifests
    offenders_path = Path(args.offenders_manifest)
    holdout_path = Path(args.holdout_manifest)

    offenders = parse_manifest(offenders_path)
    holdout = parse_manifest(holdout_path)

    print(f"Loaded {len(offenders)} offenders")
    print(f"Loaded {len(holdout)} holdout images")

    # Combine for testing
    all_entries = offenders + holdout
    print(f"Total: {len(all_entries)} images")

    profiles = [
        "gifsicle_baseline",
        "rusticle_default",
        "rusticle_two_path_auto",
        "rusticle_two_path_forced_a",
        "rusticle_two_path_forced_b",
    ]

    per_file_results: list[dict[str, Any]] = []
    for idx, entry in enumerate(all_entries, start=1):
        print(f"[{idx}/{len(all_entries)}] {entry['name']}")
        row = {
            "name": entry["name"],
            "path": entry["path"],
            "width": entry.get("width"),
            "height": entry.get("height"),
            "profiles": {},
        }
        for profile in profiles:
            try:
                row["profiles"][profile] = run_profile(
                    entry,
                    profile,
                    args.width,
                    args.height,
                    args.repeats,
                    args.rusticle_bin,
                    args.gifsicle_bin,
                )
            except Exception as e:
                print(f"  ERROR in {profile}: {e}")
                row["profiles"][profile] = {
                    "error": str(e),
                    "quality": {
                        "avg_psnr": None,
                        "avg_ssim": None,
                        "avg_ba": None,
                        "worst_ba": None,
                    },
                }
        per_file_results.append(row)

    summary: dict[str, Any] = {
        "manifest_offenders": str(offenders_path.resolve()),
        "manifest_holdout": str(holdout_path.resolve()),
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

    # Write markdown report
    markdown_output = Path(args.markdown_output)
    write_markdown_report(summary, per_file_results, markdown_output)

    ranking = sorted(
        summary["profiles"].items(),
        key=lambda item: (
            float("inf") if item[1]["avg_ba"] is None else item[1]["avg_ba"],
            float("inf")
            if item[1]["avg_runtime_ms"] is None
            else item[1]["avg_runtime_ms"],
        ),
    )

    print("\n" + "=" * 100)
    print("PROFILE RANKING (lower BA/runtime is better)")
    print("=" * 100)
    print(
        f"{'Profile':<35} {'Avg BA':>8} {'Worst BA':>10} {'Avg Runtime':>12} {'Avg Bytes':>12}"
    )
    print("-" * 100)
    for profile, metrics in ranking:
        avg_ba = metrics["avg_ba"]
        worst_ba = metrics["worst_ba"]
        avg_runtime = metrics["avg_runtime_ms"]
        avg_bytes = metrics["avg_output_bytes"]
        print(
            f"{profile:<35} "
            f"{(avg_ba if avg_ba is not None else float('nan')):8.3f} "
            f"{(worst_ba if worst_ba is not None else float('nan')):10.3f} "
            f"{(avg_runtime if avg_runtime is not None else float('nan')):12.2f} "
            f"{int(avg_bytes) if avg_bytes is not None else 0:12d}"
        )

    print("\n" + "=" * 100)
    print("PATH SELECTION ANALYSIS")
    print("=" * 100)
    print(f"{'Profile':<35} {'Path A':>10} {'Path B':>10} {'Fallback':>10}")
    print("-" * 100)
    for profile, metrics in ranking:
        if "path_a_count" not in metrics:
            continue
        path_a_rate = metrics.get("path_a_rate", 0)
        path_b_rate = metrics.get("path_b_rate", 0)
        fallback_rate = metrics.get("fallback_rate", 0)
        print(
            f"{profile:<35} {path_a_rate:9.1%} {path_b_rate:9.1%} {fallback_rate:9.1%}"
        )

    print(f"\nWrote results: {results_output}")
    print(f"Wrote summary: {summary_output}")
    print(f"Wrote report: {markdown_output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
