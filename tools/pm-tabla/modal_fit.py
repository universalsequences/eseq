"""Variable-projection modal identification for the tabla reference strokes.

Every stroke is modelled as a sum of exponentially damped sinusoids excited by
up to three contact events. For fixed poles the complex residues of every
contact are a linear least-squares problem; only frequencies, loss rates and
(for bayan modes) a glide exponent are optimised nonlinearly.

Glide: the bayan rings through the second phrase while the player's wrist
raises its pitch (meend). The phrase's measured bayan pitch F(t) is shared by
every stroke: a gliding mode's frequency is f * (F(t0 + t) / F(t0))^beta, with
t0 the window start and beta in [0, 1.2] (0 = a dayan mode that does not
glide). Contacts at later delays start on the same moving pitch.
"""
import numpy as np
from scipy.optimize import least_squares
from scipy.signal import butter, sosfiltfilt, find_peaks


HISS_RATE_PER_HZ = 0.02    # minimum loss rate (1/s) per Hz above 1.5 kHz
HISS_FROM_HZ = 1500.0
BETA_LIMIT = 1.2


def highpass(x, sr, hz=60.0):
    """Removes sub-audio rumble below the bayan's lowest mode (~90 Hz)."""
    sos = butter(4, hz, 'highpass', fs=sr, output='sos')
    return sosfiltfilt(sos, x, axis=0)


class Warp:
    """Pitch ratio log2(F(t0 + t) / F(t0)) of a stroke window from the shared bayan track."""

    def __init__(self, t0, log2_track):
        self.t0 = t0
        self.track = log2_track

    def ratio(self, t):
        return self.track(self.t0 + t) - self.track(self.t0)


def phase_integral(n, sr, betas, warp, start=0):
    """Integral of (F(t0 + tau)/F(t0))^beta dtau, per mode, at samples start..start+n-1 (and the full grid)."""
    t = np.arange(start + n)/sr
    if warp is None or not np.any(betas):
        return np.repeat(t[:, None], len(betas), 1)
    rate = 2**(np.outer(warp.ratio(t), betas))
    return np.vstack([np.zeros((1, len(betas))), np.cumsum(0.5*(rate[1:] + rate[:-1]), 0)/sr])


def basis(freqs, rates, n, sr, delays=None, betas=None, warp=None, start=0):
    """Responses of every mode to each contact (at t=0 and each delay).

    `start` offsets the time axis (in samples) so the same basis can extend a
    fitted stroke past its identification window.
    """
    betas = np.zeros(len(freqs)) if betas is None else np.asarray(betas, float)
    t = (np.arange(n) + start)/sr
    grid = phase_integral(n, sr, betas, warp, start)
    P = grid[start:]
    cols = []
    for d in [0.0] + list(delays or []):
        Pd = grid[int(round(d*sr))]
        tt = np.maximum(t - d, 0.0)
        live = (t >= d)
        e = np.exp(-np.outer(tt, rates))*live[:, None]
        w = 2*np.pi*freqs[None, :]*(P - Pd[None, :])
        cols += [e*np.sin(w), e*np.cos(w)]
    return np.hstack(cols)


def solve(freqs, rates, y, sr, delay=None, betas=None, warp=None, ridge=1e-2):
    """y: (n, channels). Returns basis matrix, coefficients and residual."""
    B = basis(freqs, rates, len(y), sr, delay, betas, warp)
    G = B.T@B
    G[np.diag_indices_from(G)] += ridge*np.trace(G)/len(G)
    C = np.linalg.solve(G, B.T@y)
    return B, C, y - B@C


def seed_frequencies(y, sr, count, fmin=80.0, fmax=6000.0, windows=((0.0015, 0.04), (0.01, 0.25)), avoid=()):
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
        if all(abs(f - g) > max(8.0, 0.02*g) for g in list(chosen) + list(avoid)):
            chosen.append(f)
        if len(chosen) >= count:
            break
    return np.sort(np.array(chosen))


def rate_floor(freqs, minimum):
    # A lightly damped high "mode" is the record's hiss, so the loss floor
    # rises with frequency.
    return np.maximum(minimum, HISS_RATE_PER_HZ*(np.asarray(freqs) - HISS_FROM_HZ))


def fit_hit(y, sr, freqs0, rates0=None, delay=None, bounds_cents=60.0, rate_bounds=(1.5, 300.0),
            glide_mask=None, betas0=None, warp=None, fixed=None):
    """Optimises free poles. `fixed` = (freqs, rates, betas) joins the basis unchanged.

    Returns (freqs, rates, betas, C, residual) over the free poles followed by
    the fixed ones, in that order.
    """
    k = len(freqs0)
    rates0 = np.full(k, 25.0) if rates0 is None else np.asarray(rates0, float)
    floor = rate_floor(freqs0, rate_bounds[0])
    rates0 = np.clip(np.maximum(rates0, floor*1.05), floor*1.01, rate_bounds[1]*0.99)
    mask = np.zeros(k, bool) if (glide_mask is None or warp is None) else np.asarray(glide_mask, bool)
    gi = np.flatnonzero(mask)
    b0 = np.zeros(k) if betas0 is None else np.asarray(betas0, float)
    fixed_f, fixed_r, fixed_b = (np.zeros(0),)*3 if fixed is None else fixed
    x0 = np.concatenate([np.zeros(k), np.log(rates0), b0[gi]])
    lo = np.concatenate([np.full(k, -bounds_cents), np.log(floor), np.zeros(len(gi))])
    hi = np.concatenate([np.full(k, bounds_cents), np.full(k, np.log(rate_bounds[1])), np.full(len(gi), BETA_LIMIT)])
    x0 = np.clip(x0, lo + 1e-9, hi - 1e-9)

    def unpack(x):
        b = np.zeros(k)
        b[gi] = x[2*k:]
        return (np.concatenate([freqs0*2**(x[:k]/1200), fixed_f]),
                np.concatenate([np.exp(x[k:2*k]), fixed_r]),
                np.concatenate([b, fixed_b]))

    def residual(x):
        f, r, b = unpack(x)
        return solve(f, r, y, sr, delay, b, warp)[2].ravel()

    sol = least_squares(residual, x0, bounds=(lo, hi), x_scale='jac', max_nfev=60, diff_step=1e-4)
    f, r, b = unpack(sol.x)
    B, C, res = solve(f, r, y, sr, delay, b, warp)
    return f, r, b, C, res


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
