#!/usr/bin/env python3
"""Direct identification of the first pluck (C#2 at ~0.10 s): the cleanest, monophonic hit.

Nothing rings before it and the next pluck is ~490 ms later, so every
partial is fitted on its own band from 2 ms (two poles where the string's
polarizations/body coupling split it), then all poles are solved jointly
full-band for complex residues at the pluck. Writes hero.json.
"""
import json
import sys
from pathlib import Path

import numpy as np
import scipy.signal as ss
import soundfile as sf
from scipy.optimize import least_squares

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
from partials import baseband, fit_poles   # noqa: E402

PARTIALS = 32


def main():
    analysis = json.loads((HERE/'analysis.json').read_text())
    p = next(p for p in analysis['pitches'] if p['name'] == 'C#2')
    take = min(p['takes'], key=lambda t: t['onset_s'])
    t0, t1 = take['onset_s'], min(take['end_s'], take['onset_s'] + 0.48)
    x, sr = sf.read(ROOT/'.local/pm-guitar/nascer-20s.wav')
    m = ss.sosfiltfilt(ss.butter(4, 40, 'highpass', fs=sr, output='sos'), x.mean(1))
    f0, B = p['f0_hz'], p['B']
    bw = 0.7*f0
    poles, info = [], []
    for k in range(1, PARTIALS + 1):
        hz = k*f0*np.sqrt(1 + B*k*k)
        settle = 1.2/bw
        z, dt = baseband(m, sr, hz, bw, int((t0 - 0.05)*sr), int((t1 + 0.05)*sr))
        tz = (t0 - 0.05) + np.arange(len(z))*dt - t0
        sel = (tz > settle) & (tz < t1 - t0 - settle)
        best = None
        for n in (1, 2):
            pp, c, r = fit_poles(z[sel], tz[sel], n, dhz_bound=min(6.0, 0.3*bw), rate_bounds=(0.2, 300.0))
            e = np.sum(np.abs(r)**2)
            if best is None or e < best[0]*10**(-1.0/10):
                best = (e, pp, c)
        e, pp, c = best
        snr = 10*np.log10(np.sum(np.abs(z[sel])**2)/max(e, 1e-30))
        for q in pp:
            poles.append((k, q))
        info.append({'k': k, 'snr_db': float(snr), 'n_poles': len(pp)})
    # joint full-band residues from 2 ms with every pole
    n0, n1 = int((t0 + 0.002)*sr), int(t1*sr)
    y = m[n0:n1]
    t = np.arange(n0, n1)/sr - t0
    hz = np.array([np.sqrt(1 + B*k*k)*k*f0 + q.imag/(2*np.pi) for k, q in poles])
    rate = np.array([-q.real for _, q in poles])
    E = np.exp(-np.outer(t, rate))
    A = np.hstack([E*np.cos(2*np.pi*np.outer(t, hz)), -E*np.sin(2*np.pi*np.outer(t, hz))])
    G = A.T@A
    G[np.diag_indices_from(G)] += 1e-6*np.trace(G)/len(G)
    sol = np.linalg.solve(G, A.T@y)
    res = y - A@sol
    c = sol[:len(hz)] + 1j*sol[len(hz):]       # y = Re(c e^{i w t}) e^{-rate t}
    out = []
    for k in range(1, PARTIALS + 1):
        idx = [i for i, (kk, _) in enumerate(poles) if kk == k]
        # the engine has one pole per partial: keep the stronger, report the pair's
        # summed residue and its energy-weighted rate
        energy = np.abs(c[idx])**2/(2*np.maximum(rate[idx], 0.2))
        j = idx[int(np.argmax(energy))]
        # both poles as identified: the second is the other polarization /
        # body-coupled partner (detune Hz, loss, complex residue)
        pair = [{'hz': float(hz[i]), 'rate': float(rate[i]), 're': float(c[i].real), 'im': float(c[i].imag)} for i in idx]
        pair.sort(key=lambda q: -abs(complex(q['re'], q['im']))**2/(2*max(q['rate'], 0.2)))
        out.append({'pair': pair, 'k': k, 'hz': float(hz[j]), 'rate': float(np.sum(energy*rate[idx])/np.sum(energy)),
                    're': float(np.sum(c[idx]).real), 'im': float(np.sum(c[idx]).imag),
                    'amp': float(abs(np.sum(c[idx]))), 'snr_db': info[k - 1]['snr_db'], 'poles': len(idx)})
    rel = 10*np.log10(np.mean(res**2)/np.mean(y**2))
    # what the partials leave: the buzz/rattle/contact target (band dB per window)
    bands = [(40, 400), (400, 1500), (1500, 4000), (4000, 9000), (9000, 16000)]
    windows = [(0.002, 0.01), (0.01, 0.06), (0.06, 0.15), (0.15, 0.3), (0.3, t1 - t0)]
    target = {}
    for name, sig in (('total', y), ('residual', res)):
        rows = []
        for lo, hi in bands:
            f = ss.sosfiltfilt(ss.butter(4, [lo, hi], 'bandpass', fs=sr, output='sos'), sig)
            rows.append([float(10*np.log10(np.mean(f[max(0, int((a - 0.002)*sr)):int((b - 0.002)*sr)]**2) + 1e-14)) for a, b in windows])
        target[name] = rows
    print('band dB per window', [f'{1000*a:.0f}-{1000*b:.0f}ms' for a, b in windows])
    for (lo, hi), tr, rr in zip(bands, target['total'], target['residual']):
        print(f'  {lo:5d}-{hi:5d} total', ' '.join(f'{v:6.1f}' for v in tr), '| residual', ' '.join(f'{v:6.1f}' for v in rr))
    print(f'hero C#2 @{t0:.3f}s f0 {f0:.2f} residual {rel:.1f} dB (full band, 2-{1000*(t1 - t0):.0f} ms)')
    for o in out[:24]:
        print(f"  k{o['k']:2d} {o['hz']:7.1f} Hz amp {20*np.log10(o['amp'] + 1e-12):6.1f} dB rate {o['rate']:6.2f}/s snr {o['snr_db']:5.1f} poles {o['poles']}")
    (HERE/'hero.json').write_text(json.dumps({'onset_s': t0, 'end_s': t1, 'f0_hz': f0, 'B': B,
                                              'residual_db': float(rel), 'partials': out, 'band_target': target}, indent=1) + '\n')


if __name__ == '__main__':
    main()
