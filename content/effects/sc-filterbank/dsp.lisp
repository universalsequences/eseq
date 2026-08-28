; Switched-capacitor dual filterbank — the Sherman FB2 core, interview-faithful.
;
; The filters are CLOCK-driven, not voltage-driven: each SVF core holds a
; CONSTANT coefficient g = 2*sin(pi/ratio) and only ever changes its internal
; clock rate f_clk = ratio * fc. Cutoff sweeps move the clock; the sampled
; core leaks ZOH stepping and inharmonic aliasing as the clock falls into the
; audible band (crunch). Filter 2 has no cutoff of its own — its clock is
; filter 1's clock through a divider (1, 1.2, 1.5, 2, 3, 4, 5, 7), so the two
; resonances stay in a harmonic relation through any sweep.
;
; (floor harmonics) below needs dgenlisp >= v0.1.6 — before that, floor on a
; scalar signal was silently a no-op and the divider would read between taps.

(def in-l (in 1 @name signal-l))
(def in-r (in 2 @name signal-r))
(def dry (* 0.5 (+ in-l in-r)))

(param drive @min 0 @max 1 @default 0.35)
(param freq @min 0 @max 1 @default 0.5)
(param res @min 0 @max 1 @default 0.55)
(param mode @min 0 @max 1 @default 0)
(param mode2 @min 0 @max 1 @default 0)
(param harmonics @min 0 @max 7 @default 3)
(param crunch @min 0 @max 1 @default 0.25)
(param lfo-rate @min 0.02 @max 8 @default 0.4)
(param lfo-depth @min 0 @max 1 @default 0)
(param blend @min 0 @max 1 @default 0.5)
(param mix-wet @min 0 @max 1 @default 1)

; --- input drive: hairy before the filters, like the hardware's input stage
(def x (tanh (* dry (+ 1 (* drive 24)))))

; --- cutoff: exponential 30 Hz .. 8 kHz, LFO rides the position pre-link
(def lfo (* (* lfo-depth 0.35) (sin (* twopi (phasor lfo-rate)))))
(def fpos (clip (+ freq lfo) 0 1))
(def fc (* 30 (exp (* 5.586 fpos))))

; --- the switched-cap clock: crunch morphs ratio 100:1 -> 25:1 (log)
(def ratio (* 100 (exp (* crunch (log 0.25)))))
(def gc (* 2 (sin (/ pi ratio))))
(def k (+ 0.08 (* (- 1 res) 1.92)))

(def fclk (clip (* fc ratio) 200 (* samplerate 0.99)))
(def ph1 (phasor fclk))
; explicit wrap detector: ramp2trig misses wraps once the clock nears the
; host rate (phase decrements are tiny), and the clock lives up there
(make-history prevph)
(def tick1 (< ph1 (read-history prevph)))
(write-history prevph ph1)

; --- clock divider (the interview's harmonics knob): F2's clock is F1's
; clock divided by the selected ratio. Fractional ratios use a subtract-N
; accumulator, which is the dataflow form of a dual-modulus divider.
(def divisor (selector (floor harmonics) 1 1.2 1.5 2 3 4 5 7))
(make-history divcnt)
(def cnt (+ (read-history divcnt) tick1))
(def fire2 (>= cnt divisor))
(write-history divcnt (- cnt (* divisor fire2)))
(def tick2 (* tick1 fire2))

; --- one switched-cap SVF core: input sampled on the tick, Chamberlin
; update gated to the tick, states held (ZOH) between ticks. tanh on the bp
; state keeps self-oscillation bounded and adds the compressed scream.
(defmacro sc-svf (sig tick morph)
  (make-history lp)
  (make-history bp)
  (def xs (latch sig tick))
  (def hp (- xs (+ (read-history lp) (* k (read-history bp)))))
  (def bpn (tanh (+ (read-history bp) (* gc hp))))
  (def lpn (+ (read-history lp) (* gc bpn)))
  (write-history bp (mix (read-history bp) bpn tick))
  (write-history lp (mix (read-history lp) lpn tick))
  (def lpw (clip (- 1 (* 2 morph)) 0 1))
  (def hpw (clip (- (* 2 morph) 1) 0 1))
  (+ (* (read-history lp) lpw)
     (+ (* (read-history bp) (- 1 (+ lpw hpw)))
        (* hp hpw))))

(def f1 (sc-svf x tick1 mode))
(def f2 (sc-svf x tick2 mode2))

; --- clock bleed: a faint square at f_clk, keyed to crunch, rising as the
; clock drops into the audible band. At crunch 0 it is gone but the sampled
; core is still active — fully clean is not on the menu, this is a Sherman.
(def bleed (* (* (* crunch crunch) (* 0.02 (clip (- 1 (/ fclk 6000)) 0 1)))
              (- (* 2 (< ph1 0.5)) 1)))

(def wet (+ (+ (* (- 1 blend) f1) (* blend f2)) bleed))

(out (+ (* (- 1 mix-wet) in-l) (* mix-wet wet)) 1 @name out-l)
(out (+ (* (- 1 mix-wet) in-r) (* mix-wet wet)) 2 @name out-r)
