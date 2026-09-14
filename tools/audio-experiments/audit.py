#!/usr/bin/env python3
"""Calibrate RTSan, then audit a saved scene through the real CoreAudio callback.

Retains diagnostics, binaries and calibration evidence. This is a violation
detector, not a certificate covering unexecuted paths or unhooked allocators.
Sanitizer timing is deliberately not reported as audio performance.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
HEAP = {"malloc", "calloc", "realloc", "reallocf", "free", "valloc",
        "posix_memalign", "aligned_alloc", "mmap", "munmap",
        "malloc_zone_malloc", "malloc_zone_free"}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def diagnostic_summary(log):
    totals = re.findall(r"Total error count: (\d+)", log)
    if len(totals) != 1:
        raise RuntimeError("Missing or ambiguous RTSan completion statistics")
    reports = []
    for chunk in re.split(r"==\d+==ERROR: RealtimeSanitizer:", log)[1:]:
        function = re.search(r"unsafe function `([^`]+)`", chunk)
        if function:
            reports.append(dict(function=function[1], heap=function[1] in HEAP,
                                stack=[line.strip() for line in chunk.splitlines()
                                       if re.match(r"\s+#\d+ ", line)]))
    return dict(total_errors=int(totals[0]), reports=reports,
                heap_report_count=sum(report["heap"] for report in reports))


def run_logged(command, stem, env):
    with stem.with_suffix(".stdout.log").open("w") as stdout, \
            stem.with_suffix(".stderr.log").open("w") as stderr:
        run = subprocess.run(command, cwd=ROOT, env=env, stdout=stdout,
                             stderr=stderr, timeout=240)
    if run.returncode:
        raise RuntimeError(f"Process failed ({run.returncode}); see {stem}.stderr.log")
    return diagnostic_summary(stem.with_suffix(".stderr.log").read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--pattern", type=int, required=True, help="one-based global scene")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--scope", choices=["all", "measured"], default="all")
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--seconds", type=float, default=60)
    parser.add_argument("--warmup-seconds", type=float, default=20)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("This calibrated CoreAudio runner currently requires macOS")
    if args.repeat < 1 or not 0.1 <= args.seconds <= 60 or not 0 <= args.warmup_seconds <= 60:
        parser.error("repeat must be positive; measurement and warmup must fit 60 seconds")
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    shutil.copy2(args.project.resolve(), out / "project.json")
    env = os.environ.copy()
    env["RTSAN_ENABLE"] = "1"
    # Never inherit suppressions or silently disabled checking from the shell.
    env["RTSAN_OPTIONS"] = "halt_on_error=false:print_stats_on_exit=true:abort_on_error=0:color=never"
    command = ["cargo", "build", "--release", "-p", "sequencer", "--features",
               "audio-experiments,audio-rtsan", "--bin", "audio_experiment",
               "--bin", "audio_rtsan_probe", "--message-format=json"]
    print("Building the opt-in audit and calibration binaries", flush=True)
    with (out / "build.jsonl").open("w") as stdout, (out / "build.log").open("w") as stderr:
        subprocess.run(command, cwd=ROOT, env=env, stdout=stdout, stderr=stderr, check=True)
    records = [json.loads(line) for line in (out / "build.jsonl").read_text().splitlines()]
    runtime = [Path(record["out_dir"]) / "libclang_rt.rtsan_osx_dynamic.dylib"
               for record in records if record.get("reason") == "build-script-executed"
               and "rtsan-standalone-sys" in record["package_id"]]
    if len(runtime) != 1:
        raise RuntimeError("Cannot identify the exact sanitizer runtime used by Cargo")
    shutil.copy2(runtime[0], out / runtime[0].name)
    for record in records:
        if record.get("reason") == "compiler-artifact" and record.get("executable"):
            shutil.copy2(record["executable"], out / record["target"]["name"])
            symbols = Path(record["executable"] + ".dSYM")
            if symbols.is_dir():
                shutil.copytree(symbols, out / (record["target"]["name"] + ".dSYM"))
    env["DYLD_LIBRARY_PATH"] = str(out)
    (out / "source.diff").write_bytes(subprocess.check_output(["git", "diff"], cwd=ROOT))
    untracked = subprocess.check_output(["git", "ls-files", "--others", "--exclude-standard"],
                                        cwd=ROOT, text=True).splitlines()
    for name in untracked:
        if name.startswith(("crates/", "tools/audio-experiments/")):
            destination = out / "untracked-source" / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, destination)
    subprocess.run(["clang", "-g", "-O0", "-dynamiclib",
                    str(ROOT / "tools/audio-experiments/rtsan-control.c"),
                    "-o", str(out / "control.dylib")], check=True)
    controls = {}
    for name, arguments, expected in [
        ("clean", ["clean"], set()), ("rust", ["rust"], {"malloc", "free"}),
        *[(f"native-{index}", ["native", str(out / "control.dylib"), str(index)], functions)
          for index, functions in enumerate([
              {"malloc", "free"}, {"calloc", "free"}, {"malloc", "realloc", "free"},
              {"posix_memalign", "free"}, {"malloc_zone_malloc", "malloc_zone_free"}])],
    ]:
        result = run_logged([str(out / "audio_rtsan_probe"), *arguments], out / name, env)
        observed = {report["function"] for report in result["reports"]}
        result["intercepts_verified"] = expected <= observed if expected else result["total_errors"] == 0
        controls[name] = result
        if name != "native-4" and not result["intercepts_verified"]:
            write_json(out / "calibration.json", controls)
            raise RuntimeError(f"Allocation calibration failed for {name}; no playback verdict is valid")
    write_json(out / "calibration.json", controls)
    gaps = [] if controls["native-4"]["intercepts_verified"] else ["direct macOS malloc_zone allocation/free"]
    metadata = dict(scope=args.scope, coverage_gaps=gaps, zero_allocation_certified=False,
                    commit=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                    rtsan_options=env["RTSAN_OPTIONS"],
                    sha256={name: hashlib.sha256((out / name).read_bytes()).hexdigest()
                            for name in ["project.json", "audio_experiment", "audio_rtsan_probe", runtime[0].name]})
    write_json(out / "metadata.json", metadata)
    summaries = []
    for repeat in range(args.repeat):
        stem = out / f"playback-{repeat}"
        config = dict(project=str(out / "project.json"), pattern=args.pattern, workers=4,
                      worker_spins=8, worker_wait_us=50, callback_spins=64, callback_wait_us=50,
                      warmup_seconds=args.warmup_seconds, measure_seconds=args.seconds,
                      offline=False, sample_rate=48000, rtsan_measure_only=args.scope == "measured")
        write_json(stem.with_suffix(".config.json"), config)
        print(f"Auditing {args.scope} scope, run {repeat + 1}/{args.repeat}", flush=True)
        summary = run_logged([str(out / "audio_experiment"), str(stem.with_suffix(".config.json"))], stem, env)
        playback = json.loads(stem.with_suffix(".stdout.log").read_text().splitlines()[-1])
        if not playback.get("rtsan_enabled") or not playback["blocks"]:
            raise RuntimeError("Playback did not complete with the requested sanitizer build")
        write_json(stem.with_suffix(".result.json"), playback)
        summary.update(blocks=len(playback["blocks"]), audio_seconds=playback["audio_seconds"])
        summaries.append(summary)
        write_json(out / "summary.json", summaries)
        print(f"  {summary['heap_report_count']} distinct heap reports; "
              f"{summary['total_errors']} total real-time violations (including blocking calls)", flush=True)
    if any(summary["total_errors"] for summary in summaries):
        return 1
    return 2 if gaps else 0


if __name__ == "__main__":
    sys.exit(main())
