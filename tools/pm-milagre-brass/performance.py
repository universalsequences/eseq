"""The reference phrase as a step-sequencer performance, and an offline host
that plays it through the compiled instrument the way the app's step
sequencer does: notes start on a step plus a Delay fraction, the track is
mono with legato trig (slurred notes overlap and keep the voice and its
breath envelope), and every step carries a `breath` p-lock that jumps at the
step boundary (the instrument smooths it with `breath_ms`)."""
import ctypes

import numpy as np

BPM = 100.0
TIMEBASE = '1/32'
STEP = 60.0/BPM/8          # 1/32 note = 75 ms
STEPS = 128                # four 4/4 bars: 9.6 s, the band enters at 7.97 s
NAMES = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B']


def midi_and_cents(hz):
    m = 69 + 12*np.log2(hz/440.0)
    note = int(round(m))
    return note, float(100*(m - note))


def note_name(m):
    return f'{NAMES[m % 12]}{m//12 - 1}'


def events(notes, slur_overlap=1.0):
    """Score notes -> sequencer notes (step, delay, duration steps, midi, cents)."""
    out = []
    for i, n in enumerate(notes):
        on = n['on']
        step = int(on//STEP)
        delay = on/STEP - step
        end = n['off']
        nxt = notes[i + 1] if i + 1 < len(notes) else None
        if nxt is not None and nxt['slur']:
            end = nxt['on'] + slur_overlap*STEP        # overlap so mono legato slurs
        midi, cents = midi_and_cents(n['hz'])
        out.append(dict(step=step, delay=round(delay, 4), length=round((end - on)/STEP, 4),
                        midi=midi, name=note_name(midi), tune=round(cents, 1), slur=n['slur']))
    return out


def tune_steps(notes, frames):
    """Per-step Tune lock (cents from the note's MIDI pitch) following the
    measured pitch of the sounding note; held between notes."""
    ev = events(notes)
    out = []
    last = ev[0]['tune']
    for k in range(STEPS):
        inside = [f for f in frames if f['hz'] and k*STEP <= f['t'] < (k + 1)*STEP]
        if inside:
            i = inside[-1]['note']                     # the note sounding as the step ends
            hz = np.median([f['hz'] for f in inside if f['note'] == i])
            last = round(float(np.clip(1200*np.log2(hz/(440*2**((ev[i]['midi'] - 69)/12))), -100, 100)), 1)
        out.append(last)
    return out


def render(inst, notes, breath_steps, params=None, seconds=None, tune_steps=None):
    """Play the performance through an audition.Instrument; returns mono audio."""
    ev = events(notes)
    sr = inst.sample_rate
    seconds = seconds or STEPS*STEP
    n = int(seconds*sr)
    blk = inst.max_frames
    mem = inst.fresh_memory()
    from common import qualify
    for k, v in qualify(inst, params or {}).items():
        mem[inst.params[k]['cellId']] = v
    gate = np.zeros(n, np.float32)
    trig = np.zeros(n, np.float32)
    pitch = np.full(n, 440*2**((ev[0]['midi'] - 69)/12), np.float32)
    tune = np.full(n, ev[0]['tune'], np.float32)
    for e in ev:
        a = int(round((e['step'] + e['delay'])*STEP*sr))
        b = min(n, int(round((e['step'] + e['delay'] + e['length'])*STEP*sr)))
        gate[a:b] = 1
        if not e['slur']:
            trig[a] = 1
        pitch[a:] = 440*2**((e['midi'] - 69)/12)
        tune[a:] = e['tune']
    ins = [np.zeros(blk, np.float32) for _ in range(inst.n_in)]
    outs = [np.zeros(blk, np.float32) for _ in range(inst.n_out)]
    inptrs = (ctypes.POINTER(ctypes.c_float)*inst.n_in)(*[a.ctypes.data_as(ctypes.POINTER(ctypes.c_float)) for a in ins])
    outptrs = (ctypes.POINTER(ctypes.c_float)*inst.n_out)(*[a.ctypes.data_as(ctypes.POINTER(ctypes.c_float)) for a in outs])
    ch = inst.inputs
    breath_cell = inst.params['blow.breath']['cellId']
    tune_cell = inst.params['pitch.tune']['cellId']
    y = np.zeros(n, np.float32)
    for s in range(0, n, blk):
        f = min(blk, n - s)
        for name, arr in (('gate', gate), ('trigger', trig), ('pitch', pitch)):
            ins[ch[name]][:f] = arr[s:s + f]
            ins[ch[name]][f:] = 0
        ins[ch['velocity']][:] = 1.0
        k = min(int(s/sr/STEP), len(breath_steps) - 1)
        mem[breath_cell] = breath_steps[k]
        mem[tune_cell] = tune[s] if tune_steps is None else tune_steps[k]
        inst.process_fn(inptrs, outptrs, f, mem.ctypes.data_as(ctypes.c_void_p), ctypes.byref(inst.context), None)
        y[s:s + f] = outs[0][:f]
    return y
