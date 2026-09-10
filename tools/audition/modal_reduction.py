"""Select a common modal subset without refitting poles or excitation levels.

Ranking covers every measured register/strength and six attack-to-tail windows.
The fourth-power residual weighting prioritizes poorly represented cases rather
than letting the loudest recording choose the bank. It is an acoustic energy
proxy, not a perceptual score; compiled audio comparisons are still required.
"""
import numpy as np

WINDOWS = ((0, .025), (.025, .1), (.1, .4), (.4, 1), (1, 3), (3, 5))


def compact_modes(arrays, count):
    rate, rise, direct = (arrays[k] for k in ('rate', 'rise', 'direct'))
    rows, modes = rate.shape
    if not 1 <= count <= modes:
        raise ValueError(f'Cannot select {count} of {modes} modes')
    amplitude = arrays['amplitude'].reshape(rows, -1, modes)
    energy = []
    for start, end in WINDOWS:
        t = np.linspace(start, end, 128)
        envelope = np.exp(-rate[..., None]*t)*(1-(1-direct[..., None])*np.exp(-t/rise[..., None]))
        e = amplitude**2*np.mean(envelope**2, axis=-1)[:, None, :]
        energy.append(e/np.maximum(np.sum(e, axis=-1, keepdims=True), 1e-30))
        # The loudest resonance must not hide the quieter modes that give a
        # bell/bar its character. Give their combined energy an equal vote.
        secondary = e.copy()
        np.put_along_axis(secondary, np.argmax(e, axis=-1, keepdims=True), 0, axis=-1)
        energy.append(secondary/np.maximum(np.sum(secondary, axis=-1, keepdims=True), 1e-30))
    energy = np.asarray(energy).reshape(-1, modes)
    selected = [0]  # Always preserve the pitch-reference mode.
    for _ in range(count-1):
        residual = np.maximum(0, 1-energy[:, selected].sum(axis=-1))
        score = np.sum(energy*residual[:, None]**4, axis=0)
        score[selected] = -1
        selected.append(int(np.argmax(score)))
    selected.sort()
    # Keep slot identity, every retained coefficient, and the original panning.
    # Taking one common subset also preserves the original interpolation path.
    compact = {name: value if name == 'tuning' else value[..., selected]
               for name, value in arrays.items()}
    report = {'full_modes': modes, 'runtime_modes': count, 'retained_slots': selected,
              'protected_slots': [0], 'energy_proxy_windows_s': WINDOWS,
              'ranking': 'equal total and secondary-mode energy; fourth-power residual weighting',
              'minimum_window_energy_fraction': float(energy[:, selected].sum(axis=-1).min())}
    return compact, report
