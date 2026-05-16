; Acoustic guitar — digital waveguide string model
; Based on a reference patch with proper one-pole LP compensation and body resonances.
;
; Key differences from basic Karplus-Strong:
; - stretch is computed to give exact T60 decay, compensating for the LP's attenuation
;   at the fundamental: stretch = pow(0.001, 1/(sustain*freq)) / |H(e^j2πf/sr)|
; - LP group delay is subtracted from the nominal delay length to keep pitch accurate
; - Body resonances (bandpass filters at guitar body modes) add acoustic realism
; - Pluck position comb on the exciter (bridge vs neck tonal character)

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))
(def mod5     (in 9  @name mod5 @modulator 5))
(def mod6     (in 10 @name mod6 @modulator 6))
(def ext1 (in 11 @name ext1 @modulator 7))
(def ext2 (in 12 @name ext2 @modulator 8))
(def ext3 (in 13 @name ext3 @modulator 9))
(def ext4 (in 14 @name ext4 @modulator 10))

(param hardness    @default 0.5  @min 0    @max 1    @mod true @mod-mode additive)
  ; 0=soft nylon/thumb, 1=hard pick — controls noise burst length and filter brightness
(param brightness  @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)
  ; LP corner in the feedback loop: lower=darker/more damped, higher=brighter/more sustain
(param pick_pos    @default 0.15 @min 0.05 @max 0.45)
  ; Pluck position as fraction of string: 0.05=bridge (bright), 0.45=neck (warm/hollow)
(param sustain_s   @default 1.5  @min 0.1  @max 10   @unit s   @mod true @mod-mode additive)
  ; T60 decay time — same perceptual sustain at all pitches
(param wood        @default 0.5  @min 0    @max 1    @mod true @mod-mode additive)
  ; Body resonance level — adds the acoustic box character
(param mic_dist_ms @default 2    @min 1    @max 10   @unit ms)
  ; Mic distance: slight delay adds air and early reflections
(param vel_bright  @default 0.3  @min 0    @max 1)
  ; How much velocity opens the brightness (harder strum = brighter attack)
(param gain        @default 0.18 @min 0    @max 1)

; ── Safe pitch: floor at 40Hz ──
(def safe_pitch (max pitch 40.0))
(def delay_nominal (/ 44100.0 safe_pitch))

; ── Exciter: sample counter reset on gate rising edge ──
; This mirrors the working editor patch more closely than relying on trigger timing.
(make-history gate_hist)
(make-history counter_hist)
(def prev_gate    (read-history gate_hist))
(def counter_prev (read-history counter_hist))
(def note_on      (gt (- gate prev_gate) 0.0))
(def counter      (gswitch note_on 0.0 (+ counter_prev 1.0)))
(def burst_len    (+ 40.0 (* (- 1.0 (clip (mod hardness) 0 1)) 120.0)))
(def noise_env    (lt counter burst_len))
(def exc_cutoff  (+ 400.0 (* (clip (mod hardness) 0 1) 12000.0)))
(def exc_q       (+ 0.3 (* (- 1.0 (clip (mod hardness) 0 1)) 0.7)))
(def burst       (biquad (* (noise) noise_env) exc_cutoff exc_q 1 0))

; Pluck position comb: notch at harmonics of 1/pickPos
; (subtracts a delayed copy of the exciter to simulate where along the string it's plucked)
(def pick_dly    (* (clip pick_pos 0.05 0.45) delay_nominal))
(def comb_exc    (- burst (delay burst pick_dly)))

; ── One-pole LP coefficient ──
; Controls both the timbre in the loop and therefore the decay curve.
; Velocity adds brightness so a hard strum sounds brighter.
(def eff_bright  (clip (+ (mod brightness) (* vel_bright velocity)) 0.05 0.99))
(def lp_freq     (max (* safe_pitch 1.5)
                       (+ 200.0 (* eff_bright 18000.0))))
(def exp1        (exp (/ (* -1.0 twopi lp_freq) 44100.0)))

; ── Stretch: compensates for LP attenuation at the fundamental ──
; |H(e^j2πf/sr)| = (1-exp1) / sqrt(1 + exp1² - 2*exp1*cos(2πf/sr))
; stretch = pow(0.001, 1/(sustain_s * pitch)) / |H|
; → guarantees -60dB decay in exactly sustain_s seconds regardless of brightness/pitch
(def cos_term (cos (* (/ safe_pitch 44100.0) twopi)))
(def mag_sq   (max 0.000001 (+ 1.0 (* exp1 exp1) (* -2.0 exp1 cos_term))))
(def mag_h    (/ (- 1.0 exp1) (sqrt mag_sq)))
(def stretch  (min 0.99999
                (/ (pow 0.001 (/ 1.0 (* (mod sustain_s) safe_pitch)))
                   mag_h)))

; ── Delay length with LP group delay compensation ──
; Group delay of one-pole LP at DC ≈ exp1 / (1 - exp1) samples.
; Subtracting this keeps the loop's effective pitch accurate.
(def lp_group_dly (/ exp1 (max 0.001 (- 1.0 exp1))))
(def delay_len    (max 1.5 (- delay_nominal lp_group_dly)))

; ── Digital waveguide loop ──
; Match the working editor patch:
; current loop input feeds the delay directly, and history stores the prior filtered output.
(make-history ks_hist)
(def ks_prev    (read-history ks_hist))
(def loop_input (+ comb_exc (* ks_prev stretch)))
(def delayed    (delay loop_input delay_len))
; One-pole LP applied to the delayed sample — inside the loop
(def ks_out     (+ (* delayed (- 1.0 exp1)) (* ks_prev exp1)))

; ── Acoustic body resonances ──
; Guitar body has strong bandpass modes that add the characteristic wood character
(def body1 (biquad ks_out 100.0 2.0 0.5 2))   ; lower body cavity
(def body2 (biquad ks_out 210.0 1.8 0.4 2))   ; main top plate resonance
(def body3 (biquad ks_out 420.0 1.4 0.3 2))   ; upper partial resonance
(def body_out (* (+ body1 body2 body3) (clip (mod wood) 0 1)))

; ── Mic distance: subtle delay adds air / early room ──
(def mic_dly  (max 1.0 (* mic_dist_ms 44.1)))
(def with_body (delay (+ ks_out body_out) mic_dly))

; ── Output: air rolloff + DC block ──
(def lp_out (biquad with_body 12000.0 0.7 0.8 0))

; Inline DC block: y = x - lpf(x, 0.999)
(make-history dc_hist)
(def dc_lp  (mix lp_out (read-history dc_hist) 0.999))
(def dc_out (- lp_out dc_lp))

; ── Commit histories ──
(write-history gate_hist gate)
(write-history counter_hist counter)
(write-history ks_hist ks_out)
(write-history dc_hist dc_lp)

(def vel_scale (+ 0.3 (* 0.7 velocity)))
(def final_out (* dc_out vel_scale gain))
(out final_out 1 @name left)
(out final_out 2 @name right)
