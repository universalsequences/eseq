;@ungated
; ======================================================================
; Particle engine (plaits/dsp/engine/particle_engine.cc): 6 dust
; generators, each exciting a band-pass SVF whose frequency is re-randomized
; on the first impulse of every block; summed, low-passed, and sent through
; the plaits Diffuser (FxEngine allpass network, 12-bit delay memory).
; OUT = filtered/diffused, AUX = raw impulses.
; ======================================================================

; One Particle. `u` is this sample's uniform draw, `uf` the draw used to
; randomize the filter frequency when this is the block's first impulse.
; Returns (raw_impulse band_passed).
(defmacro particle-one (u uf sync density gain f0 spread q bstart)
  (make-history h_cant)
  (make-history h_pg)
  (make-history h_g)
  (make-history h_r)
  (def can (gswitch bstart 1 (- 1 (read-history h_cant))))
  (def ue (gswitch (* bstart sync) density u))
  (def imp (lte ue density))
  (def s (* imp ue gain))
  (def do_set (* imp can))
  (def fr (min (* (pl-semi-ratio (* spread (- (* 2 uf) 1))) f0) 0.25))
  (def pg (gswitch do_set (/ 0.5 (sqrt (max (* q fr (sqrt density)) 0.0000000001))) (read-history h_pg)))
  (def g (gswitch do_set (pl-tan-dirty fr) (read-history h_g)))
  (def r (gswitch do_set (/ 1 q) (read-history h_r)))
  (write-history h_cant (- 1 (* can (- 1 do_set))))
  (write-history h_pg pg)
  (write-history h_g g)
  (write-history h_r r)
  (def (lp bp hp) (pl-svf (* pg s) g r))
  (tuple s bp))

; FxEngine FORMAT_12_BIT delay memory: int16(trunc(x * 4096)) / 4096.
(defmacro particle-q12 (x)
  (/ (pl-trunc (clip (* x 4096) -32768 32767)) 4096))

; FxEngine allpass stage with a TAIL read of a `len`-sample line:
; acc += k_read * tail; write acc; acc = acc * k_write + tail.
(defmacro particle-ap (acc len k_read k_write)
  (make-history h_w)
  (def tail (delay (read-history h_w) (- len 2) @max-delay 4096))
  (def a (+ acc (* k_read tail)))
  (write-history h_w (particle-q12 a))
  (+ (* a k_write) tail))

; plaits Diffuser::Process(amount, rt, in).
(defmacro particle-diffuser (in amount rt)
  ; LFO_1: CosineOscillator APPROXIMATE at 0.3/48000*32, stepped when the
  ; FxEngine write pointer is a multiple of 32 (samples 31, 63, ...).
  (make-history h_cnt)
  (make-history h_y0)   ; offset from 0.5
  (make-history h_y1)   ; offset from the initial amplitude
  (def lfo_f (- 0.25 (/ (* 0.3 32) 48000)))
  (def lfo_c (* 16 lfo_f (- 1 (* 2 lfo_f))))
  (def lfo_a0 (* lfo_c 0.25))
  (def cnt (read-history h_cnt))
  (write-history h_cnt (% (+ cnt 1) 32))
  (def step (eq cnt 31))
  (def y0 (+ 0.5 (read-history h_y0)))
  (def y1 (+ lfo_a0 (read-history h_y1)))
  (def y0n (gswitch step (- (* lfo_c y0) y1) y0))
  (def y1n (gswitch step y0 y1))
  (write-history h_y0 (- y0n 0.5))
  (write-history h_y1 (- y1n lfo_a0))
  (def lfo (+ y1n 0.5))
  (def kap 0.625)
  (def a1 (particle-ap in 126 kap (- kap)))
  (def a2 (particle-ap a1 180 kap (- kap)))
  (def a3 (particle-ap a2 269 kap (- kap)))
  ; ap4: modulated read of a 444-sample line
  (make-history h_ap4)
  (def x4 (delay (read-history h_ap4) (- (+ 400 (* 43 lfo)) 1) @max-delay 4096))
  (def a4i (+ a3 (* kap x4)))
  (write-history h_ap4 (particle-q12 a4i))
  (def a4 (+ (* a4i (- kap)) x4))
  ; del: modulated read, written at the end of the chain
  (make-history h_del)
  (def xd (delay (read-history h_del) (- (+ 3070 (* 340 lfo)) 1) @max-delay 4096))
  (def a5 (+ a4 (* rt xd)))
  (def lp (pl-one-pole a5 0.75))
  (def a6 (particle-ap lp 1653 (- kap) kap))
  (def a7 (particle-ap a6 2010 kap (- kap)))
  (write-history h_del (particle-q12 a7))
  (+ in (* amount (- (* 2 a7) in))))

; Scalar part of the engine (block-gate safe); u_k / uf_k are particle k's
; draws (see particle-one). Returns (post-filtered sum, aux, diffusion).
(defmacro particle-pre (bstart rise note timbre morph harmonics u0_ uf0 u1 uf1 u2 uf2 u3 uf3 u4 uf4 u5 uf5)
  (def f0 (pl-note-to-freq note))
  (def ds (pl-note-to-freq (+ 60 (* timbre timbre 72))))
  (def density (/ (* ds ds) 6))
  (def gain (/ 1 density))
  (def qs (pl-hold (pl-semi-ratio (gswitch (gte morph 0.5) (* (- morph 0.5) 120) 0)) bstart))
  (def q (+ 0.5 (* qs qs)))
  (def spread (* 48 harmonics harmonics))
  (def rds (* 2 (abs (- morph 0.5))))
  (def diffusion (gswitch (lt morph 0.5) (* rds rds) 0))
  (def (s0 b0) (particle-one u0_ uf0 rise density gain f0 spread q bstart))
  (def (s1 b1) (particle-one u1 uf1 rise density gain f0 spread q bstart))
  (def (s2 b2) (particle-one u2 uf2 rise density gain f0 spread q bstart))
  (def (s3 b3) (particle-one u3 uf3 rise density gain f0 spread q bstart))
  (def (s4 b4) (particle-one u4 uf4 rise density gain f0 spread q bstart))
  (def (s5 b5) (particle-one u5 uf5 rise density gain f0 spread q bstart))
  (def (plp pbp php) (pl-svf (+ b0 b1 b2 b3 b4 b5) (pl-tan-dirty (min f0 0.49)) 2))
  (tuple plp (+ s0 s1 s2 s3 s4 s5) diffusion))

; Diffuser stage (delay lines: must stay outside any block-gate).
(defmacro particle-post (plp diffusion)
  (particle-diffuser plp (* 0.8 diffusion diffusion) (+ (* 0.5 diffusion) 0.25)))

; Wichmann-Hill uniform generator (all products < 2^24, exact in float32);
; seeds s1 s2 s3 in 1..30000.
(defmacro particle-wh (seed1 seed2 seed3)
  (make-history h1)
  (make-history h2)
  (make-history h3)
  (def a (% (* 171 (+ seed1 (read-history h1))) 30269))
  (def b (% (* 172 (+ seed2 (read-history h2))) 30307))
  (def c (% (* 170 (+ seed3 (read-history h3))) 30323))
  (write-history h1 (- a seed1))
  (write-history h2 (- b seed2))
  (write-history h3 (- c seed3))
  (pl-wrap (+ (/ a 30269) (/ b 30307) (/ c 30323))))

; Scalar part with the instrument's own random streams.
(defmacro particle-pre-noise (bstart rise note timbre morph harmonics)
  ; per-sample uniform draw (same node as the driver's u0: identical
  ; (noise) expressions are merged)
  (def nz (noise))
  ; Six independent per-sample draws: nz itself and nz offset (mod 1) by
  ; five Wichmann-Hill streams (uniform + independent of nz, and of each
  ; other, while keeping nz's fine resolution near 0 for low densities).
  ; The frequency draw is the same stream's previous sample.
  (def v1 (pl-wrap (+ nz (particle-wh 1201 2203 3307))))
  (def v2 (pl-wrap (+ nz (particle-wh 4409 5501 6607))))
  (def v3 (pl-wrap (+ nz (particle-wh 7703 8807 9901))))
  (def v4 (pl-wrap (+ nz (particle-wh 11003 12101 13309))))
  (def v5 (pl-wrap (+ nz (particle-wh 14401 15511 16607))))
  (particle-pre bstart rise note timbre morph harmonics
                nz (pl-noise-tap nz) v1 (pl-noise-tap v1) v2 (pl-noise-tap v2)
                v3 (pl-noise-tap v3) v4 (pl-noise-tap v4) v5 (pl-noise-tap v5)))

; ;@ungated engine: the particles, filters and post filter run inside
; (block-gate active ...); only the Diffuser's delay network runs always.
; Outputs are 0 when `active` is 0.
(defmacro plaits-particle (bstart bphase rise high patched note timbre morph harmonics accent active)
  (def (plp aux diffusion) (block-gate active (particle-pre-noise bstart rise note timbre morph harmonics)))
  (tuple (* active (particle-post (* active plp) (* active diffusion)))
         (* active aux)))
