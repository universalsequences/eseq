"""Install the authored FM Formant panel alongside the generated DSP."""
from pathlib import Path


def write_ui(destination, params):
    (destination / 'ui.lisp').write_text(
        Path(__file__).with_name('ui.lisp').read_text())
