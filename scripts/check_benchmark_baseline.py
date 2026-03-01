#!/usr/bin/env python3
"""Compare latest benchmark run to a curated baseline.

Usage:
  python3 scripts/check_benchmark_baseline.py
  python3 scripts/check_benchmark_baseline.py --results outputs/bench_results.jsonl
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--results",
        default="outputs/bench_results.jsonl",
        help="Path to benchmark JSONL output (default: outputs/bench_results.jsonl)",
    )
    parser.add_argument(
        "--baseline",
        default="docs/bench_baseline.json",
        help="Path to curated baseline JSON (default: docs/bench_baseline.json)",
    )
    parser.add_argument(
        "--max-slowdown-pct",
        type=float,
        default=None,
        help="Allowed rusticle slowdown percentage vs baseline (default from baseline file)",
    )
    parser.add_argument(
        "--max-speedup-drop-pct",
        type=float,
        default=None,
        help="Allowed drop in rusticle-vs-gifsicle speedup percentage (default from baseline file)",
    )
    parser.add_argument(
        "--max-psnr-drop",
        type=float,
        default=None,
        help="Allowed PSNR drop in dB (default from baseline file)",
    )
    parser.add_argument(
        "--max-ssim-drop",
        type=float,
        default=None,
        help="Allowed SSIM drop (default from baseline file)",
    )
    return parser.parse_args()


def load_latest_summary(path: Path) -> dict:
    if not path.exists():
        raise FileNotFoundError(f"Benchmark results file not found: {path}")
    lines = [line.strip() for line in path.read_text().splitlines() if line.strip()]
    if not lines:
        raise ValueError(f"Benchmark results file is empty: {path}")
    return json.loads(lines[-1])


def main() -> int:
    args = parse_args()

    baseline_path = Path(args.baseline)
    results_path = Path(args.results)

    baseline = json.loads(baseline_path.read_text())
    summary = load_latest_summary(results_path)

    defaults = baseline.get("default_tolerances", {})
    max_slowdown_pct = (
        args.max_slowdown_pct
        if args.max_slowdown_pct is not None
        else float(defaults.get("max_slowdown_pct", 20.0))
    )
    max_speedup_drop_pct = (
        args.max_speedup_drop_pct
        if args.max_speedup_drop_pct is not None
        else float(defaults.get("max_speedup_drop_pct", 20.0))
    )
    max_psnr_drop = (
        args.max_psnr_drop
        if args.max_psnr_drop is not None
        else float(defaults.get("max_psnr_drop", 1.0))
    )
    max_ssim_drop = (
        args.max_ssim_drop
        if args.max_ssim_drop is not None
        else float(defaults.get("max_ssim_drop", 0.01))
    )

    results = summary.get("results", [])
    rust_map = {
        (r["test_file"], r["operation"]): r for r in results if r.get("tool", "rusticle") == "rusticle"
    }
    gif_map = {
        (r["test_file"], r["operation"]): r for r in results if r.get("tool") == "gifsicle"
    }

    failures: list[str] = []

    for case in baseline.get("cases", []):
        key = (case["test_file"], case["operation"])
        rust = rust_map.get(key)
        gif = gif_map.get(key)

        if rust is None:
            failures.append(f"missing rusticle result for {key[0]} {key[1]}")
            continue
        if gif is None:
            failures.append(f"missing gifsicle result for {key[0]} {key[1]}")
            continue

        base_rust_ms = float(case["rusticle_total_ms"])
        curr_rust_ms = float(rust["total_ms"])
        if curr_rust_ms > base_rust_ms * (1.0 + max_slowdown_pct / 100.0):
            failures.append(
                f"{key[0]} {key[1]} rusticle slowdown: {curr_rust_ms:.2f}ms > "
                f"{base_rust_ms * (1.0 + max_slowdown_pct / 100.0):.2f}ms"
            )

        base_speedup = float(case["speedup"])
        curr_speedup = float(gif["total_ms"]) / curr_rust_ms if curr_rust_ms > 0 else math.inf
        if curr_speedup < base_speedup * (1.0 - max_speedup_drop_pct / 100.0):
            failures.append(
                f"{key[0]} {key[1]} speedup drop: {curr_speedup:.2f}x < "
                f"{base_speedup * (1.0 - max_speedup_drop_pct / 100.0):.2f}x"
            )

        base_psnr = float(case["rusticle_avg_psnr"])
        curr_psnr = float(rust["avg_psnr"])
        if curr_psnr < base_psnr - max_psnr_drop:
            failures.append(
                f"{key[0]} {key[1]} PSNR drop: {curr_psnr:.2f}dB < {base_psnr - max_psnr_drop:.2f}dB"
            )

        base_ssim = float(case["rusticle_avg_ssim"])
        curr_ssim = float(rust["avg_ssim"])
        if curr_ssim < base_ssim - max_ssim_drop:
            failures.append(
                f"{key[0]} {key[1]} SSIM drop: {curr_ssim:.4f} < {base_ssim - max_ssim_drop:.4f}"
            )

    base_agg = baseline.get("aggregates", {})
    if base_agg:
        curr_speedup = summary.get("avg_speedup_vs_baseline")
        base_speedup = float(base_agg.get("avg_speedup_vs_gifsicle", 0.0))
        if curr_speedup is not None and base_speedup > 0:
            if float(curr_speedup) < base_speedup * (1.0 - max_speedup_drop_pct / 100.0):
                failures.append(
                    f"aggregate speedup drop: {float(curr_speedup):.2f}x < "
                    f"{base_speedup * (1.0 - max_speedup_drop_pct / 100.0):.2f}x"
                )

        base_psnr = float(base_agg.get("avg_psnr", 0.0))
        curr_psnr = float(summary.get("avg_psnr", 0.0))
        if curr_psnr < base_psnr - max_psnr_drop:
            failures.append(
                f"aggregate PSNR drop: {curr_psnr:.2f}dB < {base_psnr - max_psnr_drop:.2f}dB"
            )

        base_ssim = float(base_agg.get("avg_ssim", 0.0))
        curr_ssim = float(summary.get("avg_ssim", 0.0))
        if curr_ssim < base_ssim - max_ssim_drop:
            failures.append(
                f"aggregate SSIM drop: {curr_ssim:.4f} < {base_ssim - max_ssim_drop:.4f}"
            )

    if failures:
        print("Benchmark baseline check FAILED:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print("Benchmark baseline check PASSED")
    print(f"  results: {results_path}")
    print(f"  baseline: {baseline_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
