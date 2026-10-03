#!/usr/bin/env python3
"""Render the fitted performance through the built instrument and compare it
with the recording. Writes listening material to .local/pm-milagre-brass/
(local only: it contains the reference audio): ab.wav (record, then model),
model.wav, compare.png (spectrograms + level envelopes).

Usage: compare.py [path-to-mp3]
"""
import json
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from common import ROOT, SR, instrument  # noqa: E402
import analyze  # noqa: E402
import performance as perf  # noqa: E402

OUT = ROOT/'.local/pm-milagre-brass'


def render_model(inst=None):
    ph = json.loads((HERE/'phrase.json').read_text())
    A = json.loads((HERE/'analysis.json').read_text())
    inst = inst or instrument()
    return perf.render(inst, A['notes'], ph['breath'], tune_steps=ph.get('tune')).astype(np.float64)


def spectrogram(ax, y, title, vmax):
    N, hop = 4096, 240
    w = np.hanning(N)
    S = np.array([np.abs(np.fft.rfft(y[s:s + N]*w)) for s in range(0, len(y) - N, hop)]).T
    S = 20*np.log10(S + 1e-9)
    f = np.fft.rfftfreq(N, 1/SR)
    k = f < 3000
    ax.imshow(S[k], origin='lower', aspect='auto', extent=[0, S.shape[1]*hop/SR, 0, 3000],
              vmin=vmax - 70, vmax=vmax, cmap='magma')
    ax.set_title(title)
    return S.max()


def level(y):
    W = 960
    return 20*np.log10(np.sqrt(np.array([np.mean(y[s:s + W]**2) for s in range(0, len(y) - W, W//2)])) + 1e-9)


def main():
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else analyze.DEFAULT
    ref = analyze.load(path)[:int(7.95*SR)]
    model = render_model()[:len(ref)]
    OUT.mkdir(parents=True, exist_ok=True)
    gap = np.zeros(int(0.8*SR))
    sf.write(OUT/'ab.wav', np.concatenate([ref, gap, model]).astype(np.float32), SR)
    sf.write(OUT/'model.wav', model.astype(np.float32), SR)
    fig, ax = plt.subplots(3, 1, figsize=(18, 13))
    top = spectrogram(ax[0], ref, 'recording', 0)
    spectrogram(ax[0], ref, 'recording', top)
    spectrogram(ax[1], model, 'PM Milagre Brass', top)
    t = np.arange(len(level(ref)))*480/SR
    ax[2].plot(t, level(ref), label='recording')
    ax[2].plot(t, level(model), label='model')
    ax[2].set_ylim(-60, -5)
    ax[2].grid(alpha=0.3)
    ax[2].legend()
    fig.savefig(OUT/'compare.png', dpi=60, bbox_inches='tight')
    print(f'wrote {OUT}/ab.wav, model.wav, compare.png')


if __name__ == '__main__':
    main()
