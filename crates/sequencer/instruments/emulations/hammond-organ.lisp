; Hammond B3 inspired tonewheel organ
; 9 drawbars (additive sine partials), key click, percussion, Leslie speaker simulation

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

; ── Drawbars: 16', 8', 5⅓', 4', 2⅔', 2', 1⅗', 1⅓', 1' ──
; Harmonic multipliers relative to 8' fundamental:
; 0.5×, 1×, 1.5×, 2×, 3×, 4×, 5×, 6×, 8×
(param draw1  @default 0.5  @min 0 @max 1 @mod true @mod-mode additive)  ; 16' sub
(param draw2  @default 1.0  @min 0 @max 1 @mod true @mod-mode additive)  ; 8' fundamental
(param draw3  @default 0.8  @min 0 @max 1 @mod true @mod-mode additive)  ; 5⅓' quint
(param draw4  @default 0.7  @min 0 @max 1 @mod true @mod-mode additive)  ; 4' super octave
(param draw5  @default 0.3  @min 0 @max 1 @mod true @mod-mode additive)  ; 2⅔' nazard
(param draw6  @default 0.0  @min 0 @max 1 @mod true @mod-mode additive)  ; 2' block flute
(param draw7  @default 0.0  @min 0 @max 1 @mod true @mod-mode additive)  ; 1⅗' tierce
(param draw8  @default 0.0  @min 0 @max 1 @mod true @mod-mode additive)  ; 1⅓' larigot
(param draw9  @default 0.0  @min 0 @max 1 @mod true @mod-mode additive)  ; 1' sifflöte

; ── Tone & character ──
(param drive         @default 0.3   @min 0    @max 1    @mod true @mod-mode additive)  ; amp/speaker overdrive
(param click_amt     @default 0.35  @min 0    @max 1)   ; key click level
(param click_decay   @default 6     @min 1    @max 40   @unit ms)
(param perc_level    @default 0.0   @min 0    @max 1    @mod true @mod-mode additive)  ; 2nd harmonic percussion
(param perc_decay    @default 800   @min 50   @max 3000 @unit ms @mod true @mod-mode additive)

; ── Rotary speaker (Leslie) ──
(param rotary_speed  @default 0.8   @min 0    @max 8    @unit Hz @mod true @mod-mode additive)  ; horn rotation rate
(param rotary_depth  @default 0.3   @min 0    @max 1    @mod true @mod-mode additive)  ; AM tremolo depth
(param rotary_doppler @default 0.4  @min 0    @max 1)   ; Doppler pitch shimmer depth

(param gain          @default 0.10  @min 0    @max 1)

; ── Tonewheel sines ──
; Each drawbar is a sine at its harmonic multiple of fundamental
(def b1 (sin (* twopi (phasor (* pitch 0.5)))))
(def b2 (sin (* twopi (phasor (* pitch 1.0)))))
(def b3 (sin (* twopi (phasor (* pitch 1.5)))))
(def b4 (sin (* twopi (phasor (* pitch 2.0)))))
(def b5 (sin (* twopi (phasor (* pitch 3.0)))))
(def b6 (sin (* twopi (phasor (* pitch 4.0)))))
(def b7 (sin (* twopi (phasor (* pitch 5.0)))))
(def b8 (sin (* twopi (phasor (* pitch 6.0)))))
(def b9 (sin (* twopi (phasor (* pitch 8.0)))))

(def total_draw (max 0.001 (+ (mod draw1) (mod draw2) (mod draw3)
                               (mod draw4) (mod draw5) (mod draw6)
                               (mod draw7) (mod draw8) (mod draw9))))
(def organ (/ (+ (* b1 (mod draw1)) (* b2 (mod draw2)) (* b3 (mod draw3))
                  (* b4 (mod draw4)) (* b5 (mod draw5)) (* b6 (mod draw6))
                  (* b7 (mod draw7)) (* b8 (mod draw8)) (* b9 (mod draw9)))
               total_draw))

; ── Key gate: organ-style, essentially instant on/off ──
; The "click" IS the attack — no amplitude ramp needed
(def key_env        (adsr gate trigger 1 1 1.0 15))
(def velocity_scale (+ 0.4 (* 0.6 velocity)))
(def gated_organ    (* organ key_env velocity_scale))

; ── Key click: brief burst of the tonewheel signal itself ──
; Real B3 click = contacts expose tonewheel at wrong phase for a few ms
; Using the actual harmonic content (not noise) gives the correct character
(def click_env (adsr gate trigger 1 click_decay 0.0 1))
(def click_sig (* gated_organ click_env click_amt 2.5))

; ── Percussion: 2nd harmonic (4') single-triggered transient ──
; Fires on trigger, decays independently of gate (correct B3 behavior)
(def perc_env (adsr gate trigger 2 (mod perc_decay) 0.0 10))
(def perc_sig (* b4 perc_env (mod perc_level) 0.55))

; ── Mix + overdrive (amplifier / speaker breakup) ──
(def pre_drive (+ gated_organ click_sig perc_sig))
(def drive_amt (+ 1.0 (* (mod drive) 5.0)))
(def driven    (tanh (* pre_drive drive_amt)))

; ── Leslie rotary speaker simulation ──
; Horn (treble): strong AM tremolo + Doppler delay modulation
; Drum (bass rotor): gentler AM, slightly slower rotation
(def horn_rate  (max 0.01 (mod rotary_speed)))
(def drum_rate  (* horn_rate 0.78))

(def horn_lfo   (sin (* twopi (phasor horn_rate))))
(def drum_lfo   (sin (* twopi (phasor drum_rate))))

(def treble     (biquad driven 700 0.65 1 1))
(def bass_comp  (biquad driven 700 0.65 1 0))

(def rot_depth  (clip (mod rotary_depth) 0 1))

; Horn: full AM swing + Doppler shimmer
(def horn_am    (+ (- 1 rot_depth) (* rot_depth 0.5 (+ 1.0 horn_lfo))))
(def horn_dly   (+ 25.0 (* rotary_doppler horn_lfo 15.0)))
(def treble_out (* (delay treble (max 1 horn_dly)) horn_am))

; Drum: gentle AM only (bass rotor has less high-freq Doppler effect)
(def drum_am    (+ (- 1.0 (* rot_depth 0.15)) (* rot_depth 0.15 drum_lfo)))
(def bass_out   (* bass_comp drum_am))

(def leslie_out (+ treble_out bass_out))

(out (* leslie_out gain) 1 @name audio)
