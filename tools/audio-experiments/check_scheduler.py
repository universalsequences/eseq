#!/usr/bin/env python3
"""Run existing C scheduler regressions with each experimental waiting policy."""
import argparse
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
GRAPH = ROOT / "crates/sequencer/audiograph"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--normal", action="store_true", help="test the shipping policy without experiment hooks")
    parser.add_argument("--diagnostics", action="store_true")
    parser.add_argument("--tests", default="test_scheduler_queue_saturation,test_block_events,test_ordered_sum_topology,test_scheduler_completion")
    args = parser.parse_args()
    tests = args.tests.split(",")
    policies = ((8, 50, 64, 0, 0), (1, 50, 64, 0, 0), (64, 50, 64, 0, 0),
                (1024, 50, 64, 0, 0), (8, 50, 64, 0, 1),
                (8, 1000, 64, 0, 0), (8, 50, 64, 10, 0), (8, 1000, 64, 200, 0))
    with tempfile.TemporaryDirectory(prefix="eseq-audio-scheduler-") as directory:
        out = Path(directory)
        common = ["cc", "-O2", "-std=c11", "-pthread", "-I", str(GRAPH)]
        if args.normal:
            policies = ((),)
        else:
            common.append("-DAUDIOGRAPH_EXPERIMENTS")
        if args.diagnostics:
            common.append("-DAUDIOGRAPH_ENABLE_STALL_DIAGNOSTICS=1")
        objects = []
        for source in ("graph_engine", "graph_nodes", "graph_api", "graph_edit", "ready_queue", "hot_swap", "wrapper"):
            obj = out / f"{source}.o"
            subprocess.run(common + ["-c", str(GRAPH / f"{source}.c"), "-o", str(obj)], check=True)
            objects.append(str(obj))
        wrapper = out / "test_main.c"
        wrapper.write_text('''#include <stdlib.h>
extern void audiograph_configure_experiment(int, int, int, int, int);
extern int audiograph_test_main(void);
int main(int argc, char **argv) {
#ifdef AUDIOGRAPH_EXPERIMENTS
  if (argc != 6) return 2;
  audiograph_configure_experiment(atoi(argv[1]), atoi(argv[2]), atoi(argv[3]), atoi(argv[4]), atoi(argv[5]));
#endif
  return audiograph_test_main();
}
''')
        for test in tests:
            obj = out / f"{test}.o"
            subprocess.run(common + ["-Dmain=audiograph_test_main", "-c", str(GRAPH / "tests" / f"{test}.c"), "-o", str(obj)], check=True)
            binary = out / test
            subprocess.run(common + [str(wrapper), str(obj)] + objects + ["-lm", "-o", str(binary)], check=True)
            for policy in policies:
                result = subprocess.run([str(binary)] + list(map(str, policy)), capture_output=True, text=True, timeout=30)
                if result.returncode:
                    raise RuntimeError(f"{test} policy={policy} exit={result.returncode}:\n{result.stdout}\n{result.stderr}")
                print(f"PASS {test} policy={policy}", flush=True)


if __name__ == "__main__":
    main()
