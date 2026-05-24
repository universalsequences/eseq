;; =====================================================================
;; PHYSICAL MODELING PIANO SYNTHESIS
;; 
;; Features:
;; - 12-string unison modeling (String A & String B detuning)
;; - Multi-mode inharmonic physical model (6 stretched modes per string)
;; - Dynamic hammer strike excitation (felt noise + metallic impulse)
;; - Full-resonance wooden piano soundboard simulation (cabinet modes)
;; - Keyboard-tracked decay slope and authentic keybed damping release
;; =====================================================================

;; --- Host Inputs ---
(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))

;; --- Host Modulators ---
(def mod1 (in 5  @name mod1  @modulator 1))
(def mod2 (in 6  @name mod2  @modulator 2))
(def mod3 (in 7  @name mod3  @modulator 3))
(def mod4 (in 8  @name mod4  @modulator 4))

;; --- Parameters ---
(param hammer_hardness @default 0.45   @min 0.05   @max 0.95   @mod true @mod-mode additive)
(param hammer_noise    @default 0.28   @min 0.0    @max 0.85)
(param vel_sens        @default 0.85   @min 0.1    @max 1.0)
(param inharmonicity   @default 0.00028 @min 0.0000 @max 0.0018 @mod true @mod-mode additive)
(param unison_detune   @default 3.2    @min 0.0    @max 15.0   @unit cents @mod true @mod-mode additive)
(param sustain_s       @default 4.8    @min 0.4    @max 12.0   @unit s @mod true @mod-mode additive)
(param decay_slope     @default 0.38   @min 0.0    @max 0.95)
(param key_track       @default 0.58   @min 0.0    @max 1.4)
(param soundboard_mix  @default 0.62   @min 0.0    @max 1.5    @mod true @mod-mode additive)
(param wooden_damping  @default 0.52   @min 0.05   @max 0.95)
(param damper_release  @default 240.0  @min 40.0   @max 800.0  @unit ms)
(param gain            @default 0.18   @min 0.0    @max 1.0)

;; --- Physical Modeling Macros ---
(defmacro mode-freq (f0 n b)
  (* f0 (pow (+ 1.0 (* b (* n n))) 0.5)))

(defmacro mode-q (f decay)
  (* 3.1415926 (* f decay)))

;; --- 1. Dynamic Hammer Strike Excitation ---
(def scaled_velocity (+ (- 1.0 vel_sens) (* velocity vel_sens)))

;; Hammer physical strike envelope (very short transient impact)
(def hammer_env (adsr gate trigger 0.5 7.5 0.0 10.0))

;; Felt sound (lowpass filtered noise)
(def felt_noise (svf (noise) (+ 900.0 (* (mod hammer_hardness) 3600.0)) 1.1 0))

;; Hammer metal strike core (highpassed trigger pulse)
(def strike_impulse (svf trigger (+ 1800.0 (* (mod hammer_hardness) 2800.0)) 1.8 2))

;; Combined hammer exciter signal scaled by velocity dynamics
(def hammer_exciter (* hammer_env (+ (* felt_noise hammer_noise) strike_impulse)))
(def exciter (* hammer_exciter scaled_velocity))

;; --- 2. Unison String Tuning & Keyboard Tracking ---
(def ktrack (pow (/ 130.0 (max pitch 35.0)) key_track))
(def string_decay (* (mod sustain_s) ktrack))

(def detune_ratio (* (mod unison_detune) 0.0005778))
(def f_a (* pitch (- 1.0 detune_ratio)))
(def f_b (* pitch (+ 1.0 detune_ratio)))

;; --- 3. String A Physical Resonance Model ---
(def f_a1 (mode-freq f_a 1.0 (mod inharmonicity)))
(def f_a2 (mode-freq f_a 2.0 (mod inharmonicity)))
(def f_a3 (mode-freq f_a 3.0 (mod inharmonicity)))
(def f_a4 (mode-freq f_a 4.0 (mod inharmonicity)))
(def f_a5 (mode-freq f_a 5.0 (mod inharmonicity)))
(def f_a6 (mode-freq f_a 6.0 (mod inharmonicity)))

(def decay_a1 (* string_decay (pow 1.0 (- 0.0 decay_slope))))
(def decay_a2 (* string_decay (pow 2.0 (- 0.0 decay_slope))))
(def decay_a3 (* string_decay (pow 3.0 (- 0.0 decay_slope))))
(def decay_a4 (* string_decay (pow 4.0 (- 0.0 decay_slope))))
(def decay_a5 (* string_decay (pow 5.0 (- 0.0 decay_slope))))
(def decay_a6 (* string_decay (pow 6.0 (- 0.0 decay_slope))))

(def q_limit 2200.0)
(def q_a1 (clip (mode-q f_a1 decay_a1) 2.0 q_limit))
(def q_a2 (clip (mode-q f_a2 decay_a2) 2.0 q_limit))
(def q_a3 (clip (mode-q f_a3 decay_a3) 2.0 q_limit))
(def q_a4 (clip (mode-q f_a4 decay_a4) 2.0 q_limit))
(def q_a5 (clip (mode-q f_a5 decay_a5) 2.0 q_limit))
(def q_a6 (clip (mode-q f_a6 decay_a6) 2.0 q_limit))

(def res_a1 (svf exciter f_a1 q_a1 1))
(def res_a2 (svf exciter f_a2 q_a2 1))
(def res_a3 (svf exciter f_a3 q_a3 1))
(def res_a4 (svf exciter f_a4 q_a4 1))
(def res_a5 (svf exciter f_a5 q_a5 1))
(def res_a6 (svf exciter f_a6 q_a6 1))

(def string_a (+ res_a1
                 (* 0.55 res_a2)
                 (* 0.38 res_a3)
                 (* 0.24 res_a4)
                 (* 0.14 res_a5)
                 (* 0.07 res_a6)))

;; --- 4. String B Physical Resonance Model ---
(def f_b1 (mode-freq f_b 1.0 (mod inharmonicity)))
(def f_b2 (mode-freq f_b 2.0 (mod inharmonicity)))
(def f_b3 (mode-freq f_b 3.0 (mod inharmonicity)))
(def f_b4 (mode-freq f_b 4.0 (mod inharmonicity)))
(def f_b5 (mode-freq f_b 5.0 (mod inharmonicity)))
(def f_b6 (mode-freq f_b 6.0 (mod inharmonicity)))

(def q_b1 (clip (mode-q f_b1 decay_a1) 2.0 q_limit))
(def q_b2 (clip (mode-q f_b2 decay_a2) 2.0 q_limit))
(def q_b3 (clip (mode-q f_b3 decay_a3) 2.0 q_limit))
(def q_b4 (clip (mode-q f_b4 decay_a4) 2.0 q_limit))
(def q_b5 (clip (mode-q f_b5 decay_a5) 2.0 q_limit))
(def q_b6 (clip (mode-q f_b6 decay_a6) 2.0 q_limit))

(def res_b1 (svf exciter f_b1 q_b1 1))
(def res_b2 (svf exciter f_b2 q_b2 1))
(def res_b3 (svf exciter f_b3 q_b3 1))
(def res_b4 (svf exciter f_b4 q_b4 1))
(def res_b5 (svf exciter f_b5 q_b5 1))
(def res_b6 (svf exciter f_b6 q_b6 1))

(def string_b (+ res_b1
                 (* 0.55 res_b2)
                 (* 0.38 res_b3)
                 (* 0.24 res_b4)
                 (* 0.14 res_b5)
                 (* 0.07 res_b6)))

;; --- 5. Unison Sum & Damper Modeling ---
(def strings_sum (* 0.5 (+ string_a string_b)))

;; Authentic mechanical damping damper release
(def damper (adsr gate trigger 1.0 1.0 1.0 damper_release))
(def damped_strings (* strings_sum damper))

;; --- 6. Wooden Soundboard Panel Simulation ---
(def wood_q (max (* 12.0 (- 1.0 wooden_damping)) 1.2))

(def wood_res1 (svf damped_strings 88.0 wood_q 1))
(def wood_res2 (svf damped_strings 215.0 wood_q 1))
(def wood_res3 (svf damped_strings 435.0 wood_q 1))
(def wood_res4 (svf damped_strings 760.0 wood_q 1))

(def soundboard_body (+ (* 0.35 wood_res1)
                        (* 0.58 wood_res2)
                        (* 0.45 wood_res3)
                        (* 0.28 wood_res4)))

;; --- 7. Final Output Stage ---
(def piano_mono (+ damped_strings (* (mod soundboard_mix) soundboard_body)))
(def final_sig (* piano_mono gain 0.065))

(out final_sig 1 @name audio)
