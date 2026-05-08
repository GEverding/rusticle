#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import re
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


QUALITY_PATTERNS: dict[str, re.Pattern[str]] = {
    "avg_psnr": re.compile(r"Avg PSNR:\s*([0-9]+(?:\.[0-9]+)?)\s*dB"),
    "avg_ssim": re.compile(r"Avg SSIM:\s*([0-9]+(?:\.[0-9]+)?)"),
    "avg_ba": re.compile(r"Avg BA:\s*([0-9]+(?:\.[0-9]+)?)"),
}


@dataclass
class EvalResult:
    file_name: str
    source: str
    frame_count: int
    transparency_category: str
    disposal_category: str
    palette_category: str
    offset_subframe_ratio: float
    rusticle_ba: float
    rusticle_psnr: float
    rusticle_ssim: float
    gifsicle_ba: float
    gifsicle_psnr: float
    gifsicle_ssim: float
    ba_delta: float
    rusticle_bytes: int
    gifsicle_bytes: int
    rusticle_runtime_ms: float
    gifsicle_runtime_ms: float


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Evaluate resize quality on corpus GIFs."
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=Path("corpus_512_run_b2/manifest.json"),
        help="Path to corpus manifest JSON.",
    )
    parser.add_argument(
        "--rusticle-bin",
        type=Path,
        default=Path("target/release/rusticle"),
        help="Path to rusticle release binary.",
    )
    parser.add_argument(
        "--max-files",
        type=int,
        default=150,
        help="Max multi-frame files to evaluate (bounded run).",
    )
    parser.add_argument(
        "--output-json",
        type=Path,
        default=Path("outputs/corpus_quality_eval.json"),
    )
    parser.add_argument(
        "--output-md",
        type=Path,
        default=Path("outputs/corpus_quality_outliers.md"),
    )
    return parser.parse_args()


def run_cmd(cmd: list[str]) -> tuple[subprocess.CompletedProcess[str], float]:
    started = time.perf_counter()
    completed = subprocess.run(cmd, check=False, capture_output=True, text=True)
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    return completed, elapsed_ms


def parse_quality_output(output: str) -> dict[str, float] | None:
    parsed: dict[str, float] = {}
    for key, pattern in QUALITY_PATTERNS.items():
        match = pattern.search(output)
        if match is None:
            return None
        parsed[key] = float(match.group(1))
    return parsed


def load_manifest(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def write_markdown(
    path: Path,
    results: list[EvalResult],
    total_candidates: int,
    evaluated_count: int,
    skipped_single_frame: int,
    failures: list[dict[str, str]],
    max_files: int,
) -> None:
    worst_by_rusticle = sorted(results, key=lambda r: r.rusticle_ba, reverse=True)[:20]
    worst_by_delta = sorted(results, key=lambda r: r.ba_delta, reverse=True)[:20]

    def row(r: EvalResult) -> str:
        return (
            f"| {r.file_name} | {r.source} | {r.frame_count} | {r.transparency_category} | "
            f"{r.disposal_category} | {r.palette_category} | {r.offset_subframe_ratio:.3f} | "
            f"{r.rusticle_ba:.2f} | {r.gifsicle_ba:.2f} | {r.ba_delta:.2f} | "
            f"{r.rusticle_bytes} | {r.gifsicle_bytes} | {r.rusticle_runtime_ms:.1f} | {r.gifsicle_runtime_ms:.1f} |"
        )

    lines: list[str] = []
    lines.append("# Corpus Resize Quality Outliers\n")
    lines.append(f"- Candidates (success=true): {total_candidates}")
    lines.append(f"- Evaluated (multi-frame): {evaluated_count}")
    lines.append(f"- Skipped single-frame: {skipped_single_frame}")
    lines.append(f"- Failures during eval: {len(failures)}")
    lines.append(f"- Max files cap: {max_files}")
    if evaluated_count >= max_files:
        lines.append("- Note: run was bounded by --max-files cap.")
    lines.append("")

    lines.append("## Top 20 worst by rusticle BA (higher is worse)\n")
    lines.append(
        "| file | source | frames | transparency | disposal | palette | offset_ratio | rusticle_ba | gifsicle_ba | ba_delta | rusticle_bytes | gifsicle_bytes | rusticle_ms | gifsicle_ms |"
    )
    lines.append("|---|---|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    lines.extend(row(r) for r in worst_by_rusticle)
    lines.append("")

    lines.append("## Top 20 worst by BA delta vs gifsicle (rusticle - gifsicle)\n")
    lines.append(
        "| file | source | frames | transparency | disposal | palette | offset_ratio | rusticle_ba | gifsicle_ba | ba_delta | rusticle_bytes | gifsicle_bytes | rusticle_ms | gifsicle_ms |"
    )
    lines.append("|---|---|---:|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    lines.extend(row(r) for r in worst_by_delta)
    lines.append("")

    if failures:
        lines.append("## Failures\n")
        for failure in failures[:50]:
            lines.append(f"- {failure['file']}: {failure['error']}")

    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    args = parse_args()

    if not args.manifest.exists():
        raise FileNotFoundError(f"manifest not found: {args.manifest}")
    if not args.rusticle_bin.exists():
        raise FileNotFoundError(f"rusticle binary not found: {args.rusticle_bin}")

    args.output_json.parent.mkdir(parents=True, exist_ok=True)
    args.output_md.parent.mkdir(parents=True, exist_ok=True)

    manifest = load_manifest(args.manifest)
    entries: list[dict[str, Any]] = manifest.get("entries", [])

    successful: list[dict[str, Any]] = [e for e in entries if e.get("success") is True]
    multi_frame_candidates: list[dict[str, Any]] = [
        e for e in successful if int(e.get("frame_count", 0)) >= 2
    ]
    limited_candidates = multi_frame_candidates[: args.max_files]

    failures: list[dict[str, str]] = []
    results: list[EvalResult] = []

    for index, entry in enumerate(limited_candidates, start=1):
        local_path = Path(str(entry.get("local_path", "")))
        if not local_path.exists():
            failures.append({"file": str(local_path), "error": "missing input file"})
            continue

        with tempfile.TemporaryDirectory(prefix="corpus_quality_") as temp_dir:
            temp = Path(temp_dir)
            rusticle_out = temp / "rusticle.gif"
            gifsicle_out = temp / "gifsicle.gif"

            rusticle_resize_cmd = [
                str(args.rusticle_bin),
                "resize",
                str(local_path),
                str(rusticle_out),
                "--width",
                "160",
                "--height",
                "120",
                "--filter",
                "lanczos3",
                "--optimize",
                "o3",
                "--lossy",
                "80",
            ]
            rusticle_resize_proc, rusticle_ms = run_cmd(rusticle_resize_cmd)
            if rusticle_resize_proc.returncode != 0:
                failures.append(
                    {
                        "file": str(local_path),
                        "error": f"rusticle resize failed ({rusticle_resize_proc.returncode})",
                    }
                )
                continue

            gifsicle_cmd = [
                "gifsicle",
                str(local_path),
                "--resize",
                "160x120",
                "-O3",
                "-o",
                str(gifsicle_out),
            ]
            gifsicle_proc, gifsicle_ms = run_cmd(gifsicle_cmd)
            if gifsicle_proc.returncode != 0:
                failures.append(
                    {
                        "file": str(local_path),
                        "error": f"gifsicle resize failed ({gifsicle_proc.returncode})",
                    }
                )
                continue

            rusticle_quality_cmd = [
                str(args.rusticle_bin),
                "quality",
                str(local_path),
                str(rusticle_out),
            ]
            rusticle_q_proc, _ = run_cmd(rusticle_quality_cmd)
            rusticle_q_text = f"{rusticle_q_proc.stdout}\n{rusticle_q_proc.stderr}"
            rusticle_metrics = parse_quality_output(rusticle_q_text)
            if rusticle_q_proc.returncode != 0 or rusticle_metrics is None:
                failures.append(
                    {
                        "file": str(local_path),
                        "error": f"quality parse failed for rusticle output ({rusticle_q_proc.returncode})",
                    }
                )
                continue

            gifsicle_quality_cmd = [
                str(args.rusticle_bin),
                "quality",
                str(local_path),
                str(gifsicle_out),
            ]
            gifsicle_q_proc, _ = run_cmd(gifsicle_quality_cmd)
            gifsicle_q_text = f"{gifsicle_q_proc.stdout}\n{gifsicle_q_proc.stderr}"
            gifsicle_metrics = parse_quality_output(gifsicle_q_text)
            if gifsicle_q_proc.returncode != 0 or gifsicle_metrics is None:
                failures.append(
                    {
                        "file": str(local_path),
                        "error": f"quality parse failed for gifsicle output ({gifsicle_q_proc.returncode})",
                    }
                )
                continue

            result = EvalResult(
                file_name=local_path.name,
                source=str(entry.get("source_type", "unknown")),
                frame_count=int(entry.get("frame_count", 0)),
                transparency_category=str(
                    entry.get("transparency", {}).get("category", "unknown")
                ),
                disposal_category=str(
                    entry.get("disposal", {}).get("category", "unknown")
                ),
                palette_category=str(
                    entry.get("palette", {}).get("category", "unknown")
                ),
                offset_subframe_ratio=float(
                    entry.get("subframe", {}).get("offset_subframe_ratio", 0.0)
                ),
                rusticle_ba=rusticle_metrics["avg_ba"],
                rusticle_psnr=rusticle_metrics["avg_psnr"],
                rusticle_ssim=rusticle_metrics["avg_ssim"],
                gifsicle_ba=gifsicle_metrics["avg_ba"],
                gifsicle_psnr=gifsicle_metrics["avg_psnr"],
                gifsicle_ssim=gifsicle_metrics["avg_ssim"],
                ba_delta=rusticle_metrics["avg_ba"] - gifsicle_metrics["avg_ba"],
                rusticle_bytes=rusticle_out.stat().st_size,
                gifsicle_bytes=gifsicle_out.stat().st_size,
                rusticle_runtime_ms=rusticle_ms,
                gifsicle_runtime_ms=gifsicle_ms,
            )
            results.append(result)

        print(f"[{index}/{len(limited_candidates)}] ok: {local_path.name}")

    json_payload: dict[str, Any] = {
        "manifest": str(args.manifest),
        "rusticle_bin": str(args.rusticle_bin),
        "total_successful": len(successful),
        "total_multi_frame_candidates": len(multi_frame_candidates),
        "max_files": args.max_files,
        "evaluated": len(results),
        "skipped_single_frame": len(successful) - len(multi_frame_candidates),
        "failures": failures,
        "results": [r.__dict__ for r in results],
    }
    args.output_json.write_text(json.dumps(json_payload, indent=2), encoding="utf-8")

    write_markdown(
        path=args.output_md,
        results=results,
        total_candidates=len(successful),
        evaluated_count=len(results),
        skipped_single_frame=len(successful) - len(multi_frame_candidates),
        failures=failures,
        max_files=args.max_files,
    )

    print(f"evaluated={len(results)} failures={len(failures)}")
    print(f"json={args.output_json}")
    print(f"md={args.output_md}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
