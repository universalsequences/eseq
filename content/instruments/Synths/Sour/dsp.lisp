; Sour: acid mono voice with a built-in filterbank.
;
; Distilled from a Digi Syn -> Filterbank chain (acdwn-style, Autechre):
;   VOICE  pulse/saw osc A + square osc B (an octave up by default), unison,
;          glide, the Digi Syn MS2-style 4-pole low-pass (drift-type2) into
;          its resonant high-pass, a decay-to-sustain filter envelope, and a
;          "wobble": a triangle LFO whose rate is a RATIO of the note
;          frequency, on cutoff.
;   BANK   the builtin Filterbank, per voice: tube/diode drive with dynamic
;          bias, two switched-capacitor SVFs (crunch = clock ratio 100:1 ->
;          25:1), audio-rate FM from the driven input, correction, serial <->
;          parallel blend, a NOTE-triggered bipolar envelope on both cutoffs,
;          equal-power dry/wet against the voice.
;   MATRIX four slots of voice-internal sources -> destinations.
;
; Defaults reproduce the base of the chain the sound came from; the big
; knobs (cutoff, hp, octave, wobble, bank freq/mode/mix) are the ones to
; p-lock per step. Accent = velocity above accent_thresh. Slide = legato
; note (track mono + legato): pitch glides, envelopes keep running.
;
; Param order is positional for projects: append new params at the end.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

; ---- oscillators ----
(param octave    @default 0    @min -3   @max 3 @unit oct @mod true @mod-mode additive)
(param a_level   @default -25  @min -60  @max 12 @unit dB @mod true @mod-mode additive)
(param b_level   @default -10  @min -60  @max 12 @unit dB @mod true @mod-mode additive)
(param b_oct     @default 1    @min -2   @max 2 @unit oct)
(param wave      @default 1    @min 0    @max 1 @mod true @mod-mode additive)
(param pw        @default 0.59 @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param unison    @default 0.6  @min 0    @max 1 @mod true @mod-mode additive)
(param detune    @default 12   @min 0    @max 50 @unit ct @mod true @mod-mode additive)
(param glide_ms  @default 22   @min 1    @max 600 @unit ms @mod true @mod-mode additive)
; ---- voice filter ----
(param cutoff    @default 120  @min 10   @max 4000 @unit Hz @mod true @mod-mode additive)
(param reso      @default 0.88 @min 0    @max 1 @mod true @mod-mode additive)
(param hp        @default 200  @min 10   @max 2000 @unit Hz @mod true @mod-mode additive)
(param keytrack  @default 0.55 @min 0    @max 1 @mod true @mod-mode additive)
(param env_mod   @default 4.5  @min 0    @max 8 @unit oct @mod true @mod-mode additive)
(param env_sus   @default 0.35 @min 0    @max 1 @mod true @mod-mode additive)
(param decay_ms  @default 640  @min 20   @max 3000 @unit ms @mod true @mod-mode additive)
(param wob_ratio @default 0.03 @min 0.002 @max 1 @mod true @mod-mode additive)
(param wob_depth @default 1.0  @min 0    @max 4 @unit oct @mod true @mod-mode additive)
; ---- filterbank ----
(param bank_mix    @default 0.66 @min 0   @max 1 @mod true @mod-mode additive)
(param bank_freq   @default 6000 @min 30  @max 12000 @unit Hz @mod true @mod-mode additive)
(param bank_mode   @default 0.28 @min 0   @max 1 @mod true @mod-mode additive)
(param bank_res    @default 80   @min 0   @max 110 @mod true @mod-mode additive)
(param bank_drive  @default 18   @min -6  @max 30 @unit dB @mod true @mod-mode additive)
(param bank_crunch @default 1    @min 0   @max 1 @mod true @mod-mode additive)
(param bank_fm     @default 1    @min 0   @max 1 @mod true @mod-mode additive)
(param bank_corr   @default 0.26 @min 0   @max 1 @mod true @mod-mode additive)
(param bank_par    @default 1    @min 0   @max 1 @mod true @mod-mode additive)
(param bank_env    @default 0.73 @min -1  @max 1 @mod true @mod-mode additive)
(param bank_decay  @default 432  @min 1   @max 4000 @unit ms @mod true @mod-mode additive)
(param bank_sus    @default 0.27 @min 0  @max 1 @mod true @mod-mode additive)
(param bank_rel    @default 300  @min 1   @max 4000 @unit ms @mod true @mod-mode additive)
; ---- amp / accent ----
(param amp_attack   @default 4    @min 0.5 @max 500 @unit ms @mod true @mod-mode additive)
(param amp_decay    @default 350  @min 5   @max 6000 @unit ms @mod true @mod-mode additive)
(param amp_sustain  @default 0.75 @min 0   @max 1 @mod true @mod-mode additive)
(param amp_release  @default 34   @min 1   @max 2000 @unit ms @mod true @mod-mode additive)
(param accent       @default 0.5  @min 0   @max 1 @mod true @mod-mode additive)
(param accent_sweep @default 0.5  @min 0   @max 1 @mod true @mod-mode additive)
(param accent_thresh @default 0.8 @min 0   @max 0.99)
(param volume_db    @default -16  @min -36 @max 6 @unit dB @mod true @mod-mode additive)
; ---- matrix source settings ----
(param jit_rate   @default 18  @min 0.5 @max 80 @unit Hz @mod true @mod-mode additive)
(param jit_smooth @default 0.5 @min 0   @max 1 @mod true @mod-mode additive)
(param count_len  @default 4   @min 2   @max 16)
; ---- matrix ----
; sources: 0 off 1 wobble 2 jitter 3 filter-env 4 bank-env 5 amp-env
;          6 accent-mem 7 glide 8 counter 9 follower 10 chaos 11 velocity
;          12 key 13 random
; dests:   0 off 1 cutoff 2 reso 3 hp 4 env-mod 5 bank-freq 6 bank-mode
;          7 bank-mix 8 bank-fm 9 crunch 10 pitch 11 osc-B-level 12 pw 13 amp
;          14 wobble-depth 15 bank2-freq
(param m1_src @default 0 @min 0 @max 13)
(param m1_dst @default 1 @min 0 @max 15)
(param m1_amt @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param m2_src @default 0 @min 0 @max 13)
(param m2_dst @default 5 @min 0 @max 15)
(param m2_amt @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param m3_src @default 0 @min 0 @max 13)
(param m3_dst @default 6 @min 0 @max 15)
(param m3_amt @default 0 @min -1 @max 1 @mod true @mod-mode additive)
(param m4_src @default 0 @min 0 @max 13)
(param m4_dst @default 3 @min 0 @max 15)
(param m4_amt @default 0 @min -1 @max 1 @mod true @mod-mode additive)
; ---- envelope stages added for the ADSR editors ----
(param env_attack  @default 0.5 @min 0.5 @max 2000 @unit ms @mod true @mod-mode additive)
(param env_release @default 34  @min 1   @max 4000 @unit ms @mod true @mod-mode additive)
(param bank_attack @default 0.5 @min 0.5 @max 2000 @unit ms @mod true @mod-mode additive)
; ---- filter 2 of the bank (bank_freq/bank_mode/bank_env are filter 1) ----
(param bank2_freq  @default 5400 @min 30  @max 12000 @unit Hz @mod true @mod-mode additive)
(param bank2_mode  @default 0.57 @min 0   @max 1 @mod true @mod-mode additive)
(param bank2_env   @default 0.5  @min -1  @max 1 @mod true @mod-mode additive)
; 0 free, else F2 = F1 / ratio (1 1.5 2 3 4 5 6 8 9 12 16), filterbank.rs
(param bank_harm   @default 0    @min 0   @max 11)
; what drives the bank FM: 0 drive (post-drive input, the filterbank's
; normalled source) 1 osc A 2 osc B 3 osc mix 4 filtered voice 5 wobble 6 noise
(param bank_fm_src @default 0    @min 0   @max 6)

(use-defmacro drift-filter-core)
(use-defmacro filterbank)


; ---- helpers ----
(defmacro sour_oct (o)
  (exp (* 0.6931471806 o)))

(defmacro sour_dbamp (db)
  (exp (* 0.1151292546 db)))

; Filterbank time_coef: one-pole reaching 1-1/e in ms.
(defmacro sour_coeff (ms)
  (- 1.0 (exp (/ -1.0 (max 0.01 (* ms 0.001 samplerate))))))

; Filterbank one_pole_coef: one-pole at freq Hz.
(defmacro sour_pole (hz)
  (- 1.0 (exp (/ (* -6.283185307 hz) samplerate))))

(defmacro sour_onset (sig)
  (make-history onset_prev)
  (def high (gt sig 0.5))
  (def rising (* high (lte (read-history onset_prev) 0.5)))
  (write-history onset_prev high)
  rising)

; Log-pitch glide; a triggered note jumps, a legato note slides.
(defmacro sour_glide (target_hz note_on gl_ms)
  (make-history glide_lp)
  (def target_lp (log (max target_hz 1.0)))
  (def prev_lp (read-history glide_lp))
  (def slid (+ prev_lp (* (sour_coeff (/ gl_ms 3.0)) (- target_lp prev_lp))))
  (def lp (gswitch (max note_on (lte prev_lp 0.0001)) target_lp slid))
  (write-history glide_lp lp)
  lp)



; Amp: linear attack from wherever it is, decay to sustain, release.
(defmacro sour_amp_env (gate_on note_on atk_ms dec_ms sus rel_ms)
  (make-history aenv_h)
  (make-history astage_h)
  (def prev (read-history aenv_h))
  (def attacking (* gate_on (max note_on (read-history astage_h))))
  (def a_lvl (min 1.0 (+ prev (/ 1.0 (max 1.0 (* atk_ms 0.001 samplerate))))))
  (def d_lvl (+ prev (* (sour_coeff (/ dec_ms 6.9)) (- sus prev))))
  (def r_lvl (* prev (- 1.0 (sour_coeff (/ rel_ms 6.9)))))
  (def lvl (gswitch gate_on (gswitch attacking a_lvl d_lvl) r_lvl))
  (write-history astage_h (* attacking (lt a_lvl 1.0)))
  (write-history aenv_h lvl)
  lvl)

; Voice filter envelope: linear attack from wherever it is, decay to
; sustain while the gate is held, release to 0 after. Legato notes do not
; retrigger. (Same shape as the amp envelope.)
(defmacro sour_fenv (gate_on note_on atk_ms dec_ms sus rel_ms)
  (sour_amp_env gate_on note_on atk_ms dec_ms sus rel_ms))

; Filterbank envelope (filterbank.rs §5) but note-triggered: attack chases
; 1.25 and clamps at 1, decays to a BIPOLAR sustain (param 0..1, 0.5 = 0,
; so the ADSR editor can draw it), releases to 0 on gate off.
(defmacro sour_bank_env (gate_on note_on atk_ms dec_ms sus rel_ms)
  (make-history benv_h)
  (make-history bstage_h)
  (def prev (read-history benv_h))
  (def attacking (* gate_on (max note_on (read-history bstage_h))))
  (def a_raw (+ prev (* (sour_coeff atk_ms) (- 1.25 prev))))
  (def a_done (gte a_raw 1.0))
  (def a_lvl (min a_raw 1.0))
  (def d_lvl (+ prev (* (sour_coeff dec_ms) (- sus prev))))
  (def r_lvl (* prev (- 1.0 (sour_coeff rel_ms))))
  (def lvl (gswitch gate_on (gswitch attacking a_lvl d_lvl) r_lvl))
  (write-history bstage_h (* attacking (- 1.0 a_done)))
  (write-history benv_h lvl)
  lvl)

; Accent memory: back-to-back accents stack (the 303 wow).
(defmacro sour_accent_mem (raw sweep)
  (make-history amem_h)
  (def prev (read-history amem_h))
  (def c (gswitch (gt raw prev) (sour_coeff (+ 6.0 (* sweep 30.0))) (sour_coeff (+ 35.0 (* sweep 260.0)))))
  (def m (+ prev (* c (- raw prev))))
  (write-history amem_h m)
  m)

; Wobble: triangle LFO at note_hz * ratio, phase reset on each note.
(defmacro sour_wobble (note_on hz)
  (make-history wph_h)
  (def ph (wrap (+ (read-history wph_h) (/ hz samplerate)) 0 1))
  (def p (gswitch note_on 0.0 ph))
  (write-history wph_h p)
  (triangle p))

; Jitter: sample-and-hold noise at rate, slewed by smooth * period.
(defmacro sour_jitter (note_on rate smooth)
  (make-history jph_h)
  (make-history jtgt_h)
  (make-history jout_h)
  (def ph (+ (read-history jph_h) (/ (clip rate 0.05 400) samplerate)))
  (def wrapped (gte ph 1.0))
  (write-history jph_h (- ph wrapped))
  (def tgt (gswitch (max wrapped note_on) (- (* 2.0 (noise)) 1.0) (read-history jtgt_h)))
  (write-history jtgt_h tgt)
  (def tau (* (clip smooth 0 1) 0.3 (/ 1000.0 (clip rate 0.05 400))))
  (def prev (read-history jout_h))
  (def out (gswitch (lte tau 0.05) tgt (+ prev (* (sour_coeff tau) (- tgt prev)))))
  (write-history jout_h out)
  out)

(defmacro sour_counter (note_on len)
  (make-history cnt_h)
  (def n (max 2 (round len)))
  (def c (gswitch note_on (% (+ (read-history cnt_h) 1) n) (read-history cnt_h)))
  (write-history cnt_h c)
  (/ c (- n 1)))

(defmacro sour_chaos (note_on)
  (make-history chaos_h)
  (def prev (read-history chaos_h))
  (def x (gswitch (lte prev 0.0001) 0.37 prev))
  (def nx (gswitch note_on (* 3.91 x (- 1.0 x)) x))
  (write-history chaos_h (clip nx 0.0001 0.9999))
  (clip nx 0.0001 0.9999))

(defmacro sour_follower (sig rel_ms)
  (make-history fol_h)
  (def prev (read-history fol_h))
  (def a (abs sig))
  (def f (+ prev (* (gswitch (gt a prev) (sour_coeff 1.0) (sour_coeff rel_ms)) (- a prev))))
  (write-history fol_h f)
  f)

; One oscillator pair at base frequency f: A = saw<->pulse, B = square
; b_oct octaves up; returns the raw (unscaled) pair.
(defmacro sour_osc_pair (f bmul wv pwv)
  (def fa (clip f 1 16000))
  (def fb (clip (* f bmul) 1 16000))
  (def pa (phasor fa))
  (def pb (phasor fb))
  (def oa (mix (polyblep_saw pa fa) (polyblep_pulse pa pwv fa) wv))
  (def ob (polyblep_pulse pb 0.5 fb))
  (tuple oa ob))

; ---- host-modulated reads ----
(def p_oct (clip (round (mod octave)) -3 3))
(def p_alvl (mod a_level))
(def p_blvl (mod b_level))
(def p_wave (clip (mod wave) 0 1))
(def p_pw (mod pw))
(def p_uni (clip (mod unison) 0 1))
(def p_det (clip (mod detune) 0 100))
(def p_glide (clip (mod glide_ms) 1 2000))
(def p_cutoff (clip (mod cutoff) 5 16000))
(def p_reso (mod reso))
(def p_hp (clip (mod hp) 5 16000))
(def p_key (mod keytrack))
(def p_envmod (mod env_mod))
(def p_envsus (clip (mod env_sus) 0 1))
(def p_decay (clip (mod decay_ms) 5 10000))
(def p_wratio (clip (mod wob_ratio) 0.0005 4))
(def p_wdepth (mod wob_depth))
(def p_bmix (mod bank_mix))
(def p_bfreq (clip (mod bank_freq) 10 20000))
(def p_b2freq (clip (mod bank2_freq) 10 20000))
(def p_b2mode (mod bank2_mode))
(def p_b2env (mod bank2_env))
(def p_bmode (mod bank_mode))
(def p_bres (mod bank_res))
(def p_bdrive (mod bank_drive))
(def p_bcrunch (mod bank_crunch))
(def p_bfm (mod bank_fm))
(def p_bcorr (clip (mod bank_corr) 0 1))
(def p_bpar (clip (mod bank_par) 0 1))
(def p_benv (mod bank_env))
(def p_bdecay (clip (mod bank_decay) 1 20000))
(def p_bsus (- (* 2.0 (clip (mod bank_sus) 0 1)) 1.0))
(def p_brel (clip (mod bank_rel) 1 20000))
(def p_eatk (clip (mod env_attack) 0.1 5000))
(def p_erel (clip (mod env_release) 1 5000))
(def p_batk (clip (mod bank_attack) 0.1 5000))
(def p_aatk (clip (mod amp_attack) 0.1 5000))
(def p_adec (clip (mod amp_decay) 5 20000))
(def p_asus (clip (mod amp_sustain) 0 1))
(def p_arel (clip (mod amp_release) 1 5000))
(def p_accent (clip (mod accent) 0 1))
(def p_sweep (clip (mod accent_sweep) 0 1))
(def p_vol (mod volume_db))
(def p_jrate (mod jit_rate))
(def p_jsmooth (mod jit_smooth))
(def a1 (mod m1_amt))
(def a2 (mod m2_amt))
(def a3 (mod m3_amt))
(def a4 (mod m4_amt))

; ---- note events + sources ----
(def gate_on (gt gate 0.5))
(def note_on (max (sour_onset trigger) (sour_onset gate)))
(def acc (latch (clip (/ (- velocity accent_thresh) (- 1.0 accent_thresh)) 0 1) note_on))
(make-history fol_out_h)

(def glide_lp (sour_glide pitch note_on p_glide))
(def base_hz (exp glide_lp))
(def glide_src (clip (* (abs (- glide_lp (log (max pitch 1.0)))) 8.6562) 0 1))
(def wob (sour_wobble note_on (clip (* base_hz p_wratio) 0.01 (* samplerate 0.45))))
(def jit (sour_jitter note_on p_jrate p_jsmooth))
(def fenv (sour_fenv gate_on note_on p_eatk p_decay p_envsus p_erel))
(def benv (sour_bank_env gate_on note_on p_batk p_bdecay p_bsus p_brel))
(def aenv (sour_amp_env gate_on note_on p_aatk p_adec p_asus p_arel))
(def amem (sour_accent_mem (* acc fenv) p_sweep))
(def counter (sour_counter note_on count_len))
(def chaos (sour_chaos note_on))
(def note_rand (latch (- (* 2.0 (noise)) 1.0) note_on))
(def keysrc (clip (/ (log (/ base_hz 130.81)) 2.7726) -1 1))
(def folsrc (clip (* 2.0 (read-history fol_out_h)) 0 1))

(defmacro sour_source (sel)
  (selector (+ (round sel) 1)
    0 wob jit fenv benv aenv amem glide_src counter folsrc chaos acc keysrc note_rand))

(defmacro sour_route (val dst k)
  (* val (eq (round dst) k)))

(def c1 (* (sour_source m1_src) a1))
(def c2 (* (sour_source m2_src) a2))
(def c3 (* (sour_source m3_src) a3))
(def c4 (* (sour_source m4_src) a4))

(defmacro sour_dest (k)
  (+ (sour_route c1 m1_dst k) (sour_route c2 m2_dst k)
     (sour_route c3 m3_dst k) (sour_route c4 m4_dst k)))

(def d_cutoff (sour_dest 1))
(def d_reso (sour_dest 2))
(def d_hp (sour_dest 3))
(def d_envmod (sour_dest 4))
(def d_bfreq (sour_dest 5))
(def d_bmode (sour_dest 6))
(def d_bmix (sour_dest 7))
(def d_bfm (sour_dest 8))
(def d_crunch (sour_dest 9))
(def d_pitch (sour_dest 10))
(def d_mix (sour_dest 11))
(def d_pw (sour_dest 12))
(def d_amp (sour_dest 13))
(def d_wdepth (sour_dest 14))
(def d_b2freq (sour_dest 15))

; ---- oscillators ----
(def f0 (* base_hz (sour_oct p_oct) (exp (* 0.05776227 (* 12.0 d_pitch)))))
(def bmul (sour_oct (clip (round b_oct) -2 2)))
; Digi Syn gain staging: levels go straight into the nonlinear filter, so
; hotter oscillators drive it harder (that is a sound control, not a mixer).
(def ga (sour_dbamp (clip p_alvl -120 12)))
(def gb (sour_dbamp (clip (+ p_blvl (* 24.0 d_mix)) -120 12)))
(def pwv (clip (+ p_pw (* 0.45 d_pw)) 0.05 0.95))
(def det_ratio (exp (* 0.0005776227 p_det)))
(def (oa_mid ob_mid) (sour_osc_pair f0 bmul p_wave pwv))
(def (oa_up ob_up) (sour_osc_pair (* f0 det_ratio) bmul p_wave pwv))
(def (oa_dn ob_dn) (sour_osc_pair (/ f0 det_ratio) bmul p_wave pwv))
(def uni_norm (/ 1.0 (+ 1.0 (* 0.8 p_uni))))
(def osc_a (* uni_norm (+ oa_mid (* 0.75 p_uni (+ oa_up oa_dn)))))
(def osc_b (* uni_norm (+ ob_mid (* 0.75 p_uni (+ ob_up ob_dn)))))
(def osc (* 0.4 (+ (* osc_a ga) (* osc_b gb))))

; ---- voice filter: Digi Syn 4-pole LP -> resonant HP ----
(def accent_cv (+ (* acc fenv) (* p_sweep 1.4 amem)))
(def cut_oct (+ (* fenv (clip (+ p_envmod (* 4.0 d_envmod)) 0 12))
                (* accent_cv p_accent 3.0)
                (* wob (clip (+ p_wdepth (* 4.0 d_wdepth)) 0 8))
                (* 4.0 d_cutoff)))
(def cut_hz (clip (* p_cutoff (pow (/ (max base_hz 8.0) 261.63) (clip p_key 0 1)) (sour_oct cut_oct))
                  10 (min 20000 (* samplerate 0.45))))
(def lp_out (drift-type2 osc cut_hz (clip (+ p_reso (* 0.5 d_reso)) 0 1)))
(def hp_hz (clip (* p_hp (sour_oct (* 4.0 d_hp))) 10 (min 20000 (* samplerate 0.45))))
(def voice (svf lp_out hp_hz 1.469 2))
(def synth (* voice aenv (+ 1.0 (* acc p_accent 0.6)) (clip (+ 1.0 d_amp) 0 2)))

; ---- filterbank (shared macro: content/defmacros/filterbank) ----
(def post (filterbank-drive synth p_bdrive))
; FM sidechain: each source scaled so full FM is a comparable +-4 oct swing
(def fm_src (selector (+ (clip (round bank_fm_src) 0 6) 1)
  post osc_a osc_b (* 4.0 osc) (* 6.0 synth) wob (- (* 2.0 (noise)) 1.0)))
(def wet (filterbank-core post fm_src
  (* p_bfreq (sour_oct (+ (* benv p_benv 4.0) (* 4.0 d_bfreq))))
  (* p_b2freq (sour_oct (+ (* benv p_b2env 4.0) (* 4.0 (+ d_bfreq d_b2freq)))))
  bank_harm
  (clip (+ p_bmode d_bmode) 0 1)
  (clip (+ p_b2mode d_bmode) 0 1)
  (+ p_bres (* 10.0 benv))
  (+ p_bcrunch d_crunch)
  (+ p_bfm d_bfm)
  p_bcorr p_bpar))
; the bank may only speak while the voice does (no idle self-oscillation)
(def life (clip (* 30.0 aenv) 0 1))
(def theta (* (clip (+ p_bmix d_bmix) 0 1) 1.570796327))
(def out_sig (* (sour_dbamp p_vol) 1.4 (+ (* synth (cos theta)) (* wet life (sin theta)))))
(write-history fol_out_h (sour_follower out_sig 40.0))

(out out_sig 1 @name left)
(out out_sig 2 @name right)
(use-defmacro voice-amp)

(out (voice-amp aenv out_sig out_sig) 3 @name amp @amp true)
