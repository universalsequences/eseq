#!/usr/bin/env python3
"""Headless transport CPU: the number the transport bar's CPU meter shows.

The meter is whole device-callback wall time / device-period budget
(audio/metrics.rs). `audio_experiment` measures exactly that per block through
the production CoreAudio stream, scheduler and graph workers, with silent
device output. This wrapper runs one or more workloads and prints the mean.

    cargo build --release -p sequencer --features audio-experiments --bin audio_experiment
    python3 tools/audio-experiments/transport_cpu.py \
        --project .local/projects/superbasicsetting.json --new-project

`--new-project` measures the startup topology (two empty tracks, Reverb on
Bus A, Str8 Delay on Bus B) without loading a file. Stop other audio and
builds first; results are noisy below ~0.2 percentage points. With
`--baseline-binary`, runs alternate between the two builds so background load
lands on both sides.
"""
import argparse
import json
from pathlib import Path
import statistics
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def run_once(binary, workload, args):
    config = dict(project=str(workload.get("project", ROOT)), pattern=args.pattern,
                  new_project=workload.get("new_project", False), workers=args.workers,
                  worker_spins=8, worker_wait_us=50, callback_spins=64, callback_wait_us=50,  # shipping macOS policy
                  warmup_seconds=args.warmup, measure_seconds=args.seconds,
                  offline=False, sample_rate=48000, workgroups=not args.no_workgroups)
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
        json.dump(config, handle)
    run = subprocess.run([str(binary), handle.name], cwd=ROOT, capture_output=True, text=True, timeout=240)
    Path(handle.name).unlink()
    if run.returncode or not run.stdout.strip():
        raise RuntimeError(f"{workload['name']} failed:\n{run.stderr[-3000:]}")
    return json.loads(run.stdout.strip().splitlines()[-1])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--project", type=Path, action="append", default=[])
    parser.add_argument("--new-project", action="store_true")
    parser.add_argument("--pattern", type=int, default=1, help="one-based scene number")
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--no-workgroups", action="store_true",
                        help="keep helpers out of the device os_workgroup (diagnostic)")
    parser.add_argument("--warmup", type=float, default=3.0)
    parser.add_argument("--seconds", type=float, default=8.0)
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/audio_experiment")
    parser.add_argument("--baseline-binary", type=Path, help="interleave runs of a second build")
    parser.add_argument("--json", type=Path, help="write per-run summaries here")
    args = parser.parse_args()
    workloads = [dict(name=p.stem, project=p.resolve()) for p in args.project]
    if args.new_project:
        workloads.append(dict(name="new-project", new_project=True))
    if not workloads:
        parser.error("give --project and/or --new-project")
    binaries = [("candidate", args.binary)]
    if args.baseline_binary:
        binaries.append(("baseline", args.baseline_binary))
    results = {}
    for workload in workloads:
        runs = {label: [] for label, _ in binaries}
        for _ in range(args.repeat):
            for label, binary in binaries:
                result = run_once(binary, workload, args)
                runs[label].append({k: result[k] for k in ("callback_mean_pct", "callback_p50_pct", "callback_p99_pct",
                                                          "process_cpu_pct", "rms", "instrument_voice_stats")}
                                   | dict(render_us=statistics.median(b["phases"]["render_us"] for b in result["blocks"]),
                                          snapshot_us=statistics.median(b["phases"]["snapshot_transport_us"] for b in result["blocks"])))
        results[workload["name"]] = runs
        for label, _ in binaries:
            label_runs = runs[label]
            means = [r["callback_mean_pct"] for r in label_runs]
            name = workload["name"] if len(binaries) == 1 else f"{workload['name']} [{label}]"
            print(f"{name:>36}: transport CPU {statistics.median(means):.2f}% "
                  f"(runs {', '.join(f'{m:.2f}' for m in means)}) | p99 {statistics.median(r['callback_p99_pct'] for r in label_runs):.2f}% "
                  f"| render {statistics.median(r['render_us'] for r in label_runs):.0f}us "
                  f"| snapshot {statistics.median(r['snapshot_us'] for r in label_runs):.0f}us "
                  f"| process {statistics.median(r['process_cpu_pct'] for r in label_runs):.1f}% | rms {label_runs[0]['rms']:.4f}",
                  flush=True)
    if args.json:
        args.json.write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
