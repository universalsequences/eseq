; R8 Kick 03 — acoustic modal reconstruction, not sample playback.
; Source: Roland R8 / Kick03.wav, 44.1 kHz, 8032 frames.
; SHA256: 26eb639f7d6587382cdc95d297626bef8fac396f5b5ff5fc8892fb805042278e
; Identification: dgen Examples/SynthID/scripts/fit_r8_contact.py.
; Seven low membrane modes, a finite pressure-force pulse, and six broad
; contact/shell noise bands. NO stationary shell sine bank or comb feedback.
; Identified coefficients below are ordinary scalars (Hz, amplitude, 1/s,
; phase in cycles, dimensionless tension depths), not waveform/residual data.
; Musical controls are neutral at default, at C4 / velocity 1.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; BODY: squared level ranges expose strong weight/head balance changes.
; DAMP damps the whole drum, including shell and contact, not just its bass.
(param tune @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive)
(param weight @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param head @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param decay @default 1 @min 0.2 @max 4 @mod true @mod-mode additive)
(param damp @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param stretch @default 0 @min -0.3 @max 0.3 @mod true @mod-mode additive)

; MOTION: LENGTH is the base cut; DECAY/RING/CONTACT stretch their own
; layer's damping AND cut. A fixed global ROM cut must not mask their ranges.
; PUNCH speeds head energy transfer as well as increasing early impact.
(param bend @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param bend_time @default 1 @min 0.25 @max 3 @mod true @mod-mode additive)
(param attack @default 1 @min 0.25 @max 20 @mod true @mod-mode additive)
(param length @default __LENGTHMS__ @min 40 @max 1200 @unit ms @mod true @mod-mode additive)
(param punch @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param dynamics @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)

; CONTACT: KNOCK is the finite pressure pulse, BEATER is struck abrasion.
; HARDNESS changes contact duration, exciter bandwidth and modal weighting.
; RING extends diffuse shell decay, never a bank of clean fixed oscillators.
(param knock @default __FORCEAMP__ @min 0 @max 3 @mod true @mod-mode additive)
(param shell_tune @default 0 @min -12 @max 12 @unit st @mod true @mod-mode additive)
(param ring @default 1 @min 0.2 @max 4 @mod true @mod-mode additive)
(param beater @default 1 @min 0 @max 3 @mod true @mod-mode additive)
(param hardness @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param contact @default 1 @min 0.2 @max 4 @mod true @mod-mode additive)

; COLOR: track lets the shell/texture follow played notes or remain fixed.
; Drive/crush/tone are dry at zero; none disguises fit errors at default.
(param air @default 1 @min 0 @max 3 @mod true @mod-mode additive)
(param track @default 1 @min 0 @max 1 @mod true @mod-mode additive)
(param drive @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param tone @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param crush @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive)

(defmacro wrap (p) (- p (floor p)))
(defmacro semi (st) (pow 2.0 (/ st 12.0)))
(defmacro bq-hz (hz) (* (clip hz 20.0 (* samplerate 0.45)) (/ 44100.0 samplerate)))
; Control smoothing has an initialized state, so the first hit is exact.
(defmacro smooth (x)
  (make-history h)
  (make-history init-h)
  (def k (exp (/ -1.0 (* 0.003 samplerate))))
  (def y (gswitch (read-history init-h) (+ x (* k (- (read-history h) x))) x))
  (write-history h y)
  (write-history init-h 1.0)
  y)
(def dt (/ 1.0 samplerate))
(def hit (gt trigger 0.5))
(make-history count-h)
(def count (gswitch hit 0.0 (read-history count-h)))
; Count is exact up to the longest voice; stop advancing once all envelopes
; have ended. This avoids float32 integer overflow during long idle periods.
(write-history count-h (min (+ count 1.0) (* samplerate 8.0)))
(def t (* count dt))
(make-history active-h)
(def active (gswitch hit 1.0 (read-history active-h)))
(write-history active-h active)
(make-history velocity-h)
(def vel (gswitch hit (clip velocity 0 1) (read-history velocity-h)))
(write-history velocity-h vel)

(def tuning (smooth (clip (mod tune) -24 24)))
(def pitch-ratio (* (/ (max pitch 1.0) 261.63) (semi tuning)))
(def weight-v (pow (smooth (clip (mod weight) 0 2)) 2.0))
(def head-v (pow (smooth (clip (mod head) 0 2)) 2.0))
(def decay-v (smooth (clip (mod decay) 0.2 4)))
(def damp-v (smooth (clip (mod damp) 0 1)))
(def stretch-v (smooth (clip (mod stretch) -0.3 0.3)))
(def bend-v (pow (smooth (clip (mod bend) 0 2)) 2.0))
(def bend-time-v (smooth (clip (mod bend_time) 0.25 3)))
(def attack-v (smooth (clip (mod attack) 0.25 20)))
(def length-v (* 0.001 (smooth (clip (mod length) 40 1200))))
(def punch-v (smooth (clip (mod punch) 0 1)))
(def dynamics-v (smooth (clip (mod dynamics) 0 1)))
(def knock-v (smooth (clip (mod knock) 0 3)))
(def shell-ratio (* (pow pitch-ratio (smooth (clip (mod track) 0 1)))
                   (semi (smooth (clip (mod shell_tune) -12 12)))))
(def ring-v (smooth (clip (mod ring) 0.2 4)))
(def beater-v (pow (smooth (clip (mod beater) 0 3)) 1.5))
(def hard-v (smooth (clip (mod hardness) -1 1)))
(def contact-v (smooth (clip (mod contact) 0.2 4)))
(def air-v (pow (smooth (clip (mod air) 0 3)) 2.0))
(def drive-v (smooth (clip (mod drive) 0 1)))
(def tone-v (smooth (clip (mod tone) -1 1)))
(def crush-v (smooth (clip (mod crush) 0 1)))
(def level-v (smooth (clip (mod level) 0 1.5)))
(def touch (- hard-v (* dynamics-v (- 1.0 vel))))
(def brightness (pow 2.0 touch))
(def fast-time (* __FASTTIME__ bend-time-v))
(def slow-time (* __SLOWTIME__ bend-time-v))

; Stable 1-exp(-x). Direct subtraction loses precision at slow rates/high
; sample rates; the small-x series has error below float32 rounding here.
(defmacro one-minus-exp (x)
  (gswitch (lt x 0.02)
    (* x (+ 1.0 (* x (+ -0.5 (* x (+ 0.1666666667 (* x -0.0416666667)))))))
    (- 1.0 (exp (- x)))))
; Exact integral over this sample interval. Wrapped phase history allows
; live pitch/bend changes without multiplying an entire elapsed phase.
(def fast-step (* fast-time (one-minus-exp (/ dt fast-time)) (exp (/ (- t) fast-time))))
(def slow-step (* slow-time (one-minus-exp (/ dt slow-time)) (exp (/ (- t) slow-time))))
; Per-layer cubic cuts retain the reference at neutral controls but permit
; an actual longer body/shell/contact when its decay control is increased.
(defmacro layer-cut (scale)
  (def end (* length-v scale))
  (def start (* end (/ __FADESTART__ __FADEEND__)))
  (def u (clip (/ (- t start) (max 0.0001 (- end start))) 0 1))
  (- 1.0 (* u u (- 3.0 (* 2.0 u)))))
(def body-cut (layer-cut decay-v))
(def shell-cut (layer-cut ring-v))
(def contact-cut (layer-cut contact-v))
; Tone-filter memory may ring briefly after the last layer ends. Give that
; causal response its own 20 ms smooth exit, without applying the ROM fade
; twice to the source or abruptly chopping a live filter state.
(def last-end (* length-v (max decay-v (max ring-v contact-v))))
(def exit-u (clip (/ (- t last-end) 0.020) 0 1))
(def final-cut (- 1.0 (* exit-u exit-u (- 3.0 (* 2.0 exit-u)))))
(defmacro oscillator (frequency phase0 delta)
  (make-history phase-h)
  (def phase (gswitch hit (wrap phase0) (read-history phase-h)))
  (write-history phase-h (wrap (+ phase delta)))
  ; Smoothly suppress partials approaching Nyquist at extreme notes.
  (* (sin (* 2.0 pi phase))
     (clip (/ (- (* samplerate 0.49) frequency) (* samplerate 0.04)) 0 1)))
; Independent modal rise times model the transfer into the low head modes;
; the beater does not excite every resonance with one identical envelope.
(defmacro membrane (f a d p g s rise fundamental)
  (def spacing (pow (/ f __F1__) stretch-v))
  (def freq (* f pitch-ratio spacing))
  (def phase-step (* freq (+ dt (* bend-v (+ (* g fast-step) (* s slow-step))))))
  (def inst-freq (* freq (+ 1.0 (* bend-v (+ (* g (exp (/ (- t) fast-time))) (* s (exp (/ (- t) slow-time))))))))
  (def damping (+ (/ d decay-v) (* damp-v 140.0 (pow (/ f __F1__) 0.7))))
  (def impact-rise (/ (* rise attack-v) (+ 1.0 (* punch-v 15.0))))
  (* a (+ (* fundamental weight-v) (* (- 1.0 fundamental) head-v))
     (pow (/ f __F1__) (* touch 0.65)) (exp (* (- damping) t))
     (one-minus-exp (/ t impact-rise))
     (oscillator inst-freq p phase-step)))
(def low
  (+ (membrane __F1__ __A1__ __D1__ __P1__ __G1__ __S1__ __R1__ 1)
     (membrane __F2__ __A2__ __D2__ __P2__ __G2__ __S2__ __R2__ 0)
     (membrane __F3__ __A3__ __D3__ __P3__ __G3__ __S3__ __R3__ 0)
     (membrane __F4__ __A4__ __D4__ __P4__ __G4__ __S4__ __R4__ 0)
     (membrane __F5__ __A5__ __D5__ __P5__ __G5__ __S5__ __R5__ 0)
     (membrane __F6__ __A6__ __D6__ __P6__ __G6__ __S6__ __R6__ 0)
     (membrane __F7__ __A7__ __D7__ __P7__ __G7__ __S7__ __R7__ 0)))
; A single finite half-sine contact force, not a decaying oscillator.
; The filters have Q=.707: their short impulse response is a knock, not a
; sustaining pitched shell. Hardness changes the actual contact duration.
(def contact-speed (pow 2.0 (* -2.5 touch)))
(def force-width (max (* 4.0 dt) (* __FORCEWIDTH__ contact-v attack-v contact-speed)))
(def force-phase (clip (/ t force-width) 0.0 1.0))
(def force (* (sin (* pi force-phase)) (lt t force-width)
              (sqrt (/ 0.0012 force-width))))
(def force-hp (biquad force (bq-hz (* 180.0 shell-ratio)) 0.707 1.0 1))
(def force-lp (biquad force-hp (bq-hz (* 3500.0 shell-ratio brightness)) 0.707 1.0 0))
(def knock-voice (* knock-v force-lp contact-cut))

; Finite-bandwidth noise with constant per-Hz density across host rates.
(def white (* (- (* (noise) 2.0) 1.0) (sqrt (/ samplerate 48000.0))))
; Felt removes high-frequency excitation; it does not retune the shell.
; Keeping the band centers independent of hardness also avoids pushing the
; hardest strike's energy above the useful audible range.
(def exciter-cutoff (* 14000.0 (pow 2.0 (* 3.0 (min touch 0.0)))))
(def exciter (biquad white (bq-hz exciter-cutoff) 0.707 1.0 0))
(defmacro contact-band (fc q mode a d rise tail tail-d tilt)
  (def filtered (biquad exciter (bq-hz (* fc (pow shell-ratio 0.5))) q 1.0 mode))
  (def onset (one-minus-exp (/ t (* rise attack-v contact-v contact-speed))))
  (def damping (* damp-v 180.0 (sqrt (/ fc 350.0))))
  (def fast (* contact-cut beater-v a
     (exp (* (- (+ (/ d (* contact-v contact-speed)) damping)) t))))
  (def slow (* shell-cut air-v tail
     (exp (* (- (+ (/ tail-d ring-v) damping)) t))))
  (* filtered onset (+ fast slow) (pow brightness tilt)))
(def texture
  (+ (contact-band 350.0 0.65 2 __N1__ __ND1__ __NR1__ __NT1__ __NTD1__ -0.5)
     (contact-band 700.0 0.75 2 __N2__ __ND2__ __NR2__ __NT2__ __NTD2__ 0.0)
     (contact-band 1400.0 0.85 2 __N3__ __ND3__ __NR3__ __NT3__ __NTD3__ 0.5)
     (contact-band 2600.0 1.0 2 __N4__ __ND4__ __NR4__ __NT4__ __NTD4__ 1.0)
     (contact-band 4200.0 0.8 2 __N5__ __ND5__ __NR5__ __NT5__ __NTD5__ 1.5)
     (contact-band 6500.0 0.707 1 __N6__ __ND6__ __NR6__ __NT6__ __NTD6__ 2.0)))

(def mixed (+ (* low body-cut (+ 1.0 (* punch-v 0.75 (exp (* -65.0 t))))) knock-voice texture))
; A genuine bass/attack tilt around 420 Hz. Gain normalization is analytic,
; not a target-derived EQ or a level follower; neutral tone is exactly dry.
(def dark (biquad mixed (bq-hz 420.0) 0.707 1.0 0))
(def bass-gain (pow 2.0 (* -2.0 tone-v)))
(def treble-gain (pow 2.0 (* 2.0 tone-v)))
(def tilt-norm (sqrt (* 0.5 (+ (* bass-gain bass-gain) (* treble-gain treble-gain)))))
(def toned (/ (+ (* dark bass-gain) (* (- mixed dark) treble-gain)) tilt-norm))
(def gain (+ 1.0 (* drive-v 24.0)))
(def saturated (/ (tanh (* gain toned)) (tanh gain)))
(def driven (+ toned (* drive-v (- saturated toned))))
(def steps (pow 2.0 (- 16.0 (* crush-v 12.0))))
(def quantized (/ (floor (+ (* driven steps) 0.5)) steps))
(def colored (+ driven (* crush-v (- quantized driven))))
(out (* colored final-cut active vel level-v __OUTGAIN__) 1 @name audio)
