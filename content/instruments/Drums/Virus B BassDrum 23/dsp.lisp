; Access Virus B BassDrum_23, identified from the sample-library hit by
; SynthID-style scalar optimisation (dgen Examples/SynthID/scripts/
; fit_virus_kick.py). At the defaults every hit reproduces the learned render;
; every knob is a departure from the identified sound. No sample,
; target-derived table, FIR, or residual is embedded.
;
; Source provenance: BassDrum_23.wav, tags Access / Access Virus - B,
; sha256 32cee493358b8dd6e60a5e82761c21b2f7e42445f9488b522bb805668d5579cf
; (native 32.5 kHz; the 44.1 kHz library duplicate is the same signal
; resampled).
;
; Voice: a two-exponential pitch sweep drives an additive bank of ten
; harmonics. Each harmonic above the fundamental has its own level and its
; own extra decay on top of the shared attack / log-quadratic body envelope
; (the ladder H2 ~ -25 dB, H3 ~ -40, H4 ~ -43, H7 ~ -47 is the sound's
; character; the v1-v3 sine-with-decoration voice missed it). A decaying sine
; click and a lowpassed noise burst carry the first 20 ms of non-harmonic
; transient; a slowly decaying high-passed hiss is the recording /
; machine texture (-70 dBFS, under the gate metric's floor, audible as the
; sample's air); gain-normalised tanh output.
;
; Added around the identified voice, all no-ops at their defaults except
; `tune` (see below):
;   - tune ships at -9 st, where this kick sits in a track; 0 is the render;
;   - one classic ADSR that IS the identified body envelope (attack = the
;     fit's rise, decay = its T60, sustain 0, release == decay), built the
;     way factory/id808 and factory/id909 build theirs;
;   - the BANK: the sc-filterbank (Sherman FB2) core with its cutoff on a
;     per-trigger decay envelope, keytrack + tube drive, reconstruction --
;     the same bank appended to those two kicks. Exact bypass at bank 0
;     (the cores keep running, so engaging it is click-free).

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
; The one default that is NOT the identified render: the instrument ships
; nine semitones down, where this kick sits in a track. Set tune 0 to hear
; the identified sound exactly.
(param tune @default -9 @min -24 @max 24 @unit st @mod true @mod-mode additive)
(param sweep @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
; The amp envelope, as a classic ADSR (see the envelope section below). These
; four ARE the identified body envelope, not an extra stage in front of it:
; attack is the fit's 83.8566 ms rise, decay the T60 of its log-quadratic
; curve, and release == decay + sustain 0 is exactly the identified one-shot
; for any gate length.
(param attack @default 83.8566 @min 0 @max 1000 @unit ms @mod true @mod-mode additive @mod-depth-min -500 @mod-depth-max 500 @mod-unit ms)
(param decay @default 396.5 @min 40 @max 8000 @unit ms @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit ms)
(param sustain @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param release @default 396.5 @min 40 @max 8000 @unit ms @mod true @mod-mode additive @mod-depth-min -2000 @mod-depth-max 2000 @mod-unit ms)
(param harm @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param bright @default 1 @min 0.5 @max 1.5 @mod true @mod-mode additive)
(param noise @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param hiss @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param drive @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive)

; ---- BANK: the Sherman FB2 filterbank, exact bypass at bank 0 ---------------
(param bank @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_env @default 0.31 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_freq @default 0.03 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_res @default 0.75 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

; Time constant for the bank's smoothed control signals. The identified
; scalars stay immediate; only the bank's own knobs are de-zippered.
(param smoothing @default 5 @min 0 @max 100 @unit ms)

; ---- the identified scalars (recovered_params.json), editable ----
(param f_end @default 48.326 @min 40 @max 60 @unit Hz)
(param sweep_a1 @default 1271.98 @min 400 @max 3000 @unit Hz)
(param sweep_r1 @default -138.921 @min -400 @max -60)
(param sweep_a2 @default 454.765 @min 50 @max 1500 @unit Hz)
(param sweep_r2 @default -61.4 @min -150 @max -15)
(param amp_decay @default -8.19556 @min -40 @max -0.5)
(param amp_curve @default -23.2674 @min -100 @max 0)
(param body_amp @default 0.64641 @min 0.2 @max 2.5)
(param h2 @default 0.0889135 @min 0.0001 @max 0.3)
(param h3 @default 0.0353647 @min 0.0001 @max 0.3)
(param h4 @default 0.0102246 @min 0.0001 @max 0.3)
(param h5 @default 0.00301132 @min 0.0001 @max 0.3)
(param h6 @default 0.00104442 @min 0.0001 @max 0.3)
(param h7 @default 0.00476622 @min 0.0001 @max 0.3)
(param h8 @default 0.00106912 @min 0.0001 @max 0.3)
(param h9 @default 0.000271162 @min 0.0001 @max 0.3)
(param h10 @default 0.00105181 @min 0.0001 @max 0.3)
(param d2 @default -74.5383 @min -80 @max 20)
(param d3 @default -80 @min -80 @max 20)
(param d4 @default -3.01448 @min -80 @max 20)
(param d5 @default 0.41521 @min -80 @max 20)
(param d6 @default -11.6544 @min -80 @max 20)
(param d7 @default -0.357143 @min -80 @max 20)
(param d8 @default -3.83257 @min -80 @max 20)
(param d9 @default -5.29906 @min -80 @max 20)
(param d10 @default 0 @min -80 @max 20)
(param click_freq @default 619.791 @min 300 @max 4000 @unit Hz)
(param click_amp @default 0 @min 0 @max 0.02)
(param click_decay @default -2817.67 @min -8000 @max -50)
(param noise_cutoff @default 2168.62 @min 300 @max 12000 @unit Hz)
(param noise_amp @default 0.0029441 @min 0 @max 0.05)
(param noise_decay @default -12.4284 @min -800 @max -5)
(param hiss_cutoff @default 5300.34 @min 2000 @max 12000 @unit Hz)
(param hiss_amp @default 0.000451151 @min 0 @max 0.01)
(param hiss_decay @default -6.57132 @min -60 @max -2)
(param out_drive @default 0.744227 @min 0.05 @max 4)
(param out_gain @default 2.24645 @min 0.05 @max 5)

(defmacro semi (st) (pow 2 (/ st 12)))
(defmacro bq-hz (hz) (* hz (/ 44100.0 samplerate)))

; History-based one-pole parameter smoother (from drums/synthid-909). Each
; expansion owns independent value and initialization history. The first
; sample adopts the current value directly, avoiding a startup ramp away
; from the identified defaults.
(defmacro onepole-param (input time_ms)
  (make-history value_h)
  (make-history initialized_h)
  (def previous (read-history value_h))
  (def initialized (read-history initialized_h))
  (def safe_seconds (* (max time_ms 0.001) 0.001))
  (def coefficient (exp (/ -1.0 (* samplerate safe_seconds))))
  (def filtered (+ (* (- 1.0 coefficient) input) (* coefficient previous)))
  (def initialized_value (gswitch (lt initialized 0.5) input filtered))
  (def output (gswitch (lt time_ms 0.001) input initialized_value))
  (write-history value_h output)
  (write-history initialized_h 1.0)
  output)

; Resettable exponential decay envelope (T60 in ms), value 1.0 on the
; trigger sample.
(defmacro id-env (trig decay_ms)
  (make-history e_h)
  (def coef (exp (/ -6.9077553 (max 1.0 (* decay_ms 0.001 samplerate)))))
  (def next (gswitch (gt trig 0.5) 1.0 (* (read-history e_h) coef)))
  (write-history e_h next)
  next)

; BANK: the sc-filterbank core (Sherman FB2, content/effects/sc-filterbank)
; with its cutoff riding a per-trigger decay envelope — the same bank
; factory/id808 and factory/id909 carry. Serial F1(LP) -> F2(~BP through
; the /4 clock
; divider), van der Pol resonance, VCO slew + charge-injection thump,
; tube drive, clock-tracking reconstruction, shared compressing output.
; Exact bypass at bank 0 (the cores keep running so engaging is click-free).
; ======================================================================

; One switched-cap SVF core: input sampled on the tick, Chamberlin update
; gated to the tick, states held (ZOH) between ticks. Biased tanh on the
; bp state injection-locks the scream; amplitude-dependent damping (van
; der Pol) gives a hard self-osc threshold. (From sc-filterbank.)
(defmacro bank-svf (sig tick morph gcoef kbase)
  (make-history lp_h)
  (make-history bp_h)
  (def xs (latch sig tick))
  (def keff (+ kbase (* 1.2 (* (read-history bp_h) (read-history bp_h)))))
  (def hp (- xs (+ (read-history lp_h) (* keff (read-history bp_h)))))
  (def bpn (* 1.078 (- (tanh (+ (+ (read-history bp_h) (* gcoef hp)) 0.28)) (tanh 0.28))))
  (def lpn (+ (read-history lp_h) (* gcoef bpn)))
  (write-history bp_h (mix (read-history bp_h) bpn tick))
  (write-history lp_h (mix (read-history lp_h) lpn tick))
  (def lpw (clip (- 1 (* 2 morph)) 0 1))
  (def hpw (clip (- (* 2 morph) 1) 0 1))
  (+ (* (read-history lp_h) lpw)
     (+ (* (read-history bp_h) (- 1 (+ lpw hpw)))
        (* hp hpw))))

(defmacro bank-stage (sig triggered wet_a env_a freq_a res_a note_in)
  ; Defaults are the exact settings the id808/id909 gesture was
  ; discovered with:
  ; freq 0.34 -> 0.03 (floor 0.03 + env 0.31), res 0.75, mode1 0.00,
  ; mode2 0.51, harm 5, crunch 0.00, ser 1.00, blend 0.50, drive 0.81.
  ; bank_freq (FLR) and bank_res (RES) are top-level @mod params, passed
  ; in as freq_a / res_a.
  (param bank_time @default 260 @min 20 @max 2000 @unit ms)
  (param bank_harm @default 5 @min 0 @max 7)
  (param bank_crunch @default 0 @min 0 @max 1)
  (param bank_drive @default 0.81 @min 0 @max 1)
  ; Keytrack MODE (default key): 1 shifts the whole sweep (floor, start,
  ; both resonances, and the clock — so the ZOH/aliasing artifacts too)
  ; with the note, in the log-cutoff domain, relative to the learned
  ; 48.33 Hz endpoint (f_end, this voice's identified sweep floor).
  ; At that reference pitch the two modes are identical. 0 = free
  ; (fixed frequencies). Follows tune; intermediates blend.
  (param bank_track @default 1 @min 0 @max 1)
  ; Reconstruction filter (the thing after the chip that Sherman barely
  ; has): two cascaded one-poles tracking the CLOCK at 0.35*fclk — above
  ; the passband (cutoff = fclk/ratio, tone untouched) but below the ZOH
  ; image bands, so it eats the staircase aliasing wherever the sweep
  ; sits. 0 = raw hardware grit, 1 = fully reconstructed (default).
  (param bank_recon @default 1 @min 0 @max 1)

  (def wet_amt (clip wet_a 0 1))
  (def bk_hitp (max (latch (max note_in 1.0) triggered) 1.0))
  ; No glide on this voice: the note is latched at the trigger, tune rides
  ; continuously (so a tune LFO sweeps the bank with the body).
  (def bk_note (* bk_hitp tune_ratio_s))
  (def bk_key_off (* (clip bank_track 0 1) (/ (log (/ bk_note 48.326)) 5.586)))
  ; input drive: the builtin Filterbank's drive circuit
  ; (effects/filterbank.rs §2) — dynamic-bias coupling-cap sag, +6 dB
  ; pre-emphasis @ 3 kHz, 0.55·tube + 0.45·diode asymmetric shaper (roar
  ; transfer bank), matched de-emphasis, 10 Hz DC blocker. The builtin's
  ; 4x oversampling is deliberately omitted: this bank aliases by design,
  ; and bank_recon is the cleanup control.
  (def gained (* sig (+ 1 (* bank_drive 24))))

  ; dynamic bias — a 2 ms / 80 ms follower
  ; of the driven signal shifts the operating point into the asymmetric
  ; curve, so transients bloom and sustained material sits down
  (make-history bk_biash)
  (def bmag (abs gained))
  (def bprev (read-history bk_biash))
  (def bcoef (gswitch (gt bmag bprev)
                      (- 1.0 (exp (/ -1.0 (* 0.002 samplerate))))
                      (- 1.0 (exp (/ -1.0 (* 0.080 samplerate))))))
  (def benv (+ bprev (* bcoef (- bmag bprev))))
  (write-history bk_biash benv)
  (def dbias (* 0.22 (tanh benv)))
  ; pre-emphasis: +6 dB above 3 kHz so the highs clip first
  (def ecoef (- 1.0 (exp (/ (* -2.0 pi 3000.0) samplerate))))
  (make-history bk_emph)
  (def emph_lp (+ (read-history bk_emph) (* ecoef (- gained (read-history bk_emph)))))
  (write-history bk_emph emph_lp)
  (def sh_in (+ gained (- gained emph_lp) dbias))
  ; 0.55 tube + 0.45 diode, unity small-signal slope (roar transfer bank)
  (def tube_u (max sh_in -2.4))
  (def sh_tube (tanh (+ tube_u (* 0.2 tube_u tube_u))))
  ; exp argument clamped at 0 so the unselected branch stays finite for
  ; negative inputs (gswitch evaluates both sides)
  (def dpos (gswitch (lt sh_in 0.35)
                     sh_in
                     (+ 0.35 (/ (- 1.0 (exp (* -3.0 (max (- sh_in 0.35) 0.0)))) 3.0))))
  (def sh_diode (gswitch (gte sh_in 0.0) dpos (* 1.2 (tanh (/ sh_in 1.2)))))
  (def shaped_drv (+ (* 0.55 sh_tube) (* 0.45 sh_diode)))
  ; matched de-emphasis (product ~ flat when clean), then 10 Hz DC block
  ; (the asymmetric curve + bias ride on an offset)
  (make-history bk_deemph)
  (def deemph_lp (+ (read-history bk_deemph) (* ecoef (- shaped_drv (read-history bk_deemph)))))
  (write-history bk_deemph deemph_lp)
  (def de_drv (- shaped_drv (* 0.5 (- shaped_drv deemph_lp))))
  (def dc_r (exp (/ (* -2.0 pi 10.0) samplerate)))
  (make-history bk_dcx)
  (make-history bk_dcy)
  (def dcy (+ (- de_drv (read-history bk_dcx)) (* dc_r (read-history bk_dcy))))
  (write-history bk_dcx de_drv)
  (write-history bk_dcy dcy)
  (def x dcy)
  ; input envelope (charge-injection bleed keying), ~10 ms follower
  (make-history bk_envh)
  (def bk_env (+ (read-history bk_envh) (* 0.003 (- (abs x) (read-history bk_envh)))))
  (write-history bk_envh bk_env)

  ; cutoff position: floor + per-trigger decay sweep (replaces the LFO)
  (def sweep_env (id-env triggered bank_time))
  (def fpos_target (clip (+ (clip freq_a 0 1) bk_key_off (* (clip env_a 0 1) sweep_env)) 0 1))
  ; VCO slew: the expo converter lags, asymmetrically (up faster than down)
  (make-history bk_fposh)
  (def fpos_diff (- fpos_target (read-history bk_fposh)))
  (def fpos (+ (read-history bk_fposh)
               (* (mix 0.0015 0.006 (> fpos_diff 0)) fpos_diff)))
  (write-history bk_fposh fpos)
  (def fc (* 30 (exp (* 5.586 fpos))))

  ; switched-cap clock: crunch morphs ratio 100:1 -> 25:1 (log)
  (def ratio (* 100 (exp (* bank_crunch (log 0.25)))))
  (def gcoef (* 2 (sin (/ pi ratio))))
  (def kbase (- (* 2.08 (- 1 (clip res_a 0 1))) 0.22))

  ; clock jitter, depth keyed to crunch
  (make-history bk_nzh)
  (def bk_nz (+ (read-history bk_nzh) (* 0.05 (- (noise) (read-history bk_nzh)))))
  (write-history bk_nzh bk_nz)
  (def fclk (clip (* (* fc ratio) (+ 1 (* (* 0.012 (+ 0.3 bank_crunch)) bk_nz)))
                  200 (* samplerate 0.99)))
  (def ph1 (phasor fclk))
  ; explicit wrap detector: ramp2trig misses wraps near the host rate
  (make-history bk_prevph)
  (def tick1 (< ph1 (read-history bk_prevph)))
  (write-history bk_prevph ph1)

  ; clock divider: F2's clock is F1's through the selected ratio
  ; (selector is 1-based; floor needs dgenlisp >= v0.1.6). The knob moves
  ; in 0.5 steps: halves land midway between adjacent tap ratios, which
  ; the subtract-N accumulator divides as happily as the named taps.
  (def harm_q (/ (round (* (clip bank_harm 0 7) 2)) 2))
  (def harm_i (floor harm_q))
  (def harm_f (- harm_q harm_i))
  (def div_a (selector (+ 1 harm_i) 1 1.2 1.5 2 3 4 5 7))
  (def div_b (selector (+ 1 (clip (+ harm_i 1) 0 7)) 1 1.2 1.5 2 3 4 5 7))
  (def divisor (mix div_a div_b harm_f))
  (make-history bk_divcnt)
  (def cnt (+ (read-history bk_divcnt) tick1))
  (def fire2 (>= cnt divisor))
  (write-history bk_divcnt (- cnt (* divisor fire2)))
  (def tick2 (* tick1 fire2))

  ; sweep thump: charge injection puts a moving DC offset into the loop
  (def thump (* 60 fpos_diff (mix 0.0015 0.006 (> fpos_diff 0))))
  (def xin (+ x thump))

  (def f1 (bank-svf xin tick1 0.0 gcoef kbase))
  ; serial: F1's resonance overdrives the stage feeding F2
  (def f2in (tanh (* 1.7 f1)))
  (def f2 (bank-svf f2in tick2 0.51 gcoef kbase))

  ; clock bleed as charge injection, rising as the clock falls audible.
  ; Deviation from the effect port: the hardware's constant 0.3 idle-bleed
  ; floor is removed — an instrument must go silent between hits, so the
  ; bleed is keyed entirely to the input envelope (1.9 keeps the same
  ; peak level the effect has at full program).
  (def bleed (* (* (* (* bank_crunch bank_crunch)
                      (* 0.02 (clip (- 1 (/ fclk 6000)) 0 1)))
                   (* 1.9 bk_env))
                (- (* 2 (< ph1 0.5)) 1)))

  ; shared output stage: envelope-coupled gain into ONE tanh (the scream
  ; eats headroom and the program ducks under it)
  (def pre (+ (* 0.5 (+ f1 f2)) bleed))
  (make-history bk_cmph)
  (def cmpa (abs pre))
  (write-history bk_cmph (+ (read-history bk_cmph)
                            (* (mix 0.0004 0.02 (> cmpa (read-history bk_cmph)))
                               (- cmpa (read-history bk_cmph)))))
  (def cmp (/ 1 (+ 1 (* 3.2 (read-history bk_cmph)))))
  (def wet (* 0.85 (tanh (* 1.7 (* pre cmp)))))

  ; clock-tracking reconstruction filter: two cascaded one-poles
  ; (12 dB/oct) at 0.35*fclk (see bank_recon above)
  (def rc_cut (clip (* fclk 0.35) 60 18000))
  (def rc_coef (exp (/ (* -2.0 pi rc_cut) samplerate)))
  (make-history bk_rch1)
  (def rc_1 (+ (* (- 1.0 rc_coef) wet) (* rc_coef (read-history bk_rch1))))
  (write-history bk_rch1 rc_1)
  (make-history bk_rch2)
  (def rc_2 (+ (* (- 1.0 rc_coef) rc_1) (* rc_coef (read-history bk_rch2))))
  (write-history bk_rch2 rc_2)
  (def wet_recon (mix wet rc_2 (clip bank_recon 0 1)))
  (mix sig wet_recon wet_amt))

; Exact seconds-since-trigger clock: t=0 on the trigger sample, then n/sr.
(make-history time-h)
(def previous-time (read-history time-h))
(def t (gswitch (gt trigger 0.5) 0.0 previous-time))
(write-history time-h (+ t (/ 1.0 samplerate)))

; The fit was rendered with the host pitch at C4 (261.63 Hz); this ratio
; keeps that render exact while making the complete voice playable.
(def pitch-ratio (* (/ pitch 261.63) (semi (mod tune))))
(def sweep-scale (/ 1.0 (clip (mod sweep) 0.25 4)))
(def r1 (* sweep_r1 sweep-scale))
(def r2 (* sweep_r2 sweep-scale))
(def sweep-phase
  (* pitch-ratio
     (+ (* f_end t)
        (* (/ sweep_a1 r1) (- (exp (* r1 t)) 1.0))
        (* (/ sweep_a2 r2) (- (exp (* r2 t)) 1.0)))))
(def phase-frac (- sweep-phase (floor sweep-phase)))

; ---- amp envelope: the identified curve, driven as a classic ADSR ----------
; Built the way factory/id808 and factory/id909 build theirs: ONE
; gate-following envelope IS the identified body envelope, rather than a
; second VCA bolted on after it. The fit's decay is log-quadratic rather than
; a plain exponential, so it is expressed as its per-sample ratio and run
; recursively -- which reproduces the fit exactly and leaves room for a
; sustain floor and a release segment:
;
;   env(t)/env(t-dt) = exp(dt * (amp_decay + amp_curve*(2t - dt)))
;
; `decay` is that curve's T60 in ms (396.5 ms = the fit, decay-scale 1) and
; `release` scales the same instantaneous rate by decay/release, so at the
; defaults (sustain 0, release == decay) the release coefficient IS the decay
; coefficient: the gate cannot change the identified one-shot at all, whatever
; its length, and lengthening release stretches the whole tail. Sustain up +
; a short release is gated bass, no mode switch.
;
; Only the body is enveloped. The click, noise burst and hiss carry their own
; identified decays and stay outside it, exactly as in id808.
(def dt (/ 1.0 samplerate))
(def attack-seconds (max (* (mod attack) 0.001) 0.0001))
(def attack-env
  (/ (- 1.0 (exp (/ (- t) attack-seconds)))
     (- 1.0 (exp (/ -0.05 attack-seconds)))))
(def decay-scale (/ 396.5 (clip (mod decay) 40 8000)))
(def release-scale (/ 396.5 (clip (mod release) 40 8000)))
(def env-rate (+ amp_decay (* amp_curve (- (* 2.0 t) dt))))
(def coef-d (exp (* dt (* decay-scale env-rate))))
(def coef-r (exp (* dt (* release-scale env-rate))))
(def sus (clip (mod sustain) 0 1))
(make-history env-h)
(def env-prev (read-history env-h))
(def dsr-env
  (gswitch (gt trigger 0.5)
           1.0
           (gswitch (gt gate 0.5)
                    (+ sus (* (- env-prev sus) coef-d))
                    (* env-prev coef-r))))
(write-history env-h dsr-env)
(def body-env (* attack-env dsr-env))

; Harmonic bank. harm scales every partial above the fundamental; bright
; tilts them (bright^(k-1)), both exact no-ops at 1.
(def harm-scale (clip (mod harm) 0 4))
(def tilt (clip (mod bright) 0.5 1.5))
(defmacro partial (k level rate)
  (* level (pow tilt (- k 1)) (exp (* rate t)) (sin (* 2.0 pi k phase-frac))))
(def bank
  (+ (sin (* 2.0 pi phase-frac))
     (* harm-scale
        (+ (partial 2 h2 d2)
           (partial 3 h3 d3)
           (partial 4 h4 d4)
           (partial 5 h5 d5)
           (partial 6 h6 d6)
           (partial 7 h7 d7)
           (partial 8 h8 d8)
           (partial 9 h9 d9)
           (partial 10 h10 d10)))))
(def body (* bank body-env body_amp))

(def click-voice
  (* (sin (* 2.0 pi click_freq pitch-ratio t))
     (exp (* click_decay t)) click_amp))
(def bipolar-noise (- (* (noise) 2.0) 1.0))
(def filtered-noise
  (biquad bipolar-noise (bq-hz noise_cutoff) 0.707 1.0 0))
(def noise-voice
  (* filtered-noise (exp (* noise_decay t)) noise_amp
     (clip (mod noise) 0 4)))

; Recording / machine hiss: high-passed noise with its own slow decay,
; through the Virus B's fixed 16.25 kHz output band (two 16 kHz lowpasses).
(def hiss-hp (biquad bipolar-noise (bq-hz hiss_cutoff) 0.707 1.0 1))
(def hiss-band
  (biquad (biquad hiss-hp (bq-hz 16000.0) 0.707 1.0 0) (bq-hz 16000.0) 0.707 1.0 0))
(def hiss-voice
  (* hiss-band (exp (* hiss_decay t)) hiss_amp (clip (mod hiss) 0 4)))

(def mixed (+ body click-voice noise-voice hiss-voice))

; Gain-normalised saturator: out_drive / drive set the shape only.
(def drive-amount (* out_drive (clip (mod drive) 0.25 4)))
(def shaped
  (* (/ (tanh (* mixed drive-amount)) drive-amount) out_gain))

; ---- the bank ---------------------------------------------------------------
; Smoothed bank controls + the keytrack reference note (the sounding sweep
; endpoint at this pitch, before tune, which rides continuously).
(def tune_ratio_s (onepole-param (semi (mod tune)) smoothing))
(def bank_s (onepole-param (mod bank) smoothing))
(def bank_env_s (onepole-param (mod bank_env) smoothing))
(def bank_freq_s (onepole-param (mod bank_freq) smoothing))
(def bank_res_s (onepole-param (mod bank_res) smoothing))
(def bank-note-in (* f_end (/ pitch 261.63)))

(def banked
  (bank-stage shaped trigger bank_s bank_env_s bank_freq_s bank_res_s bank-note-in))

(out (* banked (clip velocity 0 1) (clip (mod level) 0 1.5)) 1 @name audio)
