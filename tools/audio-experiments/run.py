#!/usr/bin/env python3
"""Repeat isolated project renders in shuffled order and retain raw evidence.

Build first: cargo build --release -p sequencer --features audio-experiments --bin audio_experiment
This opens CoreAudio with silent device output unless --offline is selected.
"""
import argparse
import hashlib
import json
from pathlib import Path
import random
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def variants():
    base = dict(worker_spins=8, worker_wait_us=50, callback_spins=64, callback_wait_us=0, queue_hint=False)
    configs = {f"workers-{w}": dict(base, workers=w) for w in (0, 1, 2, 3, 4, 6)}
    for workers in (4, 6):
        for wait in (10, 50, 200):
            configs[f"callback-wait-{wait}-w{workers}"] = dict(base, workers=workers, callback_wait_us=wait)
    for wait in (200, 1000):
        configs[f"worker-wait-{wait}"] = dict(base, workers=4, worker_wait_us=wait)
    for spins in (1, 64, 1024):
        configs[f"worker-spins-{spins}"] = dict(base, workers=4, worker_spins=spins)
    for workers in (2, 4, 6):
        configs[f"queue-hint-w{workers}"] = dict(base, workers=workers, queue_hint=True)
    return configs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--pattern", type=int, default=4, help="one-based scene number")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--names", help="comma-separated variant names; default is the original worker/control sweep")
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--warmup-bars", type=float, default=4)
    parser.add_argument("--measure-bars", type=float, default=8)
    parser.add_argument("--offline", action="store_true")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/audio_experiment")
    parser.add_argument("--baseline-binary", type=Path, help="optionally interleave a saved baseline binary")
    parser.add_argument("--baseline-names", default="workers-4,workers-6")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=False)
    source = args.project.resolve().read_bytes()
    snapshot = args.out.resolve() / "project.json"
    snapshot.write_bytes(source)
    bpm = json.loads(source)["bpm"]
    metadata = dict(project=str(args.project.resolve()), project_sha256=hashlib.sha256(source).hexdigest(),
                    commit=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                    compiler_lock=(ROOT / "content/dgenlisp.lock").read_text(),
                    machine=subprocess.check_output(["uname", "-a"], text=True).strip())
    (args.out / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    (args.out / "source.diff").write_bytes(subprocess.check_output(["git", "diff"], cwd=ROOT))
    candidates = variants()
    # Preserve the original control sweep. Select callback and longer-worker
    # waits explicitly; these also reproduce the failures on pre-fix binaries.
    names = args.names.split(",") if args.names else [name for name in candidates
        if name != "worker-spins-1" and not name.startswith(("callback-wait-", "worker-wait-"))]
    jobs = [(name, rep, args.binary.resolve(), False) for rep in range(args.repeat) for name in names]
    if args.baseline_binary:
        jobs += [(name, rep, args.baseline_binary.resolve(), True)
                 for rep in range(args.repeat) for name in args.baseline_names.split(",")]
    metadata["binaries"] = {str(binary): hashlib.sha256(binary.read_bytes()).hexdigest()
                            for _, _, binary, _ in jobs}
    (args.out / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    random.Random(11092026).shuffle(jobs)
    summaries = []
    for number, (variant, rep, binary, baseline) in enumerate(jobs, 1):
        name = f"baseline-{variant}" if baseline else variant
        config = dict(candidates[variant], project=str(snapshot), pattern=args.pattern,
                      warmup_seconds=args.warmup_bars * 240 / bpm,
                      measure_seconds=args.measure_bars * 240 / bpm,
                      offline=args.offline, sample_rate=48000)
        stem = args.out.resolve() / f"{name}-r{rep}"
        stem.with_suffix(".config.json").write_text(json.dumps(config, indent=2) + "\n")
        print(f"[{number}/{len(jobs)}] {name} repeat {rep}", flush=True)
        with stem.with_suffix(".stdout.log").open("w") as stdout, stem.with_suffix(".stderr.log").open("w") as stderr:
            run = subprocess.run([str(binary),
                                  str(stem.with_suffix('.config.json'))], cwd=ROOT,
                                 stdout=stdout, stderr=stderr, timeout=180)
        if run.returncode:
            raise RuntimeError(f"{name} failed ({run.returncode}); see {stem}.stderr.log")
        lines = stem.with_suffix(".stdout.log").read_text().splitlines()
        if not lines:
            raise RuntimeError(f"{name} exited without a result; see {stem}.stderr.log")
        result = json.loads(lines[-1])
        stem.with_suffix(".result.json").write_text(json.dumps(result, indent=2) + "\n")
        summary = {k: v for k, v in result.items() if k != "blocks"}
        summary.update(name=name, repeat=rep, binary=str(binary), block_count=len(result["blocks"]))
        summaries.append(summary)
        (args.out / "summary.json").write_text(json.dumps(summaries, indent=2) + "\n")
        print(f"  CPU {result['process_cpu_pct']:.1f}% | callback mean {result['callback_mean_pct']:.1f}%"
              f" p99 {result['callback_p99_pct']:.1f}% | over budget {result['over_budget_blocks']}"
              f" | late {result['late_events']} | rms {result['rms']:.4f}", flush=True)


if __name__ == "__main__":
    main()
