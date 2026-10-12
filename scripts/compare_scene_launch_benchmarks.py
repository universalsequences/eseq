#!/usr/bin/env python3
"""Compare compatible saved-project launch reports; fail on latency regressions.

Thresholds are local measurement tolerances, not universal playback deadlines.
Both reports must be complete and replay the identical workload.
"""

import argparse
import json
import math
from pathlib import Path


def compare(before, after, percent, noise_ms):
    for report in (before, after):
        if report.get("schema") != 1 or report.get("complete") is not True:
            raise ValueError("need complete schema-1 reports; partial/failed runs are not baselines")
    for key in ("mode", "project_sha256", "tracks", "scenes", "racks", "rounds",
                "sample_rate", "block_frames", "bpm", "seed", "published_sequencers", "scope"):
        if before[key] != after[key]:
            raise ValueError(f"incompatible {key}: {before[key]!r} != {after[key]!r}")
    sequence = lambda report: [
        (s["round"], s["category"], s["target"], s["from_scene"], s["to_scene"])
        for s in report["samples"]
    ]
    if sequence(before) != sequence(after):
        raise ValueError("different launch order or skipped targets")
    work = lambda report: [
        [(p["rendered"], p["frontier"], p["events"]) for p in s["scheduler_passes"]]
        for s in report["samples"]
    ]
    if work(before) != work(after):
        raise ValueError("different scheduling frontiers/event counts; verify correctness before timing")
    if not before["summaries"] or before["summaries"].keys() != after["summaries"].keys():
        raise ValueError("missing/different summary groups")
    rows = []
    for group, summaries in before["summaries"].items():
        for metric in ("control_ms", "request_ms", "scheduler_pass_ms"):
            baseline = summaries[metric]
            candidate = after["summaries"][group][metric]
            if baseline["count"] != candidate["count"] or baseline["count"] <= 0:
                raise ValueError(f"{group}/{metric}: sample count differs or is empty")
            for statistic in ("p50", "p95", "p99", "max"):
                old, new = baseline[statistic], candidate[statistic]
                if any(not math.isfinite(v) or v < 0 for v in (old, new)):
                    raise ValueError("invalid timing")
                limit = old + max(old * percent / 100.0, noise_ms)
                rows.append({"group": group, "metric": metric, "statistic": statistic,
                             "before_ms": old, "after_ms": new, "limit_ms": limit,
                             "regressed": new > limit})
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--percent", type=float, default=10.0)
    parser.add_argument("--noise-ms", type=float, default=0.5)
    args = parser.parse_args()
    if any(not math.isfinite(v) or v < 0 for v in (args.percent, args.noise_ms)):
        parser.error("tolerances must be finite and nonnegative")
    try:
        rows = compare(json.loads(args.baseline.read_text()), json.loads(args.candidate.read_text()),
                       args.percent, args.noise_ms)
    except (ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    regressions = [row for row in rows if row["regressed"]]
    print(json.dumps({"percent_tolerance": args.percent, "noise_ms": args.noise_ms,
                      "checks": len(rows), "regressions": regressions}, indent=2))
    return int(bool(regressions))


if __name__ == "__main__":
    raise SystemExit(main())
