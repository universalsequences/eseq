; Factory 909 Open Hat — the identified TR-909 open hi-hat (HHOD0, decay 0),
; recovered by SynthID-style optimisation (dgen Examples/SynthID/scripts/
; fit_hat.py) rather than sampled: at the defaults below every hit reproduces
; the learned render; every knob is a departure from the identified sound.
;
; Voice: one noise stream -> a broad wash (two bandpasses + a highpass; the high
; part undulates slowly, the 909's swish) and a
; bank of twelve struck-once metal modes (the ROM hat's ring), each under the
; 909's open-hat VCA: a short attack ramp, a hold, then exponential decay ->
; gain-normalised tanh -> output highpass. A short lowpassed noise burst is the
; onset thump.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; ---- departures from the identified sound (all no-ops at their defaults) ----
(param tune @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive)
(param decay @default 1 @min 0.1 @max 6 @mod true @mod-mode additive)
(param metal @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param wash @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param bright @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param swish @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param drive @default 1 @min 0.2 @max 6 @mod true @mod-mode additive)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive)

; ---- the identified scalars (recovered_params.json), editable ----
(param fc1 @default __FC1__ @min 1500 @max 12000 @unit Hz)
(param q1 @default __Q1__ @min 0.3 @max 4)
(param fc2 @default __FC2__ @min 300 @max 16000 @unit Hz)
(param q2 @default __Q2__ @min 0.1 @max 4)
(param g2 @default __G2__ @min 0 @max 4)
(param hp_fc @default __HPFC__ @min 2000 @max 20000 @unit Hz)
(param g_hp @default __GHP__ @min 0 @max 4)
(param wash_amp @default __AB__ @min 0.05 @max 20)
(param atk @default __ATK__ @min 0.1 @max 10 @unit ms)
(param hold @default __HOLD__ @min 1 @max 80 @unit ms)
(param d_tail @default __DTAIL__ @min -60 @max -4)
(param a_fast @default __AFAST__ @min 0 @max 20)
(param d_fast @default __DFAST__ @min -200 @max -15)
(param d_mode @default __DMODE__ @min -60 @max -2)
(param sw_rate @default __SWRATE__ @min 3 @max 60 @unit Hz)
(param sw_amp @default __SWAMP__ @min 0 @max 2000)
(param click_amp @default __CLICKAMP__ @min 0 @max 5)
(param click_decay @default __CLICKDECAY__ @min -3000 @max -400)
(param click_fc @default __CLICKFC__ @min 150 @max 1500 @unit Hz)
(param out_hp @default __OUTHP__ @min 20 @max 2000 @unit Hz)
(param out_drive @default __DRIVE__ @min 0.5 @max 4)
(param out_gain @default __OUTGAIN__ @min 0.05 @max 2)
(param m1f @default __M1F__ @min 644.1 @max 650.5 @unit Hz)
(param m1d @default __M1D__ @min -80 @max -1)
(param m1g @default __M1G__ @min 0.001 @max 3)
(param m2f @default __M2F__ @min 1478.8 @max 1493.6 @unit Hz)
(param m2d @default __M2D__ @min -80 @max -1)
(param m2g @default __M2G__ @min 0.001 @max 3)
(param m3f @default __M3F__ @min 1540.7 @max 1556.1 @unit Hz)
(param m3d @default __M3D__ @min -80 @max -1)
(param m3g @default __M3G__ @min 0.001 @max 3)
(param m4f @default __M4F__ @min 3390.4 @max 3424.4 @unit Hz)
(param m4d @default __M4D__ @min -80 @max -1)
(param m4g @default __M4G__ @min 0.001 @max 3)
(param m5f @default __M5F__ @min 3921.2 @max 3960.5 @unit Hz)
(param m5d @default __M5D__ @min -80 @max -1)
(param m5g @default __M5G__ @min 0.001 @max 3)
(param m6f @default __M6F__ @min 4251.0 @max 4293.7 @unit Hz)
(param m6d @default __M6D__ @min -80 @max -1)
(param m6g @default __M6G__ @min 0.001 @max 3)
(param m7f @default __M7F__ @min 4998.1 @max 5048.2 @unit Hz)
(param m7d @default __M7D__ @min -80 @max -1)
(param m7g @default __M7G__ @min 0.001 @max 3)
(param m8f @default __M8F__ @min 5199.1 @max 5251.2 @unit Hz)
(param m8d @default __M8D__ @min -80 @max -1)
(param m8g @default __M8G__ @min 0.001 @max 3)
(param m9f @default __M9F__ @min 8270.1 @max 8353.1 @unit Hz)
(param m9d @default __M9D__ @min -80 @max -1)
(param m9g @default __M9G__ @min 0.001 @max 3)
(param m10f @default __M10F__ @min 9161.6 @max 9253.4 @unit Hz)
(param m10d @default __M10D__ @min -80 @max -1)
(param m10g @default __M10G__ @min 0.001 @max 3)
(param m11f @default __M11F__ @min 12490.2 @max 12615.5 @unit Hz)
(param m11d @default __M11D__ @min -80 @max -1)
(param m11g @default __M11G__ @min 0.001 @max 3)
(param m12f @default __M12F__ @min 13366.2 @max 13500.2 @unit Hz)
(param m12d @default __M12D__ @min -80 @max -1)
(param m12g @default __M12G__ @min 0.001 @max 3)

(defmacro semi (s) (pow 2 (/ s 12)))
; DGen's biquad hardcodes a 44.1 kHz frame in its coefficient math; scale the
; requested Hz so the filter lands on the physical frequency at any host rate.
(defmacro bq-hz (hz) (* hz (/ 44100.0 samplerate)))
; a struck-once metal mode: decaying sine, phase wrapped before the sine
(defmacro mode-sine (f d g) (* g (exp (* d t)) (sin (* 2.0 pi (- (* t f tn) (floor (* t f tn)))))))

(def tn (semi (mod tune)))
(def vel (clip velocity 0 1))
; time since the trigger: exactly 0 on the trigger sample, then n/samplerate.
; The history holds the integer sample count (exact in float32 up to 2^24) and
; t is one multiply, so no rounding accumulates: summing 1/samplerate sample by
; sample drifted 0.35 cycles on a 5 kHz mode by 200 ms.
(make-history count-h)
(def previous-count (read-history count-h))
(def n-samp (gswitch (gt trigger 0.5) 0.0 previous-count))
(write-history count-h (+ n-samp 1.0))
(def t (* n-samp (/ 1.0 samplerate)))

; sources: one noise stream
(def n (- (* (noise) 2.0) 1.0))
(def br (clip (mod bright) 0 4))
; the swish: the high wash undulates like beating cymbal partials — a slow
; lowpassed noise, exponentiated, is a positive dB-scale modulator
; two cascaded one-poles, not a biquad: at a few Hz a biquad's 1 - cos(w0) is
; one float32 ulp and its coefficients come out wrong by tens of percent
(def sw_k (- 1.0 (exp (* -2.0 pi sw_rate (/ 1.0 samplerate)))))
(make-history sw1-h)
(make-history sw2-h)
(def sw1 (+ (read-history sw1-h) (* sw_k (- n (read-history sw1-h)))))
(write-history sw1-h sw1)
(def sw2 (+ (read-history sw2-h) (* sw_k (- sw1 (read-history sw2-h)))))
(write-history sw2-h sw2)
(def sw_mod (exp (* sw_amp (clip (mod swish) 0 4) sw2)))
(def wash_src (+ (* sw_mod (+ (biquad n (bq-hz (* fc1 tn)) q1 1.0 2)
                              (* g_hp br (biquad n (bq-hz (* hp_fc tn)) 0.707 1.0 1))))
                 (* g2 (biquad n (bq-hz (* fc2 tn)) q2 1.0 2))))
(def modes (+ (mode-sine m1f m1d m1g) (mode-sine m2f m2d m2g) (mode-sine m3f m3d m3g) (mode-sine m4f m4d m4g) (mode-sine m5f m5d m5g) (mode-sine m6f m6d m6g) (mode-sine m7f m7d m7g) (mode-sine m8f m8d m8g) (mode-sine m9f m9d m9g) (mode-sine m10f m10d m10g) (mode-sine m11f m11d m11g) (mode-sine m12f m12d m12g)))

; the open-hat VCA: attack ramp, hold, exponential decay
(def dk (/ 1.0 (clip (mod decay) 0.1 6)))
(def ramp (clip (/ t (* atk 0.001)) 0 1))
(def th (gswitch (lt t (* hold 0.001)) 0.0 (- t (* hold 0.001))))
(def env_wash (* ramp (+ (exp (* d_tail dk th)) (* a_fast (exp (* d_fast t))))))
(def env_mode (* ramp (exp (* d_mode dk th))))

; onset thump: a short lowpassed noise burst
(def click (* click_amp (exp (* click_decay t)) (biquad n (bq-hz click_fc) 0.707 1.0 0)))
(def x (+ (* wash_amp (clip (mod wash) 0 4) wash_src env_wash)
          (* (clip (mod metal) 0 4) modes env_mode)
          click))
(def drv (* out_drive (clip (mod drive) 0.2 6)))
(def shaped (* (/ (tanh (* x drv)) drv) out_gain))
(def voice (biquad shaped (bq-hz out_hp) 0.707 1.0 1))
(out (* voice vel (clip (mod level) 0 1.5)) 1 @name audio)
