#!/usr/bin/env python3
"""Compare compiled graph writeback with the generated instrument sources."""
import argparse
import json
from pathlib import Path

import numpy as np

from common import FACTORY, HERE, NAMES, digest, instrument
from runtime import stream


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('export_directory', type=Path)
    parser.add_argument('--installed', action='store_true')
    args = parser.parse_args()
    factory = FACTORY if args.installed else HERE/'output/staging/content/instruments/Physical Models'
    result = []
    for slug, name in NAMES.items():
        source = factory/name/'dsp.lisp'
        saved = args.export_directory/name/'dsp.lisp'
        original, roundtrip = instrument(source), instrument(saved)
        for opening in [0, 1] if slug == 'hihat' else [0]:
            params = {'contact.openness': opening} if slug == 'hihat' else {}
            x, _ = stream(original, seconds=2, params=params, hits={137: .8, 12117: .4})
            y, _ = stream(roundtrip, seconds=2, params=params, hits={137: .8, 12117: .4})
            assert np.isfinite(x).all() and np.isfinite(y).all(), name
            error = float(abs(x-y).max())
            assert error < 2e-5, (name, opening, error)
            print(name, opening, 'maximum sample error', error, flush=True)
            result.append({'family': slug, 'openness': opening, 'max_sample_error': error,
                           'source_sha256': digest(source), 'roundtrip_sha256': digest(saved),
                           'compiler_sha256': original.compiler_sha256})
    (HERE/'roundtrip.json').write_text(json.dumps(result, indent=2)+'\n')


if __name__ == '__main__':
    main()
