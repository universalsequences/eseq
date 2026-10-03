"""Modal identification for the ride: one shared set of cymbal modes, many strokes.

A cymbal's modes (frequency, loss) belong to the plate, not to the stroke:
every stick hit injects a different complex residue into the same modes, and
the record is the linear superposition of those rings. Every pole here is
therefore shared by all strokes; only residues are per stroke.

1. Ring poles (`esprit` per `Subband`). The sample is split into 50 Hz complex
   subbands (heterodyne, zero-phase low-pass, decimate). Between strokes every
   band is a free sum of the same damped exponentials, so multi-segment
   ESPRIT finds its poles directly. Residues per (stroke, mode) are then a
   linear least-squares problem over the whole sample, solved per band with
   every pole near the band, so neighbouring bands agree.
2. Attack poles (`fit_attack`). With every ring removed, the first tens of
   milliseconds of each stroke (stick contact, heavily damped modes, and the
   correction of the ring's early transient) are fitted at the full rate with
   a second shared set of fast poles: one variable-projection problem over
   all strokes at once, with an analytic (Kaufman) Jacobian.
"""
import numpy as np
from scipy.optimize import least_squares
from scipy.signal import butter, sosfiltfilt, find_peaks, lfilter


def highpass(x, sr, hz):
    return sosfiltfilt(butter(4, hz, 'highpass', fs=sr, output='sos'), x, axis=0)


class Subband:
    """Complex baseband of one frequency band of the whole analytic signal."""

    def __init__(self, analytic, sr, centre, cutoff, rate):
        self.centre = centre
        self.step = int(round(sr/rate))
        self.rate = sr/self.step
        t = np.arange(len(analytic))/sr
        z = analytic*np.exp(-2j*np.pi*centre*t)
        sos = butter(6, cutoff, 'lowpass', fs=sr, output='sos')
        z = sosfiltfilt(sos, z.real) + 1j*sosfiltfilt(sos, z.imag)
        self.z = z[::self.step]
        self.t = np.arange(len(self.z))*self.step/sr


def ring_basis(t, onsets, offsets_hz, rates):
    """(samples, strokes*modes) complex responses e^{(-r + i2πδ)(t - t_s)} from each onset."""
    cols = []
    for o in onsets:
        tt = t - o
        live = tt >= 0
        tt = np.where(live, tt, 0.0)
        cols.append(np.exp(np.outer(tt, -rates + 2j*np.pi*offsets_hz))*live[:, None])
    return np.hstack(cols)


def solve_complex(B, y, ridge=1e-3):
    G = B.conj().T@B
    G[np.diag_indices_from(G)] += ridge*np.real(np.trace(G))/len(G)
    c = np.linalg.solve(G, B.conj().T@y)
    return c, y - B@c


def free_segments(t, onsets, after=0.03, before=0.02):
    """Index ranges of a band's samples where every stroke is ringing freely."""
    edges = list(onsets) + [t[-1] + before + 1e-9]
    out = []
    for a, b in zip(edges, edges[1:]):
        idx = np.flatnonzero((t >= a + after) & (t < b - before))
        if len(idx):
            out.append(idx)
    return out


def esprit(z, segments, order, pencil):
    """Shared poles of several free-ring segments (multi-segment ESPRIT).

    Every segment is a sum of the same damped complex exponentials with
    different amplitudes, so their Hankel matrices share one signal subspace.
    Returns complex poles per (decimated) sample.
    """
    blocks = []
    for idx in segments:
        seg = z[idx]
        if len(seg) > pencil + 2:
            blocks.append(np.lib.stride_tricks.sliding_window_view(seg, pencil).T)
    U, _, _ = np.linalg.svd(np.hstack(blocks), full_matrices=False)
    Us = U[:, :order]
    Phi = np.linalg.lstsq(Us[:-1], Us[1:], rcond=None)[0]
    return np.linalg.eigvals(Phi)


def ring_synthesis(hz, rates, residues, onset_samples, n, sr):
    """Real signal of complex residues[mode, stroke] ringing from each onset sample."""
    out = np.zeros(n)
    impulses = np.zeros(n, complex)
    for f, r, c in zip(hz, rates, residues):
        impulses[:] = 0
        impulses[onset_samples] = c
        out += lfilter([1], [1, -np.exp((-r + 2j*np.pi*f)/sr)], impulses).real
    return out


# Attack poles: real basis at the full rate, y = Σ e^{-rt}(a sin ωt + b cos ωt).
def attack_basis(freqs, rates, n, sr):
    t = np.arange(n)/sr
    e = np.exp(-np.outer(t, rates))
    w = 2*np.pi*np.outer(t, freqs)
    return np.hstack([e*np.sin(w), e*np.cos(w)])


def solve_real(B, y, ridge=1e-6):
    G = B.T@B
    G[np.diag_indices_from(G)] += ridge*np.trace(G)/len(G)
    c = np.linalg.solve(G, B.T@y)
    return c, y - B@c


def seed_attack(windows, sr, count, fmin, fmax, seconds=0.02):
    """Strongest peaks of the windows' pooled early power spectrum."""
    N = 1 << 15
    power = np.zeros(N//2 + 1)
    for y in windows:
        seg = y[:int(seconds*sr)]
        power = power + np.abs(np.fft.rfft(seg*np.hanning(len(seg)), N))**2
    fr = np.fft.rfftfreq(N, 1/sr)
    db = 10*np.log10(power/power.max() + 1e-12)
    peaks, _ = find_peaks(db, distance=max(1, int(40/(sr/N))))
    peaks = [p for p in peaks if fmin <= fr[p] <= fmax]
    peaks = sorted(peaks, key=lambda p: -db[p])[:count]
    return np.sort(fr[peaks])


def fit_attack(windows, sr, freqs0, rate_bounds=(12.0, 900.0), cents=150.0, max_nfev=40):
    """Shared fast poles for every stroke's early residual.

    Returns (hz, rates, coefficients per window [2k: sin then cos], residuals).
    """
    k = len(freqs0)
    lo = np.concatenate([np.full(k, -cents), np.full(k, np.log(rate_bounds[0]))])
    hi = np.concatenate([np.full(k, cents), np.full(k, np.log(rate_bounds[1]))])
    x0 = np.clip(np.concatenate([np.zeros(k), np.full(k, np.log(80.0))]), lo + 1e-9, hi - 1e-9)
    times = [np.arange(len(y))/sr for y in windows]

    def unpack(x):
        return freqs0*2**(x[:k]/1200), np.exp(x[k:])

    def residual(x):
        f, r = unpack(x)
        return np.concatenate([solve_real(attack_basis(f, r, len(y), sr), y)[1] for y in windows])

    def jacobian(x):
        # Kaufman's variable-projection Jacobian: -P⊥ (dB/dθ) c per window.
        f, r = unpack(x)
        rows = []
        for y, t in zip(windows, times):
            B = attack_basis(f, r, len(y), sr)
            c, _ = solve_real(B, y)
            a, b = c[:k], c[k:]
            Q, _ = np.linalg.qr(B)
            e = np.exp(-np.outer(t, r))
            w = 2*np.pi*np.outer(t, f)
            s, co = e*np.sin(w), e*np.cos(w)
            dfreq = (2*np.pi*t[:, None]*(a*co - b*s))*(f*np.log(2)/1200)
            drate = -t[:, None]*(a*s + b*co)*r
            D = np.hstack([dfreq, drate])
            rows.append(-(D - Q@(Q.T@D)))
        return np.vstack(rows)

    sol = least_squares(residual, x0, jac=jacobian, bounds=(lo, hi), x_scale='jac', max_nfev=max_nfev)
    f, r = unpack(sol.x)
    coefs, residuals = [], []
    for y in windows:
        c, res = solve_real(attack_basis(f, r, len(y), sr), y)
        coefs.append(c)
        residuals.append(res)
    return f, r, coefs, residuals
