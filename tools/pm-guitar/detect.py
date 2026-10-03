"""Onsets and new notes (multi-f0) of the reference guitar passage."""
import numpy as np
import scipy.signal as ss

NAMES = 'C C# D D# E F F# G G# A A# B'.split()


def note_name(midi):
    midi = int(round(midi))
    return f'{NAMES[midi % 12]}{midi//12 - 1}'


def hz_to_midi(hz):
    return 69 + 12*np.log2(np.asarray(hz)/440.0)


def midi_to_hz(midi):
    return 440.0*2**((np.asarray(midi, float) - 69)/12)


def onsets(m, sr, until):
    """Pluck contacts from 1.5-8 kHz spectral flux (the ringing strings mask broadband flux)."""
    hop, n = 256, 4096
    f, t, Z = ss.stft(m, sr, nperseg=n, noverlap=n - hop)
    flux = np.maximum(0, np.diff(np.log(np.abs(Z) + 1e-7), axis=1))[(f > 1500) & (f < 8000)].sum(0)
    od = flux - ss.medfilt(flux, 41)
    pk, _ = ss.find_peaks(od, height=np.percentile(od, 93), distance=int(0.05*sr/hop))
    times = t[pk + 1]
    strength = od[pk]
    keep = times < until
    return times[keep] - hop/sr/2, strength[keep]


def refine_onset(m, sr, t0, before=0.015, after=0.07):
    """Snap a flux detection to the pluck.

    The 4096-sample flux frame sees an attack up to ~40 ms before its centre,
    so search forward: the pluck is where the 1-8 kHz envelope first reaches
    a third of its peak in the search span, above the level just before it.
    """
    sos = ss.butter(4, [1000, 8000], 'bandpass', fs=sr, output='sos')
    a, b = int((t0 - before - 0.02)*sr), int((t0 + after)*sr)
    a = max(a, 0)
    seg = ss.sosfiltfilt(sos, m[a:b])
    env = np.sqrt(np.convolve(seg**2, np.ones(48)/48, 'same'))
    lo = int(0.02*sr)
    floor = np.median(env[:lo])
    span = env[lo:]
    peak = int(np.argmax(span))
    thresh = floor + (span[peak] - floor)/3
    i = int(np.argmax(span[:peak + 1] >= thresh))
    return (a + lo + i)/sr


def new_notes(m, sr, t0, gap, max_notes=4, lo_midi=36, hi_midi=79):
    """Notes whose harmonics gain energy across the onset, greedy harmonic sieve."""
    N = 1 << 16
    a = int((t0 + 0.02)*sr)
    b = a + int(min(0.15, max(0.06, gap - 0.02))*sr)
    c = int((t0 - 0.09)*sr)
    d = int((t0 - 0.005)*sr)
    def spec(s):
        return np.abs(np.fft.rfft(s*np.hanning(len(s)), N))/np.sum(np.hanning(len(s)))
    A, B = spec(m[a:b]), spec(m[max(c, 0):d])
    fr = np.fft.rfftfreq(N, 1/sr)
    D = np.maximum(0, A - 1.25*B)
    # Only spectral peaks count: window leakage of a strong partial must not
    # credit the neighbouring semitone.
    peak = np.zeros_like(D, bool)
    peak[ss.find_peaks(A)[0]] = True
    D = D*peak
    floor = A.max()*10**(-36/20)
    found, used = [], np.zeros_like(D)
    for _ in range(max_notes):
        best = None
        for midi in range(lo_midi, hi_midi + 1):
            f0 = midi_to_hz(midi)
            sal, hits = 0.0, 0
            for k in range(1, 9):
                w = (fr > k*f0*2**(-0.4/12)) & (fr < k*f0*2**(0.4/12))
                if not w.any():
                    continue
                v = max(0.0, (D[w] - used[w]).max())
                if v > floor:
                    hits += 1
                sal += v/k**0.5
            # a note needs its fundamental or second partial, and >=2 partials
            w1 = (fr > f0*0.98) & (fr < f0*1.02)
            w2 = (fr > 2*f0*0.98) & (fr < 2*f0*1.02)
            if hits >= 2 and max((D[w1] - used[w1]).max(), (D[w2] - used[w2]).max()) > floor*2:
                if best is None or sal > best[0]:
                    best = (sal, midi)
        if best is None or (found and best[0] < 0.25*found[0][0]):
            break
        # a bass string's weak fundamental: prefer the octave below when it
        # gains energy too (every partial of the upper note is its partial)
        low = midi_to_hz(best[1] - 12)
        w0 = (fr > low*0.985) & (fr < low*1.015)
        if best[1] - 12 >= lo_midi and w0.any() and (D[w0] - used[w0]).max() > floor:
            best = (best[0], best[1] - 12)
        found.append(best)
        f0 = midi_to_hz(best[1])
        for k in range(1, 30):
            w = (fr > k*f0*2**(-0.5/12)) & (fr < k*f0*2**(0.5/12))
            used[w] = D[w]
    return [(midi, float(sal)) for sal, midi in found]
