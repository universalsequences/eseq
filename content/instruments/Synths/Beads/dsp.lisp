; Beads — the Mutable Instruments Beads texture synthesizer's wavetable
; synth mode (beads/dsp/synth.cc), ported from the MIT-licensed eurorack
; sources (Emilie Gillet; see README.md). With nothing patched into its audio
; input, Beads plays a continuous wavetable oscillator (the dry signal) and
; scatters up to 32 wavetable grains on top of it (the wet signal), then runs
; both through its Griesinger reverb and output limiter.
;
; Module controls -> params:
; - TIME (+ attenuverter): wave position inside the bank; `time_amt` > 0
;   scatters each grain's position and makes the dry oscillator wander,
;   `time_amt` < 0 does both more sparsely/slowly.
; - SIZE / SHAPE / PITCH (+ attenuverters): grain length, grain envelope
;   (square -> decay -> bell -> reverse), and grain detune in semitones;
;   each `*_amt` randomizes the value per grain like the module's knobs.
; - DENSITY: below noon a regular grain clock (fast at 0), above noon random
;   intervals (dense at 1), noon = one grain per note.
; - FEEDBACK knob -> `bank` (12 wavetable banks, crossfaded on change).
; - DRY/WET -> oscillator vs grains; REVERB; FREEZE; quality -> `quality`
;   (the module's four media settings set the wavetable bit depth).
;
; The host note's gate is the SEED gate: holding a note runs the grain clock,
; its onset fires a grain immediately. Note pitch replaces Beads' fixed C3
; root (130.81 Hz) for both oscillator and grains.
;
; Differences from the module, all deliberate:
; - Grains are stolen by least remaining time (the module's slow-rate
;   scheduler) at every rate, idle grains before release tails, and the
;   four safety fades that follow each new grain land over the next four
;   samples instead of the same one.
; - Grains start one sample after their trigger so the differentiator can be
;   primed from the table at the start phase.
; - An attack/release envelope shapes the dry oscillator per note.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; 12 banks x 16 integrated waves x (256 + 4) samples; channel = bank*16 + wave.
(def bank_table (tensor @shape [260 192] @file "waves/bank.json"))

; ======================================================================
; params
; ======================================================================

(param time @default 0.35 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param time_amt @default 0 @min -1 @max 1)
(param size @default 0.7 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param size_amt @default 0 @min -1 @max 1)
(param shape @default 0.55 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param shape_amt @default 0 @min -1 @max 1)
(param detune @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param pitch_amt @default 0 @min -1 @max 1)
(param density @default 0.2 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank @default 0 @min 0 @max 11)
(param dry_wet @default 0.7 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param reverb @default 0.35 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param quality @default 3 @min 0 @max 3)
(param freeze @default 0 @min 0 @max 1)
(param amp_attack @default 5 @min 1 @max 4000 @unit ms)
(param amp_release @default 400 @min 5 @max 8000 @unit ms)
(param level_db @default -8 @min -36 @max 6 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)

; ======================================================================
; math helpers
; ======================================================================

(defmacro semi-ratio (semi)
  (exp (* 0.05776226505 semi)))

; stmlib SemitonesToRatio: its lookup table truncates the fractional semitone
; to 1/256, which sets Beads' grain clock rates and grain lengths exactly.
(defmacro beads-semi-ratio (semi)
  (def whole (floor semi))
  (semi-ratio (+ whole (/ (floor (* (- semi whole) 256)) 256))))

(defmacro db-amp (db)
  (exp (* 0.1151292546 db)))

; C's int() cast: truncate toward zero.
(defmacro trunc (x)
  (* (sign x) (floor (abs x))))

(defmacro smooth-step (x)
  (* x x (- 3 (* 2 x))))

; One previous sample of `x`: a free extra uniform draw from one noise source
; (identical `(noise)` expressions are merged into a single node).
(defmacro noise-tap (x)
  (make-history h)
  (def previous (read-history h))
  (write-history h x)
  previous)

; Hermite read of one integrated wave, linearly blended across `chan`
; (fractional bank*16 + wave), as InterpolateWaveHermite x2 in Beads.
(defmacro wave-read (chan phase)
  (def p (* phase 256))
  (def pint (floor p))
  (def pf (- p pint))
  (def xm1 (peek bank_table pint chan))
  (def x0 (peek bank_table (+ pint 1) chan))
  (def x1 (peek bank_table (+ pint 2) chan))
  (def x2 (peek bank_table (+ pint 3) chan))
  (def c (* (- x1 xm1) 0.5))
  (def v (- x0 x1))
  (def w (+ c v))
  (def a (+ w v (* (- x2 x0) 0.5)))
  (def b_neg (+ w a))
  (+ (* (+ (* (- (* a pf) b_neg) pf) c) pf) x0))

; Beads' RandomizedParameter::value() for a knob with no CV patched, without
; a random-walk source: `amt` > 0 blends toward a fresh random value, `amt` < 0
; does so with a random (sparser) blend. clamp = true variant: range 0..1.
(defmacro randomized-unit (base amt u_blend u_value)
  (def blend (gswitch (gt amt 0) amt (min 1 (* amt u_blend -3))))
  (clip (+ base (* (- u_value base) blend)) 0 1))

; Pitch variant: +/- 24 semitones, unclamped.
(defmacro randomized-semis (base amt u_blend u_value)
  (def blend (gswitch (gt amt 0) amt (min 1 (* amt u_blend -3))))
  (+ base (* (- (* 2 u_value) 1) 24 blend)))

; ======================================================================
; trigger generator (TriggerGenerator::Render, internal clock modes)
; ======================================================================

; Returns (trigger fractional_sample regularity).
(defmacro grain-clock (gate onset density u_ratio)
  (make-history h_phase)
  (make-history h_ratio)
  (def max_f (/ 130.81278 samplerate))
  (def regular (lt density 0.48))
  (def rand_mode (gt density 0.52))
  (def taps (* (not regular) (not rand_mode)))
  (def f_regular (* max_f (beads-semi-ratio (* density -192))))
  (def f_random (* 0.5 max_f (beads-semi-ratio (* (- 1 density) -192))))
  (def ratio0 (gswitch rand_mode (read-history h_ratio) 1))
  (def f (clip (* (gswitch rand_mode f_random f_regular) ratio0) 0.000001 0.25))
  (def ph1 (+ (read-history h_phase) (* (gt gate 0.5) f)))
  (def wrapped (* (gte ph1 1) (not onset)))
  (def reset (or onset wrapped))
  (write-history h_phase (gswitch onset 0 (gswitch wrapped (- ph1 1) ph1)))
  (def frac (gswitch wrapped (/ (- ph1 1) f) 0))
  ; -1 / LogOneMinusX(0.1 + 0.9u): exponential-ish random interval ratio.
  (def x (+ 0.1 (* 0.9 u_ratio)))
  (def log1mx (* (- x) (+ 1 (* x (+ 0.5 (* x (+ 0.3333 (* x 0.25))))))))
  (write-history h_ratio (gswitch (* reset rand_mode) (/ -1 log1mx) ratio0))
  (def pulse (* reset (gt gate 0.5)))
  (tuple (gswitch taps onset pulse)
         (gswitch taps 0 frac)
         (gswitch taps (clip (- 1 (* (- density 0.48) 25)) 0 1) (not rand_mode))))

; ======================================================================
; one wavetable grain (WavetableGrain + GrainEnvelope + VoiceStealingEnvelope)
; ======================================================================

; Envelope stage: 1 attack, 2 sustain, 3 release, 0 done (zeroed memory = idle).
; Returns (left right steal_rank fade_rank active): steal_rank is the remaining
; time (idle grains far below any playing one); fade_rank is the same but
; pushed past every grain once this grain has taken a safety-fade slot since
; the last new grain.
(defmacro beads-grain (k victim fade_target trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm n_frac n_su)
  (make-history h_phase)
  (make-history h_stage)
  (make-history h_eph)
  (make-history h_vval)
  (make-history h_vdec)
  (make-history h_rem)
  (make-history h_lp)
  (make-history h_lp2)
  (make-history h_prev)
  (make-history h_envs)
  (def mine (eq victim k))
  (def start (* trig mine))
  ; Idle grains latch zeros: keep every division finite (a NaN times
  ; act = 0 is still NaN and would silence the whole grain sum).
  (def f (max (latch n_f start) 0.000001))
  (def chan (latch n_chan start))
  (def gl (latch n_gl start))
  (def gr (latch n_gr start))
  (def ia (latch n_ia start))
  (def is (latch n_is start))
  (def ir (latch n_ir start))
  (def sm (latch n_sm start))
  (def su (max (latch n_su start) 1))
  (def st0 (read-history h_stage))
  (def eph0 (read-history h_eph))
  (def vval0 (read-history h_vval))
  (def rem0 (read-history h_rem))
  (def done0 (or (eq st0 0) (lte vval0 0)))
  (def act (* (not done0) (not start)))
  ; FadeOut(): after a new grain, the grains next in line to be stolen fade
  ; out (idle ones take a slot as a no-op, as in the module's sorted list).
  (make-history h_faded)
  (def fade_mine (* fade_hit (eq fade_target k)))
  (def faded (gswitch trig 0 (gswitch fade_mine 1 (read-history h_faded))))
  (write-history h_faded faded)
  (def fading (* fade_mine (gt rem0 fade_len) act))
  (def vdec1 (gswitch fading (/ 1 fade_len) (read-history h_vdec)))
  (def rem1 (gswitch fading (floor (* vval0 fade_len)) rem0))
  ; oscillator + differentiator
  (def phase0 (read-history h_phase))
  (def phase (gswitch start (* n_frac f) (gswitch act (wrap (+ phase0 f) 0 1) phase0)))
  (write-history h_phase phase)
  (def s (wave-read chan phase))
  (def prev0 (read-history h_prev))
  (def lp0 (read-history h_lp))
  (def lp20 (read-history h_lp2))
  (def cut (min (* 256 f) 1))
  (def lpa (+ lp0 (* cut (- (/ (- s prev0) (* f 131072)) lp0))))
  (def lpb (+ lp20 (* cut (- lpa lp20))))
  (def lp1 (gswitch act lpa (gswitch start 0 lp0)))
  (def lp21 (gswitch act lpb (gswitch start 0 lp20)))
  (write-history h_lp lp1)
  (write-history h_lp2 lp21)
  (write-history h_prev (gswitch (or act start) s prev0))
  (def q (/ (trunc (* lp21 su)) su))
  ; grain envelope, advanced only while not frozen
  (def adv (* act (not frozen)))
  (def in_attack (eq st0 1))
  (def tt (gswitch in_attack eph0 (- 1 eph0)))
  (def base (gswitch in_attack tt (* tt tt tt tt)))
  (def ev (gswitch (eq st0 2) 1 (+ base (* (- (smooth-step tt) base) sm))))
  (def inc (selector st0 ia is ir))
  (def ph2 (+ eph0 inc))
  (def wrapst (gte ph2 1))
  (def eph_adv (gswitch wrapst (* (/ (- ph2 1) inc) (selector st0 is ir 0)) ph2))
  (def st_adv (gswitch wrapst (% (+ st0 1) 4) st0))
  (def vval_adv (- vval0 vdec1))
  (def st1 (gswitch start 1 (gswitch adv st_adv st0)))
  (def eph1 (gswitch start (* n_ia n_frac) (gswitch adv eph_adv eph0)))
  (def vval1 (gswitch start 1 (gswitch adv vval_adv vval0)))
  (def envs (gswitch start 0 (gswitch adv (* ev (max vval_adv 0)) (read-history h_envs))))
  (def done1 (or (eq st1 0) (lte vval1 0)))
  (def rem2 (gswitch start (floor n_dur) (gswitch adv (gswitch done1 0 (- rem1 1)) rem1)))
  (write-history h_stage st1)
  (write-history h_eph eph1)
  (write-history h_vval vval1)
  (write-history h_vdec (gswitch start 0 vdec1))
  (write-history h_rem rem2)
  (write-history h_envs envs)
  (def y (* act q envs))
  (def steal_rank (gswitch done1 -1000000000 rem2))
  (tuple (* y gl) (* y gr) steal_rank (gswitch faded 1000000000 steal_rank) (not done1)))

; Lower remaining time wins; ties keep the lower index.
(defmacro argmin2 (ra ia rb ib)
  (def take_b (lt rb ra))
  (tuple (gswitch take_b rb ra) (gswitch take_b ib ia)))

; Four grains: returns (left right min_steal steal_index min_fade fade_index
; active_count).
(defmacro grain-quad (k0 victim fade_target trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm n_frac n_su)
  (def (l0 r0 s0 f0 a0) (beads-grain k0 victim fade_target trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm n_frac n_su))
  (def (l1 r1 s1 f1 a1) (beads-grain (+ k0 1) victim fade_target trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm n_frac n_su))
  (def (l2 r2 s2 f2 a2) (beads-grain (+ k0 2) victim fade_target trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm n_frac n_su))
  (def (l3 r3 s3 f3 a3) (beads-grain (+ k0 3) victim fade_target trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm n_frac n_su))
  (def (sa sia) (argmin2 s0 k0 s1 (+ k0 1)))
  (def (sb sib) (argmin2 s2 (+ k0 2) s3 (+ k0 3)))
  (def (sq siq) (argmin2 sa sia sb sib))
  (def (fa fia) (argmin2 f0 k0 f1 (+ k0 1)))
  (def (fb fib) (argmin2 f2 (+ k0 2) f3 (+ k0 3)))
  (def (fq fiq) (argmin2 fa fia fb fib))
  (tuple (+ l0 l1 l2 l3) (+ r0 r1 r2 r3) sq siq fq fiq (+ a0 a1 a2 a3)))

; Index of the smallest of eight (value index) pairs.
(defmacro argmin8 (m0 i0 m1 i1 m2 i2 m3 i3 m4 i4 m5 i5 m6 i6 m7 i7)
  (def (m01 i01) (argmin2 m0 i0 m1 i1))
  (def (m23 i23) (argmin2 m2 i2 m3 i3))
  (def (m45 i45) (argmin2 m4 i4 m5 i5))
  (def (m67 i67) (argmin2 m6 i6 m7 i7))
  (def (m03 i03) (argmin2 m01 i01 m23 i23))
  (def (m47 i47) (argmin2 m45 i45 m67 i67))
  (def (m07 i07) (argmin2 m03 i03 m47 i47))
  i07)

; ======================================================================
; continuous wavetable oscillator (WavetableVoice) — the dry signal
; ======================================================================

; RandomWalkModulation: smoothstep glide between random points at
; 1.01 Hz x (4 when amt > 0, 0.25 when amt <= 0); holds while frozen.
(defmacro random-walk (amt frozen u)
  (make-history h_phase)
  (make-history h_from)
  (make-history h_to)
  (def rate (* (gswitch (gt amt 0) 4 0.25) (/ 1.01 samplerate) (not frozen)))
  (def ph1 (+ (read-history h_phase) rate))
  (def wrapped (gte ph1 1))
  (def ph (gswitch wrapped (- ph1 1) ph1))
  (def from (gswitch wrapped (read-history h_to) (read-history h_from)))
  (def to (gswitch wrapped (- (* 2 u) 1) (read-history h_to)))
  (write-history h_phase ph)
  (write-history h_from from)
  (write-history h_to to)
  (+ from (* (- to from) (smooth-step ph))))

; Requested bank changes crossfade over 25 ms through silence, switching
; tables at the midpoint. Returns (bank_in_use xfade_gain 0..1).
(defmacro bank-xfade (requested)
  (make-history h_bank)
  (make-history h_next)
  (make-history h_count)
  (def dur (floor (* 0.025 samplerate)))
  (def half (floor (* 0.5 dur)))
  (def count0 (read-history h_count))
  (def begin (* (not (eq requested (read-history h_bank))) (eq count0 0)))
  (def next (gswitch begin requested (read-history h_next)))
  (def count1 (gswitch begin dur count0))
  (def count (max (- count1 1) 0))
  (def bank_now (gswitch (* (gt count1 0) (eq count half)) next (read-history h_bank)))
  (write-history h_next next)
  (write-history h_count count)
  (write-history h_bank bank_now)
  (tuple bank_now (/ (abs (- count half)) half)))

(defmacro wavetable-voice (freq position chan_base bits xfade)
  (make-history h_phase)
  (make-history h_lp)
  (make-history h_lp2)
  (make-history h_prev)
  (make-history h_wave)
  (def wave (+ (read-history h_wave) (* 0.025 (- position (read-history h_wave)))))
  (write-history h_wave wave)
  (def phase (wrap (+ (read-history h_phase) freq) 0 1))
  (write-history h_phase phase)
  (def s (wave-read (+ chan_base (* wave 15)) phase))
  (def cut (min (* 256 freq) 1))
  (def lp (+ (read-history h_lp) (* cut (- (/ (- s (read-history h_prev)) (* freq 131072)) (read-history h_lp)))))
  (def lp2 (+ (read-history h_lp2) (* cut (- lp (read-history h_lp2)))))
  (write-history h_lp lp)
  (write-history h_lp2 lp2)
  (write-history h_prev s)
  (clip (* (/ (trunc (* lp2 bits)) bits) xfade) -1 1))

; ======================================================================
; mix stages
; ======================================================================

; Beads' Limiter: peak follower with instant-ish attack, `decay` per sample.
(defmacro beads-limiter (l r decay)
  (make-history h_peak)
  (make-history h_lp)
  (def peak (min (max (abs l) (abs r)) 40))
  (def p0 (read-history h_peak))
  (def p1 (+ p0 (* (- peak p0) (gswitch (gt peak p0) 0.1 decay))))
  (def q0 (read-history h_lp))
  (def q1 (+ q0 (* (- p1 q0) (gswitch (gt p1 q0) 0.5 decay))))
  (write-history h_peak p1)
  (write-history h_lp q1)
  (def g (gswitch (lte q1 0.9) 1 (/ 0.9 q1)))
  (tuple (* l g) (* r g)))

; ~10 ms one-pole glide for knob moves; starts at the knob's value.
(defmacro smoothed (x)
  (make-history h)
  (make-history started)
  (def previous (gswitch (read-history started) (read-history h) x))
  (write-history started 1)
  (write-history h (+ previous (* 0.002 (- x previous)))))

; FxEngine stores its delay memory as int16: clip and truncate every write.
(defmacro fx-store (x)
  (/ (trunc (clip (* x 32768) -32768 32767)) 32768))

; FxEngine LFO: stmlib's approximate cosine recurrence, stepped once every 32
; samples (on the 32nd sample, as FxEngine's write pointer counts down).
; Returns 0..1, starting at its peak.
(defmacro fx-lfo (hz)
  (make-history h_y0)
  (make-history h_y1)
  (make-history h_count)
  (make-history h_started)
  (def f_raw (- (* hz (/ 32 samplerate)) 0.25))
  (def f_abs (abs f_raw))
  (def f_fold (gswitch (lt f_raw 0) f_abs (gswitch (gt f_raw 0.5) (- f_raw 0.5) f_raw)))
  (def sign_c (gswitch (lt f_raw 0) 16 (gswitch (gt f_raw 0.5) 16 -16)))
  (def coef (* sign_c f_fold (- 1 (* 2 f_fold))))
  (def started (read-history h_started))
  (def y0 (gswitch started (read-history h_y0) 0.5))
  (def y1 (gswitch started (read-history h_y1) (* coef 0.25)))
  (def count (+ (read-history h_count) 1))
  (def step (eq (% count 32) 0))
  (def y0n (gswitch step (- (* coef y0) y1) y0))
  (def y1n (gswitch step y0 y1))
  (write-history h_started 1)
  (write-history h_count (% count 32))
  (write-history h_y0 y0n)
  (write-history h_y1 y1n)
  (+ y1n 0.5))

; FxEngine allpass: tail read of an L-sample line (L - 1 samples back).
; `len` may be modulated (fractional reads interpolate).
(defmacro reverb-allpass (x len g)
  (make-history h)
  (def r (delay (read-history h) (- (* len (/ samplerate 48000)) 2) @max-delay 16384))
  (def w (fx-store (+ x (* r g))))
  (write-history h w)
  (+ (* w (- g)) r))

; Griesinger/Dattorro reverb (beads/dsp/fx/reverb.h) at amount 0..1.
(defmacro beads-reverb (l r amount)
  (make-history h_del1)
  (make-history h_del2)
  (make-history h_lp1)
  (make-history h_lp2)
  (def sr_scale (/ samplerate 48000))
  (def rv_mix (* amount amount 0.8))
  (def rt (+ 0.3 (* 0.6 amount amount amount)))
  (def input_gain (* 0.2 (- 1 (* 0.333 rt))))
  (def send_sqrt (* rv_mix (- 2 rv_mix)))
  (def send (* send_sqrt (- 2 send_sqrt) input_gain))
  (def rv_wet (/ 0.4 input_gain))
  (def rv_dry (* (- 1 rv_mix) (+ 1 rv_mix)))
  (def lfo1 (fx-lfo 0.5))
  (def lfo2 (fx-lfo 0.3))
  ; The first diffuser's in-loop smear (read at 10 + 80 lfo, rewrite at 100)
  ; makes its tail read come from 59 + 80 lfo samples back: a 60 + 80 lfo line.
  (def ap_a (reverb-allpass (* (+ l r) send) (+ 60 (* 80 lfo1)) 0.625))
  (def ap_b (reverb-allpass ap_a 214 0.625))
  (def ap_c (reverb-allpass ap_b 319 0.625))
  (def apout (reverb-allpass ap_c 527 0.625))
  ; loop half 1: apout + del2 tail -> lp -> 2 allpasses -> del1
  (def d2 (delay (read-history h_del2) (- (* (+ 6261 (* 50 lfo2)) sr_scale) 1) @max-delay 16384))
  (def lp1 (+ (read-history h_lp1) (* 0.6 (- (+ apout (* d2 rt)) (read-history h_lp1)))))
  (write-history h_lp1 lp1)
  (def wet_l (reverb-allpass (reverb-allpass lp1 2182 -0.625) 2690 0.625))
  (def del1_in (fx-store wet_l))
  (write-history h_del1 del1_in)
  ; loop half 2: apout + del1 tail -> lp -> 2 allpasses -> del2
  (def d1 (delay del1_in (* (+ 4460 (* 40 lfo1)) sr_scale) @max-delay 16384))
  (def lp2 (+ (read-history h_lp2) (* 0.6 (- (+ apout (* d1 rt)) (read-history h_lp2)))))
  (write-history h_lp2 lp2)
  (def wet_r (reverb-allpass (reverb-allpass lp2 2525 0.625) 2197 -0.625))
  (write-history h_del2 (fx-store wet_r))
  (tuple (+ (* l rv_dry) (* wet_l rv_wet))
         (+ (* r rv_dry) (* wet_r rv_wet))))

; ======================================================================
; shared per-sample controls
; ======================================================================

(def frozen (gt freeze 0.5))
(def onset (gt trigger 0.5))
(def time_v (clip (mod time) 0 1))
(def size_v (clip (mod size) 0 1))
(def shape_v (clip (mod shape) 0 1))
(def density_v (clip (mod density) 0 1))
(def bank_req (clip (floor (+ bank 0.5)) 0 11))
(def quality_i (clip (floor (+ quality 0.5)) 0 3))

; Eleven independent uniforms per sample from one noise source.
(def u0 (noise))
(def u1 (noise-tap u0))
(def u2 (noise-tap u1))
(def u3 (noise-tap u2))
(def u4 (noise-tap u3))
(def u5 (noise-tap u4))
(def u6 (noise-tap u5))
(def u7 (noise-tap u6))
(def u8 (noise-tap u7))
(def u9 (noise-tap u8))
(def u10 (noise-tap u9))

(def (clock_trig clock_frac regularity) (grain-clock gate onset density_v u0))
(def g_trig (* clock_trig (not frozen)))

; Grain scheduler state: the next victim (least remaining time, chosen at
; the end of the previous sample), the active count, the inter-grain gap, and
; the safety-fade target (next in line among grains not yet faded).
(make-history h_victim)
(make-history h_fade_target)
(make-history h_active)
(make-history h_interval)
(make-history h_fade_step)
(def victim (read-history h_victim))
(def fade_target (read-history h_fade_target))
(def interval0 (read-history h_interval))
(write-history h_interval (gswitch g_trig 1 (+ interval0 1)))

; GranularPlayer::SortGrainList fade times for the four next-in-line grains:
; a fast stream (< 25 ms apart) fades the oldest just before it is reused, a
; slow one fades two instantly and stretches the other two.
(def instant_xf (floor (* 0.005 samplerate)))
(def preemptive_xf (floor (* 0.1 samplerate)))
(def gap (latch interval0 g_trig))
(def fast_stream (lt gap (floor (* 0.025 samplerate))))
(def fast_swap (gt gap instant_xf))
(def slow_swap (gt gap preemptive_xf))
(def fade0 (gswitch fast_stream (gswitch fast_swap instant_xf (+ gap 1)) instant_xf))
(def fade1 instant_xf)
(def fade2 (gswitch fast_stream (gswitch fast_swap (+ gap 1) instant_xf) (gswitch slow_swap preemptive_xf gap)))
(def fade3 (gswitch fast_stream preemptive_xf (gswitch slow_swap gap preemptive_xf)))
(def fade_step (gswitch g_trig 0 (min (+ (read-history h_fade_step) 1) 5)))
(write-history h_fade_step fade_step)
(def fade_hit (* (gte fade_step 1) (lte fade_step 4)))
(def fade_len (max (selector fade_step fade0 fade1 fade2 fade3) 1))

; ---- new-grain parameters (latched by whichever grain starts) ----
(def g_time (randomized-unit time_v time_amt u1 u2))
(def g_size (randomized-unit size_v size_amt u3 u4))
(def g_shape (randomized-unit shape_v shape_amt u5 u6))
(def g_semis (randomized-semis (mod detune) pitch_amt u7 u8))

(def n_f (clip (* pitch (beads-semi-ratio g_semis) (/ 1 samplerate)) 0.000001 0.25))
(def n_chan (min (+ (* bank_req 16) (* g_time 15)) 191))

; ComputeGrainDuration: V-shaped around size = 1/3, 4 s down to 4 s / 512.
(def shortness (min 1 (* 1.025 (gswitch (lt g_size 0.3333333)
                                         (* g_size 3 (- 2 (* g_size 3)))
                                         (- 1 (* 1.5 (- g_size 0.3333333)))))))
(def n_dur (* (beads-semi-ratio (* shortness -108)) 4 samplerate))

; GrainEnvelope::Start
(def env_sustain (max (- 1 (* 3 g_shape)) 0))
(def shape_s (* (- g_shape 0.33333333) 1.5))
(def env_attack (gswitch (lt shape_s 0) 0 (gswitch (lt shape_s 0.5) (* shape_s shape_s shape_s 4) shape_s)))
(def env_release (* (- 1 env_attack) (- 1 env_sustain)))
(def n_ia (/ 1 (max (* n_dur env_attack) (* 0.0005 samplerate (+ 1 (* 3 env_sustain))))))
(def n_is (/ 1 (max (* n_dur env_sustain) 2)))
(def n_ir (/ 1 (max (* n_dur env_release) (* 0.010 samplerate))))
(def n_sm (gswitch (lt env_attack 0.5) (* 2 env_attack) (- 2 (* 2 env_attack))))

; ComputeGrainAmplitudeAndPan
(def n_active (+ (read-history h_active) 1))
(def spread (min 1 (+ (abs time_amt) (abs size_amt) (abs shape_amt) (abs pitch_amt) (- 1 regularity))))
(def g_pan (+ 0.5 (* (- u9 0.5) (min (* 0.5 n_active spread u10) 1))))
(def g_amp (/ 1 (sqrt n_active)))
(def n_gl (* g_amp (- 1 g_pan) (+ 1 g_pan)))
(def n_gr (* g_amp g_pan (- 2 g_pan)))

(def n_su (selector (+ quality_i 1) 16 64 128 65536))
(def osc_bits (selector (+ quality_i 1) 16 65536 128 65536))

; ======================================================================
; 32 grains
; ======================================================================

(def (gl0 gr0 gs0 gsi0 gf0 gfi0 ga0) (grain-quad 0 victim fade_target g_trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm clock_frac n_su))
(def (gl1 gr1 gs1 gsi1 gf1 gfi1 ga1) (grain-quad 4 victim fade_target g_trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm clock_frac n_su))
(def (gl2 gr2 gs2 gsi2 gf2 gfi2 ga2) (grain-quad 8 victim fade_target g_trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm clock_frac n_su))
(def (gl3 gr3 gs3 gsi3 gf3 gfi3 ga3) (grain-quad 12 victim fade_target g_trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm clock_frac n_su))
(def (gl4 gr4 gs4 gsi4 gf4 gfi4 ga4) (grain-quad 16 victim fade_target g_trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm clock_frac n_su))
(def (gl5 gr5 gs5 gsi5 gf5 gfi5 ga5) (grain-quad 20 victim fade_target g_trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm clock_frac n_su))
(def (gl6 gr6 gs6 gsi6 gf6 gfi6 ga6) (grain-quad 24 victim fade_target g_trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm clock_frac n_su))
(def (gl7 gr7 gs7 gsi7 gf7 gfi7 ga7) (grain-quad 28 victim fade_target g_trig fade_hit fade_len frozen n_f n_chan n_gl n_gr n_dur n_ia n_is n_ir n_sm clock_frac n_su))

(write-history h_victim (argmin8 gs0 gsi0 gs1 gsi1 gs2 gsi2 gs3 gsi3 gs4 gsi4 gs5 gsi5 gs6 gsi6 gs7 gsi7))
(write-history h_fade_target (argmin8 gf0 gfi0 gf1 gfi1 gf2 gfi2 gf3 gfi3 gf4 gfi4 gf5 gfi5 gf6 gfi6 gf7 gfi7))
(write-history h_active (+ ga0 ga1 ga2 ga3 ga4 ga5 ga6 ga7))

(def grains_l (+ gl0 gl1 gl2 gl3 gl4 gl5 gl6 gl7))
(def grains_r (+ gr0 gr1 gr2 gr3 gr4 gr5 gr6 gr7))

; ======================================================================
; dry oscillator, mix, reverb, output
; ======================================================================

(def walk (random-walk time_amt frozen u0))
(def osc_position (clip (+ time_v (* time_amt walk)) 0 1))
(def (bank_now xfade) (bank-xfade bank_req))
(def osc_f (clip (* pitch (beads-semi-ratio (mod detune)) (/ 1 samplerate)) 0.000001 0.25))
(def amp_env (adsr gate trigger amp_attack 10 1 amp_release))
(def osc (* (wavetable-voice osc_f osc_position (* bank_now 16) osc_bits xfade) amp_env))

(def dw (smoothed (clip (mod dry_wet) 0 1)))
(def dry_gain (* (- 1 dw) (+ 1 dw) 0.5))
(def wet_gain (* dw (- 2 dw)))
(def mixed_l (+ (* osc dry_gain) (* grains_l wet_gain)))
(def mixed_r (+ (* osc dry_gain) (* grains_r wet_gain)))

(def (pre_l pre_r) (beads-limiter mixed_l mixed_r (/ 1 (* samplerate 4))))
(def (rev_l rev_r) (beads-reverb pre_l pre_r (smoothed (clip (mod reverb) 0 1))))
(def (lim_l lim_r) (beads-limiter rev_l rev_r (/ 1 (* samplerate 6))))

(def out_gain (* (db-amp (smoothed (clip (mod level_db) -36 6))) (+ 0.5 (* 0.5 velocity))))
(def voice_left (* lim_l out_gain))
(def voice_right (* lim_r out_gain))
(out voice_left 1 @name left)
(out voice_right 2 @name right)
(use-defmacro voice-amp)
(use-defmacro release-window)
(out (voice-amp (release-window gate amp_release) voice_left voice_right) 3 @name amp @amp true)
