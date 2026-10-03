#!/usr/bin/env python3
"""Write phrase.lisp: a scratch-buffer script that programs the current track
with the reference phrase from phrase.json (notes with Delay micro-timing and
Duration, a Breath p-lock on every step, Tune p-locks following the player's
intonation)."""
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main():
    ph = json.loads((HERE/'phrase.json').read_text())
    lines = [
        ';; PM Milagre Brass: the opening horn phrase of "Bodas (Ao Vivo)"',
        ';; (Milton Nascimento, Milagre Dos Peixes), played verbatim.',
        ';;',
        ';; One-time track setup (track params are not scriptable from the scratch',
        f';; runtime): put PM Milagre Brass on the current track, preset "Milagre",',
        f";; BPM {ph['bpm']:g}, timebase {ph['timebase']}, length {ph['steps']} steps, Poly off (mono),",
        ';; Trig: legato. Then C-x C-b evaluates this buffer.',
        ';;',
        ';; Notes start on a step plus a Delay fraction (the player was in free time).',
        ';; Slurred notes overlap the next note by one step so mono legato slurs them.',
        ';; Every step locks Breath (the blowing contour) and Tune (cents, the',
        ";; player's intonation); the instrument glides Breath over Breath ms.",
        '',
        '(def mb-param (full short i)',
        '  (let ((name (seq-instrument-param-name i)))',
        '    (if (or (= name full) (= name short)) i (mb-param full short (+ i 1)))))',
        '(def mb-breath (mb-param "blow.breath" "breath" 0))',
        '(def mb-tune (mb-param "pitch.tune" "tune" 0))',
        '(seq-clear-track)',
        '',
    ]
    for n in ph['notes']:
        s = n['step']
        lines.append(f";; {n['name']}{' (slur)' if n['slur'] else ''}")
        lines.append(f"(seq-step-on {s}) (seq-set-transpose {s} {n['midi'] - 60}) "
                     f"(seq-plock-step {s} :velocity 1) (seq-plock-step {s} :duration {n['length']:.3f}) "
                     f"(seq-plock-step {s} :delay {n['delay']:.4f})")
    lines.append('')
    for k, (b, t) in enumerate(zip(ph['breath'], ph['tune'])):
        lines.append(f'(seq-plock-instrument-raw {k} mb-breath {b:.4f}) (seq-plock-instrument-raw {k} mb-tune {t:.1f})')
    (HERE/'phrase.lisp').write_text('\n'.join(lines) + '\n')
    print('wrote phrase.lisp')


if __name__ == '__main__':
    main()
