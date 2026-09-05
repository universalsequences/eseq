; R8 Kick 03 — acoustic modal reconstruction, not sample playback.
; Source: Roland R8 / Kick03.wav, 44.1 kHz, 8032 frames.
; SHA256: 26eb639f7d6587382cdc95d297626bef8fac396f5b5ff5fc8892fb805042278e
; Identification: dgen Examples/SynthID/scripts/fit_r8_acoustic.py.
; Seventeen freely phased modes + six bands of decaying contact noise.
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

; BODY: weight is the lowest mode; head is the other membrane modes.
; Damping preferentially shortens upper modes; stretch changes their spacing.
(param tune @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive)
(param weight @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param head @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param decay @default 1 @min 0.2 @max 4 @mod true @mod-mode additive)
(param damp @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param stretch @default 0 @min -0.3 @max 0.3 @mod true @mod-mode additive)

; MOTION: finite ROM length is independent of the membrane's natural decay.
; Dynamics changes brightness with velocity, not random tuning/level drift.
(param bend @default 1 @min 0 @max 2 @mod true @mod-mode additive)
(param bend_time @default 1 @min 0.25 @max 3 @mod true @mod-mode additive)
(param attack @default 1 @min 0.25 @max 20 @mod true @mod-mode additive)
(param length @default __LENGTHMS__ @min 40 @max 1200 @unit ms @mod true @mod-mode additive)
(param punch @default 0 @min 0 @max 1 @mod true @mod-mode additive)
(param dynamics @default 0.5 @min 0 @max 1 @mod true @mod-mode additive)

; CONTACT: independent shell and beater. Hardness tilts/retunes contact
; texture; contact is its duration, rather than another copy of volume.
(param knock @default 1 @min 0 @max 3 @mod true @mod-mode additive)
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
(write-history count-h (min (+ count 1.0) (* samplerate 4.0)))
(def t (* count dt))
(make-history active-h)
(def active (gswitch hit 1.0 (read-history active-h)))
(write-history active-h active)
(make-history velocity-h)
(def vel (gswitch hit (clip velocity 0 1) (read-history velocity-h)))
(write-history velocity-h vel)

(def tuning (smooth (clip (mod tune) -24 24)))
(def pitch-ratio (* (/ (max pitch 1.0) 261.63) (semi tuning)))
(def weight-v (smooth (clip (mod weight) 0 2)))
(def head-v (smooth (clip (mod head) 0 2)))
(def decay-v (smooth (clip (mod decay) 0.2 4)))
(def damp-v (smooth (clip (mod damp) 0 1)))
(def stretch-v (smooth (clip (mod stretch) -0.3 0.3)))
(def bend-v (smooth (clip (mod bend) 0 2)))
(def bend-time-v (smooth (clip (mod bend_time) 0.25 3)))
(def attack-v (smooth (clip (mod attack) 0.25 20)))
(def length-v (* 0.001 (smooth (clip (mod length) 40 1200))))
(def punch-v (smooth (clip (mod punch) 0 1)))
(def dynamics-v (smooth (clip (mod dynamics) 0 1)))
(def knock-v (smooth (clip (mod knock) 0 3)))
(def shell-ratio (* (pow pitch-ratio (smooth (clip (mod track) 0 1)))
                   (semi (smooth (clip (mod shell_tune) -12 12)))))
(def ring-v (smooth (clip (mod ring) 0.2 4)))
(def beater-v (smooth (clip (mod beater) 0 3)))
(def hard-v (smooth (clip (mod hardness) -1 1)))
(def contact-v (smooth (clip (mod contact) 0.2 4)))
(def air-v (smooth (clip (mod air) 0 3)))
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
(def attack-env (one-minus-exp (/ t (* __ATTACKTIME__ attack-v))))
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
  (def damping (+ (/ d decay-v) (* damp-v 65.0 (pow (/ f __F1__) 0.7))))
  (* a (+ (* fundamental weight-v) (* (- 1.0 fundamental) head-v)) (exp (* (- damping) t))
     (one-minus-exp (/ t (* rise attack-v)))
     (oscillator inst-freq p phase-step)))
(def low
  (+ (membrane __F1__ __A1__ __D1__ __P1__ __G1__ __S1__ __R1__ 1)
     (membrane __F2__ __A2__ __D2__ __P2__ __G2__ __S2__ __R2__ 0)
     (membrane __F3__ __A3__ __D3__ __P3__ __G3__ __S3__ __R3__ 0)
     (membrane __F4__ __A4__ __D4__ __P4__ __G4__ __S4__ __R4__ 0)
     (membrane __F5__ __A5__ __D5__ __P5__ __G5__ __S5__ __R5__ 0)
     (membrane __F6__ __A6__ __D6__ __P6__ __G6__ __S6__ __R6__ 0)
     (membrane __F7__ __A7__ __D7__ __P7__ __G7__ __S7__ __R7__ 0)))
(defmacro shell-mode (f a d p)
  (def freq (* f shell-ratio))
  (* a knock-v (pow brightness 0.5) (exp (* (- (/ d ring-v)) t)) attack-env
     (oscillator freq p (* freq dt))))
(def shell
  (+ (shell-mode __F8__ __A8__ __D8__ __P8__)
     (shell-mode __F9__ __A9__ __D9__ __P9__)
     (shell-mode __F10__ __A10__ __D10__ __P10__)
     (shell-mode __F11__ __A11__ __D11__ __P11__)
     (shell-mode __F12__ __A12__ __D12__ __P12__)
     (shell-mode __F13__ __A13__ __D13__ __P13__)
     (shell-mode __F14__ __A14__ __D14__ __P14__)
     (shell-mode __F15__ __A15__ __D15__ __P15__)
     (shell-mode __F16__ __A16__ __D16__ __P16__)
     (shell-mode __F17__ __A17__ __D17__ __P17__)))

; Contact friction follows negative membrane pressure. It modulates only
; the struck noise, not the independent quiet decay, and adds no feedback.
(def pressure (+ 1.0 (* __PRESSURE__ 4.0 (max (- (+ low shell)) 0.0))))
(def white (- (* (noise) 2.0) 1.0))
(defmacro contact-band (fc q mode a d rise tail tail-d tilt)
  (def filtered (biquad white (bq-hz (* fc (pow shell-ratio 0.5) (pow brightness 0.4))) q 1.0 mode))
  (def onset (one-minus-exp (/ t (* rise attack-v contact-v))))
  (def fast (* beater-v a pressure (exp (* (- (/ d contact-v)) t))))
  (def slow (* air-v tail (exp (* (- (/ tail-d ring-v)) t))))
  (* filtered onset (+ fast slow) (pow brightness tilt)))
(def texture
  (+ (contact-band 350.0 0.9 2 __N1__ __ND1__ __NR1__ __NT1__ __NTD1__ -0.5)
     (contact-band 700.0 0.9 2 __N2__ __ND2__ __NR2__ __NT2__ __NTD2__ 0.0)
     (contact-band 1400.0 1.2 2 __N3__ __ND3__ __NR3__ __NT3__ __NTD3__ 0.5)
     (contact-band 2600.0 2.0 2 __N4__ __ND4__ __NR4__ __NT4__ __NTD4__ 1.0)
     (contact-band 4200.0 1.5 2 __N5__ __ND5__ __NR5__ __NT5__ __NTD5__ 1.5)
     (contact-band 6500.0 0.707 1 __N6__ __ND6__ __NR6__ __NT6__ __NTD6__ 2.0)))

; Cubic ROM fade: value and slope reach zero together. Length stretches the
; truncation boundary only; DECAY and RING remain independent damping controls.
(def fade-start (* length-v (/ __FADESTART__ __FADEEND__)))
(def u (clip (/ (- t fade-start) (max 0.001 (- length-v fade-start))) 0 1))
(def fade (- 1.0 (* u u (- 3.0 (* 2.0 u)))))
(def mixed (* (+ low shell texture) (+ 1.0 (* punch-v 1.5 (exp (* -65.0 t))))))
; Bipolar tone: dark blends in a lowpass; bright adds its complementary
; high band. Exact dry path at zero, no hidden coloration in the reference.
(def dark (biquad mixed (bq-hz 1400.0) 0.707 1.0 0))
(def toned (+ mixed (* tone-v (- mixed dark))))
(def gain (+ 1.0 (* drive-v 8.0)))
(def saturated (/ (tanh (* gain toned)) (tanh gain)))
(def driven (+ toned (* drive-v (- saturated toned))))
(def steps (pow 2.0 (- 16.0 (* crush-v 10.0))))
(def quantized (/ (floor (+ (* driven steps) 0.5)) steps))
(def colored (+ driven (* crush-v (- quantized driven))))
(out (* colored fade active vel level-v __OUTGAIN__) 1 @name audio)
