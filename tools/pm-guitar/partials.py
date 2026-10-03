"""Per-partial string identification by complex demodulation and variable projection.

Every partial k of a plucked note is a narrow band around k*f0*sqrt(1+B k^2).
In that band the record is a few damped complex exponentials: the new
partial (one or two poles - the string's two polarizations decay at
different rates) plus whatever was already ringing there (earlier notes,
fitted on the pre-onset window and allowed any new amplitude, since a finger
may damp them). Residues are complex amplitudes at the onset time.
"""
import numpy as np
import scipy.signal as ss
from scipy.optimize import least_squares

DECIM_HZ = 2000.0


def baseband(m, sr, hz, bw, a, b):
    """Complex envelope of m[a:b] around hz, lowpassed to +-bw/2, decimated."""
    n = np.arange(a, b)
    z = m[a:b]*np.exp(-2j*np.pi*hz*n/sr)
    sos = ss.butter(4, bw/2, 'lowpass', fs=sr, output='sos')
    z = ss.sosfiltfilt(sos, z.real) + 1j*ss.sosfiltfilt(sos, z.imag)
    step = max(1, int(sr/DECIM_HZ))
    return z[::step]*2, step/sr          # x2: one-sided analytic amplitude


def vp_solve(z, t, poles, ridge=1e-6):
    E = np.exp(np.outer(t, poles))
    G = E.conj().T@E
    G[np.diag_indices_from(G)] += ridge*np.real(np.trace(G))/len(G)
    c = np.linalg.solve(G, E.conj().T@z)
    return c, z - E@c


def fit_poles(z, t, n_new, fixed=(), dhz_bound=6.0, rate_bounds=(0.05, 400.0), seeds=None):
    """Free poles (detune Hz, decay 1/s) with linear complex amplitudes; `fixed` poles join the basis."""
    fixed = np.asarray(fixed, complex)
    best = None
    seed_sets = seeds or [[(0.0, 2.0)], [(0.0, 15.0)]]
    for seed in seed_sets:
        seed = (list(seed) + [(0.3*(-1)**i, 6.0*(i + 1)) for i in range(n_new)])[:n_new]
        x0 = np.array([s[0] for s in seed] + [np.log(s[1]) for s in seed])
        lo = np.r_[np.full(n_new, -dhz_bound), np.full(n_new, np.log(rate_bounds[0]))]
        hi = np.r_[np.full(n_new, dhz_bound), np.full(n_new, np.log(rate_bounds[1]))]
        x0 = np.clip(x0, lo + 1e-6, hi - 1e-6)

        def poles(x):
            return np.r_[-np.exp(x[n_new:]) + 2j*np.pi*x[:n_new], fixed]

        def res(x):
            r = vp_solve(z, t, poles(x))[1]
            return np.r_[r.real, r.imag]

        sol = least_squares(res, x0, bounds=(lo, hi), x_scale='jac', max_nfev=200)
        if best is None or sol.cost < best.cost:
            best = sol
    p = poles(best.x)
    c, r = vp_solve(z, t, p)
    return p, c, r


def partial_fit(m, sr, t0, hz, bw, pre_from, post_to, settle=None):
    """Fit one partial band. Returns dict with new poles/residues at t0 and diagnostics."""
    settle = settle if settle is not None else 1.6/bw
    pad = int(3/bw*sr)
    a = max(0, int(pre_from*sr) - pad)
    b = min(len(m), int(post_to*sr) + pad)
    z, dt = baseband(m, sr, hz, bw, a, b)
    tz = a/sr + np.arange(len(z))*dt - t0        # time relative to onset
    pre = (tz > pre_from - t0 + settle) & (tz < -settle)
    post = (tz > settle) & (tz < post_to - t0 - settle)
    if post.sum() < 20:
        return None
    zp, tp = z[post], tz[post]
    post_power = np.mean(np.abs(zp[:max(5, len(zp)//8)])**2)
    old = []
    if pre.sum() >= 20:
        pre_power = np.mean(np.abs(z[pre][-max(5, pre.sum()//4):])**2)
        if pre_power > post_power*10**(-35/10):
            # what rang before the pluck: up to two poles, any detune in band
            p_old, c_old, r_old = fit_poles(z[pre], tz[pre], 2 if pre.sum() > 80 else 1, dhz_bound=bw*0.45)
            # drop insignificant old poles
            amp = np.abs(c_old*np.exp(p_old.real*0))
            old = [p for p, aa in zip(p_old, amp) if aa > 0.05*amp.max()]
    results = []
    for n_new in (1, 2):
        if n_new == 2 and post.sum() < 300:      # need >=150 ms to separate polarizations
            break
        p, c, r = fit_poles(zp, tp, n_new, fixed=old, dhz_bound=min(6.0, bw*0.3))
        results.append((n_new, p, c, r))
    # second pole must earn >=1.5 dB residual improvement
    choice = results[0]
    if len(results) == 2:
        e1 = np.sum(np.abs(results[0][3])**2)
        e2 = np.sum(np.abs(results[1][3])**2)
        if e2 < e1*10**(-1.5/10):
            choice = results[1]
    n_new, p, c, r = choice
    new_p, new_c = p[:n_new], c[:n_new]
    # an old pole that sits on the new one (same string re-plucked, coincident
    # partial) is not separable: attribute that energy to the new note
    for j, po in enumerate(p[n_new:]):
        if np.min(np.abs(po.imag - new_p.imag))/(2*np.pi) < 0.25 and abs(po.real - new_p.real[0]) < 3:
            new_c = new_c.copy()
            k = np.argmin(np.abs(po.imag - new_p.imag))
            new_c[k] += c[n_new + j]
    fit_e = np.sum(np.abs(zp)**2)
    old_c = c[n_new:]
    old_e = sum(np.sum(np.abs(cc*np.exp(pp*tp))**2) for cc, pp in zip(old_c, p[n_new:]))
    return {
        'hz': [float(hz + q.imag/(2*np.pi)) for q in new_p],
        'rate': [float(-q.real) for q in new_p],
        'residue': [[float(q.real), float(q.imag)] for q in new_c],
        'snr_db': float(10*np.log10(fit_e/max(np.sum(np.abs(r)**2), 1e-30))),
        'old_fraction': float(old_e/max(fit_e, 1e-30)),
        'window_s': [float(settle), float(post_to - t0)],
        'level_db': float(10*np.log10(post_power + 1e-30)),
    }


def string_tuning(m, sr, takes, midi, k_max=30, fmax=7000.0):
    """f0 and inharmonicity B of a pitch from spectral peaks pooled over its takes."""
    from detect import midi_to_hz
    N = 1 << 17
    fr = np.fft.rfftfreq(N, 1/sr)
    acc = np.zeros(len(fr))
    for t0, t1 in takes:
        a, b = int((t0 + 0.03)*sr), int(min(t1, t0 + 0.6)*sr)
        seg = m[a:b]
        acc += np.abs(np.fft.rfft(seg*np.hanning(len(seg)), N))**2/np.sum(np.hanning(len(seg))**2)
    nominal = midi_to_hz(midi)
    # coarse f0 (cents) from the first four partials
    best = max(np.arange(-35, 36, 1), key=lambda c: sum(
        acc[np.abs(fr - k*nominal*2**(c/1200)).argmin()] for k in range(1, 5)))
    f0, B = nominal*2**(best/1200), 0.0
    for _ in range(3):
        ks, fs, ws = [], [], []
        for k in range(1, k_max + 1):
            c = k*f0*np.sqrt(1 + B*k*k)
            if c > fmax:
                break
            w = np.flatnonzero(np.abs(fr - c) < min(0.25*f0, 30, c*(2**(40/1200) - 1)))
            if len(w) < 3:
                continue
            i = w[np.argmax(acc[w])]
            if 0 < i < len(acc) - 1 and acc[i] > acc[w].mean()*4:
                # parabolic interpolation on log power
                y0, y1, y2 = np.log(acc[i - 1:i + 2] + 1e-30)
                d = 0.5*(y0 - y2)/(y0 - 2*y1 + y2)
                ks.append(k); fs.append(fr[i] + d*(fr[1] - fr[0])); ws.append(np.log(acc[i]/acc[w].mean()))
        ks, fs, ws = map(np.array, (ks, fs, ws))
        if len(ks) < 3:
            break
        # (f_k/k)^2 = f0^2 + f0^2 B k^2 : linear in k^2
        A = np.c_[np.ones(len(ks)), ks**2]
        sol = np.linalg.lstsq(A*ws[:, None], (fs/ks)**2*ws, rcond=None)[0]
        if sol[0] <= 0:
            break
        f0 = float(np.sqrt(sol[0])); B = float(max(0.0, sol[1]/sol[0]))
    return float(f0), float(B), [int(k) for k in ks]


def joint_partial(m, sr, takes, hz, bw, n_new_max=2, rate_floor=0.3, dhz=None):
    """Shared new poles at `hz` across takes; per-take residues and per-take old (pre-onset) poles.

    takes: list of (t0, t_end, pre_from). Returns poles, per-take residues, diagnostics.
    """
    settle = 1.6/bw
    dhz = dhz if dhz is not None else min(8.0, 0.3*bw)
    bands = []
    for t0, t1, pre_from in takes:
        pad = int(3/bw*sr)
        a = max(0, int(pre_from*sr) - pad)
        b = min(len(m), int(t1*sr) + pad)
        z, dt = baseband(m, sr, hz, bw, a, b)
        tz = a/sr + np.arange(len(z))*dt - t0
        pre = (tz > pre_from - t0 + settle) & (tz < -settle)
        post = (tz > settle) & (tz < t1 - t0 - settle)
        if post.sum() < 20:
            bands.append(None)
            continue
        zp, tp = z[post], tz[post]
        old = []
        T = tp[-1] - tp[0]
        if pre.sum() >= 20:
            pp = np.mean(np.abs(z[pre][-max(5, pre.sum()//4):])**2)
            if pp > np.mean(np.abs(zp[:max(5, len(zp)//8)])**2)*10**(-35/10):
                p_old, c_old, _ = fit_poles(z[pre], tz[pre], 2 if pre.sum() > 80 else 1, dhz_bound=bw*0.45)
                amp = np.abs(c_old)
                for p, aa in zip(p_old, amp):
                    # a pre-onset pole on the target itself is this string (or an
                    # unseparable coincident partial): it is replaced, not kept
                    if aa > 0.05*amp.max() and abs(p.imag/(2*np.pi)) > max(1.5, 1.2/max(T, 0.05)):
                        old.append(p)
        bands.append((zp, tp, np.array(old, complex)))
    live = [b for b in bands if b is not None]
    if not live:
        return None

    def solve_all(new_p):
        cs, rs = [], []
        for zp, tp, old in live:
            c, r = vp_solve(zp, tp, np.r_[new_p, old])
            cs.append(c); rs.append(r)
        return cs, rs

    def run(n_new):
        best = None
        for d0, s0 in (([0, 0], [1.0, 8.0]), ([1.0, -1.0], [3.0, 25.0]), ([-0.5, 1.0], [0.6, 4.0])):
            x0 = np.r_[d0[:n_new], np.log(s0[:n_new])]
            lo = np.r_[np.full(n_new, -dhz), np.full(n_new, np.log(rate_floor))]
            hi = np.r_[np.full(n_new, dhz), np.full(n_new, np.log(400))]
            def poles(x):
                return -np.exp(x[n_new:]) + 2j*np.pi*x[:n_new]
            def res(x):
                rs = solve_all(poles(x))[1]
                out = np.concatenate([np.r_[r.real, r.imag]/np.sqrt(len(r)) for r in rs])
                if n_new == 2:      # keep the poles distinct (no cancelling twins)
                    near = max(0.0, 1 - abs(x[1] - x[0])/0.4)*max(0.0, 1 - abs(x[3] - x[2])/np.log(2.0))
                    out = np.r_[out, 50*near*np.sqrt(np.mean(out**2))]
                return out
            sol = least_squares(res, np.clip(x0, lo + 1e-6, hi - 1e-6), bounds=(lo, hi), x_scale='jac', max_nfev=150)
            if best is None or sol.cost < best[0]:
                best = (sol.cost, poles(sol.x))
        p = best[1]
        cs, rs = solve_all(p)
        e = sum(np.sum(np.abs(r)**2)/len(r) for r in rs)
        return p, cs, e

    p, cs, e = run(1)
    if n_new_max >= 2 and max(tp[-1] for _, tp, _ in live) > 0.25:
        p2, cs2, e2 = run(2)
        if e2 < e*10**(-1.0/10):
            p, cs, e = p2, cs2, e2
    n = len(p)
    total = sum(np.sum(np.abs(zp)**2)/len(zp) for zp, _, _ in live)
    out_res, j = [], 0
    for b in bands:
        if b is None:
            out_res.append(None)
            continue
        out_res.append([[float(c.real), float(c.imag)] for c in cs[j][:n]])
        j += 1
    return {
        'hz': [float(hz + q.imag/(2*np.pi)) for q in p],
        'rate': [float(-q.real) for q in p],
        'residues': out_res,
        'snr_db': float(10*np.log10(total/max(e, 1e-30))),
    }
