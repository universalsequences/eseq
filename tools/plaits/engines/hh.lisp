; ======================================================================
; Hi-hat engine (plaits/dsp/engine/hi_hat_engine.cc):
;   OUT = HiHat<SquareNoise, SwingVCA, resonance, one-stage envelope>
;   AUX = HiHat<RingModNoise, LinearVCA, no resonance, two-stage envelope>
; Unpatched trigger (patched = 0) = sustain mode.
; ======================================================================

; Exact stmlib ParameterInterpolator (see bd-pi in bd.lisp).
(defmacro hh-pi (target bstart called init)
  (make-history h_v)
  (make-history h_inc)
  (make-history h_started)
  (def v0 (gswitch (read-history h_started) (read-history h_v) init))
  (write-history h_started 1)
  (def inc (gswitch bstart (/ (- target v0) 12) (read-history h_inc)))
  (write-history h_inc inc)
  (def v (+ v0 (* called inc)))
  (write-history h_v v)
  v)

; First-sample flag (histories start at 0): 0 on the very first sample.
(defmacro hh-started ()
  (make-history h)
  (def v (read-history h))
  (write-history h 1)
  v)

; One uint32 phase accumulator of SquareNoise, emulated exactly with two
; 16-bit halves held in floats; returns the top bit (phase >> 31).
(defmacro hh-sq-bit (f)
  (make-history h_hi)
  (make-history h_lo)
  (def inc (floor (* (min f 0.499) 4294967296)))
  (def inc_hi (floor (/ inc 65536)))
  (def inc_lo (- inc (* inc_hi 65536)))
  (def lo0 (+ (read-history h_lo) inc_lo))
  (def carry (gte lo0 65536))
  (def lo (- lo0 (* carry 65536)))
  (def hi0 (+ (read-history h_hi) inc_hi carry))
  (def hi (- hi0 (* (gte hi0 65536) 65536)))
  (write-history h_lo lo)
  (write-history h_hi hi)
  (gte hi 32768))

; 808 metallic noise: 6 square oscillators.
(defmacro hh-square-noise (f0)
  (- (* 0.33 (+ (hh-sq-bit f0) (hh-sq-bit (* f0 1.304)) (hh-sq-bit (* f0 1.466))
                (hh-sq-bit (* f0 1.787)) (hh-sq-bit (* f0 1.932)) (hh-sq-bit (* f0 2.536))))
     1))

; plaits Oscillator<OSCILLATOR_SHAPE_SQUARE>(f, pw 0.5), Init state.
(defmacro hh-osc-square (f0 bstart)
  (make-history h_ph)
  (make-history h_next)
  (make-history h_low)    ; 1 - high_
  (def fc (clip f0 0.000001 0.25))
  (def freq (hh-pi fc bstart 1 0.001))
  (def pw (hh-pi (clip 0.5 (* 2 fc) (- 1 (* 2 fc))) bstart 1 0.5))
  (def ph1 (+ (gswitch (hh-started) (read-history h_ph) 0.5) freq))
  (def high0 (- 1 (read-history h_low)))
  (def above (gte ph1 pw))
  (def cross (abs (- high0 above)))
  (def t1 (/ (- ph1 pw) freq))
  (def high1 (gswitch cross above high0))
  (def wrap (gte ph1 1))
  (def ph (- ph1 wrap))
  (def t2 (/ ph freq))
  (def this_s (+ (read-history h_next)
                 (* cross (pl-this-blep t1))
                 (- (* wrap (pl-this-blep t2)))))
  (def next_s (+ (* cross (pl-next-blep t1))
                 (- (* wrap (pl-next-blep t2)))
                 (gte ph pw)))
  (write-history h_ph ph)
  (write-history h_next next_s)
  (write-history h_low (- 1 (* high1 (- 1 wrap))))
  (- (* 2 this_s) 1))

; plaits Oscillator<OSCILLATOR_SHAPE_SAW>, Init state.
(defmacro hh-osc-saw (f0 bstart)
  (make-history h_ph)
  (make-history h_next)
  (def fc (clip f0 0.000001 0.25))
  (def freq (hh-pi fc bstart 1 0.001))
  (def ph1 (+ (gswitch (hh-started) (read-history h_ph) 0.5) freq))
  (def wrap (gte ph1 1))
  (def ph (- ph1 wrap))
  (def t (/ ph freq))
  (def this_s (- (read-history h_next) (* wrap (pl-this-blep t))))
  (write-history h_next (+ (- (* wrap (pl-next-blep t))) ph))
  (write-history h_ph ph)
  (- (* 2 this_s) 1))

; RingModNoise: 3 square x saw pairs.
(defmacro hh-ringmod-noise (f0 bstart)
  (def ratio (/ f0 (+ 0.01 f0)))
  (def k (/ ratio samplerate))
  (+ (* (hh-osc-square (* 200 k) bstart) (hh-osc-saw (* 7530 k) bstart))
     (* (hh-osc-square (* 510 k) bstart) (hh-osc-saw (* 8075 k) bstart))
     (* (hh-osc-square (* 730 k) bstart) (hh-osc-saw (* 10500 k) bstart))))

; HiHat body: BPF colouration, clocked noise, VCA, HPF.
; resonance / two_stage / swing are 0/1 constants.
(defmacro hh-hat (metal bstart trig sustain accent f0 tone decay noisiness0 u_n resonance two_stage swing)
  (make-history h_env)
  (make-history h_clock)
  (make-history h_nsample)
  (def env_decay (- 1 (* 0.003 (pl-semi-ratio (* decay -84)))))
  (def cut_decay (- 1 (* 0.0025 (pl-semi-ratio (* decay -36)))))
  (def cutoff (clip (* (/ 150 samplerate) (pl-semi-ratio (* tone 72))) 0 (/ 16000 samplerate)))
  (def (c_lp c_bp c_hp) (pl-svf metal (pl-tan-accurate cutoff)
                                (/ 1 (gswitch resonance (+ 3 (* 3 tone)) 1))))
  (def noisiness (* noisiness0 noisiness0))
  (def noise_f (clip (* f0 (+ 16 (* 16 (- 1 noisiness)))) 0 0.5))
  (def clk0 (+ (read-history h_clock) noise_f))
  (def tick (gte clk0 1))
  (write-history h_clock (- clk0 tick))
  (def nsample (gswitch tick (- u_n 0.5) (read-history h_nsample)))
  (write-history h_nsample nsample)
  (def s (+ c_bp (* noisiness (- nsample c_bp))))
  (def env_prev (gswitch trig (* (+ 1.5 (* 0.5 (- 1 decay))) (+ 0.3 (* 0.7 accent)))
                         (read-history h_env)))
  (def env (* env_prev (gswitch (+ (gt env_prev 0.5) (- 1 two_stage)) env_decay cut_decay)))
  (write-history h_env env)
  (def sgain (hh-pi (* accent decay) bstart sustain 0))
  (def gain (gswitch sustain sgain env))
  (def sw0 (* s (gswitch (gt s 0) 4 0.1)))
  (def sw (* (+ (/ sw0 (+ 1 (abs sw0))) 0.1) gain))
  (def v (gswitch swing sw (* s gain)))
  (def (h_lp h_bp h_hp) (pl-svf v (pl-tan-accurate cutoff) 2))
  h_hp)

(defmacro plaits-hh (bstart bphase rise high patched note timbre morph harmonics accent)
  (def nz (noise))   ; per-sample uniform draw (identical (noise) nodes merge)
  (def f0 (pl-note-to-freq note))
  (def sustain (- 1 patched))
  (def trig (* bstart rise))
  (def out (hh-hat (hh-square-noise (* 2 f0)) bstart trig sustain accent f0 timbre morph harmonics
                   nz 1 0 1))
  (def aux (hh-hat (hh-ringmod-noise (* 2 f0) bstart) bstart trig sustain accent f0 timbre morph harmonics
                   (pl-noise-tap nz) 0 1 0))
  (tuple out aux))
