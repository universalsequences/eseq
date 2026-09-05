; Triton — Korg Triton (HI synthesis) style PCM workstation synth.
; Two wavetable "PCM" oscillators reading a 32-set x 16-wave AKWF-derived
; single-cycle ROM bank, velocity wave switching, dual filter modes
; (A: 24dB resonant LP, B: 12dB LP + 12dB HP), Korg-style multi-stage
; envelopes (4 times / 4-5 levels) for filter and amp, simple pitch EG,
; two fade-in LFOs, an AMS-lite mod matrix, and portamento for the
; classic gliding grime/hip-hop basses.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(def bank (tensor @shape [512 512] @file "waves/bank.json"))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro dbamp (db)
  (exp (* 0.1151292546 db)))

; Latch a signal when trig fires; hold it otherwise (per-voice randoms).
(defmacro latch_on_trigger (sig trig)
  (make-history hold_hist)
  (def held (read-history hold_hist))
  (def latched (gswitch (gt trig 0.5) sig held))
  (write-history hold_hist latched)
  latched)

; Resettable phase accumulator: restarts at 0 when reset fires.
(defmacro retrig_phasor (freq reset)
  (make-history ph_hist)
  (def prev_ph (read-history ph_hist))
  (def next_ph (wrap (+ prev_ph (/ freq samplerate)) 0 1))
  (def ph (gswitch (gt reset 0.5) 0.0 next_ph))
  (write-history ph_hist ph)
  ph)

; Seconds elapsed since `reset` last fired (capped so it can't overflow).
(defmacro elapsed_since (reset)
  (make-history t_hist)
  (def t_prev (read-history t_hist))
  (def t_now (gswitch (gt reset 0.5) 0.0 (min (+ t_prev (/ 1.0 samplerate)) 120.0)))
  (write-history t_hist t_now)
  t_now)

; Korg HI multi-stage EG: on gate-on runs start->attack->break->sustain
; through attack/decay/slope times, holds sustain; on gate-off ramps from
; the frozen current value to the release level over the release time.
(defmacro mseg (gate_s trig_s atk_ms dec_ms slp_ms rel_ms
                l_start l_atk l_brk l_sus l_rel)
  (def ton (elapsed_since trig_s))
  (def ta (max 0.0005 (* atk_ms 0.001)))
  (def td (max 0.0005 (* dec_ms 0.001)))
  (def ts (max 0.0005 (* slp_ms 0.001)))
  (def tr (max 0.0005 (* rel_ms 0.001)))
  (def b2 (+ ta td))
  (def b3 (+ ta td ts))
  (def seg1 (lt ton ta))
  (def seg2 (* (gte ton ta) (lt ton b2)))
  (def seg3 (* (gte ton b2) (lt ton b3)))
  (def seg4 (gte ton b3))
  (def von (+ (* seg1 (+ l_start (* (- l_atk l_start) (/ ton ta))))
              (* seg2 (+ l_atk (* (- l_brk l_atk) (/ (- ton ta) td))))
              (* seg3 (+ l_brk (* (- l_sus l_brk) (/ (- ton b2) ts))))
              (* seg4 l_sus)))
  ; freeze the on-phase value at note-off, then ramp to the release level
  (def toff (elapsed_since gate_s))
  (make-history frz_hist)
  (def frz_prev (read-history frz_hist))
  (def frz (gswitch (gt gate_s 0.5) von frz_prev))
  (write-history frz_hist frz)
  (def vrel (+ frz (* (- l_rel frz) (clip (/ toff tr) 0 1))))
  (gswitch (gt gate_s 0.5) von vrel))

; Triton-style LFO: waveform select, optional key sync, fade-in.
(defmacro triton_lfo (wave rate fade_ms keysync trig_s)
  (def lreset (* (gte keysync 0.5) trig_s))
  (def lph (retrig_phasor rate lreset))
  (make-history lph_prev_hist)
  (def lph_prev (read-history lph_prev_hist))
  (def lwrapped (lt lph lph_prev))
  (write-history lph_prev_hist lph)
  (def lsh (latch_on_trigger (noise) (max lwrapped lreset)))
  (def lraw (selector (+ (clip (round wave) 0 4) 1)
              (triangle lph)
              (- 1 (* lph 2))
              (scale (lt lph 0.5) 0 1 1 -1)
              (sin (* lph twopi))
              lsh))
  (def lfade (gswitch (gt fade_ms 1.0)
                      (clip (/ (elapsed_since trig_s) (max 0.001 (* fade_ms 0.001))) 0 1)
                      1.0))
  (* lraw lfade))

; ---- oscillators ----
(param osc1_set @default 0 @min 0 @max 31)
(param osc1_wave @default 0 @min 0 @max 15 @mod true @mod-mode additive @mod-depth-min -15 @mod-depth-max 15)
(param osc1_octave @default 0 @min -2 @max 2 @unit oct)
(param osc1_tune @default 0 @min -50 @max 50 @unit cents)
(param osc1_gain_db @default -6 @min -36 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param osc1_vel_wave @default 0 @min -15 @max 15)

(param osc2_on @default 0 @min 0 @max 1)
(param osc2_set @default 0 @min 0 @max 31)
(param osc2_wave @default 0 @min 0 @max 15 @mod true @mod-mode additive @mod-depth-min -15 @mod-depth-max 15)
(param osc2_octave @default -1 @min -2 @max 2 @unit oct)
(param osc2_detune @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -12 @mod-depth-max 12 @mod-unit st)
(param osc2_gain_db @default -6 @min -36 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param osc2_vel_wave @default 0 @min -15 @max 15)

; ---- pitch ----
(param glide_ms @default 0 @min 0 @max 1000 @unit ms)
(param peg_amt_st @default 0 @min -24 @max 24 @unit st)
(param peg_attack_ms @default 1 @min 1 @max 2000 @unit ms)
(param peg_decay_ms @default 120 @min 5 @max 5000 @unit ms)

; ---- filter ----
(param filter_mode @default 0 @min 0 @max 1)
(param cutoff @default 3500 @min 20 @max 18000 @unit Hz @mod true @mod-mode additive @mod-depth-min -8000 @mod-depth-max 8000 @mod-unit Hz)
(param resonance @default 0.25 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param hp_freq @default 20 @min 10 @max 8000 @unit Hz)
(param keytrack @default 0.4 @min 0 @max 1)
(param feg_int_oct @default 1.5 @min -8 @max 8 @unit oct)
(param feg_vel_oct @default 0 @min -4 @max 4 @unit oct)

; ---- filter EG (bipolar multi-stage) ----
(param feg_start @default 0 @min -1 @max 1)
(param feg_atk_lvl @default 1 @min -1 @max 1)
(param feg_break @default 0.5 @min -1 @max 1)
(param feg_sustain @default 0.3 @min -1 @max 1)
(param feg_rel_lvl @default 0 @min -1 @max 1)
(param feg_attack_ms @default 2 @min 1 @max 8000 @unit ms)
(param feg_decay_ms @default 180 @min 1 @max 8000 @unit ms)
(param feg_slope_ms @default 400 @min 1 @max 8000 @unit ms)
(param feg_release_ms @default 200 @min 1 @max 8000 @unit ms)

; ---- amp EG (unipolar multi-stage; start/release pinned to 0) ----
(param aeg_break @default 1 @min 0 @max 1)
(param aeg_sustain @default 0.85 @min 0 @max 1)
(param aeg_attack_ms @default 2 @min 1 @max 8000 @unit ms)
(param aeg_decay_ms @default 150 @min 1 @max 8000 @unit ms)
(param aeg_slope_ms @default 350 @min 1 @max 8000 @unit ms)
(param aeg_release_ms @default 180 @min 1 @max 8000 @unit ms)
(param vel_to_amp @default 0.4 @min 0 @max 1)

; ---- LFOs ----
(param lfo1_wave @default 0 @min 0 @max 4)
(param lfo1_rate_hz @default 5 @min 0.01 @max 30 @unit Hz)
(param lfo1_fade_ms @default 0 @min 0 @max 5000 @unit ms)
(param lfo1_keysync @default 1 @min 0 @max 1)
(param lfo1_to_pitch @default 0 @min -200 @max 200 @unit cents)
(param lfo1_to_cutoff @default 0 @min -4 @max 4 @unit oct)

(param lfo2_wave @default 0 @min 0 @max 4)
(param lfo2_rate_hz @default 0.8 @min 0.01 @max 30 @unit Hz)
(param lfo2_fade_ms @default 0 @min 0 @max 5000 @unit ms)
(param lfo2_keysync @default 0 @min 0 @max 1)
(param lfo2_to_cutoff @default 0 @min -4 @max 4 @unit oct)
(param lfo2_to_amp @default 0 @min 0 @max 1)

; ---- AMS-lite mod matrix ----
(param ams1_src @default 0 @min 0 @max 5)
(param ams1_dest @default 0 @min 0 @max 6)
(param ams1_amt @default 0 @min -1 @max 1)
(param ams2_src @default 3 @min 0 @max 5)
(param ams2_dest @default 6 @min 0 @max 6)
(param ams2_amt @default 0 @min -1 @max 1)

; ---- output ----
(param drive @default 0.1 @min 0 @max 1)
(param spread @default 0.15 @min 0 @max 1)
(param voice_pan @default 0 @min -1 @max 1)
(param volume_db @default -10 @min -36 @max 6 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)

; ---- glide / base pitch (Hz) ----
(def glide_coeff (exp (/ -1.0 (max 1.0 (* glide_ms 0.001 samplerate)))))
(make-history glide_hist)
(def prev_glide (read-history glide_hist))
(def glide_pitch (+ (* pitch (- 1 glide_coeff)) (* prev_glide glide_coeff)))
(def base_pitch (gswitch (gt glide_ms 0.1) glide_pitch pitch))
(write-history glide_hist base_pitch)

; key follow: 0 at C4, +/-1 per two octaves
(def key_val (clip (* (/ (log (/ (max base_pitch 8.0) 261.63)) (log 2)) 0.5) -1 1))

; ---- envelopes ----
(def feg (mseg gate trigger feg_attack_ms feg_decay_ms feg_slope_ms feg_release_ms
               feg_start feg_atk_lvl feg_break feg_sustain feg_rel_lvl))
(def aeg (clip (mseg gate trigger aeg_attack_ms aeg_decay_ms aeg_slope_ms aeg_release_ms
                     0.0 1.0 aeg_break aeg_sustain 0.0)
               0 1))

; pitch EG: rise to 1 over attack, decay to 0
(def peg_t (elapsed_since trigger))
(def peg_ta (max 0.0005 (* peg_attack_ms 0.001)))
(def peg_td (max 0.0005 (* peg_decay_ms 0.001)))
(def peg_env (gswitch (lt peg_t peg_ta)
                      (/ peg_t peg_ta)
                      (clip (- 1.0 (/ (- peg_t peg_ta) peg_td)) 0 1)))
(def peg_st (* peg_env peg_amt_st))

; ---- LFOs ----
(def lfo1 (triton_lfo lfo1_wave lfo1_rate_hz lfo1_fade_ms lfo1_keysync trigger))
(def lfo2 (triton_lfo lfo2_wave lfo2_rate_hz lfo2_fade_ms lfo2_keysync trigger))

; ---- AMS matrix (src: feg aeg lfo1 lfo2 key vel) ----
(defmacro ams_src_select (idx)
  (selector (+ (clip (round idx) 0 5) 1) feg aeg lfo1 lfo2 key_val velocity))

(def ams1_val (* ams1_amt (ams_src_select ams1_src)))
(def ams2_val (* ams2_amt (ams_src_select ams2_src)))

; dests: pitch wav1 wav2 cutoff res amp pan
(defmacro ams_dest_sum (d)
  (+ (* (eq (clip (round ams1_dest) 0 6) d) ams1_val)
     (* (eq (clip (round ams2_dest) 0 6) d) ams2_val)))

(def ams_pitch (ams_dest_sum 0))
(def ams_wav1 (ams_dest_sum 1))
(def ams_wav2 (ams_dest_sum 2))
(def ams_cut (ams_dest_sum 3))
(def ams_res (ams_dest_sum 4))
(def ams_amp (ams_dest_sum 5))
(def ams_pan (ams_dest_sum 6))

; ---- oscillator frequencies ----
(def pitch_ratio (* (semi_ratio (+ peg_st (* ams_pitch 12)))
                    (semi_ratio (* lfo1 (/ lfo1_to_pitch 100.0)))))
(def f1 (* base_pitch pitch_ratio
           (semi_ratio (+ (* (clip (round osc1_octave) -2 2) 12)
                          (/ osc1_tune 100.0)))))
(def f2 (* base_pitch pitch_ratio
           (semi_ratio (+ (* (clip (round osc2_octave) -2 2) 12)
                          (clip (mod osc2_detune) -24 24)))))

; ---- oscillator 1 ----
(def o1_pos_target (clip (+ (mod osc1_wave) (* velocity osc1_vel_wave) (* ams_wav1 15)) 0 15))
(make-history o1_scan_hist)
(def o1_scan_prev (read-history o1_scan_hist))
(def scan_coeff (- 1.0 (exp (/ -1.0 (* 0.008 samplerate)))))
(def o1_scan (gswitch trigger
               o1_pos_target
               (+ o1_scan_prev (* scan_coeff (- o1_pos_target o1_scan_prev)))))
(write-history o1_scan_hist o1_scan)
(def ph1 (phasor f1 trigger))
(def o1_idx (+ (* (clip (floor osc1_set) 0 31) 16) (clip o1_scan 0 15)))
(def osc1 (wavetable-read bank o1_idx ph1))

; ---- oscillator 2 ----
(def o2_pos_target (clip (+ (mod osc2_wave) (* velocity osc2_vel_wave) (* ams_wav2 15)) 0 15))
(make-history o2_scan_hist)
(def o2_scan_prev (read-history o2_scan_hist))
(def o2_scan (gswitch trigger
               o2_pos_target
               (+ o2_scan_prev (* scan_coeff (- o2_pos_target o2_scan_prev)))))
(write-history o2_scan_hist o2_scan)
(def ph2 (phasor f2 trigger))
(def o2_idx (+ (* (clip (floor osc2_set) 0 31) 16) (clip o2_scan 0 15)))
(def osc2 (wavetable-read bank o2_idx ph2))

; ---- mix + drive ----
(def o1_gain (dbamp (clip (mod osc1_gain_db) -36 12)))
(def o2_gain (* (gte osc2_on 0.5) (dbamp (clip (mod osc2_gain_db) -36 12))))
(def mix (+ (* osc1 o1_gain) (* osc2 o2_gain)))
(def drive_g (+ 1.0 (* drive 8.0)))
(def driven (/ (tanh (* mix drive_g)) (tanh drive_g)))

; ---- filter ----
(def cut_oct_mod (+ (* feg (+ feg_int_oct (* feg_vel_oct velocity)))
                    (* lfo1 lfo1_to_cutoff)
                    (* lfo2 lfo2_to_cutoff)
                    (* ams_cut 4)))
(def cut (clip (* (mod cutoff)
                  (pow (/ (max base_pitch 8.0) 261.63) keytrack)
                  (semi_ratio (* cut_oct_mod 12)))
               20 16000))
(def res (clip (+ (mod resonance) ams_res) 0 1))
; mode A: 24dB resonant LP — resonance on the first stage, soft clip between
(def lp_a1 (svf driven cut (+ 0.55 (* res 9.0)) 0))
(def lp_a (svf (/ (tanh (* lp_a1 0.9)) 0.9) cut 0.707 0))
; mode B: 12dB LP + 12dB HP
(def lp_b1 (svf driven cut (+ 0.55 (* res 6.0)) 0))
(def lp_b (svf lp_b1 (clip hp_freq 10 8000) 0.6 2))
(def filt_out (gswitch (gte filter_mode 0.5) lp_b lp_a))

; ---- amp / output ----
(def vel_gain (+ (- 1 vel_to_amp) (* velocity vel_to_amp)))
(def trem (max 0.0 (+ 1.0 (* lfo2 lfo2_to_amp))))
(def amp_scale (clip (+ 1.0 ams_amp) 0 2))
(def vol (dbamp (clip (mod volume_db) -36 6)))
(def amp (* filt_out aeg vel_gain trem amp_scale vol))
(def rnd_pan (latch_on_trigger (noise) trigger))
(def pan (clip (+ voice_pan (* rnd_pan spread) (* ams_pan 0.5)) -1 1))
(def left (* amp (clip (- 1 (* pan 0.5)) 0 1.5)))
(def right (* amp (clip (+ 1 (* pan 0.5)) 0 1.5)))

(out (tanh left) 1 @name left)
(out (tanh right) 2 @name right)
