import numpy as np
def cen(x):
    X = np.abs(np.fft.rfft(x * np.hanning(len(x))))
    f = np.fft.rfftfreq(len(x), 1 / 48000)
    return float((X * f).sum() / max(X.sum(), 1e-12))
def stats(label, r, p, chans=(0, 1), seg=4800, n=6):
    for ch in chans:
        nm = "out" if ch == 0 else "aux"
        er = [float(np.sqrt(np.mean(r[i:i+seg, ch]**2))) for i in range(0, len(r) - seg + 1, seg)]
        ep = [float(np.sqrt(np.mean(p[i:i+seg, ch]**2))) for i in range(0, len(p) - seg + 1, seg)]
        print(f"   {nm} stats: centroid ref {cen(r[:,ch]):.0f} port {cen(p[:,ch]):.0f} Hz; "
              f"rms/100ms ref {' '.join(f'{v:.3f}' for v in er[:n])} | port {' '.join(f'{v:.3f}' for v in ep[:n])}")
