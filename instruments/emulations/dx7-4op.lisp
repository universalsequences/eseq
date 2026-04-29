; DX7-style 4-operator FM synthesizer
; Four algorithms: cascade, dual-pair, fan, additive
; Key addition over fmplus: per-operator envelope control and true algorithm switching

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

; ── Global envelope ──
(param amp_attack_ms   @default 4    @min 1    @max 5000 @unit ms)
(param amp_decay_ms    @default 500  @min 1    @max 5000 @unit ms)
(param amp_sustain     @default 0.7  @min 0    @max 1)
(param amp_release_ms  @default 120  @min 1    @max 5000 @unit ms)

; ── Algorithm selection ──
; 1=cascade (4→3→2→1), 2=dual-pair (2→1 + 4→3), 3=fan ((2+3+4)→1), 4=additive (1+2+3+4)
(param algorithm       @default 2    @min 1    @max 4)

; ── Operator frequency ratios ──
(param ratio1          @default 1.0  @min 0.25 @max 16  @mod true @mod-mode additive)
(param ratio2          @default 1.0  @min 0.25 @max 16  @mod true @mod-mode additive)
(param ratio3          @default 14.0 @min 0.25 @max 16  @mod true @mod-mode additive)  ; classic DX7 EP modulator
(param ratio4          @default 1.0  @min 0.25 @max 16  @mod true @mod-mode additive)

; ── Modulation indices ──
; For algorithms where op3/op4 are modulators, their "level" = modulation depth
; For additive, all ops are carriers and these become amplitude weights
(param index2          @default 2.2  @min 0    @max 12   @mod true @mod-mode additive)  ; op2 mod depth / carrier level
(param index3          @default 3.5  @min 0    @max 12   @mod true @mod-mode additive)  ; op3 mod depth
(param index4          @default 0.5  @min 0    @max 12   @mod true @mod-mode additive)  ; op4 mod depth
(param feedback        @default 0.15 @min 0    @max 2    @mod true @mod-mode additive)  ; op1 self-feedback

; ── Carrier levels (used in dual-pair and additive) ──
(param level1          @default 1.0  @min 0    @max 1    @mod true @mod-mode additive)
(param level3          @default 0.8  @min 0    @max 1    @mod true @mod-mode additive)

; ── Per-modulator envelope decay ──
; Modulators can have fast-decaying envelopes for transient shaping (key DX7 technique)
; mod_decay_ms = how fast the modulation index decays over time
(param mod_decay_ms    @default 400  @min 10   @max 5000 @unit ms @mod true @mod-mode additive)
; mod_sustain = final modulation level (0 = fully decays to zero, 1 = stays at full index)
(param mod_sustain     @default 0.0  @min 0    @max 1)

(param vel_to_index    @default 0.5  @min 0    @max 1)  ; velocity scales modulation depth
(param amp_vel_amt     @default 0.4  @min 0    @max 1)
(param gain            @default 0.12 @min 0    @max 1)

; ── Signal path ──
(def amp_env    (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
; Modulator envelope: fired on trigger, decays independently (like DX7 L4 slope)
(def mod_env    (adsr gate trigger 2 (mod mod_decay_ms) mod_sustain 20))
(def vel_idx    (+ (- 1 vel_to_index) (* vel_to_index velocity)))

; Operator phases
(def ph1 (phasor (* pitch (clip (mod ratio1) 0.25 16))))
(def ph2 (phasor (* pitch (clip (mod ratio2) 0.25 16))))
(def ph3 (phasor (* pitch (clip (mod ratio3) 0.25 16))))
(def ph4 (phasor (* pitch (clip (mod ratio4) 0.25 16))))

; Scaled indices (velocity + modulator envelope = DX7's velocity sensitivity + time-varying timbre)
(def idx2 (* (clip (mod index2) 0 12) mod_env vel_idx))
(def idx3 (* (clip (mod index3) 0 12) mod_env vel_idx))
(def idx4 (* (clip (mod index4) 0 12) mod_env vel_idx))

; Op1 self-feedback (1-sample delay)
(make-history op1_fb_hist)
(def op1_fb (read-history op1_fb_hist))

; ──────────────────────────────────────────────
; Algorithm 1: CASCADE  4 → 3 → 2 → 1(out)
; Deep harmonic stacking, good for brass, complex bass, metallic pads
(def op4_c1  (sin (* twopi ph4)))
(def op3_c1  (sin (+ (* twopi ph3) (* op4_c1 idx4))))
(def op2_c1  (sin (+ (* twopi ph2) (* op3_c1 idx3))))
(def op1_c1  (sin (+ (* twopi ph1) (* op2_c1 idx2) (* op1_fb (mod feedback)))))
(def alg1    (* op1_c1 (mod level1)))

; ──────────────────────────────────────────────
; Algorithm 2: DUAL PAIR  [2→1] + [4→3]   ← DX7 Algorithm 5 territory
; Each pair: modulator → carrier. Output = carrier1 + carrier2
; Classic DX7 electric piano, vibraphone, marimba
(def op2_c2  (sin (* twopi ph2)))
(def op4_c2  (sin (* twopi ph4)))
(def op1_c2  (sin (+ (* twopi ph1) (* op2_c2 idx2) (* op1_fb (mod feedback)))))
(def op3_c2  (sin (+ (* twopi ph3) (* op4_c2 idx4))))
(def alg2    (+ (* op1_c2 (mod level1)) (* op3_c2 (mod level3))))

; ──────────────────────────────────────────────
; Algorithm 3: FAN  [2 + 3 + 4] → 1(out)
; Three modulators feeding one carrier: dense complex single tone
; Good for vocal-like tones, complex leads, metallic single notes
(def op2_c3  (sin (* twopi ph2)))
(def op3_c3  (sin (* twopi ph3)))
(def op4_c3  (sin (* twopi ph4)))
(def op1_c3  (sin (+ (* twopi ph1)
                     (* op2_c3 idx2)
                     (* op3_c3 idx3)
                     (* op4_c3 idx4)
                     (* op1_fb (mod feedback)))))
(def alg3    (* op1_c3 (mod level1)))

; ──────────────────────────────────────────────
; Algorithm 4: ADDITIVE  1 + 2 + 3 + 4   (all carriers)
; All four operators output directly — pure additive with harmonic ratios
; Good for Hammond-like organ, additive pads, bells
(def op1_c4  (sin (+ (* twopi ph1) (* op1_fb (mod feedback)))))
(def op2_c4  (sin (* twopi ph2)))
(def op3_c4  (sin (* twopi ph3)))
(def op4_c4  (sin (* twopi ph4)))
(def alg4    (+ (* op1_c4 (mod level1) idx2)
                (* op2_c4 (mod level1) idx3)
                (* op3_c4 (mod level3) idx4)
                (* op4_c4 (mod level3) 0.5)))

; ── Algorithm selector (1-based: matches algorithm param 1-4) ──
(def selected (selector algorithm alg1 alg2 alg3 alg4))
(write-history op1_fb_hist selected)  ; feedback from the full output (algorithm-independent)

; ── Soft limiting + output ──
(def amp_vel  (+ (- 1 amp_vel_amt) (* amp_vel_amt velocity)))
(out (* (tanh (* selected 1.1)) amp_env amp_vel gain) 1 @name audio)
