; Beads — the Mutable Instruments Beads granular texture processor
; (beads/dsp/processor.cc, granular_processor.h), ported from the
; MIT-licensed eurorack sources (Emilie Gillet; see README.md). The input is
; recorded into a looping buffer that up to 32 grains read from; with SIZE
; fully clockwise the grains give way to a tempo-free multitap delay (or, in
; freeze, a beat-slicing looper). Both run through Beads' feedback path,
; reverb and limiters.
;
; Module controls -> params:
; - TIME / SIZE / SHAPE / PITCH (+ attenuverters `*_amt`): grain position in
;   the buffer, length (reversed below 1/3), envelope, and transposition; a
;   positive `*_amt` randomizes each grain, a negative one does so sparsely.
;   In delay mode SIZE's attenuverter is the LP/HP filter, PITCH's adds a
;   random-walk vibrato, TIME picks the delay multiple (or the frozen slice).
; - DENSITY: below noon a regular grain clock (fast at 0), above noon random
;   intervals (dense at 1), noon = only SEED presses.
; - FEEDBACK, DRY/WET, REVERB; FREEZE stops recording.
; - SEED: `seed` > 0.5 holds the SEED button (each rising edge fires a grain);
;   `latch` is the button's latched state (grains run without SEED). In delay
;   mode an unlatched SEED taps the tempo.
; - `quality`: the module's four media, 0 dirty tape (24 kHz, 8-bit mu-law,
;   16 s, wow & flutter), 1 clean tape (24 kHz, 12-bit, 10.7 s), 2 vintage
;   digital (32 kHz, 12-bit, 8 s), 3 hi-fi (48 kHz, 16-bit, 4 s).
; - `input_db`: manual input gain (the module's gain setting).
;
; Differences from the module, all deliberate:
; - Everything runs at the host rate. The slower media keep their recording
;   time, bit depth and time constants, and are band-limited to their codec
;   rate (4th-order low-pass at 0.45 x the media rate) instead of resampled.
; - No auto gain; clean tape's soft clipping follows a peak follower of the
;   gained input rather than the auto-gain's estimate.
; - Grains are stolen by least remaining time at every rate, idle grains
;   before release tails, and the four safety fades that follow each new
;   grain land over the next four samples instead of the same one.
; - SEED presses fire a grain even while latched.
; - Stereo only (no mono 2x-time mode); no gate output.

(def in_l (in 1 @name left))
(def in_r (in 2 @name right))
(def mod1 (in 3 @name mod1 @modulator 1))
(def mod2 (in 4 @name mod2 @modulator 2))
(def mod3 (in 5 @name mod3 @modulator 3))
(def mod4 (in 6 @name mod4 @modulator 4))

(def ulaw_table (tensor @shape [256 1] @file "data/ulaw.json"))

; The recording buffer: 16 s at 48 kHz, + 4 interpolation-tail frames
; (mirrors of frames 0..3, as DelayLine stores them) + 1 scratch frame that
; absorbs the writes that don't happen.
(def kCapacity 768000)
(def kScratch 768004)
(def ring (tensor @shape [768005 2]))
; FREEZE's tail: 256 frames recorded while frozen, crossfaded back in after.
(def freeze_tail (tensor @shape [257 2]))

; ======================================================================
; params
; ======================================================================

(param time @default 0.15 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param time_amt @default 0.4 @min -1 @max 1)
(param size @default 0.75 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param size_amt @default 0 @min -1 @max 1)
(param shape @default 0.6 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param shape_amt @default 0 @min -1 @max 1)
(param pitch @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit st)
(param pitch_amt @default 0.1 @min -1 @max 1)
(param density @default 0.75 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param feedback @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param dry_wet @default 0.8 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param reverb @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param freeze @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param seed @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param latch @default 1 @min 0 @max 1)
(param quality @default 3 @min 0 @max 3)
(param input_db @default 0 @min -24 @max 24 @unit dB)
(param level_db @default 0 @min -24 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)

; ======================================================================
; math helpers
; ======================================================================

(defmacro semi-ratio (semi)
  (exp (* 0.05776226505 semi)))

; stmlib SemitonesToRatio: its lookup table truncates the fractional semitone
; to 1/256.
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

; A one-pole coefficient `c` set for a rate k times slower than the host's.
(defmacro rate-coef (c k)
  (gswitch (eq k 1) c (- 1 (pow (- 1 c) (/ 1 k)))))

; One previous sample of `x`: a free extra uniform draw from one noise source
; (identical `(noise)` expressions are merged into a single node).
(defmacro noise-tap (x)
  (make-history h)
  (def previous (read-history h))
  (write-history h x)
  previous)

; stmlib SoftClip.
(defmacro soft-clip (x)
  (gswitch (lt x -3) -1 (gswitch (gt x 3) 1 (/ (* x (+ 27 (* x x))) (+ 27 (* 9 x x))))))

; stmlib OnePole::tan<FREQUENCY_FAST>.
(defmacro tan-fast (f)
  (def f2 (* f f))
  (* f (+ 3.14159265 (* f2 (+ 10.108047 (* f2 55.787388))))))

; stmlib Svf, low- or high-pass output; `hold` keeps the state.
(defmacro st-svf (x g r hp_mode hold)
  (make-history h_s1)
  (make-history h_s2)
  (def s1 (read-history h_s1))
  (def s2 (read-history h_s2))
  (def h (/ 1 (+ 1 (* r g) (* g g))))
  (def hp (* (- x (* r s1) (* g s1) s2) h))
  (def bp (+ (* g hp) s1))
  (def lp (+ (* g bp) s2))
  (write-history h_s1 (gswitch hold s1 (+ (* g hp) bp)))
  (write-history h_s2 (gswitch hold s2 (+ (* g bp) lp)))
  (gswitch hp_mode hp lp))

; 4th-order Butterworth low-pass standing in for a slower codec.
(defmacro codec-lp (x fc)
  (biquad (biquad x fc 0.5411961 1 0) fc 1.3065630 1 0))

; ----------------------------------------------------------------------
; sample formats (DelayLine Item<resolution, 2>::Store / value)
; ----------------------------------------------------------------------

(defmacro clip16 (x)
  (clip (trunc (* x 32768)) -32768 32767))

(defmacro q16 (x)
  (/ (clip16 x) 32768))

(defmacro q12 (x)
  (/ (floor (/ (clip16 x) 16)) 2048))

; Lin2MuLaw then MuLaw2Lin (beads/dsp/mu_law.h) of one sample.
(defmacro mu-law (x)
  (def pv (floor (/ (clip16 x) 4)))
  (def m (min (+ (min (abs pv) 8159) 33) 8191))
  (def seg (+ (gt m 63) (gt m 127) (gt m 255) (gt m 511) (gt m 1023) (gt m 2047) (gt m 4095)))
  (def p2 (selector (+ seg 1) 2 4 8 16 32 64 128 256))
  (def mant (- (floor (/ m p2)) (* 16 (floor (/ m (* p2 16))))))
  (def code (+ (* seg 16) mant))
  (peek ulaw_table (gswitch (lt pv 0) (- 127 code) (- 255 code)) 0))

(defmacro store-q (x qsel)
  (gswitch (eq qsel 1) (block-gate dirty (mu-law x)) (gswitch (eq qsel 4) (q16 x) (q12 x))))

; ----------------------------------------------------------------------
; buffer reads
; ----------------------------------------------------------------------

; DelayLine::Read<INTERPOLATION_HERMITE>(delay_integral, delay_fractional),
; both channels. `wp` is the write pointer after this sample's write.
(defmacro ring-read (wp len di frac)
  ; (wp + di + len - 1) mod len, for 0 <= di < 2 len
  (def i0 (+ wp di len -1))
  ; (floor: a no-op that lets the compiler skip peek's interpolating reads)
  (def i (floor (- i0 (* len (+ (gte i0 len) (gte i0 (* 2 len)))))))
  (def l0 (peek ring i 0))
  (def l1 (peek ring (+ i 1) 0))
  (def l2 (peek ring (+ i 2) 0))
  (def l3 (peek ring (+ i 3) 0))
  (def r0 (peek ring i 1))
  (def r1 (peek ring (+ i 1) 1))
  (def r2 (peek ring (+ i 2) 1))
  (def r3 (peek ring (+ i 3) 1))
  (def cl (* (- l2 l0) 0.5))
  (def vl (- l1 l2))
  (def wl (+ cl vl))
  (def al (+ wl vl (* (- l3 l1) 0.5)))
  (def bl (+ wl al))
  (def cr (* (- r2 r0) 0.5))
  (def vr (- r1 r2))
  (def wr (+ cr vr))
  (def ar (+ wr vr (* (- r3 r1) 0.5)))
  (def br (+ wr ar))
  (tuple (+ (* (+ (* (- (* al frac) bl) frac) cl) frac) l1)
         (+ (* (+ (* (- (* ar frac) br) frac) cr) frac) r1)))

; ======================================================================
; randomized parameters, random walks
; ======================================================================

; RandomizedParameter::value() with no CV and no random-walk source:
; `amt` > 0 blends toward a fresh random value, `amt` < 0 does so with a
; random (sparser) blend. clamp = true variant: range 0..1.
(defmacro randomized-unit (base amt u_blend u_value)
  (def blend (gswitch (gt amt 0) amt (min 1 (* amt u_blend -3))))
  (clip (+ base (* (- u_value base) blend)) 0 1))

; Pitch variant: +/- 24 semitones, unclamped.
(defmacro randomized-semis (base amt u_blend u_value)
  (def blend (gswitch (gt amt 0) amt (min 1 (* amt u_blend -3))))
  (+ base (* (- (* 2 u_value) 1) 24 blend)))

; RandomWalkModulation: smoothstep glide between random points at
; `hz` x (4 when amt > 0, 0.25 when amt <= 0).
(defmacro random-walk (hz amt u hold)
  (make-history h_phase)
  (make-history h_from)
  (make-history h_to)
  (def rate (* (not hold) (gswitch (gt amt 0) 4 0.25) (/ hz samplerate)))
  (def ph1 (+ (read-history h_phase) rate))
  (def wrapped (gte ph1 1))
  (def ph (gswitch wrapped (- ph1 1) ph1))
  (def from (gswitch wrapped (read-history h_to) (read-history h_from)))
  (def to (gswitch wrapped (- (* 2 u) 1) (read-history h_to)))
  (write-history h_phase ph)
  (write-history h_from from)
  (write-history h_to to)
  (+ from (* (- to from) (smooth-step ph))))

; ======================================================================
; trigger generator (TriggerGenerator::Render, internal clock modes)
; ======================================================================

; `gate` is the merged SEED gate, `onset` its rising edge (or a SEED press
; while latched), `tap` a SEED press. Returns (trigger fractional_sample
; regularity).
(defmacro grain-clock (gate onset tap latched density u_ratio)
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
  (def ph1 (+ (read-history h_phase) (* gate f)))
  (def wrapped (* (gte ph1 1) (not onset)))
  (def reset (or onset wrapped))
  (write-history h_phase (gswitch onset 0 (gswitch wrapped (- ph1 1) ph1)))
  (def frac (gswitch wrapped (/ (- ph1 1) f) 0))
  ; -1 / LogOneMinusX(0.1 + 0.9u): exponential-ish random interval ratio.
  (def x (+ 0.1 (* 0.9 u_ratio)))
  (def log1mx (* (- x) (+ 1 (* x (+ 0.5 (* x (+ 0.3333 (* x 0.25))))))))
  (write-history h_ratio (gswitch (* reset rand_mode) (/ -1 log1mx) ratio0))
  (def pulse (* reset gate))
  (tuple (gswitch taps tap pulse)
         (gswitch taps 0 frac)
         (gswitch taps (gswitch (* latched (not tap)) 0 (clip (- 1 (* (- density 0.48) 25)) 0 1)) (not rand_mode))))

; ======================================================================
; one delay grain (DelayGrain + GrainEnvelope + VoiceStealingEnvelope)
; ======================================================================

; Envelope stage: 1 attack, 2 sustain, 3 release, 0 done (zeroed memory =
; idle). A new grain renders in the sample it starts. Returns (left right
; steal_rank fade_rank active): steal_rank is the remaining time (idle grains
; far below any playing one); fade_rank is the same but pushed past every
; grain once this grain has taken a safety-fade slot since the last new grain.
(defmacro beads-grain (k victim fade_target trig fade_hit fade_len run frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 n_dur n_ia n_is n_ir n_sm n_frac)
  (make-history h_di)
  (make-history h_df)
  (make-history h_stage)
  (make-history h_eph)
  (make-history h_vval)
  (make-history h_vdec)
  (make-history h_rem)
  (make-history h_faded)
  (def start (* trig (eq victim k)))
  (def speed (latch n_speed start))
  (def g00 (latch n_g00 start))
  (def g01 (latch n_g01 start))
  (def g10 (latch n_g10 start))
  (def g11 (latch n_g11 start))
  (def ia (latch n_ia start))
  (def is (latch n_is start))
  (def ir (latch n_ir start))
  (def sm (latch n_sm start))
  ; Start(): the state this sample renders from.
  (def di0 (gswitch start n_di (read-history h_di)))
  (def df0 (gswitch start n_df (read-history h_df)))
  (def st0 (gswitch start 1 (read-history h_stage)))
  (def eph0 (gswitch start (* n_ia n_frac) (read-history h_eph)))
  (def vval0 (gswitch start 1 (read-history h_vval)))
  (def vdec0 (gswitch start 0 (read-history h_vdec)))
  (def rem0 (gswitch start (floor n_dur) (read-history h_rem)))
  (def act (* run (not (or (eq st0 0) (lte vval0 0)))))
  ; FadeOut(): after a new grain, the grains next in line to be stolen fade
  ; out (idle ones take a slot as a no-op, as in the module's sorted list).
  (def fade_mine (* fade_hit (eq fade_target k)))
  (def faded (gswitch trig 0 (gswitch fade_mine 1 (read-history h_faded))))
  (write-history h_faded faded)
  (def fading (* fade_mine (gt rem0 fade_len) act))
  (def vdec1 (gswitch fading (/ 1 fade_len) vdec0))
  (def rem1 (gswitch fading (floor (* vval0 fade_len)) rem0))
  ; Render(): move the head (1024ths of a sample), stop at either end.
  (def nf (+ df0 (- (gswitch frozen 0 1024) speed)))
  (def carry (floor (/ nf 1024)))
  (def di1 (+ di0 carry))
  (def cross (or (lt di1 0) (gte di1 len)))
  (def di (gswitch cross 0 di1))
  (def df (gswitch cross 0 (- nf (* carry 1024))))
  (def st (gswitch cross 0 st0))
  (def d (gswitch (gte di (* len 0.5)) (- (- len di) (/ df 1024)) (+ di (/ df 1024))))
  (def head_fade (smooth-step (min (/ d zone) 1)))
  (def (sl sr) (ring-read wp len di (/ df 1024)))
  ; grain envelope
  (def in_attack (eq st 1))
  (def tt (gswitch in_attack eph0 (- 1 eph0)))
  (def base (gswitch in_attack tt (* tt tt tt tt)))
  (def ev (* (not (eq st 0)) (gswitch (eq st 2) 1 (+ base (* (- (smooth-step tt) base) sm)))))
  (def inc (selector st ia is ir))
  (def ph2 (+ eph0 inc))
  (def wrapst (gte ph2 1))
  (def eph_adv (gswitch wrapst (* (/ (- ph2 1) inc) (selector st is ir 0)) ph2))
  (def st_adv (gswitch wrapst (% (+ st 1) 4) st))
  (def vval_adv (- vval0 vdec1))
  (def done1 (or (eq st_adv 0) (lte vval_adv 0)))
  (def rem_adv (gswitch done1 0 (- rem1 1)))
  (write-history h_di (gswitch act di di0))
  (write-history h_df (gswitch act df df0))
  (write-history h_stage (gswitch act st_adv st0))
  (write-history h_eph (gswitch act eph_adv eph0))
  (write-history h_vval (gswitch act vval_adv vval0))
  (write-history h_vdec vdec1)
  (write-history h_rem (gswitch act rem_adv rem1))
  (def y (* act head_fade ev (max vval_adv 0)))
  (def idle (gswitch act done1 (or (eq st0 0) (lte vval0 0))))
  (def steal_rank (gswitch idle -1000000000 (gswitch act rem_adv rem1)))
  (tuple (* y (+ (* sl g00) (* sr g01)))
         (* y (+ (* sl g10) (* sr g11)))
         steal_rank
         (gswitch faded 1000000000 steal_rank)
         (not idle)))

; Lower remaining time wins; ties keep the lower index.
(defmacro argmin2 (ra ia rb ib)
  (def take_b (lt rb ra))
  (tuple (gswitch take_b rb ra) (gswitch take_b ib ia)))

; Four grains: returns (left right min_steal steal_index min_fade fade_index
; active_count).
(defmacro grain-quad (k0 victim fade_target trig fade_hit fade_len run frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 n_dur n_ia n_is n_ir n_sm n_frac)
  (def (l0 r0 s0 f0 a0) (beads-grain k0 victim fade_target trig fade_hit fade_len run frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 n_dur n_ia n_is n_ir n_sm n_frac))
  (def (l1 r1 s1 f1 a1) (beads-grain (+ k0 1) victim fade_target trig fade_hit fade_len run frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 n_dur n_ia n_is n_ir n_sm n_frac))
  (def (l2 r2 s2 f2 a2) (beads-grain (+ k0 2) victim fade_target trig fade_hit fade_len run frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 n_dur n_ia n_is n_ir n_sm n_frac))
  (def (l3 r3 s3 f3 a3) (beads-grain (+ k0 3) victim fade_target trig fade_hit fade_len run frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 n_dur n_ia n_is n_ir n_sm n_frac))
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
; delay mode: DelayController play heads and tap envelopes
; ======================================================================

; PlayHead::Triangle.
(defmacro pw-triangle (phase pw)
  (gswitch (lt phase pw) (/ phase pw) (- 1 (/ (- phase pw) (- 1 pw)))))

; stmlib::Interpolate over { 0, 0.001, 0.01, 0.5, 0.75, 0.999 } at shape * 5.
(defmacro pw-table (shape)
  (def x (* shape 5))
  (def i (floor x))
  (def a (selector (+ i 1) 0 0.001 0.01 0.5 0.75 0.999))
  (def b (selector (+ i 2) 0 0.001 0.01 0.5 0.75 0.999))
  (+ a (* (- b a) (- x i))))

; TapEnvelope::Render / PlayHead::Render (master) envelope; `square` is the
; flat top of the shape < 0.2 morph (1, or a 1% ramped square for heads).
(defmacro tap-shape (phase shape square)
  (gswitch (lt shape 0.2)
           (+ square (* (- (pw-triangle phase 0.001) square) shape 5))
           (pw-triangle phase (pw-table shape))))

; One TapEnvelope: returns its gain.
(defmacro tap-envelope (reset inc shape hold)
  (make-history h_ph)
  (def ph0 (gswitch reset 0 (read-history h_ph)))
  (def env (tap-shape ph0 shape 1))
  (def ph1 (+ ph0 inc))
  (write-history h_ph (gswitch hold ph0 (gswitch (gte ph1 1) (- ph1 1) ph1)))
  env)

; One master PlayHead with its slice quantizer (HysteresisQuantizer2).
; `go` = the frozen branch is running. Returns (delay gain slave_go s_start
; s_dur s_gain): the slave starts on the master's half-way point, reading
; the same newly chosen slice.
(defmacro play-head-master (init go started interval tap_ratio gain slice_smooth len min_delay shape)
  (make-history h_start)
  (make-history h_rem)
  (make-history h_ph)
  (make-history h_inc)
  (make-history h_gain)
  (make-history h_mid)
  (make-history h_qn)
  (make-history h_qh)
  (make-history h_q)
  (def rem0 (gswitch init 0 (read-history h_rem)))
  (def mid0 (gswitch init 0 (read-history h_mid)))
  (def done (eq rem0 0))
  (def half (* (not done) (eq rem0 mid0)))
  (def need (* go (or done half)))
  (def slice_dur (max (floor (* interval tap_ratio)) 1))
  (def num (max (floor (/ len slice_dur)) 1))
  (def qn0 (gswitch started (read-history h_qn) 2))
  (def qh0 (gswitch started (read-history h_qh) 0.05))
  (def reinit (* need (not (eq qn0 num))))
  (def qn (gswitch reinit num qn0))
  (def qh (gswitch reinit (* 0.01 num) qh0))
  (def q0 (gswitch reinit 0 (read-history h_q)))
  (def v (- (* slice_smooth qn) 0.5))
  (def qv (clip (floor (+ v (* (gswitch (gt v q0) -1 1) qh) 0.5)) 0 (- qn 1)))
  (def q (gswitch need qv q0))
  (write-history h_qn qn)
  (write-history h_qh qh)
  (write-history h_q q)
  (def s_start (+ (* slice_dur q) min_delay))
  (def go_master (* go done))
  (def start (gswitch go_master s_start (gswitch init 0 (read-history h_start))))
  (def rem (gswitch go_master slice_dur rem0))
  (def ph (gswitch go_master 0 (gswitch init 0 (read-history h_ph))))
  (def inc (gswitch go_master (/ 1 slice_dur) (gswitch init 0 (read-history h_inc))))
  (def g (gswitch go_master gain (gswitch init 0 (read-history h_gain))))
  (def on (gt rem 0))
  (def env (tap-shape ph shape (min (* 100 (pw-triangle ph 0.5)) 1)))
  (write-history h_start start)
  (write-history h_mid (gswitch go_master (floor (* slice_dur 0.5)) mid0))
  (write-history h_rem (gswitch (* go on) (- rem 1) rem))
  (write-history h_ph (gswitch (* go on) (+ ph inc) ph))
  (write-history h_inc inc)
  (write-history h_gain g)
  (tuple (* on (+ start rem)) (* on g env) (* go half) s_start slice_dur gain))

; Slave PlayHead: a triangle that only sounds for shapes around 0.6.
(defmacro play-head-slave (init go go_slave s_start s_dur s_gain shape)
  (make-history h_start)
  (make-history h_rem)
  (make-history h_ph)
  (make-history h_inc)
  (make-history h_gain)
  (def start (gswitch go_slave s_start (gswitch init 0 (read-history h_start))))
  (def rem (gswitch go_slave s_dur (gswitch init 0 (read-history h_rem))))
  (def ph (gswitch go_slave 0 (gswitch init 0 (read-history h_ph))))
  (def inc (gswitch go_slave (/ 1 s_dur) (gswitch init 0 (read-history h_inc))))
  (def g (gswitch go_slave s_gain (gswitch init 0 (read-history h_gain))))
  (def on (gt rem 0))
  (def x (* shape 5))
  (def xi (floor x))
  (def tri (pw-triangle ph 0.5))
  (def env (* tri (gswitch (eq xi 2) (- x xi) (gswitch (eq xi 3) (- 1 (- x xi)) 0))))
  (write-history h_start start)
  (write-history h_rem (gswitch (* go on) (- rem 1) rem))
  (write-history h_ph (gswitch (* go on) (+ ph inc) ph))
  (write-history h_inc inc)
  (write-history h_gain g)
  (tuple (* on (+ start rem)) (* on g env)))

; DelayController::RenderInternal (both clock modes), one sample. Returns the
; eight (delay gain) taps; delay 0 = tap off.
(defmacro delay-controller (run sync tap frozen started density_in time time_amt shape_base shape_amt u_pos u_shape len min_delay)
  ; the position and shape random walks (RandomizedParameter::value with a
  ; RandomWalkModulation at 0.1 Hz)
  (def slice_mod (* 1.005 (clip (+ time (* time_amt (random-walk 0.1 time_amt u_pos (not run)))) 0 1)))
  (def shape (* 0.995 (clip (+ shape_base (* shape_amt (random-walk 0.1 shape_amt u_shape (not run)))) 0 1)))
  ; density through a HysteresisFilter (0.0025)
  (make-history h_density)
  (def density_err (- density_in (read-history h_density)))
  (def density (gswitch (gt density_err 0.0025) (- density_in 0.0025)
                        (gswitch (lt density_err -0.0025) (+ density_in 0.0025) (read-history h_density))))
  (write-history h_density (gswitch run density (read-history h_density)))
  ; ---- DelayClock ----
  (make-history h_i0)
  (make-history h_i1)
  (make-history h_avg)
  (make-history h_fade)
  (make-history h_finc)
  (make-history h_sens)
  (make-history h_next)
  (make-history h_rot)
  (def quarter (* 0.25 samplerate))
  (def i0_0 (gswitch started (read-history h_i0) quarter))
  (def i1_0 (gswitch started (read-history h_i1) quarter))
  (def avg0 (gswitch started (read-history h_avg) quarter))
  (def fade0 (gswitch started (read-history h_fade) 1))
  (def finc0 (gswitch started (read-history h_finc) (/ 50 samplerate)))
  (def sens0 (gswitch started (read-history h_sens) 1))
  (def rot0 (read-history h_rot))
  ; synchronized: Process(false, tap)
  (def next1 (+ (read-history h_next) 1))
  (def fade_s (min (+ fade0 finc0) 1))
  (def jitter (* 0.001 samplerate))
  (def accept (* tap (lt next1 (* 4 samplerate))))
  (def big (gt (abs (- next1 i1_0)) jitter))
  (def avg_s (gswitch accept (gswitch big next1 (+ avg0 (* 0.1 (- next1 avg0)))) avg0))
  (def sens_s (gswitch accept (gswitch big 1 (max 0.05 (* sens0 0.95))) sens0))
  (def change (* accept sync))
  (def finc_new (clip (/ 2 avg_s) (/ 4 samplerate) (/ 50 samplerate)))
  ; unsynchronized: Update(new_interval, lp)
  (def new_interval (max min_delay (* len (beads-semi-ratio (* (abs (- density 0.5)) -216)))))
  (def lp_coef (clip (/ 10 new_interval) 0.005 0.7))
  (def i_u (+ i0_0 (* lp_coef (- new_interval i0_0))))
  (def i0 (gswitch sync (gswitch change i1_0 i0_0) i_u))
  (def i1 (gswitch sync (gswitch change avg_s i1_0) i_u))
  (def fade (gswitch sync (gswitch change 0 fade_s) 1))
  (def rot (gswitch sync (gswitch change (- 1 rot0) rot0) 0))
  (def next (gswitch sync (gswitch tap 0 next1) (read-history h_next)))
  (write-history h_i0 (gswitch run i0 i0_0))
  (write-history h_i1 (gswitch run i1 i1_0))
  (write-history h_avg (gswitch run (gswitch sync avg_s avg0) avg0))
  (write-history h_fade (gswitch run fade fade0))
  (write-history h_finc (gswitch run (gswitch change finc_new finc0) finc0))
  (write-history h_sens (gswitch run (gswitch sync sens_s sens0) sens0))
  (write-history h_next (gswitch run next (read-history h_next)))
  (write-history h_rot (gswitch run rot (read-history h_rot)))
  (def retrig (* (eq fade 0) (eq next 0)))
  (def new_idx (- 1 rot))
  ; interval(i) / gain(i) with the rotation
  (def int_a (gswitch rot i1 i0))
  (def int_b (gswitch rot i0 i1))
  (def gain_a (gswitch rot fade (- 1 fade)))
  (def gain_b (gswitch rot (- 1 fade) fade))
  ; ---- ratios and gains ----
  (def ratio (* density 20))
  (def ri (floor ratio))
  (def rf (clip (+ (* (- (- ratio ri) 0.5) (gswitch (lt density 0.5) 4 1.5)) 0.5) 0 1))
  (def flip (% ri 2))
  (def k_lo (selector (+ ri 1) 0.0625 0.0625 0.125 0.125 0.25 0.25 0.5 0.5 1 1 1 1 0.5 0.33333333 0.25 0.16666667 0.125 0.1 0.08333333 0.0625 0.0625 0.0625))
  (def k_hi (selector (+ ri 2) 0.0625 0.0625 0.125 0.125 0.25 0.25 0.5 0.5 1 1 1 1 0.5 0.33333333 0.25 0.16666667 0.125 0.1 0.08333333 0.0625 0.0625 0.0625))
  ; tap_modulation_: a 0.17 Hz sine, stepped only in the free-running clock
  (make-history h_tap_mod)
  (def tap_mod_phase (wrap (+ (read-history h_tap_mod) (* run (not sync) (/ 0.17 samplerate))) 0 1))
  (write-history h_tap_mod tap_mod_phase)
  (def odd_fade (* (clip (* (- density 0.5) 25) 0 1)
                   (+ 0.25 (* 0.75 (+ 0.5 (* 0.5 (sin (* twopi tap_mod_phase))))))))
  (def ratio0 (gswitch sync (gswitch flip k_hi k_lo) 1))
  (def ratio1 (gswitch sync (gswitch flip k_lo k_hi) 0.77))
  (def tgain0 (gswitch sync (gswitch flip rf (- 1 rf)) (- 1 (* odd_fade 0.2))))
  (def tgain1 (gswitch sync (gswitch flip (- 1 rf) rf) (* odd_fade 0.8)))
  ; ---- freeze transitions ----
  (make-history h_was_frozen)
  (def was_frozen (read-history h_was_frozen))
  (write-history h_was_frozen (gswitch run frozen (read-history h_was_frozen)))
  (def head_init_all (* run frozen (not was_frozen)))
  (def env_init_all (* run (not frozen) was_frozen))
  ; ---- frozen: play heads over slices of the buffer ----
  (make-history h_slice)
  (def slice_err (- slice_mod (read-history h_slice)))
  (def slice_smooth (gswitch frozen
                             (gswitch (gt slice_err 0.0025) (- slice_mod 0.0025)
                                      (gswitch (lt slice_err -0.0025) (+ slice_mod 0.0025) (read-history h_slice)))
                             (read-history h_slice)))
  (write-history h_slice (gswitch run slice_smooth (read-history h_slice)))
  (def go (* run frozen))
  (def init_a (or head_init_all (* run retrig (eq new_idx 0))))
  (def init_b (or head_init_all (* run retrig (eq new_idx 1))))
  (def (hd0 hg0 sg0 ss0 sd0 sga0) (play-head-master init_a go started int_a ratio0 (* tgain0 gain_a) slice_smooth len min_delay shape))
  (def (hd1 hg1) (play-head-slave init_a go sg0 ss0 sd0 sga0 shape))
  (def (hd2 hg2 sg1 ss1 sd1 sga1) (play-head-master init_a go started int_a ratio1 (* tgain1 gain_a) slice_smooth len min_delay shape))
  (def (hd3 hg3) (play-head-slave init_a go sg1 ss1 sd1 sga1 shape))
  (def (hd4 hg4 sg2 ss2 sd2 sga2) (play-head-master init_b go started int_b ratio0 (* tgain0 gain_b) slice_smooth len min_delay shape))
  (def (hd5 hg5) (play-head-slave init_b go sg2 ss2 sd2 sga2 shape))
  (def (hd6 hg6 sg3 ss3 sd3 sga3) (play-head-master init_b go started int_b ratio1 (* tgain1 gain_b) slice_smooth len min_delay shape))
  (def (hd7 hg7) (play-head-slave init_b go sg3 ss3 sd3 sga3 shape))
  ; ---- running: eight taps at multiples of the clock ----
  (make-history h_mult)
  (def mult_lp (gswitch frozen (read-history h_mult) (+ (read-history h_mult) (* 0.01 (- slice_mod (read-history h_mult))))))
  (write-history h_mult (gswitch run mult_lp (read-history h_mult)))
  (def mult (clip (+ (* mult_lp 15) 1) 1 16))
  (def mi (floor mult))
  (def mf (clip (+ 0.5 (* 8 (- (- mult mi) 0.5))) 0 1))
  (def odd (% mi 2))
  (def ta (gswitch odd 0 1))
  (def tb (gswitch odd 1 0))
  (def e_a (* run (not frozen) (or env_init_all (* retrig (eq new_idx 0)))))
  (def e_b (* run (not frozen) (or env_init_all (* retrig (eq new_idx 1)))))
  (def t0 (clip (* int_a ratio0 (+ mi ta)) min_delay len))
  (def t1 (clip (* int_a ratio0 (+ mi tb)) min_delay len))
  (def t2 (clip (* int_a ratio1 (+ mi ta)) min_delay len))
  (def t3 (clip (* int_a ratio1 (+ mi tb)) min_delay len))
  (def t4 (clip (* int_b ratio0 (+ mi ta)) min_delay len))
  (def t5 (clip (* int_b ratio0 (+ mi tb)) min_delay len))
  (def t6 (clip (* int_b ratio1 (+ mi ta)) min_delay len))
  (def t7 (clip (* int_b ratio1 (+ mi tb)) min_delay len))
  (def fa (gswitch ta mf (- 1 mf)))
  (def fb (gswitch tb mf (- 1 mf)))
  (def te0 (tap-envelope e_a (/ 1 t0) shape (or frozen (not run))))
  (def te1 (tap-envelope e_a (/ 1 t1) shape (or frozen (not run))))
  (def te2 (tap-envelope e_a (/ 1 t2) shape (or frozen (not run))))
  (def te3 (tap-envelope e_a (/ 1 t3) shape (or frozen (not run))))
  (def te4 (tap-envelope e_b (/ 1 t4) shape (or frozen (not run))))
  (def te5 (tap-envelope e_b (/ 1 t5) shape (or frozen (not run))))
  (def te6 (tap-envelope e_b (/ 1 t6) shape (or frozen (not run))))
  (def te7 (tap-envelope e_b (/ 1 t7) shape (or frozen (not run))))
  (tuple (gswitch frozen hd0 t0) (gswitch frozen hg0 (* tgain0 fa gain_a te0))
         (gswitch frozen hd1 t1) (gswitch frozen hg1 (* tgain0 fb gain_a te1))
         (gswitch frozen hd2 t2) (gswitch frozen hg2 (* tgain1 fa gain_a te2))
         (gswitch frozen hd3 t3) (gswitch frozen hg3 (* tgain1 fb gain_a te3))
         (gswitch frozen hd4 t4) (gswitch frozen hg4 (* tgain0 fa gain_b te4))
         (gswitch frozen hd5 t5) (gswitch frozen hg5 (* tgain0 fb gain_b te5))
         (gswitch frozen hd6 t6) (gswitch frozen hg6 (* tgain1 fa gain_b te6))
         (gswitch frozen hd7 t7) (gswitch frozen hg7 (* tgain1 fb gain_b te7))))

; Read a 1024ths-of-a-sample head position as (integral, fraction).
(defmacro fixed-read (wp len pi pf)
  (ring-read wp len pi (/ pf 1024)))

; One MultitapDelay tap pair: Tap<window> (pitch-shifting, two crossfaded
; heads) and Tap<1> (plain), crossfaded by how far `speed` is from 1.
(defmacro multitap (wp len delay gain pg speed run window)
  (make-history h_phase)
  (def on (* run (gt delay 0)))
  (def full (* window 1024))
  (def half_w (* window 512))
  (def p0 (read-history h_phase))
  (def p1 (+ p0 (trunc (* 1024 (- speed 1)))))
  (def ph (gswitch on (gswitch (gte p1 full) (- p1 full) (gswitch (lt p1 0) (+ p1 full) p1)) p0))
  (write-history h_phase ph)
  (def di (floor delay))
  (def df (floor (* (- delay di) 1024)))
  (def m (+ di (* window 0.5)))
  (def past (or (gt m window) (* (eq m window) (gte df 1))))
  (def dI (gswitch past m window))
  (def dF (gswitch past df 1))
  ; head 1 at D - phase, head 2 at D - ((phase + half) % full)
  (def p_i (floor (/ ph 1024)))
  (def p_f (- ph (* p_i 1024)))
  (def b1 (lt dF p_f))
  (def ph2 (gswitch (gte (+ ph half_w) full) (- (+ ph half_w) full) (+ ph half_w)))
  (def p2_i (floor (/ ph2 1024)))
  (def p2_f (- ph2 (* p2_i 1024)))
  (def b2 (lt dF p2_f))
  (def (h1l h1r) (fixed-read wp len (- dI p_i b1) (+ (- dF p_f) (* 1024 b1))))
  (def (h2l h2r) (fixed-read wp len (- dI p2_i b2) (+ (- dF p2_f) (* 1024 b2))))
  (def phase (/ ph full))
  (def tri (gswitch (lt phase 0.5) (* 2 phase) (- 2 (* 2 phase))))
  (def (nl nr) (fixed-read wp len di df))
  (def gp (* on gain pg))
  (def gn (* on gain (- 1 pg)))
  (tuple (+ (* gp (+ h2l (* (- h1l h2l) tri))) (* gn nl))
         (+ (* gp (+ h2r (* (- h1r h2r) tri))) (* gn nr))))

; ======================================================================
; mix stages
; ======================================================================

; Beads' Limiter: peak follower (attack 0.1 then 0.5 per sample at the media
; rate `k` x slower), release `decay` per host sample.
(defmacro beads-limiter (l r decay max_peak k)
  (make-history h_peak)
  (make-history h_lp)
  (def peak (min (max (abs l) (abs r)) 40))
  (def p0 (read-history h_peak))
  (def p1 (+ p0 (* (- peak p0) (gswitch (gt peak p0) (rate-coef 0.1 k) decay))))
  (def q0 (read-history h_lp))
  (def q1 (+ q0 (* (- p1 q0) (gswitch (gt p1 q0) (rate-coef 0.5 k) decay))))
  (write-history h_peak p1)
  (write-history h_lp q1)
  (def g (gswitch (lte q1 max_peak) 1 (/ max_peak q1)))
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
; samples. Returns 0..1, starting at its peak.
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

; FxEngine allpass: tail read of an L-sample line (L - 1 samples back), L in
; samples at the media rate (`k` host samples each).
(defmacro reverb-allpass (x len g k)
  (make-history h)
  (def r (delay (read-history h) (- (* len k) 2) @max-delay 16384))
  (def w (fx-store (+ x (* r g))))
  (write-history h w)
  (+ (* w (- g)) r))

; Griesinger/Dattorro reverb (beads/dsp/fx/reverb.h).
(defmacro beads-reverb (l r rv_mix rt klp k)
  (make-history h_del1)
  (make-history h_del2)
  (make-history h_lp1)
  (make-history h_lp2)
  (def input_gain (* 0.2 (- 1 (* 0.333 rt))))
  (def send_sqrt (* rv_mix (- 2 rv_mix)))
  (def send (* send_sqrt (- 2 send_sqrt) input_gain))
  (def rv_wet (/ 0.4 input_gain))
  (def rv_dry (* (- 1 rv_mix) (+ 1 rv_mix)))
  (def lp (rate-coef klp k))
  (def lfo1 (fx-lfo 0.5))
  (def lfo2 (fx-lfo 0.3))
  ; The first diffuser's in-loop smear (read at 10 + 80 lfo, rewrite at 100)
  ; makes its tail read come from 59 + 80 lfo samples back: a 60 + 80 lfo line.
  (def ap_a (reverb-allpass (* (+ l r) send) (+ 60 (* 80 lfo1)) 0.625 k))
  (def ap_b (reverb-allpass ap_a 214 0.625 k))
  (def ap_c (reverb-allpass ap_b 319 0.625 k))
  (def apout (reverb-allpass ap_c 527 0.625 k))
  ; loop half 1: apout + del2 tail -> lp -> 2 allpasses -> del1
  (def d2 (delay (read-history h_del2) (- (* (+ 6261 (* 50 lfo2)) k) 1) @max-delay 16384))
  (def lp1 (+ (read-history h_lp1) (* lp (- (+ apout (* d2 rt)) (read-history h_lp1)))))
  (write-history h_lp1 lp1)
  (def wet_l (reverb-allpass (reverb-allpass lp1 2182 -0.625 k) 2690 0.625 k))
  (def del1_in (fx-store wet_l))
  (write-history h_del1 del1_in)
  ; loop half 2: apout + del1 tail -> lp -> 2 allpasses -> del2
  (def d1 (delay del1_in (* (+ 4460 (* 40 lfo1)) k) @max-delay 16384))
  (def lp2 (+ (read-history h_lp2) (* lp (- (+ apout (* d1 rt)) (read-history h_lp2)))))
  (write-history h_lp2 lp2)
  (def wet_r (reverb-allpass (reverb-allpass lp2 2525 0.625 k) 2197 -0.625 k))
  (write-history h_del2 (fx-store wet_r))
  (tuple (+ (* l rv_dry) (* wet_l rv_wet))
         (+ (* r rv_dry) (* wet_r rv_wet))))

; WowAndFlutter (dirty tape): a 0.23 Hz wow whose rate and depth wander,
; plus a 1.99 Hz flutter, modulating a short delay; then a one-pole.
(defmacro wow-and-flutter (l r k lp u)
  (make-history h_rph)
  (make-history h_rf)
  (make-history h_wph)
  (make-history h_fph)
  (make-history h_lpl)
  (make-history h_lpr)
  (def rf0 (gswitch (eq (read-history h_rf) 0) (/ 0.125 samplerate) (read-history h_rf)))
  (def rph0 (read-history h_rph))
  (def rph1 (wrap (+ rph0 rf0) 0 1))
  (def crossed (* (gte rph1 0.25) (lt rph0 0.25)))
  (write-history h_rph rph1)
  (write-history h_rf (gswitch crossed (/ (+ 0.05 (* u 0.2)) samplerate) rf0))
  (def rm (+ 0.5 (* 0.5 (sin (* twopi rph1)))))
  (def fm (+ 0.5 (* 1.5 rm rm)))
  (def amount (- 1 (* 0.4 rm)))
  (def wph (wrap (+ (read-history h_wph) (* fm (/ 0.23 samplerate))) 0 1))
  (def fph (wrap (+ (read-history h_fph) (/ 1.99 samplerate)) 0 1))
  (write-history h_wph wph)
  (write-history h_fph fph)
  (def wow (+ 0.5 (* 0.5 (sin (* twopi wph)))))
  (def flutter (+ 0.5 (* 0.5 (sin (* twopi fph)))))
  (def d (* k (+ 1 (* (+ (* 105 wow) (* 17 flutter)) amount))))
  (def c (rate-coef lp k))
  (def ol (+ (read-history h_lpl) (* c (- (delay l d @max-delay 1024) (read-history h_lpl)))))
  (def orr (+ (read-history h_lpr) (* c (- (delay r d @max-delay 1024) (read-history h_lpr)))))
  (write-history h_lpl ol)
  (write-history h_lpr orr)
  (tuple ol orr))

; ======================================================================
; media
; ======================================================================

(make-history h_started)
(def started (read-history h_started))
(write-history h_started 1)

(def quality_i (clip (floor (+ quality 0.5)) 0 3))
(def qsel (+ quality_i 1))
(def hifi (eq quality_i 3))
(def dirty (eq quality_i 0))
; granular core rate, processor (codec) rate, buffer frames at the core rate
(def g_sr (selector qsel 24000 24000 32000 48000))
(def p_sr (selector qsel 24000 48000 32000 48000))
(def frames (selector qsel 384000 256000 256000 192000))
(def kg (/ samplerate g_sr))
(def kp (/ samplerate p_sr))
(def len (min (floor (* frames kg)) kCapacity))
(def safety (* 257 kg))
(def zone (* 256 kg))
(def min_delay (* 68 kp))
(def window (* 2 (floor (+ (* 1024 kg) 0.5))))
(def band_limit (lt g_sr (* samplerate 0.99)))
(def codec_fc (min (* 0.45 g_sr) (* 0.45 samplerate)))

; ======================================================================
; input: gain control and codec
; ======================================================================

(def in_gain (db-amp input_db))
(make-history h_in_peak)
(def raw_peak (max (abs in_l) (abs in_r)))
(def in_peak (+ (read-history h_in_peak) (* (- raw_peak (read-history h_in_peak)) (gswitch (gt raw_peak (read-history h_in_peak)) (/ 10 samplerate) (/ 0.5 samplerate)))))
(write-history h_in_peak in_peak)
; GainControl soft clipping: off for the digital media, on for dirty tape,
; level-dependent for clean tape.
(def soft_amt (selector qsel 1 (clip (* 2 (- (/ (* (min in_peak 0.9) in_gain) 0.9) 0.75)) 0 1) 0 0))
(defmacro gain-stage (x)
  (def g (* x in_gain))
  (def soft (- (* 0.95 (soft-clip (+ g 0.05))) 0.05))
  (def hard (clip g -1 1))
  (+ hard (* (- soft hard) soft_amt)))
(def x_l (gain-stage in_l))
(def x_r (gain-stage in_r))
(def band_l (gswitch band_limit (block-gate band_limit (codec-lp x_l codec_fc)) x_l))
(def band_r (gswitch band_limit (block-gate band_limit (codec-lp x_r codec_fc)) x_r))
; The codec runs at the media rate for dirty tape and vintage digital (the dry
; signal is band-limited and quantized too); clean tape resamples the wet path.
(def codec_l (gswitch (eq quality_i 1) x_l band_l))
(def codec_r (gswitch (eq quality_i 1) x_r band_r))
(def dry_l (selector qsel (block-gate dirty (mu-law codec_l)) codec_l (/ (trunc (* codec_l 4096)) 4096) codec_l))
(def dry_r (selector qsel (block-gate dirty (mu-law codec_r)) codec_r (/ (trunc (* codec_r 4096)) 4096) codec_r))

; ======================================================================
; seed, freeze, granular <-> delay mode
; ======================================================================

(def frozen (gt (clip (mod freeze) 0 1) 0.5))
(def latched (gt latch 0.5))
(def seed_held (gt (clip (mod seed) 0 1) 0.5))
(make-history h_seed)
(make-history h_gate)
(def seed_onset (* seed_held (not (read-history h_seed))))
(write-history h_seed seed_held)
(def gate (or latched seed_held))
(def gate_onset (or (* gate (not (read-history h_gate))) (* latched seed_onset)))
(write-history h_gate gate)

(def time_v (clip (mod time) 0 1))
(def size_v (clip (mod size) 0 1))
(def shape_v (clip (mod shape) 0 1))
(def pitch_v (mod pitch))
(def density_v (clip (mod density) 0 1))

; The module's control block (0.25 ms): the mode decision and the delay
; mode's speed / bandwidth interpolators step once per block.
(def block_size (max (floor (/ samplerate 4000)) 1))
(make-history h_bcount)
(def bcount (read-history h_bcount))
(def block_start (eq bcount 0))
(write-history h_bcount (% (+ bcount 1) block_size))

; Delay mode once SIZE passes 0.99 (back below 0.98), through a 1.5 s
; transition whose middle 1.5 s silences the wet signal.
(make-history h_transition)
(make-history h_delay_mode)
(def transition0 (gswitch started (read-history h_transition) -1.5))
(def delay_mode (gswitch block_start (gt transition0 0) (read-history h_delay_mode)))
(write-history h_delay_mode delay_mode)
(def transition (gswitch block_start
                         (clip (+ transition0 (* (gswitch (gt size_v (gswitch delay_mode 0.98 0.99)) 1 -1) (/ block_size samplerate))) -1.5 1.5)
                         transition0))
(write-history h_transition transition)
(def wet_fade (smooth-step (min 1 (/ (abs transition) 0.75))))
(def granular (not delay_mode))

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

; ======================================================================
; feedback and recording (GranularProcessor::ProcessInternal, DelayLine)
; ======================================================================

(def fb (smoothed (clip (mod feedback) 0 1)))
(def max_fb_gain (gswitch delay_mode 1.5 3))
(def soft_limit_gain (selector qsel 1.75 1.3 1.3 1))
(def fb_lp (rate-coef (selector qsel 0.75 0.9 0.9 0.999) kg))
(make-history h_prev_l)
(make-history h_prev_r)
(def prev_l (read-history h_prev_l))
(def prev_r (read-history h_prev_r))
(def fb_gain (gswitch hifi (* fb fb max_fb_gain) (* fb (/ max_fb_gain soft_limit_gain))))
(def (lim_in_l lim_in_r) (beads-limiter (+ band_l (* prev_l fb_gain)) (+ band_r (* prev_r fb_gain)) (/ 1 samplerate) 0.95 kg))
(def s_l (gswitch hifi lim_in_l (+ band_l (* fb (- (soft-clip (* lim_in_l soft_limit_gain)) band_l)))))
(def s_r (gswitch hifi lim_in_r (+ band_r (* fb (- (soft-clip (* lim_in_r soft_limit_gain)) band_r)))))

; FREEZE: record the first 256 frozen frames into the tail; after it, fade
; from the tail back into the live input.
(make-history h_was_frozen)
(make-history h_tail_wp)
(make-history h_tail_dur)
(def was_frozen (read-history h_was_frozen))
(write-history h_was_frozen frozen)
(def tail_wp0 (read-history h_tail_wp))
(def tw_start (gswitch (* frozen (not was_frozen)) 0 tail_wp0))
(def tail_rec (* frozen (lt tw_start 256)))
(def tail_slot (gswitch tail_rec tw_start 256))
(def tail_dur (gswitch (* was_frozen (gt tail_wp0 0)) tail_wp0 (read-history h_tail_dur)))
(write-history h_tail_dur tail_dur)
(def tail_on (* (not frozen) (gt tail_wp0 0)))
(def tail_pos (floor (max (- tail_dur tail_wp0) 0)))
(def tail_g (* tail_on (smooth-step (/ tail_wp0 (max tail_dur 1)))))
(def w_l (+ s_l (* (- (peek freeze_tail tail_pos 0) s_l) tail_g)))
(def w_r (+ s_r (* (- (peek freeze_tail tail_pos 1) s_r) tail_g)))
(write-history h_tail_wp (gswitch frozen (+ tw_start tail_rec) (- tail_wp0 tail_on)))

(make-history h_wp)
(def wp0 (min (read-history h_wp) (- len 1)))
(def store_l (store-q w_l qsel))
(def store_r (store-q w_r qsel))
(def slot (gswitch frozen kScratch wp0))
(def mirror (gswitch (* (not frozen) (lt wp0 4)) (+ wp0 len) kScratch))
(def wp1 (gswitch frozen wp0 (gswitch (eq wp0 0) (- len 1) (- wp0 1))))
(write-history h_wp wp1)
; Every read below indexes from `wp`, so it follows these writes.
(def wp (seq (poke freeze_tail tail_slot 0 s_l)
             (poke freeze_tail tail_slot 1 s_r)
             (poke ring slot 0 store_l)
             (poke ring slot 1 store_r)
             (poke ring mirror 0 store_l)
             (poke ring mirror 1 store_r)
             wp1))

; ======================================================================
; granular mode: trigger generator, scheduler, 32 grains
; ======================================================================

(def (clock_trig clock_frac regularity) (grain-clock gate gate_onset seed_onset latched density_v u0))
(def g_trig (* clock_trig granular))

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
(write-history h_interval (gswitch g_trig 1 (+ interval0 granular)))

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
(def g_semis (randomized-semis pitch_v pitch_amt u7 u8))

; ComputeGrainDuration: V-shaped around size = 1/3 (reversed below), 4 s down
; to 4 s / 512.
(def reverse (lt g_size 0.3333333))
(def shortness (min 1 (* 1.025 (gswitch reverse
                                         (* g_size 3 (- 2 (* g_size 3)))
                                         (- 1 (* 1.5 (- g_size 0.3333333)))))))
(def g_speed (* (gswitch reverse -1 1) (beads-semi-ratio g_semis)))
(def g_dur0 (* (beads-semi-ratio (* shortness -108)) 4 samplerate))

; DelayGrain::Start: keep the grain clear of the record head.
(def consumption (- g_speed (gswitch frozen 0 1)))
(def g_delay0 (- (+ (* g_time (- len (* 2 safety))) safety) (* clock_frac consumption)))
(def g_dur (gswitch (eq consumption 0) g_dur0 (min g_dur0 (/ len (abs consumption)))))
(def g_delay (gswitch (gt consumption 0) (max g_delay0 (* g_dur consumption))
                      (gswitch (lt consumption 0) (min g_delay0 (+ len (* g_dur consumption))) g_delay0)))
(def n_di (floor g_delay))
(def n_df (floor (* (- g_delay n_di) 1024)))
(def n_speed (trunc (* g_speed 1024)))

; GrainEnvelope::Start
(def env_sustain (max (- 1 (* 3 g_shape)) 0))
(def shape_s (* (- g_shape 0.33333333) 1.5))
(def env_attack (gswitch (lt shape_s 0) 0 (gswitch (lt shape_s 0.5) (* shape_s shape_s shape_s 4) shape_s)))
(def env_release (* (- 1 env_attack) (- 1 env_sustain)))
(def n_ia (/ 1 (max (* g_dur env_attack) (* 0.0005 samplerate (+ 1 (* 3 env_sustain))))))
(def n_is (/ 1 (max (* g_dur env_sustain) 2)))
(def n_ir (/ 1 (max (* g_dur env_release) (* 0.010 samplerate))))
(def n_sm (gswitch (lt env_attack 0.5) (* 2 env_attack) (- 2 (* 2 env_attack))))

; ComputeGrainAmplitudeAndPan, then DelayGrain's stereo pan matrix
; (destination x source, roughly equal power).
(def n_active (+ (read-history h_active) 1))
(def spread (min 1 (+ (abs time_amt) (abs size_amt) (abs shape_amt) (abs pitch_amt) (- 1 regularity))))
(def g_pan (+ 0.5 (* (- u9 0.5) (min (* 0.5 n_active spread u10) 1))))
(def g_amp (/ 1 (sqrt n_active)))
(def pan_left (lt g_pan 0.5))
(def pan_p (gswitch pan_left (* 2 g_pan) (* 2 (- 1 g_pan))))
(def pan_near (* pan_p (- 2 pan_p) g_amp))
(def pan_far (* (- 1 pan_p) (+ 1 pan_p) g_amp))
(def n_g00 (gswitch pan_left g_amp pan_near))
(def n_g01 (gswitch pan_left pan_far 0))
(def n_g10 (gswitch pan_left 0 pan_far))
(def n_g11 (gswitch pan_left pan_near g_amp))

(def (gl0 gr0 gs0 gsi0 gf0 gfi0 ga0) (grain-quad 0 victim fade_target g_trig fade_hit fade_len granular frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 g_dur n_ia n_is n_ir n_sm clock_frac))
(def (gl1 gr1 gs1 gsi1 gf1 gfi1 ga1) (grain-quad 4 victim fade_target g_trig fade_hit fade_len granular frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 g_dur n_ia n_is n_ir n_sm clock_frac))
(def (gl2 gr2 gs2 gsi2 gf2 gfi2 ga2) (grain-quad 8 victim fade_target g_trig fade_hit fade_len granular frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 g_dur n_ia n_is n_ir n_sm clock_frac))
(def (gl3 gr3 gs3 gsi3 gf3 gfi3 ga3) (grain-quad 12 victim fade_target g_trig fade_hit fade_len granular frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 g_dur n_ia n_is n_ir n_sm clock_frac))
(def (gl4 gr4 gs4 gsi4 gf4 gfi4 ga4) (grain-quad 16 victim fade_target g_trig fade_hit fade_len granular frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 g_dur n_ia n_is n_ir n_sm clock_frac))
(def (gl5 gr5 gs5 gsi5 gf5 gfi5 ga5) (grain-quad 20 victim fade_target g_trig fade_hit fade_len granular frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 g_dur n_ia n_is n_ir n_sm clock_frac))
(def (gl6 gr6 gs6 gsi6 gf6 gfi6 ga6) (grain-quad 24 victim fade_target g_trig fade_hit fade_len granular frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 g_dur n_ia n_is n_ir n_sm clock_frac))
(def (gl7 gr7 gs7 gsi7 gf7 gfi7 ga7) (grain-quad 28 victim fade_target g_trig fade_hit fade_len granular frozen wp len zone n_di n_df n_speed n_g00 n_g01 n_g10 n_g11 g_dur n_ia n_is n_ir n_sm clock_frac))

(write-history h_victim (argmin8 gs0 gsi0 gs1 gsi1 gs2 gsi2 gs3 gsi3 gs4 gsi4 gs5 gsi5 gs6 gsi6 gs7 gsi7))
(write-history h_fade_target (argmin8 gf0 gfi0 gf1 gfi1 gf2 gfi2 gf3 gfi3 gf4 gfi4 gf5 gfi5 gf6 gfi6 gf7 gfi7))
(write-history h_active (+ ga0 ga1 ga2 ga3 ga4 ga5 ga6 ga7))

(def grains_l (+ gl0 gl1 gl2 gl3 gl4 gl5 gl6 gl7))
(def grains_r (+ gr0 gr1 gr2 gr3 gr4 gr5 gr6 gr7))

; ======================================================================
; delay mode: DelayController and MultitapDelay
; The controller holds its state in granular mode. (Its predicate comes from
; the same per-sample loop, so it cannot sit in a block-gate.)
; ======================================================================


(def (td0 tg0 td1 tg1 td2 tg2 td3 tg3 td4 tg4 td5 tg5 td6 tg6 td7 tg7)
  (delay-controller delay_mode (not latched) (* seed_onset delay_mode) frozen started density_v time_v time_amt shape_v shape_amt u1 u2 len min_delay))

; Delay-mode transposition: PITCH plus a random-walk vibrato from its
; attenuverter.
; (ParameterInterpolator: per-block ramps that only move in delay mode, so
; the first delay block ramps from the module's initial 0.)
(defmacro block-ramp (target run)
  (make-history h_state)
  (make-history h_from)
  (make-history h_to)
  (def from (gswitch block_start (read-history h_state) (read-history h_from)))
  (def to (gswitch block_start target (read-history h_to)))
  (write-history h_from from)
  (write-history h_to to)
  (def v (+ from (* (- to from) (/ (+ bcount 1) block_size))))
  (write-history h_state (gswitch (* run (eq bcount (- block_size 1))) v (read-history h_state)))
  v)
(def tap_speed (block-ramp (beads-semi-ratio (+ pitch_v (* pitch_amt 24 (random-walk 0.2 pitch_amt u4 0)))) delay_mode))
(def pg (clip (* 100 (- (abs (- tap_speed 1)) 0.01)) 0 1))
(def (tl0 tr0) (multitap wp len td0 tg0 pg tap_speed delay_mode window))
(def (tl1 tr1) (multitap wp len td1 tg1 pg tap_speed delay_mode window))
(def (tl2 tr2) (multitap wp len td2 tg2 pg tap_speed delay_mode window))
(def (tl3 tr3) (multitap wp len td3 tg3 pg tap_speed delay_mode window))
(def (tl4 tr4) (multitap wp len td4 tg4 pg tap_speed delay_mode window))
(def (tl5 tr5) (multitap wp len td5 tg5 pg tap_speed delay_mode window))
(def (tl6 tr6) (multitap wp len td6 tg6 pg tap_speed delay_mode window))
(def (tl7 tr7) (multitap wp len td7 tg7 pg tap_speed delay_mode window))
(def taps_l (+ tl0 tl1 tl2 tl3 tl4 tl5 tl6 tl7))
(def taps_r (+ tr0 tr1 tr2 tr3 tr4 tr5 tr6 tr7))

; Bandwidth (SIZE's attenuverter): low-pass to the left, high-pass to the right.
(def bandwidth (block-ramp (clip (* 0.5 (+ 1 size_amt)) 0 1) delay_mode))
(def bw_amount (abs (- (* 2 bandwidth) 1)))
(def bw_octave (gswitch (lt bandwidth 0.5) (+ -7 (* bandwidth 10)) (+ -8 (* (- bandwidth 0.5) 10))))
(def bw_f (* 0.49 (beads-semi-ratio (* 12 bw_octave))))
(def bw_g (tan-fast (/ (* bw_f g_sr) samplerate)))
(def bw_r (/ 1 (+ 1 (* 16 bw_amount bw_f))))
(def bw_wet_sqrt (* bw_amount (- 2 bw_amount)))
(def bw_wet (* bw_wet_sqrt (- 2 bw_wet_sqrt)))
(def bw_hp (gte bandwidth 0.5))
(def filt_l (st-svf taps_l bw_g bw_r bw_hp granular))
(def filt_r (st-svf taps_r bw_g bw_r bw_hp granular))
(def delay_l (+ taps_l (* (- filt_l taps_l) bw_wet)))
(def delay_r (+ taps_r (* (- filt_r taps_r) bw_wet)))

; ======================================================================
; wet signal, feedback tap, mix
; ======================================================================

(def wet_raw_l (+ grains_l (* delay_mode delay_l)))
(def wet_raw_r (+ grains_r (* delay_mode delay_r)))
(def wet_l (gswitch band_limit (block-gate band_limit (codec-lp wet_raw_l codec_fc)) wet_raw_l))
(def wet_r (gswitch band_limit (block-gate band_limit (codec-lp wet_raw_r codec_fc)) wet_raw_r))

; previous_sample: 40 Hz DC blocker, then a resolution-dependent low-pass.
(def dc_g (tan-fast (/ 40 samplerate)))
(write-history h_prev_l (+ prev_l (* fb_lp (- (st-svf wet_l dc_g 2 1 0) prev_l))))
(write-history h_prev_r (+ prev_r (* fb_lp (- (st-svf wet_r dc_g 2 1 0) prev_r))))

(def dw (smoothed (clip (mod dry_wet) 0 1)))
(def dry_gain (* (- 1 dw) (+ 1 dw) 0.75))
(def wet_gain (* dw (- 2 dw) 1.5 wet_fade))
(def mixed_l (+ (* dry_l dry_gain) (* wet_l wet_gain)))
(def mixed_r (+ (* dry_r dry_gain) (* wet_r wet_gain)))

(def (wf_l wf_r) (block-gate dirty (wow-and-flutter mixed_l mixed_r kp 0.95 u5)))
(def tape_l (gswitch dirty wf_l mixed_l))
(def tape_r (gswitch dirty wf_r mixed_r))

(def (pre_l pre_r) (beads-limiter tape_l tape_r (/ 1 (* samplerate 4)) 0.9 kp))
(def rv (smoothed (clip (mod reverb) 0 1)))
(def caveman (not hifi))
(def rv_time (* rv rv rv))
(def (rev_l rev_r) (beads-reverb pre_l pre_r (* rv rv 0.9)
                                 (gswitch caveman (+ 0.45 (* 0.5 rv_time)) (+ 0.3 (* 0.6 rv_time)))
                                 (gswitch caveman 0.4 0.6)
                                 kp))
(def (lim_l lim_r) (beads-limiter rev_l rev_r (/ 1 (* samplerate 6)) 0.9 kp))

(def out_gain (db-amp (smoothed (clip (mod level_db) -24 12))))
(out (* lim_l out_gain) 1 @name left)
(out (* lim_r out_gain) 2 @name right)
