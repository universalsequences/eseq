; ======================================================================
; Swarm engine (plaits/dsp/engine/swarm_engine.cc): 8 voices, each a
; GrainEnvelope (control rate, random grain pitch/duration) driving a
; PolyBLEP saw (OUT) and a FastSineOscillator (AUX).
; Random draws come from the per-sample uniform `(noise)` (taps of the
; last 12 samples at each block start), so grain timing/pitch is not
; sample-identical to the module's LCG; everything else is exact.
; ======================================================================

; One SwarmVoice. `r1`/`r2` are this voice's two uniform draws for a grain
; randomization (only consumed on block starts where one happens).
; Returns (saw sine).
(defmacro swarm-voice (rank size_ratio f0 density burst start spread r1 r2 bstart bphase)
  ; ---- GrainEnvelope::Step (block rate) ----
  (make-history h_from)
  (make-history h_interval)  ; offset from 1
  (make-history h_phase)     ; offset from 1
  (make-history h_fm)
  (make-history h_amp)       ; offset from 0.5
  (make-history h_prev_big)
  (make-history h_fc)
  (def from (read-history h_from))
  (def interval (+ 1 (read-history h_interval)))
  (def phase (+ 1 (read-history h_phase)))
  (def fm (read-history h_fm))
  (def amp (+ 0.5 (read-history h_amp)))
  (def phase_a (gswitch start 0.5 (+ phase (* density fm))))
  (def wrap (* (- 1 start) (gte phase_a 1)))
  (def phase_b (- phase_a (* wrap (floor phase_a))))
  (def fm_a (gswitch start 16 fm))
  (def rnd (max start wrap))
  (def from_b (gswitch rnd (+ from interval) from))
  (def interval_b (gswitch rnd (- r1 from_b) interval))
  (def fm_b (gswitch rnd (gswitch burst (* fm_a (+ 0.8 (* 0.2 r2))) (+ 0.5 (* 1.5 r2))) fm_a))
  ; ---- GrainEnvelope::amplitude ----
  (def big (gte size_ratio 1))
  (def eph (clip (* (- phase_b 0.5) size_ratio) -1 1))
  (def target (gswitch big (* 0.5 (+ (cos (* 3.14159265 eph)) 1)) 1))
  (def fc_a (gswitch (abs (- big (read-history h_prev_big))) 0.5 (read-history h_fc)))
  (def fc_b (* fc_a 0.95))
  (def amp_b (+ amp (* (- 0.5 fc_b) (- target amp))))
  (def expo (gswitch big from_b (- (* 2 (+ from_b (* interval_b phase_b))) 1)))
  (write-history h_from (gswitch bstart from_b from))
  (write-history h_interval (- (gswitch bstart interval_b interval) 1))
  (write-history h_phase (- (gswitch bstart phase_b phase) 1))
  (write-history h_fm (gswitch bstart fm_b fm))
  (write-history h_amp (- (gswitch bstart amp_b amp) 0.5))
  (write-history h_prev_big (gswitch bstart big (read-history h_prev_big)))
  (write-history h_fc (gswitch bstart fc_b (read-history h_fc)))
  ; amp_b / expo are only consumed through interpolators latched on bstart.
  (def amplitude (* amp_b 0.125))
  (def f (* f0 (pl-semi-ratio (* 48 expo spread rank)) (+ 1 (* rank (+ rank 0.01) spread 0.25))))
  ; ---- AdditiveSawOscillator ----
  (make-history h_sp)
  (make-history h_sn)
  (def sf (pl-interp (min f 0.25) bstart bphase 0.01))
  (def sg (pl-interp amplitude bstart bphase 0))
  (def spa (+ (read-history h_sp) sf))
  (def sw (gte spa 1))
  (def sp1 (- spa sw))
  (def st (/ sp1 (max sf 0.000000001)))
  (def this_s (- (read-history h_sn) (* sw (pl-this-blep st))))
  (write-history h_sp sp1)
  (write-history h_sn (+ sp1 (- (* sw (pl-next-blep st)))))
  ; ---- FastSineOscillator (ADDITIVE) ----
  (make-history h_x)  ; offset from 1
  (make-history h_y)
  (def fq (min f 0.25))
  (def fpi (* fq 3.14159265))
  (def eps (pl-interp (* fpi (- 2 (* 0.32 fpi fpi))) bstart bphase 0))
  (def sam (pl-interp (gswitch (gte f 0.25) 0 (* amplitude (- 1 (* f 4)))) bstart bphase 0))
  (def x0 (+ 1 (read-history h_x)))
  (def y0 (read-history h_y))
  (def norm (+ (* x0 x0) (* y0 y0)))
  (def renorm (* bstart (max (lte norm 0.5) (gte norm 2))))
  (def scale (gswitch renorm (/ 1 (sqrt (max norm 0.000001))) 1))
  (def x1 (+ (* x0 scale) (* eps (* y0 scale))))
  (def y1 (- (* y0 scale) (* eps x1)))
  (write-history h_x (- x1 1))
  (write-history h_y y1)
  (tuple (* (- (* 2 this_s) 1) sg) (* sam x1)))

(defmacro plaits-swarm (bstart bphase rise high patched note timbre morph harmonics accent)
  ; per-sample uniform draw (same node as the driver's u0: identical
  ; (noise) expressions are merged)
  (def nz (noise))
  (def f0 (pl-note-to-freq note))
  (def density (* (pl-note-to-freq (* timbre 120)) 0.025 12))
  (def spread (* harmonics harmonics harmonics))
  (def sr0 (* 0.25 (pl-hold (pl-semi-ratio (* (- 1 morph) 84)) bstart)))
  (def burst patched)
  (def start (* bstart rise))
  ; uniform draws: the last 12 driver samples
  (def t0 nz)
  (def t1 (pl-noise-tap t0))
  (def t2 (pl-noise-tap t1))
  (def t3 (pl-noise-tap t2))
  (def t4 (pl-noise-tap t3))
  (def t5 (pl-noise-tap t4))
  (def t6 (pl-noise-tap t5))
  (def t7 (pl-noise-tap t6))
  (def t8 (pl-noise-tap t7))
  (def t9 (pl-noise-tap t8))
  (def t10 (pl-noise-tap t9))
  (def t11 (pl-noise-tap t10))
  (def (a0 b0) (swarm-voice -1 sr0 f0 density burst start spread t0 (pl-wrap (* t4 4099.37)) bstart bphase))
  (def (a1 b1) (swarm-voice -0.66666667 (* sr0 0.97) f0 density burst start spread t1 (pl-wrap (* t5 4099.37)) bstart bphase))
  (def (a2 b2) (swarm-voice -0.33333333 (* sr0 0.9409) f0 density burst start spread t2 (pl-wrap (* t6 4099.37)) bstart bphase))
  (def (a3 b3) (swarm-voice 0 (* sr0 0.912673) f0 density burst start spread t3 (pl-wrap (* t7 4099.37)) bstart bphase))
  (def (a4 b4) (swarm-voice 0.33333333 (* sr0 0.88529281) f0 density burst start spread t8 (pl-wrap (* t0 4099.37)) bstart bphase))
  (def (a5 b5) (swarm-voice 0.66666667 (* sr0 0.858734026) f0 density burst start spread t9 (pl-wrap (* t1 4099.37)) bstart bphase))
  (def (a6 b6) (swarm-voice 1 (* sr0 0.832972005) f0 density burst start spread t10 (pl-wrap (* t2 4099.37)) bstart bphase))
  (def (a7 b7) (swarm-voice 1.33333333 (* sr0 0.807982845) f0 density burst start spread t11 (pl-wrap (* t3 4099.37)) bstart bphase))
  (tuple (+ a0 a1 a2 a3 a4 a5 a6 a7) (+ b0 b1 b2 b3 b4 b5 b6 b7)))
