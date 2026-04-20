#!/usr/bin/env python3
"""Retest voyager GIF after subframe reference-state fix."""

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
        default="outputs/voyager_subframe_fix_results.json",
        help="Per-file detailed output JSON",
    )
    parser.add_argument(
        "--report-output",
        default="outputs/voyager_subframe_fix_report.json",
        help="Comparison report JSON",
    )
    parser.add_argument(
        "--markdown-output",
        default="outputs/voyager_subframe_fix_report.md",
        help="Markdown report",
    )
    parser.add_argument(
        "--holdout-baseline",
        default="outputs/holdout_profile_results.json",
        help="Pre-fix holdout baseline for comparison",
    )
    parser.add_argument(
        "--disposal-fix-baseline",
        default="outputs/disposal_fix_offender_results.json",
        help="Post-disposal-fix baseline for comparison",
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

    with tempfile.TemporaryDirectory(prefix="voyager_subframe_fix_") as tmpdir:
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


def extract_voyager_from_holdout(holdout_path: Path) -> dict[str, Any] | None:
    """Extract voyager metrics from holdout baseline."""
    if not holdout_path.exists():
        return None
    data = json.loads(holdout_path.read_text(encoding="utf-8"))
    for entry in data:
        if entry.get("name") == "790106_0203_voyager_58m_to_31m_reduced":
            return entry
    return None


def extract_voyager_from_disposal_fix(disposal_fix_path: Path) -> dict[str, Any] | None:
    """Extract voyager metrics from disposal fix baseline."""
    if not disposal_fix_path.exists():
        return None
    data = json.loads(disposal_fix_path.read_text(encoding="utf-8"))
    return data.get("790106_0203_voyager_58m_to_31m_reduced")


def compute_deltas(
    pre: dict[str, float | None],
    post: dict[str, float | None],
) -> dict[str, float | None]:
    """Compute deltas between pre and post metrics."""
    deltas = {}
    for key in ["avg_psnr", "avg_ssim", "avg_ba", "worst_ba"]:
        pre_val = pre.get(key)
        post_val = post.get(key)
        if pre_val is not None and post_val is not None:
            deltas[f"{key}_delta"] = post_val - pre_val
        else:
            deltas[f"{key}_delta"] = None

    # Runtime and bytes deltas
    pre_runtime = pre.get("median_runtime_ms")
    post_runtime = post.get("median_runtime_ms")
    if pre_runtime is not None and post_runtime is not None:
        deltas["runtime_delta_ms"] = post_runtime - pre_runtime
    else:
        deltas["runtime_delta_ms"] = None

    pre_bytes = pre.get("median_output_bytes")
    post_bytes = post.get("median_output_bytes")
    if pre_bytes is not None and post_bytes is not None:
        deltas["bytes_delta"] = post_bytes - pre_bytes
    else:
        deltas["bytes_delta"] = None

    return deltas


def main() -> int:
    args = parse_args()

    # Voyager file
    voyager_name = "790106_0203_voyager_58m_to_31m_reduced"
    holdout_dir = Path("test_gifs/holdout_suite")
    voyager_path = None
    for gif_file in holdout_dir.glob(f"{voyager_name}.gif"):
        voyager_path = gif_file
        break

    if voyager_path is None:
        raise ValueError(f"Could not find {voyager_name}.gif in {holdout_dir}")

    profiles = [
        "gifsicle_baseline",
        "rusticle_default",
        "rusticle_optimized_global",
    ]

    # Run benchmarks
    print(
        f"Running voyager subframe-fix retest ({args.width}x{args.height}, repeats={args.repeats})"
    )
    print(f"  {voyager_name}...")
    post_fix_results = {
        "path": str(voyager_path),
        "profiles": {},
    }
    for profile in profiles:
        print(f"    {profile}...")
        post_fix_results["profiles"][profile] = run_profile(
            voyager_name,
            voyager_path,
            profile,
            args.width,
            args.height,
            args.repeats,
            args.rusticle_bin,
            args.gifsicle_bin,
        )

    # Load baselines
    holdout_baseline = extract_voyager_from_holdout(Path(args.holdout_baseline))
    disposal_fix_baseline = extract_voyager_from_disposal_fix(
        Path(args.disposal_fix_baseline)
    )

    # Build comprehensive report
    report: dict[str, Any] = {
        "timestamp": datetime.now().isoformat(),
        "experiment": "EXP-009: Voyager Subframe Reference-State Fix Retest",
        "description": "Retest of voyager GIF after subframe reference-state fix in precompute_reference_canvases",
        "test_parameters": {
            "width": args.width,
            "height": args.height,
            "repeats": args.repeats,
        },
        "voyager_file": voyager_name,
        "baselines": {
            "holdout_pre_fix": holdout_baseline is not None,
            "disposal_fix_post_fix": disposal_fix_baseline is not None,
        },
        "per_profile_analysis": {},
    }

    # Analyze each profile
    for profile in profiles:
        post_fix = post_fix_results["profiles"][profile]
        post_quality = post_fix.get("quality", {})

        # Get baseline metrics
        holdout_quality = None
        if holdout_baseline:
            holdout_quality = (
                holdout_baseline.get("profiles", {}).get(profile, {}).get("quality", {})
            )

        disposal_fix_quality = None
        if disposal_fix_baseline:
            disposal_fix_quality = (
                disposal_fix_baseline.get("profiles", {})
                .get(profile, {})
                .get("quality", {})
            )

        analysis = {
            "post_fix": post_fix,
            "deltas_vs_holdout_pre_fix": {},
            "deltas_vs_disposal_fix_post_fix": {},
        }

        # Compute deltas
        if holdout_quality:
            analysis["deltas_vs_holdout_pre_fix"] = compute_deltas(
                holdout_quality, post_quality
            )

        if disposal_fix_quality:
            analysis["deltas_vs_disposal_fix_post_fix"] = compute_deltas(
                disposal_fix_quality, post_quality
            )

        report["per_profile_analysis"][profile] = analysis

    # Write results
    results_output = Path(args.results_output)
    results_output.parent.mkdir(parents=True, exist_ok=True)
    results_output.write_text(json.dumps(post_fix_results, indent=2), encoding="utf-8")
    print(f"\nWrote results: {results_output}")

    # Write report
    report_output = Path(args.report_output)
    report_output.parent.mkdir(parents=True, exist_ok=True)
    report_output.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"Wrote report: {report_output}")

    # Generate markdown report
    md_lines = [
        "# Voyager Subframe Reference-State Fix Retest Report",
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
        "Retesting voyager GIF after subframe reference-state fix in `precompute_reference_canvases()`.",
        "This fix addresses the issue where Keep/None disposal frames were stored as subframe patches",
        "instead of being composited onto the full canvas, corrupting reference state for lossy() optimization.",
        "",
        "## Key Questions",
        "",
        "1. **Did rusticle_default BA materially improve?**",
        "2. **Does it now approach gifsicle?**",
        "3. **Does optimized_global remain perfect?**",
        "",
    ]

    # Per-profile analysis
    md_lines.append("## Per-Profile Analysis")
    md_lines.append("")

    for profile in profiles:
        analysis = report["per_profile_analysis"][profile]
        post_fix = analysis["post_fix"]
        post_quality = post_fix.get("quality", {})

        md_lines.append(f"### {profile}")
        md_lines.append("")

        # Current metrics
        md_lines.append("**Current (Post-Subframe-Fix):**")
        md_lines.append("")
        if post_fix.get("quality_error"):
            md_lines.append(f"- **ERROR**: {post_fix['quality_error']}")
        else:
            md_lines.append(f"- avg_psnr: {post_quality.get('avg_psnr', 'N/A')}")
            md_lines.append(f"- avg_ssim: {post_quality.get('avg_ssim', 'N/A')}")
            md_lines.append(f"- avg_ba: {post_quality.get('avg_ba', 'N/A')}")
            md_lines.append(f"- worst_ba: {post_quality.get('worst_ba', 'N/A')}")
            md_lines.append(
                f"- median_runtime_ms: {post_fix.get('median_runtime_ms', 'N/A'):.2f}"
            )
            md_lines.append(
                f"- median_output_bytes: {post_fix.get('median_output_bytes', 'N/A')}"
            )

        md_lines.append("")

        # Deltas vs holdout pre-fix
        deltas_holdout = analysis["deltas_vs_holdout_pre_fix"]
        if deltas_holdout:
            md_lines.append("**Deltas vs Holdout Pre-Fix:**")
            md_lines.append("")
            for key, value in deltas_holdout.items():
                if value is not None:
                    if isinstance(value, float):
                        md_lines.append(f"- {key}: {value:+.2f}")
                    else:
                        md_lines.append(f"- {key}: {value:+d}")
                else:
                    md_lines.append(f"- {key}: N/A")
            md_lines.append("")

        # Deltas vs disposal fix post-fix
        deltas_disposal = analysis["deltas_vs_disposal_fix_post_fix"]
        if deltas_disposal:
            md_lines.append("**Deltas vs Disposal-Fix Post-Fix:**")
            md_lines.append("")
            for key, value in deltas_disposal.items():
                if value is not None:
                    if isinstance(value, float):
                        md_lines.append(f"- {key}: {value:+.2f}")
                    else:
                        md_lines.append(f"- {key}: {value:+d}")
                else:
                    md_lines.append(f"- {key}: N/A")
            md_lines.append("")

    # Analysis section
    md_lines.append("## Analysis")
    md_lines.append("")

    # Question 1: Did rusticle_default BA materially improve?
    md_lines.append("### Q1: Did rusticle_default BA materially improve?")
    md_lines.append("")
    default_analysis = report["per_profile_analysis"].get("rusticle_default", {})
    default_deltas_disposal = default_analysis.get(
        "deltas_vs_disposal_fix_post_fix", {}
    )
    ba_delta = default_deltas_disposal.get("avg_ba_delta")
    if ba_delta is not None:
        if ba_delta < -5.0:
            md_lines.append(
                f"✓ **YES** - BA improved by {abs(ba_delta):.2f} (delta: {ba_delta:+.2f})"
            )
        elif ba_delta < 0:
            md_lines.append(
                f"⚠ **PARTIAL** - BA improved by {abs(ba_delta):.2f} (delta: {ba_delta:+.2f})"
            )
        else:
            md_lines.append(
                f"✗ **NO** - BA degraded or unchanged (delta: {ba_delta:+.2f})"
            )
    else:
        md_lines.append("? **UNKNOWN** - Could not compute delta")
    md_lines.append("")

    # Question 2: Does it now approach gifsicle?
    md_lines.append("### Q2: Does rusticle_default now approach gifsicle?")
    md_lines.append("")
    default_quality = default_analysis.get("post_fix", {}).get("quality", {})
    gifsicle_quality = (
        report["per_profile_analysis"]
        .get("gifsicle_baseline", {})
        .get("post_fix", {})
        .get("quality", {})
    )
    default_ba = default_quality.get("avg_ba")
    gifsicle_ba = gifsicle_quality.get("avg_ba")
    if default_ba is not None and gifsicle_ba is not None:
        divergence = default_ba - gifsicle_ba
        if divergence < 1.0:
            md_lines.append(
                f"✓ **YES** - Within 1.0 BA of gifsicle (divergence: {divergence:.2f})"
            )
        elif divergence < 5.0:
            md_lines.append(
                f"⚠ **PARTIAL** - Within 5.0 BA of gifsicle (divergence: {divergence:.2f})"
            )
        else:
            md_lines.append(f"✗ **NO** - Still {divergence:.2f} BA away from gifsicle")
    else:
        md_lines.append("? **UNKNOWN** - Could not compute divergence")
    md_lines.append("")

    # Question 3: Does optimized_global remain perfect?
    md_lines.append("### Q3: Does rusticle_optimized_global remain perfect?")
    md_lines.append("")
    optimized_quality = (
        report["per_profile_analysis"]
        .get("rusticle_optimized_global", {})
        .get("post_fix", {})
        .get("quality", {})
    )
    opt_ba = optimized_quality.get("avg_ba")
    opt_psnr = optimized_quality.get("avg_psnr")
    if opt_ba == 0.0 and opt_psnr == 100.0:
        md_lines.append("✓ **YES** - Perfect metrics maintained (BA=0.0, PSNR=100.0)")
    elif opt_ba is not None and opt_ba < 0.1:
        md_lines.append(
            f"⚠ **MOSTLY** - Near-perfect metrics (BA={opt_ba:.2f}, PSNR={opt_psnr:.2f})"
        )
    else:
        md_lines.append(f"✗ **NO** - Degraded metrics (BA={opt_ba}, PSNR={opt_psnr})")
    md_lines.append("")

    # Conclusion
    md_lines.append("## Conclusion")
    md_lines.append("")
    if ba_delta is not None and ba_delta < -5.0:
        md_lines.append(
            "The subframe reference-state fix has **materially improved** rusticle_default's quality on voyager."
        )
        if divergence is not None and divergence < 5.0:
            md_lines.append(
                "The default profile now **approaches gifsicle** in quality."
            )
        if opt_ba == 0.0:
            md_lines.append(
                "The optimized_global profile **remains perfect**, confirming the fix is correct."
            )
        md_lines.append("")
        md_lines.append("**Status: VOYAGER ISSUE RESOLVED** ✓")
    else:
        md_lines.append(
            "The subframe reference-state fix did not materially improve rusticle_default on voyager."
        )
        md_lines.append("Further investigation needed.")
        md_lines.append("")
        md_lines.append("**Status: VOYAGER ISSUE STILL OPEN** ⚠")

    md_lines.append("")

    markdown_output = Path(args.markdown_output)
    markdown_output.parent.mkdir(parents=True, exist_ok=True)
    markdown_output.write_text("\n".join(md_lines), encoding="utf-8")
    print(f"Wrote markdown: {markdown_output}")

    return 0


if __name__ == "__main__":
    exit(main())
