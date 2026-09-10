"""One auditable banded-waveguide engine; explicit scalar delay paths."""
from string import Template
import numpy as np
from common import HERE

BANDS = np.geomspace(100, 18500, 18)
LENGTHS = [[round(x*f) for x in [149, 631, 2461]] for f in [.73, .87, 1.03, 1.19, 1.37, 1.57]]
MODES = 8
INPUTS = '\n'.join(f'(def {n} (in {i+1} @name {n}' + (f' @modulator {i-4}' if i >= 5 else '') + '))'
    for i, n in enumerate(['gate', 'pitch', 'velocity', 'trigger', 'clock', 'mod1', 'mod2', 'mod3', 'mod4']))
# name, group, default, lower, upper, modulatable
PARAMS = [('character', 'voicing', .5, 0, 1, True), ('size', 'body', 1, .5, 2, True),
          ('decay', 'body', 1, .2, 3, True), ('damping', 'body', 1, .25, 3, True),
          ('hardness', 'stick', .5, 0, 1, True), ('bell', 'body', 1, 0, 2, True),
          ('wash', 'body', 1, 0, 2, True), ('touch', 'contact', 0, 0, 1, True),
          ('color', 'output', 0, -1, 1, True), ('width', 'output', 0, 0, 1, True),
          ('gain', 'output', 1, 0, 2, True), ('tracking', 'tuning', 0, 0, 1, False)]


def parameters(params):
    return '\n'.join(f'(param {n} @group {group} @default {v:g} @min {lo:g} @max {hi:g}'
        + (' @mod true @mod-mode additive' if mod else '') + ')' for n, group, v, lo, hi, mod in params)


def network(hat=False):
    lines = []
    for group, lengths in enumerate(LENGTHS):
        lines.append(f'(def region_rate{group} (+ (/ (* base_rate{group} (pow damping_v {group/5:.9g})) decay_v) contact_loss))')
        for lane, length in enumerate(lengths):
            i = group*3+lane
            lines.append(f""";; Region {group}, path {lane}: lossless fractional propagation.
(make-history outgoing{i})
(def path_samples{i} (max 8 (* {length} scale (/ samplerate 48000))))
(def integer_delay{i} (- (floor path_samples{i}) 2))
(def fractional{i} (+ 1 (- path_samples{i} (floor path_samples{i}))))
(def allpass_a{i} (/ (- 1 fractional{i}) (+ 1 fractional{i})))
(def incoming{i} (delay (read-history outgoing{i}) integer_delay{i}))
(make-history ap_x{i})
(make-history ap_y{i})
(def arrival{i} (- (+ (* allpass_a{i} incoming{i}) (read-history ap_x{i})) (* allpass_a{i} (read-history ap_y{i}))))
(write-history ap_x{i} incoming{i})
(write-history ap_y{i} arrival{i})
(def path_seconds{i} (/ path_samples{i} samplerate))
(def damped{i} (* (exp (* (- region_rate{group}) path_seconds{i})) arrival{i}))""")
    if hat:
        lines.append((HERE/'contact.lisp.in').read_text())
    for group in range(6):
        lanes = list(range(group*3, group*3+3))
        for lane, i in enumerate(lanes):
            value = f'(+ damped{i} contact_delta)' if hat and lane == 0 else f'damped{i}'
            lines.append(f'(def scattered{i} {value})')
        if hat:
            a, b, c = lanes
            lines.append(f'(def junction{group} (* scatter_norm (+ scattered{a} (* scatter_u scattered{b}) scattered{c})))')
        else:
            lines.append(f'(def junction{group} (* 0.666666666666667 (+ '+' '.join(f'scattered{i}' for i in lanes)+')))')
        for lane, i in enumerate(lanes):
            junction = f'(* scatter_u junction{group})' if hat and lane == 1 else f'junction{group}'
            lines.append(f'(write-history outgoing{i} (+ (- {junction} scattered{i}) (* force {(-1)**lane/np.sqrt(3):.12g})))')
        lines.append(f'(def plate{group} (+ (* force base_direct) '+' '.join(f'(* scattered{i} {np.sin(lane*2.39996323+1)/np.sqrt(3):.12g})' for lane,i in enumerate(lanes))+'))')
    return '\n'.join(lines)


def modes():
    return '''(def frequencies (/ mode_frequencies scale))
(def omega (* twopi (/ (min frequencies (* samplerate 0.47)) samplerate)))
(def radius (exp (/ (- (+ (/ mode_rates decay_v) contact_loss)) samplerate)))
(def c (* radius (cos omega)))
(def s (* radius (sin omega)))
(def band (clip (/ (- (* samplerate 0.47) frequencies) (* samplerate 0.06)) 0 1))
;; Deconvolve the reference two-pole contact response, without recorded phase.
(def reference_pole (exp (/ -1 (* samplerate 0.000025))))
(def compensation (/ (+ (* (- 1 reference_pole) (- 1 reference_pole))
  (* 2 reference_pole (- 1 (cos omega)))) (* (- 1 reference_pole) (- 1 reference_pole))))
(make-tensor-history mode_r @shape [8])
(make-tensor-history mode_i @shape [8])
(def x (read-tensor-history mode_r))
(def y (read-tensor-history mode_i))
(def next_r (+ (- (* c x) (* s y)) (* point_force band compensation)))
(def next_i (+ (* s x) (* c y)))
(write-tensor-history mode_r next_r)
(write-tensor-history mode_i next_i)
(def resolved next_i)'''


def basis_modes():
    lines = []
    for i in range(8):
        lines.append(f"""(def omega{i} (* twopi (/ (min (/ hz{i} scale) (* samplerate 0.47)) samplerate)))
(def radius{i} (exp (/ (- (+ (/ rate{i} decay_v) contact_loss)) samplerate)))
(make-history mx{i})
(make-history my{i})
(def mc{i} (* radius{i} (cos omega{i})))
(def ms{i} (* radius{i} (sin omega{i})))
(def mp{i} (exp (/ -1 (* samplerate 0.000025))))
(def mi{i} (/ (+ (* (- 1 mp{i}) (- 1 mp{i})) (* 2 mp{i} (- 1 (cos omega{i})))) (* (- 1 mp{i}) (- 1 mp{i}))))
(def mr{i} (+ (- (* mc{i} (read-history mx{i})) (* ms{i} (read-history my{i}))) (* point_force mi{i})))
(def resolved{i} (+ (* ms{i} (read-history mx{i})) (* mc{i} (read-history my{i}))))
(write-history mx{i} mr{i})
(write-history my{i} resolved{i})""")
    return '\n'.join(lines)


def source(name, tables, voicing, basis=False, extra_params=None, hat=False, defaults=None):
    radiation = '\n'.join(f'(def band{i} (svf plate{i//3} (min (/ {hz:.12g} scale) (* samplerate 0.43)) 3.2 1))'
                          for i, hz in enumerate(BANDS))
    if basis:
        output = '\n'.join(f'(out band{i} {i+1} @name band{i})' for i in range(18))
        output += '\n'+'\n'.join(f'(out resolved{i} {19+i} @name mode{i})' for i in range(8))
    else:
        # One vector recurrence for the radiation bands avoids materializing
        # eighteen scalar table lookups, especially for two-state hi-hat fits.
        radiation = '(def radiation_hz (tensor @shape [18] @data ['+' '.join(f'{hz:.12g}' for hz in BANDS)+']))\n'
        radiation += '(def region_ids (tensor @shape [18] @data [0 0 0 1 1 1 2 2 2 3 3 3 4 4 4 5 5 5]))\n'
        radiation += '(def plate_vector (+ '+' '.join(f'(* plate{i} (eq region_ids {i}))' for i in range(6))+'))\n'
        radiation += """(def filter_hz (min (/ radiation_hz scale) (* samplerate 0.43)))
(def filter_g (tan (* pi (/ filter_hz samplerate))))
(def filter_a1 (/ 1 (+ 1 (* filter_g (+ filter_g 0.3125)))))
(def filter_a2 (* filter_g filter_a1))
(def filter_a3 (* filter_g filter_a2))
(make-tensor-history filter_ic1 @shape [18])
(make-tensor-history filter_ic2 @shape [18])
(def ic1 (read-tensor-history filter_ic1))
(def ic2 (read-tensor-history filter_ic2))
(def v3 (- plate_vector ic2))
(def v1 (+ (* filter_a1 ic1) (* filter_a2 v3)))
(def v2 (+ ic2 (* filter_a2 ic1) (* filter_a3 v3)))
(write-tensor-history filter_ic1 (- (* 2 v1) ic1))
(write-tensor-history filter_ic2 (- (* 2 v2) ic2))
(def radiation_bands v1)"""
        output = """(def color_v (latch (cymbal-smooth (clip (mod color) -1 1) 8) tick))
(def wash_v (cymbal-smooth (clip (mod wash) 0 2) 8))
(def bell_v (cymbal-smooth (clip (mod bell) 0 2) 8))
(def width_v (cymbal-smooth (clip (mod width) 0 1) 8))
(def gain_v (cymbal-smooth (clip (mod gain) 0 2) 8))
(def radiation (* radiation_bands band_gains (pow (/ radiation_hz 2800) (* color_v 0.6))))
"""
        output += '(def band_pan (tensor @shape [18] @data ['+' '.join(f'{np.sin(i*2.39996323)*.45:.9g}' for i in range(18))+']))\n'
        output += """(def body_left (sum (* radiation (+ 1 (* width_v band_pan)))))
(def body_right (sum (* radiation (- 1 (* width_v band_pan)))))
(def modal_pan (tensor @shape [8] @data [0 0.4 -0.5 0.3 -0.2 0.5 -0.4 0.1]))
(def mode_signal (* resolved mode_gains))
(def bell_left (sum (* mode_signal (+ 1 (* width_v modal_pan)))))
(def bell_right (sum (* mode_signal (- 1 (* width_v modal_pan)))))
(out (* 0.65 gain_v (+ (* wash_v body_left) (* bell_v bell_left))) 1 @name left)
(out (* 0.65 gain_v (+ (* wash_v body_right) (* bell_v bell_right))) 2 @name right)"""
    params = [(n, group, (defaults or {}).get(n, value), lo, hi, mod)
              for n, group, value, lo, hi, mod in PARAMS+(extra_params or [])]
    return Template((HERE/'engine.lisp.in').read_text()).substitute(name=name, inputs=INPUTS,
        parameters=parameters(params), tables=tables, voicing=voicing,
        network=network(hat), radiation=radiation, modes=basis_modes() if basis else modes(), output=output)


def basis_source(hat=False):
    params = [(f'r{i}', 'fit', 3, .03, 120, False) for i in range(6)]
    params += [('contact_s', 'fit', .002, .0001, .025, False), ('direct', 'fit', .3, 0, 1, False)]
    for i in range(8):
        params.extend([(f'hz{i}', 'fit', 300*(i+1), 50, 20000, False),
                       (f'rate{i}', 'fit', 3, .03, 120, False)])
    voicing = '\n'.join(f'(def base_rate{i} r{i})' for i in range(6))+'\n(def base_contact_s contact_s)\n(def base_direct direct)\n'
    if hat:
        params += [('openness', 'contact', 0, 0, 1, True)]
        voicing += '(def openness_v (latch (cymbal-smooth (clip (mod openness) 0 1) 2) tick))\n'
    return source('Cymbal calibration basis', '', voicing, True, params, hat=hat)
