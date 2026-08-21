; Acoustic Grand Piano Synth
; Physical-Additive-FM Hybrid Engine with Key-Tracked Dampers & Inharmonic Overtones

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))

; Modulator inputs for host modulation matrix mapping
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))

; --- Sound Shaping Parameters ---
(param decay        @default 3200 @min 150  @max 10000 @unit ms @mod true @mod-mode additive)
(param release      @default 280  @min 20   @max 2000  @unit ms @mod true @mod-mode additive)
(param brightness   @default 0.65 @min 0.1  @max 1.0   @mod true @mod-mode additive)
(param detune       @default 0.28 @min 0.0  @max 1.5   @unit cents @mod true @mod-mode additive)
(param inharmonic   @default 0.18 @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param hammer_felt  @default 0.40 @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param hammer_hard  @default 0.45 @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param soundboard   @default 0.35 @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param dynamics     @default 0.80 @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param gain         @default 0.22 @min 0.0  @max 1.0   @mod true @mod-mode additive)

; --- Safe pitch ---
(def safe_pitch (max pitch 20.0))

; --- Acoustic Key Tracking Logic ---
; 1. Decay Scaling: Higher strings are shorter and decay extremely rapidly
(def key_decay_factor (pow (/ 261.63 (+ 180.0 safe_pitch)) 0.78))
(def k_decay (clip key_decay_factor 0.05 3.5))

; 2. Brightness Scaling: High notes are naturally more warm/sine-like
(def key_bright_factor (pow (/ 320.0 (+ 320.0 safe_pitch)) 0.48))
(def k_bright (clip key_bright_factor 0.12 1.0))

; 3. Damper Key Tracking: Acoustic pianos have no dampers on high notes (above ~1200 Hz).
; Releasing the key below 600 Hz damps the string fully. Above 1200 Hz, releasing key has no effect.
(def k_damper (clip (/ (- 1200.0 safe_pitch) 600.0) 0.0 1.0))

; 4. Effective Gate: Merges damper-off state with held gate
(def gate_eff (max gate (- 1.0 k_damper)))

; --- Envelopes ---
(def decay_ms (clip (* (mod decay) k_decay) 15.0 14000.0))
(def release_ms (clip (mod release) 15.0 3000.0))

; Main amplitude envelope for the string decay
(def amp_env (adsr gate_eff trigger 1.5 decay_ms 0.0 release_ms))

; Short attack hammer envelope
(def hammer_env (adsr gate trigger 0.5 (* 28.0 k_decay) 0.0 5.0))

; Fast-decaying FM string bloom envelope (brassy attack)
(def bloom_env (adsr gate trigger 0.5 (* 80.0 k_decay) 0.0 10.0))

; --- Dual-String Modeling (Chlorusing / Beating) ---
(def detune_ratio (* (mod detune) 0.00065))
(def freq_a (* safe_pitch (+ 1.0 detune_ratio)))
(def freq_b (* safe_pitch (- 1.0 detune_ratio)))

; --- Physical Inharmonicity ---
; Euler-Bernoulli beam theory: stretches higher harmonics slightly for stiff steel wires
(def B (* (mod inharmonic) 0.00035))
(def mult1 1.0)
(def mult2 (* 2.0 (pow (+ 1.0 (* B 4.0)) 0.5)))
(def mult3 (* 3.0 (pow (+ 1.0 (* B 9.0)) 0.5)))
(def mult4 (* 4.0 (pow (+ 1.0 (* B 16.0)) 0.5)))
(def mult5 (* 5.0 (pow (+ 1.0 (* B 25.0)) 0.5)))

; Velocity overtone response
(def vel_bright (+ 0.32 (* 0.68 velocity)))

; Harmonic levels
(def h2_vol (* 0.44 vel_bright (mod brightness) k_bright))
(def h3_vol (* 0.23 (* vel_bright vel_bright) (mod brightness) k_bright))
(def h4_vol (* 0.11 (* vel_bright vel_bright vel_bright) (mod brightness) k_bright))
(def h5_vol (* 0.05 (* vel_bright vel_bright vel_bright vel_bright) (mod brightness) k_bright))

; String A Partials
(def hA1 (sin (* twopi (phasor freq_a))))
(def hA2 (sin (* twopi (phasor (* freq_a mult2)))))
(def hA3 (sin (* twopi (phasor (* freq_a mult3)))))
(def hA4 (sin (* twopi (phasor (* freq_a mult4)))))
(def hA5 (sin (* twopi (phasor (* freq_a mult5)))))

; String B Partials
(def hB1 (sin (* twopi (phasor freq_b))))
(def hB2 (sin (* twopi (phasor (* freq_b mult2)))))
(def hB3 (sin (* twopi (phasor (* freq_b mult3)))))
(def hB4 (sin (* twopi (phasor (* freq_b mult4)))))
(def hB5 (sin (* twopi (phasor (* freq_b mult5)))))

; Summed Strings
(def string_a_sum (+ hA1 (* hA2 h2_vol) (* hA3 h3_vol) (* hA4 h4_vol) (* hA5 h5_vol)))
(def string_b_sum (+ hB1 (* hB2 h2_vol) (* hB3 h3_vol) (* hB4 h4_vol) (* hB5 h5_vol)))

; --- Non-linear FM String "Bloom" ---
(def fm_mod_freq (* safe_pitch 4.0))
(def fm_index (* 4.2 (mod brightness) vel_bright bloom_env))
(def fm_mod_phase (sin (* twopi (phasor fm_mod_freq))))
(def fm_carrier_phase (+ (* twopi (phasor safe_pitch)) (* fm_mod_phase fm_index)))
(def fm_sig (* (sin fm_carrier_phase) 0.35 bloom_env vel_bright))

; --- Hammer Strike Excitation ---
; Felt Knock (Woody thud)
(def raw_noise (noise))
(def knock_cutoff (clip (+ 350.0 (* (mod hammer_hard) 3500.0) (* velocity 1800.0)) 120.0 7500.0))
(def knock_filt (svf raw_noise knock_cutoff 1.4 1)) ; Bandpass filter
(def knock_sig (* knock_filt (mod hammer_felt) hammer_env velocity 1.6))

; Metallic Ping (High-frequency string-strike ring)
(def ping_freq (clip (* safe_pitch (+ 6.5 (* (mod hammer_hard) 10.5))) 1000.0 17000.0))
(def ping_sig (* (sin (* twopi (phasor ping_freq))) (mod hammer_hard) 0.30 hammer_env velocity))

; --- Stereo Image & Resonance Cabinet ---
(def left_string (+ (* string_a_sum 0.65) (* string_b_sum 0.35) (* fm_sig 0.50)))
(def right_string (+ (* string_a_sum 0.35) (* string_b_sum 0.65) (* fm_sig 0.50)))

(def left_dry (+ left_string knock_sig ping_sig))
(def right_dry (+ right_string knock_sig ping_sig))

; Cabinet/Soundboard parallel resonators (Peak SVF filters)
(def res_l_1 (svf left_dry 115.0 1.6 4))
(def res_l_2 (svf left_dry 265.0 1.3 4))
(def res_l_3 (svf left_dry 550.0 1.1 4))
(def res_l (+ (* res_l_1 0.35) (* res_l_2 0.30) (* res_l_3 0.20)))

(def res_r_1 (svf right_dry 120.0 1.6 4))
(def res_r_2 (svf right_dry 275.0 1.3 4))
(def res_r_3 (svf right_dry 570.0 1.1 4))
(def res_r (+ (* res_r_1 0.35) (* res_r_2 0.30) (* res_r_3 0.20)))

(def sb_amt (clip (mod soundboard) 0.0 1.0))
(def left_mixed (+ left_dry (* res_l sb_amt 1.1)))
(def right_mixed (+ right_dry (* res_r sb_amt 1.1)))

; --- Lid Tone Filter ---
(def lid_cutoff (clip (+ 1000.0 (* (mod brightness) 14000.0)) 750.0 19500.0))
(def left_final (svf left_mixed lid_cutoff 0.65 0))
(def right_final (svf right_mixed lid_cutoff 0.65 0))

; --- Dynamic Velocity Response Curve ---
(def vel_curve (+ (- 1.0 (mod dynamics)) (* (mod dynamics) (* velocity velocity))))

; --- Outputs ---
(def out_l (* left_final amp_env vel_curve (mod gain)))
(def out_r (* right_final amp_env vel_curve (mod gain)))

(out out_l 1 @name left)
(out out_r 2 @name right)
