"""Shared paths and production-ABI compiler for the measured saron model."""
import os
from pathlib import Path
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SAMPLES = ROOT / 'samples-to-analyze/gamelan'
SOURCE = Path(os.environ.get('ESEQ_PM_FACTORY_DIR', ROOT / 'content/instruments/Physical Models')) / 'PM Saron/dsp.lisp'
STRENGTHS = ['softest', 'soft', 'medium', 'harder', 'hardest']
VELOCITIES = [.2, .4, .6, .8, 1.0]
sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument


def instrument(source=SOURCE, sr=48000, block=128):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    compiler = os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT / 'crates/sequencer/tools' / target))
    result = Instrument(source, compiler=compiler,
                        toolchain_root=str(ROOT / 'crates/sequencer/tools/dgen-toolchain'),
                        sample_rate=sr, max_frames=block)
    audit = subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                            str(Path(result.build_dir) / 'patch.c')], capture_output=True, text=True)
    if audit.returncode:
        raise RuntimeError('Generated-C fusion audit failed:\n'+audit.stdout+audit.stderr)
    return result
