; Poseidon — Korg Triton (HI synthesis) style PCM workstation synth, rebuilt
; from core/triton in the factory macro-vocabulary style (eseq-i2pw). Two
; wavetable "PCM" oscillators reading a 32-set x 16-wave AKWF-derived
; single-cycle ROM bank, velocity wave switching, dual filter modes
; (A: 24dB resonant LP, B: 12dB LP + 12dB HP), Korg-style multi-stage
; envelopes for filter and amp, simple pitch EG, two fade-in LFOs, an
; AMS-lite mod matrix, and portamento.
;
; Patch-editor structure mirrors factory/drift: the top level is section
; macro nodes with no bare math; non-modulated params are declared inside
; the section that owns them; host-modulatable params stay top-level and
; reach their sections as inline (mod p) arguments. Param-less cores
; (korg-eg, korg-lfo, pcm-osc) are shared by thin param-owning wrappers
; (filter-eg/amp-eg, lfo-one/lfo-two, osc-one/osc-two).
;
; Envelope rework vs core/triton — THE CLICK FIX: the old mseg jumped to a
; fixed start level the sample a note retriggered, discontinuous from
; wherever the envelope actually was (sustain, mid-release), which clicked
; hard on close notes. korg-eg latches its own output at the trigger and
; starts the attack from there (retrigger-from-current), so the feg_start
; param is gone. Oscillator phase is free-running (the old per-trigger
; phase reset was a second discontinuity at now-nonzero amplitude), and the
; always-on drive tanh is now the shared crossfade drive stage (clean at 0).

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

; ======================================================================
; host-modulatable params (must live at top level)
; ======================================================================

(param osc1_wave @default 0 @min 0 @max 15 @mod true @mod-mode additive @mod-depth-min -15 @mod-depth-max 15)
(param osc1_warp @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param osc1_fold @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param osc1_gain_db @default -6 @min -36 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param osc2_wave @default 0 @min 0 @max 15 @mod true @mod-mode additive @mod-depth-min -15 @mod-depth-max 15)
(param osc2_detune @default 0 @min -24 @max 24 @unit st @mod true @mod-mode additive @mod-depth-min -12 @mod-depth-max 12 @mod-unit st)
(param osc2_warp @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param osc2_fold @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param osc2_gain_db @default -6 @min -36 @max 12 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)
(param cutoff @default 3500 @min 20 @max 18000 @unit Hz @mod true @mod-mode additive @mod-depth-min -8000 @mod-depth-max 8000 @mod-unit Hz)
(param resonance @default 0.25 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param volume_db @default -10 @min -36 @max 6 @unit dB @mod true @mod-mode additive @mod-depth-min -24 @mod-depth-max 24 @mod-unit dB)

; ======================================================================
; math / state helpers (leaf macros, collapsed inside section layers)
; ======================================================================

(defmacro semi-ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro db-amp (db)
  (exp (* 0.1151292546 db)))

; Resettable phase accumulator: restarts at 0 when reset fires.
; Not the builtin (accum inc reset 0 1): probe 2026-09-01 showed accum
; differs at reset/wrap samples, and hard phase=0 on the trigger sample is
; what keeps retriggered LFO starts deterministic.
(defmacro retrig-phasor (freq reset)
  (make-history ph_hist)
  (def prev_ph (read-history ph_hist))
  (def next_ph (wrap (+ prev_ph (/ freq samplerate)) 0 1))
  (def ph (gswitch (gt reset 0.5) 0.0 next_ph))
  (write-history ph_hist ph)
  ph)

; 1-sample pulse when a 0..1 phasor wraps around.
(defmacro wrap-trigger (ph)
  (make-history prev_hist)
  (def prev (read-history prev_hist))
  (def wrapped (lt ph prev))
  (write-history prev_hist ph)
  wrapped)

; Seconds elapsed since `reset` last fired (capped so it can't overflow).
(defmacro elapsed-since (reset)
  (make-history t_hist)
  (def t_prev (read-history t_hist))
  (def t_now (gswitch (gt reset 0.5) 0.0 (min (+ t_prev (/ 1.0 samplerate)) 120.0)))
  (write-history t_hist t_now)
  t_now)

; The shared drive stage: crossfade from pure linear (amount 0, transparent)
; into peak-normalized tanh saturation (amount 1, compressed + harmonics).
(defmacro drive-stage (x amount)
  (def g (+ 1.0 (* amount 7.0)))
  (def shaped (/ (tanh (* x g)) (tanh g)))
  (mix x shaped amount))

; One mod-matrix slot routed to destination d: amt*src when dest==d else 0.
(defmacro route-if-dest (dest d amt src_val)
  (* (eq (clip (round dest) 0 6) d) amt src_val))

; ======================================================================
; shared cores (param-less; instantiated by param-owning wrappers)
; ======================================================================

; Korg HI multi-stage EG: on gate-on runs attack->break->sustain through
; attack/decay/slope times, holds sustain; on gate-off ramps from the frozen
; current value to the release level over the release time.
; CLICK FIX vs core/triton mseg: the attack starts from the envelope's own
; output latched at the trigger sample (retrigger-from-current), never from
; a fixed start level, so retriggering near a previous note is continuous.
(defmacro korg-eg (gate_s trig_s atk_ms dec_ms slp_ms rel_ms
                   l_atk l_brk l_sus l_rel)
  (make-history out_hist)
  (def prev_out (read-history out_hist))
  (def start_lvl (latch prev_out trig_s))
  (def ton (elapsed-since trig_s))
  (def ta (max 0.0005 (* atk_ms 0.001)))
  (def td (max 0.0005 (* dec_ms 0.001)))
  (def ts (max 0.0005 (* slp_ms 0.001)))
  (def tr (max 0.0005 (* rel_ms 0.001)))
  (def b2 (+ ta td))
  (def b3 (+ b2 ts))
  (def seg1 (lt ton ta))
  (def seg2 (* (gte ton ta) (lt ton b2)))
  (def seg3 (* (gte ton b2) (lt ton b3)))
  (def seg4 (gte ton b3))
  (def von (+ (* seg1 (+ start_lvl (* (- l_atk start_lvl) (/ ton ta))))
              (* seg2 (+ l_atk (* (- l_brk l_atk) (/ (- ton ta) td))))
              (* seg3 (+ l_brk (* (- l_sus l_brk) (/ (- ton b2) ts))))
              (* seg4 l_sus)))
  ; freeze the on-phase value at note-off, then ramp to the release level
  (def toff (elapsed-since gate_s))
  (make-history frz_hist)
  (def frz_prev (read-history frz_hist))
  (def frz (gswitch (gt gate_s 0.5) von frz_prev))
  (write-history frz_hist frz)
  (def vrel (+ frz (* (- l_rel frz) (clip (/ toff tr) 0 1))))
  (def outv (gswitch (gt gate_s 0.5) von vrel))
  (write-history out_hist outv)
  outv)

; Triton-style LFO core: waveform select, optional key sync, fade-in.
(defmacro korg-lfo (wave rate fade_ms keysync trig_s)
  (def lreset (* (gte keysync 0.5) trig_s))
  (def lph (retrig-phasor rate lreset))
  (def lsh (latch (noise) (max (wrap-trigger lph) lreset)))
  (def lraw (selector (+ (clip (round wave) 0 4) 1)
              (triangle lph)
              (- 1 (* lph 2))
              (scale (lt lph 0.5) 0 1 1 -1)
              (sin (* lph twopi))
              lsh))
  (def lfade (gswitch (gt fade_ms 1.0)
                      (clip (/ (elapsed-since trig_s) (max 0.001 (* fade_ms 0.001)))
                            0 1)
                      1.0))
  (* lraw lfade))

; PCM oscillator core: smoothed wave-position scan (snaps at note start)
; into a 32x16 single-cycle ROM read. Phase is free-running — the old
; per-trigger phase reset was a click source once the amp EG retriggers
; from its current (nonzero) value.
; warp_v (0..1): Möbius phase warp before the table read; fold_v (0..1):
; triangle wavefold after it. Same math as core/wavetable, which the
; wavetable-viewer widget mirrors for its display.
(defmacro pcm-osc (bank_t set_idx wave_target freq warp_v fold_v trig)
  (make-history scan_hist)
  (def scan_prev (read-history scan_hist))
  (def scan_coeff (- 1.0 (exp (/ -1.0 (* 0.008 samplerate)))))
  (def scan (gswitch trig
              wave_target
              (+ scan_prev (* scan_coeff (- wave_target scan_prev)))))
  (write-history scan_hist scan)
  (def idx (+ (* (clip (floor set_idx) 0 31) 16) (clip scan 0 15)))
  (def phase_raw (phasor freq))
  (def k (+ 1 (* 6 (clip warp_v 0 1))))
  (def phase (/ (* k phase_raw) (+ 1 (* (- k 1) phase_raw))))
  (def raw (wavetable-read bank_t idx phase))
  (def foldg (+ 1 (* 6 (clip fold_v 0 1))))
  (- 1 (abs (- (wrap (+ (* raw foldg) 1) 0 4) 2))))

; ======================================================================
; section macros (one collapsed node each at the top level)
; ======================================================================

; Exponential glide toward the played pitch; bypasses when glide time ~0.
(defmacro glide-to (target)
  (param glide_ms @default 0 @min 0 @max 1000 @unit ms)
  (def coeff (exp (/ -1.0 (max 1.0 (* glide_ms 0.001 samplerate)))))
  (make-history gl_hist)
  (def prev (read-history gl_hist))
  (def glided (+ (* target (- 1 coeff)) (* prev coeff)))
  (def held (gswitch (gt glide_ms 0.1) glided target))
  (write-history gl_hist held)
  held)

; Key follow value: 0 at C4 (261.63 Hz), +/-1 per two octaves.
(defmacro key-follow (hz)
  (clip (* (/ (log (/ (max hz 8.0) 261.63)) (log 2)) 0.5) -1 1))

; Bipolar multi-stage filter EG (start level is always the current value).
(defmacro filter-eg (gate_s trig_s)
  (param feg_atk_lvl @default 1 @min -1 @max 1)
  (param feg_break @default 0.5 @min -1 @max 1)
  (param feg_sustain @default 0.3 @min -1 @max 1)
  (param feg_rel_lvl @default 0 @min -1 @max 1)
  (param feg_attack_ms @default 2 @min 1 @max 8000 @unit ms)
  (param feg_decay_ms @default 180 @min 1 @max 8000 @unit ms)
  (param feg_slope_ms @default 400 @min 1 @max 8000 @unit ms)
  (param feg_release_ms @default 200 @min 1 @max 8000 @unit ms)
  (korg-eg gate_s trig_s feg_attack_ms feg_decay_ms feg_slope_ms feg_release_ms
           feg_atk_lvl feg_break feg_sustain feg_rel_lvl))

; Unipolar multi-stage amp EG (attack to 1, release to 0).
(defmacro amp-eg (gate_s trig_s)
  (param aeg_break @default 1 @min 0 @max 1)
  (param aeg_sustain @default 0.85 @min 0 @max 1)
  (param aeg_attack_ms @default 2 @min 1 @max 8000 @unit ms)
  (param aeg_decay_ms @default 150 @min 1 @max 8000 @unit ms)
  (param aeg_slope_ms @default 350 @min 1 @max 8000 @unit ms)
  (param aeg_release_ms @default 180 @min 1 @max 8000 @unit ms)
  (clip (korg-eg gate_s trig_s aeg_attack_ms aeg_decay_ms aeg_slope_ms
                 aeg_release_ms 1.0 aeg_break aeg_sustain 0.0)
        0 1))

; Pitch EG in semitones: rise to full amount over attack, decay to 0.
(defmacro pitch-eg (trig_s)
  (param peg_amt_st @default 0 @min -24 @max 24 @unit st)
  (param peg_attack_ms @default 1 @min 1 @max 2000 @unit ms)
  (param peg_decay_ms @default 120 @min 5 @max 5000 @unit ms)
  (def t (elapsed-since trig_s))
  (def ta (max 0.0005 (* peg_attack_ms 0.001)))
  (def td (max 0.0005 (* peg_decay_ms 0.001)))
  (def env (gswitch (lt t ta)
                    (/ t ta)
                    (clip (- 1.0 (/ (- t ta) td)) 0 1)))
  (* env peg_amt_st))

; LFO1 with its routing depths. -> (lfo pitch_st cutoff_oct)
(defmacro lfo-one (trig_s)
  (param lfo1_wave @default 0 @min 0 @max 4)
  (param lfo1_rate_hz @default 5 @min 0.01 @max 30 @unit Hz)
  (param lfo1_fade_ms @default 0 @min 0 @max 5000 @unit ms)
  (param lfo1_keysync @default 1 @min 0 @max 1)
  (param lfo1_to_pitch @default 0 @min -200 @max 200 @unit cents)
  (param lfo1_to_cutoff @default 0 @min -4 @max 4 @unit oct)
  (def lfo (korg-lfo lfo1_wave lfo1_rate_hz lfo1_fade_ms lfo1_keysync trig_s))
  (tuple lfo
         (* lfo (/ lfo1_to_pitch 100.0))
         (* lfo lfo1_to_cutoff)))

; LFO2 with its routing depths. -> (lfo cutoff_oct tremolo_gain)
(defmacro lfo-two (trig_s)
  (param lfo2_wave @default 0 @min 0 @max 4)
  (param lfo2_rate_hz @default 0.8 @min 0.01 @max 30 @unit Hz)
  (param lfo2_fade_ms @default 0 @min 0 @max 5000 @unit ms)
  (param lfo2_keysync @default 0 @min 0 @max 1)
  (param lfo2_to_cutoff @default 0 @min -4 @max 4 @unit oct)
  (param lfo2_to_amp @default 0 @min 0 @max 1)
  (def lfo (korg-lfo lfo2_wave lfo2_rate_hz lfo2_fade_ms lfo2_keysync trig_s))
  (tuple lfo
         (* lfo lfo2_to_cutoff)
         (max 0.0 (+ 1.0 (* lfo lfo2_to_amp)))))

; AMS-lite 2-slot mod matrix (src: feg aeg lfo1 lfo2 key vel).
; -> (pitch wav1 wav2 cutoff res amp pan)
(defmacro ams-matrix (feg_s aeg_s l1 l2 ky vl)
  (param ams1_src @default 0 @min 0 @max 5)
  (param ams1_dest @default 0 @min 0 @max 6)
  (param ams1_amt @default 0 @min -1 @max 1)
  (param ams2_src @default 3 @min 0 @max 5)
  (param ams2_dest @default 6 @min 0 @max 6)
  (param ams2_amt @default 0 @min -1 @max 1)
  (def v1 (selector (+ (clip (round ams1_src) 0 5) 1) feg_s aeg_s l1 l2 ky vl))
  (def v2 (selector (+ (clip (round ams2_src) 0 5) 1) feg_s aeg_s l1 l2 ky vl))
  (tuple
    (+ (route-if-dest ams1_dest 0 ams1_amt v1) (route-if-dest ams2_dest 0 ams2_amt v2))
    (+ (route-if-dest ams1_dest 1 ams1_amt v1) (route-if-dest ams2_dest 1 ams2_amt v2))
    (+ (route-if-dest ams1_dest 2 ams1_amt v1) (route-if-dest ams2_dest 2 ams2_amt v2))
    (+ (route-if-dest ams1_dest 3 ams1_amt v1) (route-if-dest ams2_dest 3 ams2_amt v2))
    (+ (route-if-dest ams1_dest 4 ams1_amt v1) (route-if-dest ams2_dest 4 ams2_amt v2))
    (+ (route-if-dest ams1_dest 5 ams1_amt v1) (route-if-dest ams2_dest 5 ams2_amt v2))
    (+ (route-if-dest ams1_dest 6 ams1_amt v1) (route-if-dest ams2_dest 6 ams2_amt v2))))

; Common oscillator frequency: base pitch through pitch EG, AMS pitch, and
; LFO1 vibrato.
(defmacro pitch-mods (base_hz peg_st lfo1_pitch_st ams_pitch)
  (* base_hz (semi-ratio (+ peg_st (* ams_pitch 12) lfo1_pitch_st))))

; Oscillator 1: PCM set/wave with velocity wave switching.
(defmacro osc-one (bank_t common_hz wave_v warp_v fold_v vel ams_wav trig)
  (param osc1_set @default 0 @min 0 @max 31)
  (param osc1_octave @default 0 @min -2 @max 2 @unit oct)
  (param osc1_tune @default 0 @min -50 @max 50 @unit cents)
  (param osc1_vel_wave @default 0 @min -15 @max 15)
  (def target (clip (+ wave_v (* vel osc1_vel_wave) (* ams_wav 15)) 0 15))
  (def freq (* common_hz (semi-ratio (+ (* (clip (round osc1_octave) -2 2) 12)
                                        (/ osc1_tune 100.0)))))
  (pcm-osc bank_t osc1_set target freq warp_v fold_v trig))

; Oscillator 2: PCM set/wave with detune and velocity wave switching.
(defmacro osc-two (bank_t common_hz wave_v warp_v fold_v detune_v vel ams_wav trig)
  (param osc2_set @default 0 @min 0 @max 31)
  (param osc2_octave @default -1 @min -2 @max 2 @unit oct)
  (param osc2_vel_wave @default 0 @min -15 @max 15)
  (def target (clip (+ wave_v (* vel osc2_vel_wave) (* ams_wav 15)) 0 15))
  (def freq (* common_hz (semi-ratio (+ (* (clip (round osc2_octave) -2 2) 12)
                                        (clip detune_v -24 24)))))
  (pcm-osc bank_t osc2_set target freq warp_v fold_v trig))

; Mixer: dB gain staging with osc2 on/off.
(defmacro source-mixer (o1 o2 gain1_db gain2_db)
  (param osc2_on @default 0 @min 0 @max 1)
  (def g1 (db-amp (clip gain1_db -36 12)))
  (def g2 (* (gte osc2_on 0.5) (db-amp (clip gain2_db -36 12))))
  (+ (* o1 g1) (* o2 g2)))

; Filter block: shared drive stage into mode A (24dB resonant LP, drive-
; faded interstage clip) or mode B (12dB LP + 12dB HP). Cutoff is keytracked
; and modulated by the filter EG (with velocity intensity), both LFOs, and
; the AMS matrix. Linear at drive 0 — the drive knob owns the color.
(defmacro poseidon-filter (x base_hz cutoff_v res_v feg_s vel
                           lfo1_cut lfo2_cut ams_cut ams_res)
  (param filter_mode @default 0 @min 0 @max 1)
  (param hp_freq @default 20 @min 10 @max 8000 @unit Hz)
  (param keytrack @default 0.4 @min 0 @max 1)
  (param feg_int_oct @default 1.5 @min -8 @max 8 @unit oct)
  (param feg_vel_oct @default 0 @min -4 @max 4 @unit oct)
  (param drive @default 0.1 @min 0 @max 1)
  (def cut_oct_mod (+ (* feg_s (+ feg_int_oct (* feg_vel_oct vel)))
                      lfo1_cut lfo2_cut (* ams_cut 4)))
  (def cut (clip (* cutoff_v
                    (pow (/ (max base_hz 8.0) 261.63) keytrack)
                    (semi-ratio (* cut_oct_mod 12)))
                 20 16000))
  (def res (clip (+ res_v ams_res) 0 1))
  (def driven (drive-stage x drive))
  ; mode A: 24dB resonant LP — resonance on the first stage, interstage
  ; soft clip faded with drive so drive 0 stays linear
  (def lp_a1 (svf driven cut (+ 0.55 (* res 9.0)) 0))
  (def a_mid (mix lp_a1 (/ (tanh (* lp_a1 0.9)) 0.9) drive))
  (def lp_a (svf a_mid cut 0.707 0))
  ; mode B: 12dB LP + 12dB HP
  (def lp_b1 (svf driven cut (+ 0.55 (* res 6.0)) 0))
  (def lp_b (svf lp_b1 (clip hp_freq 10 8000) 0.6 2))
  (gswitch (gte filter_mode 0.5) lp_b lp_a))

; Amp EG, velocity, tremolo, AMS amp/pan, per-voice pan spread, volume,
; soft-limited stereo out. -> (left right)
(defmacro output-stage (filt aeg_s vel trem ams_amp ams_pan vol_db trig)
  (param vel_to_amp @default 0.4 @min 0 @max 1)
  (param spread @default 0.15 @min 0 @max 1)
  (param voice_pan @default 0 @min -1 @max 1)
  (def vel_gain (+ (- 1 vel_to_amp) (* vel vel_to_amp)))
  (def amp_scale (clip (+ 1.0 ams_amp) 0 2))
  (def vol (db-amp (clip vol_db -36 6)))
  (def amp (* filt aeg_s vel_gain trem amp_scale vol))
  (def rnd_pan (latch (noise) trig))
  (def pan (clip (+ voice_pan (* rnd_pan spread) (* ams_pan 0.5)) -1 1))
  (tuple (tanh (* amp (clip (- 1 (* pan 0.5)) 0 1.5)))
         (tanh (* amp (clip (+ 1 (* pan 0.5)) 0 1.5)))))

; ======================================================================
; voice: section nodes only
; ======================================================================

(def base_pitch (glide-to pitch))
(def key_val (key-follow base_pitch))

(def feg (filter-eg gate trigger))
(def aeg (amp-eg gate trigger))
(def peg_st (pitch-eg trigger))

(def (lfo1 lfo1_pitch_st lfo1_cut_oct) (lfo-one trigger))
(def (lfo2 lfo2_cut_oct trem) (lfo-two trigger))

(def (ams_pitch ams_wav1 ams_wav2 ams_cut ams_res ams_amp ams_pan)
     (ams-matrix feg aeg lfo1 lfo2 key_val velocity))

(def common_hz (pitch-mods base_pitch peg_st lfo1_pitch_st ams_pitch))

(def osc1 (osc-one bank common_hz (mod osc1_wave) (mod osc1_warp) (mod osc1_fold)
                   velocity ams_wav1 trigger))
(def osc2 (osc-two bank common_hz (mod osc2_wave) (mod osc2_warp) (mod osc2_fold)
                   (mod osc2_detune) velocity ams_wav2 trigger))

(def mixed (source-mixer osc1 osc2 (mod osc1_gain_db) (mod osc2_gain_db)))

(def filt (poseidon-filter mixed base_pitch (mod cutoff) (mod resonance)
                           feg velocity lfo1_cut_oct lfo2_cut_oct
                           ams_cut ams_res))

(def (left right) (output-stage filt aeg velocity trem ams_amp ams_pan
                                (mod volume_db) trigger))

(out left 1 @name left)
(out right 2 @name right)
