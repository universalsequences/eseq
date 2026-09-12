"""Six physical waveguide regions in one shared-cursor tensor delay bank."""
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


def tensor(name, values):
    return f'(def {name} (tensor @shape [{len(values)}] @data ['+' '.join(f'{v:.12g}' for v in values)+']))'


def network(hat=False):
    # Each lane owns its original delay line and allpass state. Tensor delay
    # shares only the write cursor; there is no reduction in physical paths.
    tables = [tensor('path_lengths', [length for region in LENGTHS for length in region]),
              tensor('path_regions', [i//3 for i in range(18)]),
              tensor('path_lanes', [i%3 for i in range(18)]),
              tensor('region_first', list(range(0, 18, 3))),
              tensor('region_second', list(range(1, 18, 3))),
              tensor('region_third', list(range(2, 18, 3))),
              tensor('force_weights', [(-1)**i/np.sqrt(3) for _ in range(6) for i in range(3)])]
    if hat:
        contact = (HERE/'contact.lisp.in').read_text()
        scattering = '(def scattered (+ damped (* contact_delta (eq path_lanes 0))))'
        scattering += '\n(def junction (* scatter_norm (+ (gather scattered region_first) (* scatter_u (gather scattered region_second)) (gather scattered region_third))))'
        junction = '(* (gather junction path_regions) (gswitch (eq path_lanes 1) scatter_u 1))'
    else:
        contact = ''
        scattering = '(def scattered damped)\n(def junction (* 0.666666666666667 (+ (gather scattered region_first) (gather scattered region_second) (gather scattered region_third))))'
        junction = '(gather junction path_regions)'
    pickup = ' '.join(f'(* (gather scattered {field}) {np.sin(i*2.39996323+1)/np.sqrt(3):.12g})'
                      for i, field in enumerate(['region_first', 'region_second', 'region_third']))
    return Template((HERE/'network.lisp.in').read_text()).substitute(
        tables='\n'.join(tables), contact=contact, scattering=scattering, junction=junction, pickup=pickup)


def modes():
    return '''(def frequencies (/ mode_frequencies scale))
(def omega (* twopi (/ (min frequencies (* samplerate 0.47)) samplerate)))
(def radius (exp (/ (- (+ (/ mode_rates decay_v) contact_loss)) samplerate)))
(def c (latch (* radius (cos omega)) tick))
(def s (latch (* radius (sin omega)) tick))
(def band (clip (/ (- (* samplerate 0.47) frequencies) (* samplerate 0.06)) 0 1))
;; Deconvolve the reference two-pole contact response, without recorded phase.
(def reference_pole (exp (/ -1 (* samplerate 0.000025))))
(def compensation (/ (+ (* (- 1 reference_pole) (- 1 reference_pole))
  (* 2 reference_pole (- 1 (cos omega)))) (* (- 1 reference_pole) (- 1 reference_pole))))
(make-tensor-history mode_r @shape [8])
(make-tensor-history mode_i @shape [8])
(def x (read-tensor-history mode_r))
(def y (read-tensor-history mode_i))
(def next_r (+ (- (* c x) (* s y)) (* point_force (latch (* band compensation) tick))))
(def next_i (+ (* s x) (* c y)))
(write-tensor-history mode_r next_r)
(write-tensor-history mode_i next_i)
(def resolved next_i)'''


def source(name, tables, voicing, extra_params=None, hat=False, defaults=None):
    # One vector recurrence for the radiation bands avoids materializing
    # eighteen scalar table lookups, especially for two-state hi-hat fits.
    radiation = '(def radiation_hz (tensor @shape [18] @data ['+' '.join(f'{hz:.12g}' for hz in BANDS)+']))\n'
    radiation += '(def region_ids (tensor @shape [18] @data [0 0 0 1 1 1 2 2 2 3 3 3 4 4 4 5 5 5]))\n'
    radiation += '(def plate_vector (gather plates region_ids))\n'
    radiation += """(def filter_hz (min (/ radiation_hz scale) (* samplerate 0.43)))
(def filter_g (tan (* pi (/ filter_hz samplerate))))
(def filter_a1 (/ 1 (+ 1 (* filter_g (+ filter_g 0.3125)))))
(def filter_a2 (* filter_g filter_a1))
(def filter_a3 (* filter_g filter_a2))
(make-tensor-history filter_ic1 @shape [18])
(make-tensor-history filter_ic2 @shape [18])
(def ic1 (read-tensor-history filter_ic1))
(def ic2 (read-tensor-history filter_ic2))
(def a1 (latch filter_a1 tick))
(def a2 (latch filter_a2 tick))
(def a3 (latch filter_a3 tick))
(def v3 (- plate_vector ic2))
(def v1 (+ (* a1 ic1) (* a2 v3)))
(def v2 (+ ic2 (* a2 ic1) (* a3 v3)))
(write-tensor-history filter_ic1 (- (* 2 v1) ic1))
(write-tensor-history filter_ic2 (- (* 2 v2) ic2))
(def radiation_bands v1)"""
    output = """(def color_v (event-hold (cymbal-smooth (clip (mod color) -1 1) 8) tick))
(def wash_v (cymbal-smooth (clip (mod wash) 0 2) 8))
(def bell_v (cymbal-smooth (clip (mod bell) 0 2) 8))
(def width_v (cymbal-smooth (clip (mod width) 0 1) 8))
(def gain_v (cymbal-smooth (clip (mod gain) 0 2) 8))
(def radiation (* radiation_bands (latch (* band_gains (pow (/ radiation_hz 2800) (* color_v 0.6))) tick)))
"""
    output += '(def band_pan (tensor @shape [18] @data ['+' '.join(f'{np.sin(i*2.39996323)*.45:.9g}' for i in range(18))+']))\n'
    output += """(def body_left (sum (* radiation (+ 1 (* width_v band_pan)))))
(def body_right (sum (* radiation (- 1 (* width_v band_pan)))))
(def modal_pan (tensor @shape [8] @data [0 0.4 -0.5 0.3 -0.2 0.5 -0.4 0.1]))
(def mode_signal (* resolved (latch mode_gains tick)))
(def bell_left (sum (* mode_signal (+ 1 (* width_v modal_pan)))))
(def bell_right (sum (* mode_signal (- 1 (* width_v modal_pan)))))
(out (* 0.65 gain_v (+ (* wash_v body_left) (* bell_v bell_left))) 1 @name left)
(out (* 0.65 gain_v (+ (* wash_v body_right) (* bell_v bell_right))) 2 @name right)"""
    params = [(n, group, (defaults or {}).get(n, value), lo, hi, mod)
              for n, group, value, lo, hi, mod in PARAMS+(extra_params or [])]
    return Template((HERE/'engine.lisp.in').read_text()).substitute(name=name, inputs=INPUTS,
        parameters=parameters(params), tables=tables, voicing=voicing,
        network=network(hat), radiation=radiation, modes=modes(), output=output)


def basis_source(hat=False):
    # These are the exact forward engines against which the published physical
    # coefficients were identified. Keep calibration provenance independent of
    # scheduling/vectorization changes; compare.py validates the runtime sound.
    return (HERE/('calibration-basis-hihat.lisp' if hat else 'calibration-basis.lisp')).read_text()
