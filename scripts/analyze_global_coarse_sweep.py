#!/usr/bin/env python3
"""Analyze global coarse sweep results and emit shortlist/outlier artifacts."""

from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


BASELINE_CONFIG_ID = "f=lanczos3|opt=o3|lossy=80"
SPEEDUP_PROXY_MIN = 1.0
OUTLIER_DELTA_THRESHOLD = 0.25


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--input",
        default="outputs/sweep_global_coarse_train.jsonl",
        help="Sweep JSONL input path (default: outputs/sweep_global_coarse_train.jsonl)",
    )
    parser.add_argument(
        "--shortlist-out",
        default="outputs/global_coarse_shortlist.json",
        help="Shortlist JSON output path (default: outputs/global_coarse_shortlist.json)",
    )
    parser.add_argument(
        "--outliers-out",
        default="outputs/global_coarse_outliers.json",
        help="Outlier JSON output path (default: outputs/global_coarse_outliers.json)",
    )
    parser.add_argument(
        "--top-n",
        type=int,
        default=5,
        help="Number of shortlist entries to emit (default: 5)",
    )
    parser.add_argument(
        "--baseline-config-id",
        default=BASELINE_CONFIG_ID,
        help=(
            "Baseline config used for speedup proxy "
            "(default: f=lanczos3|opt=o3|lossy=80)"
        ),
    )
    return parser.parse_args()


def load_jsonl_records(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        raise FileNotFoundError(f"sweep results file not found: {path}")
    if not path.is_file():
        raise ValueError(f"sweep results path is not a file: {path}")

    records: list[dict[str, Any]] = []
    for index, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.strip()
        if not line:
            continue
        try:
            loaded = json.loads(line)
        except json.JSONDecodeError as exc:
            raise ValueError(f"line {index} is not valid JSON: {exc}") from exc
        if not isinstance(loaded, dict):
            raise ValueError(f"line {index} must be a JSON object")
        records.append(loaded)

    if not records:
        raise ValueError(f"sweep results file has no JSON records: {path}")
    return records


def as_float(value: Any) -> float | None:
    if isinstance(value, (int, float)):
        return float(value)
    return None


def extract_metric(record: dict[str, Any], path: list[str]) -> float | None:
    current: Any = record
    for key in path:
        if not isinstance(current, dict):
            return None
        current = current.get(key)
    return as_float(current)


def compute_speedup(
    record: dict[str, Any], baseline_runtime_ms: float | None
) -> float | None:
    explicit = extract_metric(record, ["avg_speedup_vs_baseline"])
    if explicit is not None:
        return explicit

    runtime = extract_metric(record, ["runtime_ms", "avg"])
    if baseline_runtime_ms is None or runtime is None or runtime <= 0.0:
        return None
    return baseline_runtime_ms / runtime


def max_per_category_avg_ba(record: dict[str, Any]) -> float | None:
    per_category = record.get("per_category_ba")
    if not isinstance(per_category, dict):
        return None

    max_value: float | None = None
    for payload in per_category.values():
        if not isinstance(payload, dict):
            continue
        avg_ba = as_float(payload.get("avg_ba"))
        if avg_ba is None:
            continue
        if max_value is None or avg_ba > max_value:
            max_value = avg_ba
    return max_value


def get_category_avg_ba(record: dict[str, Any], category: str) -> float | None:
    per_category = record.get("per_category_ba")
    if not isinstance(per_category, dict):
        return None
    payload = per_category.get(category)
    if not isinstance(payload, dict):
        return None
    return as_float(payload.get("avg_ba"))


def round_or_none(value: float | None, digits: int = 6) -> float | None:
    if value is None:
        return None
    return round(value, digits)


def quality_score(record: dict[str, Any]) -> float:
    median_ba = extract_metric(record, ["ba", "median"])
    if median_ba is not None:
        return median_ba

    avg_ba = extract_metric(record, ["ba", "avg"])
    if avg_ba is not None:
        return avg_ba
    return float("inf")


def sort_key(record: dict[str, Any]) -> tuple[float, float, float]:
    ba_score = quality_score(record)
    p95 = extract_metric(record, ["ba", "p95"]) or float("inf")
    runtime = extract_metric(record, ["runtime_ms", "avg"]) or float("inf")
    return ba_score, p95, runtime


def shortlist_item(
    rank: int,
    record: dict[str, Any],
    guardrails_passed: bool,
) -> dict[str, Any]:
    params = record.get("params") if isinstance(record.get("params"), dict) else {}
    return {
        "rank": rank,
        "config_id": record.get("config_id"),
        "filter": params.get("filter"),
        "optimize": params.get("optimize"),
        "lossy": params.get("lossy"),
        "avg_ba": round_or_none(extract_metric(record, ["ba", "avg"])),
        "p95_ba": round_or_none(extract_metric(record, ["ba", "p95"])),
        "worst_ba": round_or_none(extract_metric(record, ["ba", "worst"])),
        "avg_runtime_ms": round_or_none(extract_metric(record, ["runtime_ms", "avg"])),
        "avg_size_ratio": round_or_none(extract_metric(record, ["size_ratio", "avg"])),
        "speedup_vs_default_proxy": round_or_none(
            as_float(record.get("_speedup_vs_baseline"))
        ),
        "speedup_guardrail_is_proxy": True,
        "guardrails_passed": guardrails_passed,
    }


def main() -> int:
    args = parse_args()
    if args.top_n < 1:
        print("Error: --top-n must be >= 1", file=sys.stderr)
        return 1

    input_path = Path(args.input)
    shortlist_out = Path(args.shortlist_out)
    outliers_out = Path(args.outliers_out)

    try:
        records = load_jsonl_records(input_path)
    except (FileNotFoundError, OSError, ValueError) as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1

    baseline = next(
        (
            record
            for record in records
            if record.get("config_id") == args.baseline_config_id
        ),
        None,
    )
    if baseline is None:
        print(
            f"Error: baseline config not found in sweep: {args.baseline_config_id}",
            file=sys.stderr,
        )
        return 1

    baseline_runtime_ms = extract_metric(baseline, ["runtime_ms", "avg"])
    baseline_cartoon = get_category_avg_ba(baseline, "cartoon")
    baseline_large = get_category_avg_ba(baseline, "large")

    evaluated: list[dict[str, Any]] = []
    for record in records:
        speedup = compute_speedup(record, baseline_runtime_ms)
        size_ratio = extract_metric(record, ["size_ratio", "avg"])
        max_category_ba = max_per_category_avg_ba(record)

        speedup_ok = speedup is not None and speedup >= SPEEDUP_PROXY_MIN
        size_ok = size_ratio is not None and size_ratio <= 1.1
        category_ok = max_category_ba is not None and max_category_ba <= 8.0
        guardrails_passed = speedup_ok and size_ok and category_ok

        enriched = dict(record)
        enriched["_speedup_vs_baseline"] = speedup
        enriched["_guardrails_passed"] = guardrails_passed
        evaluated.append(enriched)

    ranked_all = sorted(evaluated, key=sort_key)
    passed = [record for record in ranked_all if record.get("_guardrails_passed")]

    selected: list[tuple[dict[str, Any], bool]] = []
    for record in passed[: args.top_n]:
        selected.append((record, True))

    if len(selected) < args.top_n:
        selected_ids = {record.get("config_id") for record, _ in selected}
        for record in ranked_all:
            config_id = record.get("config_id")
            if config_id in selected_ids:
                continue
            selected.append((record, bool(record.get("_guardrails_passed"))))
            selected_ids.add(config_id)
            if len(selected) >= args.top_n:
                break

    shortlist = [
        shortlist_item(index, record, passed_guardrails)
        for index, (record, passed_guardrails) in enumerate(selected, start=1)
    ]

    transparent_outliers: list[dict[str, Any]] = []
    cartoon_outliers: list[dict[str, Any]] = []
    large_outliers: list[dict[str, Any]] = []

    for record in ranked_all:
        config_id = record.get("config_id")
        params = record.get("params") if isinstance(record.get("params"), dict) else {}
        transparent_ba = get_category_avg_ba(record, "transparent")
        if transparent_ba is not None and transparent_ba > 8.0:
            transparent_outliers.append(
                {
                    "config_id": config_id,
                    "filter": params.get("filter"),
                    "optimize": params.get("optimize"),
                    "lossy": params.get("lossy"),
                    "transparent_avg_ba": round_or_none(transparent_ba),
                }
            )

        cartoon_ba = get_category_avg_ba(record, "cartoon")
        if (
            baseline_cartoon is not None
            and cartoon_ba is not None
            and cartoon_ba > (baseline_cartoon + OUTLIER_DELTA_THRESHOLD)
        ):
            cartoon_outliers.append(
                {
                    "config_id": config_id,
                    "filter": params.get("filter"),
                    "optimize": params.get("optimize"),
                    "lossy": params.get("lossy"),
                    "cartoon_avg_ba": round_or_none(cartoon_ba),
                    "default_cartoon_avg_ba": round_or_none(baseline_cartoon),
                    "delta_vs_default": round_or_none(cartoon_ba - baseline_cartoon),
                }
            )

        large_ba = get_category_avg_ba(record, "large")
        if (
            baseline_large is not None
            and large_ba is not None
            and large_ba > (baseline_large + OUTLIER_DELTA_THRESHOLD)
        ):
            large_outliers.append(
                {
                    "config_id": config_id,
                    "filter": params.get("filter"),
                    "optimize": params.get("optimize"),
                    "lossy": params.get("lossy"),
                    "large_avg_ba": round_or_none(large_ba),
                    "default_large_avg_ba": round_or_none(baseline_large),
                    "delta_vs_default": round_or_none(large_ba - baseline_large),
                }
            )

    outliers = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "input": str(input_path),
        "baseline_config_id": args.baseline_config_id,
        "baseline_runtime_ms": round_or_none(baseline_runtime_ms),
        "speedup_guardrail": {
            "min_speedup_vs_default_proxy": SPEEDUP_PROXY_MIN,
            "note": "Proxy speedup derived from runtime ratio vs default config because gifsicle speedup is unavailable in coarse sweep artifacts.",
        },
        "baseline_category_avg_ba": {
            "cartoon": round_or_none(baseline_cartoon),
            "large": round_or_none(baseline_large),
            "transparent": round_or_none(get_category_avg_ba(baseline, "transparent")),
        },
        "worse_than_default_delta_threshold": {
            "cartoon": OUTLIER_DELTA_THRESHOLD,
            "large": OUTLIER_DELTA_THRESHOLD,
        },
        "transparent_avg_ba_gt_8": transparent_outliers,
        "cartoon_worse_than_default": cartoon_outliers,
        "large_worse_than_default": large_outliers,
    }

    shortlist_out.parent.mkdir(parents=True, exist_ok=True)
    outliers_out.parent.mkdir(parents=True, exist_ok=True)
    shortlist_out.write_text(json.dumps(shortlist, indent=2) + "\n", encoding="utf-8")
    outliers_out.write_text(json.dumps(outliers, indent=2) + "\n", encoding="utf-8")

    print(f"records={len(records)}")
    print(
        f"guardrail_pass={sum(1 for record in evaluated if record['_guardrails_passed'])}"
    )
    print(f"shortlist={shortlist_out}")
    print(f"outliers={outliers_out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
