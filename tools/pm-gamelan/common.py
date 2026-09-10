"""Production compiler/ABI, shared by identification validation and listening."""
import os
import platform
import subprocess
import sys
from pathlib import Path

from families import ROOT

sys.path.insert(0, str(ROOT / 'tools/audition'))
from audition import Instrument


def instrument(source, sr=48000, block=128):
    target = {('Darwin', 'arm64'): 'DGenLisp-macos-arm64',
              ('Linux', 'x86_64'): 'DGenLisp-linux-x86_64'}[(platform.system(), platform.machine())]
    compiler = os.environ.get('ESEQ_DGENLISP_TOOL', str(ROOT / 'crates/sequencer/tools' / target))
    result = Instrument(source, compiler=compiler,
                        toolchain_root=str(ROOT / 'crates/sequencer/tools/dgen-toolchain'),
                        sample_rate=sr, max_frames=block)
    audit = subprocess.run([sys.executable, str(ROOT / 'tools/audition/check_fusion.py'),
                            str(Path(result.build_dir) / 'patch.c')], capture_output=True, text=True)
    if audit.returncode:
        raise RuntimeError('Generated-C fusion audit failed:\n' + audit.stdout + audit.stderr)
    return result
