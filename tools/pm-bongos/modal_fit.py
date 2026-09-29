"""Variable-projection modal identification for the bongo reference hits.

Every hit is modelled as a sum of exponentially damped sinusoids (the kit's
structural modes) excited by up to two strikes. For fixed poles the modal
amplitudes of both strikes and both channels are a linear least-squares
problem; only frequencies and loss rates are optimised nonlinearly.
"""
import numpy as np
from scipy.optimize import least_squares
from scipy.signal import butter, sosfiltfilt, find_peaks


HISS_RATE_PER_HZ = 0.025   # minimum loss rate (1/s) per Hz above 1 kHz


def highpass(x, sr, hz=110.0):
    """Removes the record's stationary 7 Hz rumble and 68 Hz hum (below every head mode)."""
    sos = butter(4, hz, 'highpass', fs=sr, output='sos')
    return sosfiltfilt(sos, x, axis=0)


def basis(freqs, rates, n, sr, delays=None):
    """Impulse responses of every mode to each strike (at t=0 and each delay)."""
    t = np.arange(n)/sr
    cols = []
    for start in [0.0] + list(delays or []):
        tt = np.maximum(t - start, 0.0)
        live = (t >= start)
        e = np.exp(-np.outer(tt, rates))*live[:, None]
        w = 2*np.pi*np.outer(tt, freqs)
        cols += [e*np.sin(w), e*np.cos(w)]
    return np.hstack(cols)


def solve(freqs, rates, y, sr, delay=None, ridge=1e-2):
    """y: (n, channels). Returns basis matrix, coefficients and residual."""
    B = basis(freqs, rates, len(y), sr, delay)
    G = B.T@B
    G[np.diag_indices_from(G)] += ridge*np.trace(G)/len(G)
    C = np.linalg.solve(G, B.T@y)
    return B, C, y - B@C


def seed_frequencies(y, sr, count, fmin=120.0, fmax=6000.0, windows=((0.0015, 0.04), (0.01, 0.25))):
    """Pooled spectral peaks from an early (short-mode) and a later (ringing) window."""
    mono = y.mean(1)
    found = {}
    for a, b in windows:
        seg = mono[int(a*sr):min(len(mono), int(b*sr))]
        if len(seg) < 128:
            continue
        N = 1 << 16
        spec = np.abs(np.fft.rfft(seg*np.hanning(len(seg)), N))
        fr = np.fft.rfftfreq(N, 1/sr)
        db = 20*np.log10(spec/spec.max() + 1e-12)
        peaks, _ = find_peaks(db, height=-45, distance=max(1, int(12/(sr/N))))
        for p in peaks:
            if fmin <= fr[p] <= fmax:
                key = round(fr[p]/6)
                found[key] = max(found.get(key, (-999, 0)), (db[p], fr[p]))
    ranked = sorted(found.values(), reverse=True)
    chosen = []
    for level, f in ranked:
        if all(abs(f - g) > max(8.0, 0.02*g) for g in chosen):
            chosen.append(f)
        if len(chosen) >= count:
            break
    return np.sort(np.array(chosen))


def fit_hit(y, sr, freqs0, rates0=None, delay=None, bounds_cents=60.0, rate_bounds=(2.0, 250.0)):
    k = len(freqs0)
    rates0 = np.full(k, 25.0) if rates0 is None else np.clip(rates0, *rate_bounds)
    # Membrane modes above ~1 kHz are strongly air-damped. A lightly damped
    # high "mode" is the record's hiss, so the loss floor rises with frequency.
    floor = np.maximum(rate_bounds[0], HISS_RATE_PER_HZ*(freqs0 - 1000.0))
    rates0 = np.maximum(rates0, floor*1.05)
    x0 = np.concatenate([np.zeros(k), np.log(rates0)])
    lo = np.concatenate([np.full(k, -bounds_cents), np.log(floor)])
    hi = np.concatenate([np.full(k, bounds_cents), np.full(k, np.log(rate_bounds[1]))])
    x0 = np.clip(x0, lo + 1e-9, hi - 1e-9)

    def unpack(x):
        return freqs0*2**(x[:k]/1200), np.exp(x[k:])

    def residual(x):
        f, r = unpack(x)
        return solve(f, r, y, sr, delay)[2].ravel()

    sol = least_squares(residual, x0, bounds=(lo, hi), x_scale='jac', max_nfev=60, diff_step=1e-4)
    f, r = unpack(sol.x)
    B, C, res = solve(f, r, y, sr, delay)
    return f, r, C, res


def mode_table(f, r, C, n_strikes):
    """Amplitude per mode per strike per channel from the (sin, cos) coefficients."""
    k = len(f)
    amps = np.zeros((n_strikes, k, C.shape[1]))
    phases = np.zeros((n_strikes, k, C.shape[1]))
    for s in range(n_strikes):
        sn = C[(2*s)*k:(2*s+1)*k]
        cs = C[(2*s+1)*k:(2*s+2)*k]
        amps[s] = np.hypot(sn, cs)
        phases[s] = np.arctan2(cs, sn)
    return amps, phases
