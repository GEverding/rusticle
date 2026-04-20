#!/usr/bin/env python3
"""Retest disposal-heavy offenders after disposal-aware fix wave."""

from __future__ import annotations

import argparse
import json
import re
import statistics
import subprocess
import tempfile
import time
from datetime import datetime
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--width", type=int, default=160)
    parser.add_argument("--height", type=int, default=120)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument(
        "--results-output",
        default="outputs/disposal_fix_offender_results.json",
        help="Per-file detailed output JSON",
    )
    parser.add_argument(
        "--report-output",
        default="outputs/disposal_fix_offender_report.json",
        help="Comparison report JSON",
    )
    parser.add_argument(
        "--markdown-output",
        default="outputs/disposal_fix_offender_report.md",
        help="Markdown report",
    )
    parser.add_argument(
        "--pre-fix-results",
        default="outputs/offender_retest_report.json",
        help="Pre-fix results for comparison",
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
    name: str,
    src: Path,
    profile: str,
    width: int,
    height: int,
    repeats: int,
    rusticle_bin: str,
    gifsicle_bin: str,
) -> dict[str, Any]:
    runtimes_ms: list[float] = []
    sizes: list[int] = []

    with tempfile.TemporaryDirectory(prefix="disposal_fix_offender_") as tmpdir:
        tmp = Path(tmpdir)
        outputs: list[Path] = []
        for repeat in range(repeats):
            out = tmp / f"{name}__{profile}__r{repeat}.gif"
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
                    f"{profile} failed for {name}: exit={result.returncode} stderr={result.stderr.strip()}"
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


def load_pre_fix_results(path: Path) -> dict[str, Any]:
    """Load pre-fix results from offender_retest_report.json."""
    if not path.exists():
        return {}
    return json.loads(path.read_text(encoding="utf-8"))


def compute_deltas(
    pre_fix: dict[str, float | None],
    post_fix: dict[str, float | None],
) -> dict[str, float]:
    """Compute deltas between pre-fix and post-fix metrics."""
    deltas = {}
    for key in ["avg_psnr", "avg_ssim", "avg_ba", "worst_ba"]:
        pre = pre_fix.get(key)
        post = post_fix.get(key)
        if pre is not None and post is not None:
            deltas[f"{key}_delta"] = post - pre
        else:
            deltas[f"{key}_delta"] = None

    # Runtime and bytes deltas
    pre_runtime = pre_fix.get("median_runtime_ms")
    post_runtime = post_fix.get("median_runtime_ms")
    if pre_runtime is not None and post_runtime is not None:
        deltas["runtime_delta_ms"] = post_runtime - pre_runtime
    else:
        deltas["runtime_delta_ms"] = None

    pre_bytes = pre_fix.get("median_output_bytes")
    post_bytes = post_fix.get("median_output_bytes")
    if pre_bytes is not None and post_bytes is not None:
        deltas["bytes_delta"] = post_bytes - pre_bytes
    else:
        deltas["bytes_delta"] = None

    return deltas


def main() -> int:
    args = parse_args()

    # Offender files
    offender_files = [
        "trapezius_animation_small2",
        "galilean_moon_laplace_resonance_animation_2",
        "790106_0203_voyager_58m_to_31m_reduced",
    ]

    # Find actual paths
    holdout_dir = Path("test_gifs/holdout_suite")
    file_paths = {}
    for name in offender_files:
        for gif_file in holdout_dir.glob(f"{name}.gif"):
            file_paths[name] = gif_file
            break

    if len(file_paths) != len(offender_files):
        missing = set(offender_files) - set(file_paths.keys())
        raise ValueError(f"Missing offender files: {missing}")

    profiles = [
        "gifsicle_baseline",
        "rusticle_default",
        "rusticle_optimized_global",
    ]

    # Run benchmarks
    print(
        f"Running disposal-fix offender retest ({args.width}x{args.height}, repeats={args.repeats})"
    )
    per_file_results: dict[str, dict[str, Any]] = {}
    for name in offender_files:
        print(f"  {name}...")
        src = file_paths[name]
        per_file_results[name] = {
            "path": str(src),
            "profiles": {},
        }
        for profile in profiles:
            print(f"    {profile}...")
            per_file_results[name]["profiles"][profile] = run_profile(
                name,
                src,
                profile,
                args.width,
                args.height,
                args.repeats,
                args.rusticle_bin,
                args.gifsicle_bin,
            )

    # Load pre-fix results
    pre_fix_data = load_pre_fix_results(Path(args.pre_fix_results))
    pre_fix_per_file = pre_fix_data.get("per_file_results", {})

    # Build comparison report
    report: dict[str, Any] = {
        "timestamp": datetime.now().isoformat(),
        "experiment": "EXP-008: Disposal-Fix Offender Retest",
        "description": "Retest of three disposal-heavy offenders after disposal-aware fix wave",
        "test_parameters": {
            "width": args.width,
            "height": args.height,
            "repeats": args.repeats,
        },
        "offender_files": offender_files,
        "per_file_results": {},
    }

    # Compute per-file deltas
    for name in offender_files:
        post_fix_profiles = per_file_results[name]["profiles"]
        pre_fix_profiles = pre_fix_per_file.get(name, {}).get("post_fix", {})

        file_report = {
            "pre_fix": pre_fix_profiles,
            "post_fix": post_fix_profiles,
            "deltas": {},
        }

        for profile in profiles:
            pre = pre_fix_profiles.get(profile, {}).get("quality", {})
            post = post_fix_profiles.get(profile, {}).get("quality", {})
            pre_runtime = pre_fix_profiles.get(profile, {}).get("median_runtime_ms")
            post_runtime = post_fix_profiles.get(profile, {}).get("median_runtime_ms")
            pre_bytes = pre_fix_profiles.get(profile, {}).get("median_output_bytes")
            post_bytes = post_fix_profiles.get(profile, {}).get("median_output_bytes")

            deltas = compute_deltas(pre, post)
            if pre_runtime is not None and post_runtime is not None:
                deltas["runtime_delta_ms"] = post_runtime - pre_runtime
            if pre_bytes is not None and post_bytes is not None:
                deltas["bytes_delta"] = post_bytes - pre_bytes

            file_report["deltas"][profile] = deltas

        report["per_file_results"][name] = file_report

    # Write results
    results_output = Path(args.results_output)
    results_output.parent.mkdir(parents=True, exist_ok=True)
    results_output.write_text(json.dumps(per_file_results, indent=2), encoding="utf-8")
    print(f"\nWrote results: {results_output}")

    # Write report
    report_output = Path(args.report_output)
    report_output.parent.mkdir(parents=True, exist_ok=True)
    report_output.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"Wrote report: {report_output}")

    # Generate markdown report
    md_lines = [
        "# Disposal-Fix Offender Retest Report",
        "",
        f"**Timestamp:** {report['timestamp']}",
        f"**Experiment:** {report['experiment']}",
        "",
        "## Test Parameters",
        f"- Width: {args.width}",
        f"- Height: {args.height}",
        f"- Repeats: {args.repeats}",
        "",
        "## Summary",
        "",
        "Retesting three disposal-heavy offenders after disposal-aware fix wave.",
        "",
        "### Key Metrics Tracked",
        "- **avg_psnr**: Average PSNR across frames",
        "- **avg_ssim**: Average SSIM across frames",
        "- **avg_ba**: Average Butteraugli distance",
        "- **worst_ba**: Maximum Butteraugli distance",
        "- **median_runtime_ms**: Median processing time",
        "- **median_output_bytes**: Median output file size",
        "",
    ]

    # Per-file results
    for name in offender_files:
        file_report = report["per_file_results"][name]
        md_lines.append(f"## {name}")
        md_lines.append("")

        # Detailed per-profile breakdown
        for profile in profiles:
            pre = file_report["pre_fix"].get(profile, {})
            post = file_report["post_fix"].get(profile, {})
            deltas = file_report["deltas"].get(profile, {})

            md_lines.append(f"### {profile}")
            md_lines.append("")
            md_lines.append("**Pre-fix:**")
            md_lines.append("")

            pre_quality = pre.get("quality", {})
            if pre_quality.get("avg_ba") is not None:
                md_lines.append(f"- avg_psnr: {pre_quality.get('avg_psnr', 'N/A')}")
                md_lines.append(f"- avg_ssim: {pre_quality.get('avg_ssim', 'N/A')}")
                md_lines.append(f"- avg_ba: {pre_quality.get('avg_ba', 'N/A')}")
                md_lines.append(f"- worst_ba: {pre_quality.get('worst_ba', 'N/A')}")
            else:
                md_lines.append(
                    f"- quality_error: {pre.get('quality_error', 'Unknown')}"
                )

            md_lines.append(
                f"- median_runtime_ms: {pre.get('median_runtime_ms', 'N/A'):.2f}"
            )
            md_lines.append(
                f"- median_output_bytes: {pre.get('median_output_bytes', 'N/A')}"
            )
            md_lines.append("")

            md_lines.append("**Post-fix:**")
            md_lines.append("")

            post_quality = post.get("quality", {})
            if post_quality.get("avg_ba") is not None:
                md_lines.append(f"- avg_psnr: {post_quality.get('avg_psnr', 'N/A')}")
                md_lines.append(f"- avg_ssim: {post_quality.get('avg_ssim', 'N/A')}")
                md_lines.append(f"- avg_ba: {post_quality.get('avg_ba', 'N/A')}")
                md_lines.append(f"- worst_ba: {post_quality.get('worst_ba', 'N/A')}")
            else:
                md_lines.append(
                    f"- quality_error: {post.get('quality_error', 'Unknown')}"
                )

            md_lines.append(
                f"- median_runtime_ms: {post.get('median_runtime_ms', 'N/A'):.2f}"
            )
            md_lines.append(
                f"- median_output_bytes: {post.get('median_output_bytes', 'N/A')}"
            )
            md_lines.append("")

            md_lines.append("**Deltas:**")
            md_lines.append("")
            for key, value in deltas.items():
                if value is not None:
                    if isinstance(value, float):
                        md_lines.append(f"- {key}: {value:+.2f}")
                    else:
                        md_lines.append(f"- {key}: {value:+d}")
                else:
                    md_lines.append(f"- {key}: N/A")
            md_lines.append("")

    # Summary section
    md_lines.append("## Analysis")
    md_lines.append("")

    # Check for catastrophic improvements
    catastrophic_improved = False
    for name in offender_files:
        if "Background-disposal" in name or "voyager" in name:
            file_report = report["per_file_results"][name]
            for profile in ["rusticle_default", "rusticle_optimized_global"]:
                deltas = file_report["deltas"].get(profile, {})
                ba_delta = deltas.get("avg_ba_delta")
                if ba_delta is not None and ba_delta < -5.0:
                    catastrophic_improved = True

    if catastrophic_improved:
        md_lines.append(
            "✓ **Catastrophic BA divergences improved materially** after disposal-aware fix"
        )
    else:
        md_lines.append("⚠ Catastrophic BA divergences did not improve materially")

    md_lines.append("")

    # Check for remaining divergences
    md_lines.append("### Remaining Divergences >1.0 BA vs gifsicle")
    md_lines.append("")
    for name in offender_files:
        file_report = report["per_file_results"][name]
        gifsicle_ba = (
            file_report["post_fix"]
            .get("gifsicle_baseline", {})
            .get("quality", {})
            .get("avg_ba")
        )
        for profile in ["rusticle_default", "rusticle_optimized_global"]:
            rusticle_ba = (
                file_report["post_fix"]
                .get(profile, {})
                .get("quality", {})
                .get("avg_ba")
            )
            if gifsicle_ba is not None and rusticle_ba is not None:
                divergence = rusticle_ba - gifsicle_ba
                if divergence > 1.0:
                    md_lines.append(
                        f"- **{name}** / {profile}: {divergence:.2f} BA divergence"
                    )

    md_lines.append("")

    # Voyager measurement behavior
    md_lines.append("### Voyager Measurement Behavior")
    md_lines.append("")
    voyager_report = report["per_file_results"].get(
        "790106_0203_voyager_58m_to_31m_reduced", {}
    )
    for profile in profiles:
        post = voyager_report.get("post_fix", {}).get(profile, {})
        quality_error = post.get("quality_error")
        if quality_error:
            md_lines.append(f"- {profile}: **ERROR** - {quality_error}")
        else:
            quality = post.get("quality", {})
            md_lines.append(
                f"- {profile}: Valid (avg_ba={quality.get('avg_ba', 'N/A')})"
            )

    md_lines.append("")

    markdown_output = Path(args.markdown_output)
    markdown_output.parent.mkdir(parents=True, exist_ok=True)
    markdown_output.write_text("\n".join(md_lines), encoding="utf-8")
    print(f"Wrote markdown: {markdown_output}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
