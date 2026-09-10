"""Factory cello compile and render helpers."""
import os
from pathlib import Path
import platform
import subprocess
import sys

from analyze import ROOT
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument, write_wav

SOURCE = ROOT / 'content/instruments/Physical Models/PM Cello/dsp.lisp'


def instrument(source=SOURCE, sr=48000, block=128):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    compiler = os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT / 'crates/sequencer/tools' / target))
    result = Instrument(source, compiler=compiler,
                        toolchain_root=str(ROOT / 'crates/sequencer/tools/dgen-toolchain'),
                        sample_rate=sr, max_frames=block)
    subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                    str(Path(result.build_dir) / 'patch.c')], check=True, capture_output=True)
    return result
