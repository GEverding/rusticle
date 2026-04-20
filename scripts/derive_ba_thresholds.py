#!/usr/bin/env python3
"""Derive Butteraugli threshold recommendations from benchmark summaries.

Usage:
  python3 scripts/derive_ba_thresholds.py
  python3 scripts/derive_ba_thresholds.py --input outputs/bench_results.jsonl
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class CategoryStats:
    count: int
    avg_ba: float
    p95_ba: float
    worst_ba: float
    suggested_threshold: float
    needs_investigation: bool


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--input",
        default="outputs/bench_results.jsonl",
        help="Path to benchmark JSONL output (default: outputs/bench_results.jsonl)",
    )
    parser.add_argument(
        "--json-out",
        default="outputs/ba_thresholds.json",
        help="Path to machine-readable JSON output (default: outputs/ba_thresholds.json)",
    )
    parser.add_argument(
        "--report-out",
        default="outputs/ba_thresholds_report.md",
        help="Path to markdown report output (default: outputs/ba_thresholds_report.md)",
    )
    return parser.parse_args()


def load_latest_summary(path: Path) -> dict[str, Any]:
    if not path.exists():
        raise FileNotFoundError(f"Benchmark results file not found: {path}")
    if not path.is_file():
        raise ValueError(f"Benchmark results path is not a file: {path}")

    lines = [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not lines:
        raise ValueError(f"Benchmark results file is empty: {path}")

    try:
        summary = json.loads(lines[-1])
    except json.JSONDecodeError as exc:
        raise ValueError(f"Latest benchmark summary is not valid JSON: {exc}") from exc

    if not isinstance(summary, dict):
        raise ValueError("Latest benchmark summary must be a JSON object")
    return summary


def percentile(values: list[float], pct: float) -> float:
    if not values:
        raise ValueError("Cannot compute percentile of empty list")
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


def derive_threshold(p95_ba: float) -> float:
    threshold = float(math.ceil(p95_ba * 1.2))
    return min(6.0, max(1.0, threshold))


def compute_stats(values: list[float]) -> CategoryStats:
    count = len(values)
    if count == 0:
        raise ValueError("Cannot compute stats for empty value set")
    avg_ba = sum(values) / count
    p95_ba = percentile(values, 0.95)
    worst_ba = max(values)
    suggested_threshold = derive_threshold(p95_ba)
    needs_investigation = worst_ba > (suggested_threshold * 1.5)
    return CategoryStats(
        count=count,
        avg_ba=avg_ba,
        p95_ba=p95_ba,
        worst_ba=worst_ba,
        suggested_threshold=suggested_threshold,
        needs_investigation=needs_investigation,
    )


def round_float(value: float, digits: int = 6) -> float:
    return round(value, digits)


def format_num(value: float | None, digits: int = 3) -> str:
    if value is None:
        return "-"
    return f"{value:.{digits}f}"


def render_summary_table(rows: list[dict[str, Any]]) -> str:
    headers = [
        "category",
        "count",
        "avg_ba",
        "p95_ba",
        "worst_ba",
        "threshold",
        "investigate",
        "delta_vs_gifsicle",
        "pair_count",
    ]
    table_rows = [
        [
            str(row["category"]),
            str(row["count"]),
            format_num(row["avg_ba"]),
            format_num(row["p95_ba"]),
            format_num(row["worst_ba"]),
            format_num(row["suggested_threshold"]),
            "yes" if row["needs_investigation"] else "no",
            format_num(row.get("avg_delta_vs_gifsicle")),
            str(row.get("pair_count", 0)),
        ]
        for row in rows
    ]

    widths = [len(header) for header in headers]
    for row in table_rows:
        for idx, col in enumerate(row):
            widths[idx] = max(widths[idx], len(col))

    def fmt_row(cols: list[str]) -> str:
        return " | ".join(col.ljust(widths[idx]) for idx, col in enumerate(cols))

    separator = "-+-".join("-" * width for width in widths)
    lines = [fmt_row(headers), separator]
    lines.extend(fmt_row(row) for row in table_rows)
    return "\n".join(lines)


def render_markdown_report(
    generated_at: str,
    commit: str,
    input_path: str,
    category_rows: list[dict[str, Any]],
    global_row: dict[str, Any],
) -> str:
    lines: list[str] = [
        "# Butteraugli Threshold Recommendations",
        "",
        f"- Generated at: `{generated_at}`",
        f"- Commit: `{commit}`",
        f"- Source: `{input_path}` (latest summary line)",
        "",
        "## Category breakdown",
        "",
        "| Category | Count | Avg BA | P95 BA | Worst BA | Suggested Threshold | Needs Investigation | Avg Δ vs gifsicle | Pair Count |",
        "|---|---:|---:|---:|---:|---:|:---:|---:|---:|",
    ]

    for row in category_rows:
        lines.append(
            "| "
            + " | ".join(
                [
                    str(row["category"]),
                    str(row["count"]),
                    format_num(row["avg_ba"]),
                    format_num(row["p95_ba"]),
                    format_num(row["worst_ba"]),
                    format_num(row["suggested_threshold"]),
                    "Yes" if row["needs_investigation"] else "No",
                    format_num(row.get("avg_delta_vs_gifsicle")),
                    str(row.get("pair_count", 0)),
                ]
            )
            + " |"
        )

    lines.extend(
        [
            "",
            "## Global",
            "",
            "| Count | Avg BA | P95 BA | Worst BA | Suggested Threshold | Needs Investigation |",
            "|---:|---:|---:|---:|---:|:---:|",
            "| "
            + " | ".join(
                [
                    str(global_row["count"]),
                    format_num(global_row["avg_ba"]),
                    format_num(global_row["p95_ba"]),
                    format_num(global_row["worst_ba"]),
                    format_num(global_row["suggested_threshold"]),
                    "Yes" if global_row["needs_investigation"] else "No",
                ]
            )
            + " |",
            "",
            "## Methodology",
            "",
            '- Use only rusticle records (`tool == "rusticle"`) where `avg_butteraugli` is present.',
            "- Group by `category`.",
            "- `suggested_threshold = ceil(p95_ba * 1.2)`, clamped to `[1.0, 6.0]`.",
            "- `needs_investigation = worst_ba > suggested_threshold * 1.5`.",
            "- Compare rusticle vs gifsicle BA when both are present for the same `(test_file, operation, category)` key.",
            "",
        ]
    )

    return "\n".join(lines)


def main() -> int:
    args = parse_args()

    input_path = Path(args.input)
    json_out_path = Path(args.json_out)
    report_out_path = Path(args.report_out)

    try:
        summary = load_latest_summary(input_path)
    except (FileNotFoundError, OSError, ValueError) as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1

    results = summary.get("results")
    if not isinstance(results, list) or not results:
        print("Error: latest benchmark summary has no results", file=sys.stderr)
        return 1

    rusticle_by_category: dict[str, list[float]] = {}
    all_rusticle_values: list[float] = []

    rusticle_map: dict[tuple[str, str, str], float] = {}
    gifsicle_map: dict[tuple[str, str, str], float] = {}

    for item in results:
        if not isinstance(item, dict):
            continue

        tool = item.get("tool", "rusticle")
        category = str(item.get("category") or "uncategorized")
        test_file = item.get("test_file")
        operation = item.get("operation")
        avg_ba = item.get("avg_butteraugli")

        if test_file is not None and operation is not None and avg_ba is not None:
            try:
                ba_val = float(avg_ba)
            except (TypeError, ValueError):
                ba_val = None
            if ba_val is not None:
                key = (str(test_file), str(operation), category)
                if tool == "rusticle":
                    rusticle_map[key] = ba_val
                elif tool == "gifsicle":
                    gifsicle_map[key] = ba_val

        if tool != "rusticle" or avg_ba is None:
            continue

        try:
            ba = float(avg_ba)
        except (TypeError, ValueError):
            continue

        rusticle_by_category.setdefault(category, []).append(ba)
        all_rusticle_values.append(ba)

    if not all_rusticle_values:
        print(
            "Error: no rusticle avg_butteraugli values found in latest summary",
            file=sys.stderr,
        )
        return 1

    category_rows: list[dict[str, Any]] = []
    categories_json: dict[str, dict[str, Any]] = {}

    for category in sorted(rusticle_by_category):
        values = rusticle_by_category[category]
        stats = compute_stats(values)

        delta_values = [
            rusticle_map[key] - gifsicle_map[key]
            for key in rusticle_map.keys() & gifsicle_map.keys()
            if key[2] == category
        ]
        pair_count = len(delta_values)
        avg_delta = (sum(delta_values) / pair_count) if pair_count > 0 else None

        row = {
            "category": category,
            "count": stats.count,
            "avg_ba": round_float(stats.avg_ba),
            "p95_ba": round_float(stats.p95_ba),
            "worst_ba": round_float(stats.worst_ba),
            "suggested_threshold": round_float(stats.suggested_threshold),
            "needs_investigation": stats.needs_investigation,
            "avg_delta_vs_gifsicle": round_float(avg_delta)
            if avg_delta is not None
            else None,
            "pair_count": pair_count,
        }
        category_rows.append(row)

        categories_json[category] = {
            "count": row["count"],
            "avg_ba": row["avg_ba"],
            "p95_ba": row["p95_ba"],
            "worst_ba": row["worst_ba"],
            "suggested_threshold": row["suggested_threshold"],
            "needs_investigation": row["needs_investigation"],
            "avg_delta_vs_gifsicle": row["avg_delta_vs_gifsicle"],
            "pair_count": row["pair_count"],
        }

    global_stats = compute_stats(all_rusticle_values)
    global_json = {
        "count": global_stats.count,
        "avg_ba": round_float(global_stats.avg_ba),
        "p95_ba": round_float(global_stats.p95_ba),
        "worst_ba": round_float(global_stats.worst_ba),
        "suggested_threshold": round_float(global_stats.suggested_threshold),
        "needs_investigation": global_stats.needs_investigation,
    }

    commit = str(summary.get("commit_hash", "unknown"))
    generated_at = datetime.now(timezone.utc).isoformat()

    output_json = {
        "generated_at": generated_at,
        "commit": commit,
        "categories": categories_json,
        "global": global_json,
        "methodology": {
            "input_summary": "latest non-empty line from benchmark JSONL",
            "source_tool": "rusticle",
            "metric": "avg_butteraugli",
            "percentile": "p95 computed by linear interpolation over sorted values",
            "threshold_formula": "ceil(p95_ba * 1.2), clamped to [1.0, 6.0]",
            "investigation_rule": "worst_ba > suggested_threshold * 1.5",
            "delta_vs_gifsicle": "avg(rusticle.avg_butteraugli - gifsicle.avg_butteraugli) for matched (test_file, operation, category)",
        },
    }

    for out_path in (json_out_path, report_out_path):
        try:
            out_path.parent.mkdir(parents=True, exist_ok=True)
        except OSError as exc:
            print(
                f"Error: failed to create output directory for {out_path}: {exc}",
                file=sys.stderr,
            )
            return 1

    try:
        json_out_path.write_text(
            json.dumps(output_json, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    except OSError as exc:
        print(
            f"Error: failed to write JSON output {json_out_path}: {exc}",
            file=sys.stderr,
        )
        return 1

    report = render_markdown_report(
        generated_at=generated_at,
        commit=commit,
        input_path=str(input_path),
        category_rows=category_rows,
        global_row=global_json,
    )

    try:
        report_out_path.write_text(report, encoding="utf-8")
    except OSError as exc:
        print(
            f"Error: failed to write report output {report_out_path}: {exc}",
            file=sys.stderr,
        )
        return 1

    stdout_rows = category_rows + [
        {
            "category": "GLOBAL",
            "count": global_json["count"],
            "avg_ba": global_json["avg_ba"],
            "p95_ba": global_json["p95_ba"],
            "worst_ba": global_json["worst_ba"],
            "suggested_threshold": global_json["suggested_threshold"],
            "needs_investigation": global_json["needs_investigation"],
            "avg_delta_vs_gifsicle": None,
            "pair_count": 0,
        }
    ]

    print(render_summary_table(stdout_rows))
    print(f"\nWrote {json_out_path}")
    print(f"Wrote {report_out_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
