#!/usr/bin/env python3
"""Derive the checked-in `drift-switch` perf fixture from a private project.

eseq-pgru reproduces same-instrument track switching from a local project
(`.local/projects/slow-switch.json`) whose sampler tracks reference
content-addressed WAVs from the author's personal sample library. Those WAVs
are not in the repository, so the fixture that ships with the tests replaces
every sample reference with the sentinel `@PROBE_SAMPLE@`; the probe rewrites
that sentinel to a checked-in factory WAV when it loads the fixture. Sample
display names are normalised for the same reason.

Usage:
    python3 scripts/make_drift_switch_fixture.py \
        .local/projects/slow-switch.json \
        crates/sequencer/tests/fixtures/projects/drift-switch.json
"""

import json
import re
import sys

SENTINEL = "@PROBE_SAMPLE@"
SAMPLE_NAME = "probe-sample"
WAV_REF = re.compile(r"^samples/[0-9a-f]{8,}\.wav$")


def scrub(node, key=None):
    if isinstance(node, dict):
        return {k: scrub(v, k) for k, v in node.items()}
    if isinstance(node, list):
        return [scrub(v, key) for v in node]
    if isinstance(node, str):
        if WAV_REF.match(node):
            return SENTINEL
        if key in ("sample_name", "sample_names", "name") and node.startswith("samples/"):
            return SAMPLE_NAME
    return node


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    project = json.load(open(sys.argv[1]))
    project["name"] = "drift-switch"
    project = scrub(project)
    # Sampler tracks keep their kind and topology but lose the private
    # library titles that named them.
    for track in project["tracks"]:
        if track.get("kind") == "sampler":
            track["name"] = SAMPLE_NAME
    for pattern in project["patterns"]:
        names = pattern.get("sample_names")
        if not names:
            continue
        for idx, track in enumerate(project["tracks"]):
            if track.get("kind") == "sampler" and idx < len(names) and names[idx]:
                names[idx] = SAMPLE_NAME
    with open(sys.argv[2], "w") as out:
        json.dump(project, out, separators=(",", ":"))
        out.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
