; Factory ID808 Clap — the identified Roland R-8 MkII '808Clap' sample,
; recovered by SynthID-style optimisation (dgen Examples/SynthID/scripts/
; fit_clap.py) rather than sampled: at the defaults below every hit reproduces
; the learned render; every knob is a departure from the identified sound.
;
; Voice: one noise stream -> two bandpasses (hand cup + slap) and a highpass
; -> four bursts on a measured flam (0 / 9.0 / 11.8 / 8.1 ms) each with a
; 1.7 ms sub-burst -> a two-stage tail (fast bright, slow dark) from the last
; burst -> tanh -> the R-8's 13 kHz output stage (fixed 12 kHz lowpass) and a
; fitted output highpass.

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
(param flam @default 1 @min 0.25 @max 4 @mod true @mod-mode additive)
(param snap @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param body @default 1 @min 0 @max 4 @mod true @mod-mode additive)
(param decay @default 1 @min 0.1 @max 6 @mod true @mod-mode additive)
(param bright @default 1 @min 0 @max 6 @mod true @mod-mode additive)
(param drive @default 1 @min 0.2 @max 6 @mod true @mod-mode additive)
(param level @default 1 @min 0 @max 1.5 @mod true @mod-mode additive)

; ---- the identified scalars (recovered_params.json), editable ----
(param fc1 @default 954.987 @min 300 @max 3000 @unit Hz)
(param q1 @default 1.82718 @min 0.5 @max 6)
(param fc2 @default 1822.78 @min 800 @max 6000 @unit Hz)
(param q2 @default 0.617773 @min 0.5 @max 6)
(param g2 @default 2.08088 @min 0 @max 4)
(param sp1 @default 8.96755 @min 4 @max 16 @unit ms)
(param sp2 @default 11.8699 @min 4 @max 16 @unit ms)
(param sp3 @default 8.48419 @min 4 @max 16 @unit ms)
(param bdecay @default -368.838 @min -1500 @max -100)
(param l2 @default 1.66352 @min 0 @max 3)
(param l3 @default 1.02267 @min 0 @max 1.5)
(param l4 @default 0 @min 0 @max 1.5)
(param sub_delay @default 1.521 @min 1.5 @max 6 @unit ms)
(param sub_gain @default 0.863983 @min 0 @max 2)
(param burst_amp @default 0.480209 @min 0.05 @max 20)
(param tail_a1 @default 0.569816 @min 0 @max 10)
(param tail_d1 @default -46.9733 @min -150 @max -10)
(param tail_a2 @default 0.478803 @min 0 @max 10)
(param tail_d2 @default -14.9908 @min -40 @max -4)
(param tail_lpf @default 12000 @min 500 @max 12000 @unit Hz)
(param hp_fc @default 500 @min 500 @max 10000 @unit Hz)
(param b_hp @default 0.00830026 @min 0 @max 1.5)
(param t_hp @default 0 @min 0 @max 1.5)
(param out_hp @default 655.089 @min 20 @max 2000 @unit Hz)
(param out_drive @default 2.31957 @min 0.5 @max 4)
(param out_gain @default 0.509693 @min 0.05 @max 2)


; Optional Sherman bank; dry clap remains the default.
(param bank @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_env @default 0.31 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_freq @default 0.03 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param bank_res @default 0.75 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param smoothing @default 5 @min 0 @max 100 @unit ms)

(defmacro semi (s) (pow 2 (/ s 12)))
; DGen's biquad hardcodes a 44.1 kHz frame in its coefficient math; scale the
; requested Hz so the filter lands on the physical frequency at any host rate.
(defmacro bq-hz (hz) (* hz (/ 44100.0 samplerate)))
; exponential segment starting at t0 seconds, zero before it
(defmacro seg (t t0 rate) (gswitch (lt t t0) 0.0 (exp (* rate (- t t0)))))

; incoming note tracks the filter set (C4 = the identified sound); tune offsets
; it in semitones. Clamped to +-2 octaves so extreme notes stay a clap.
(def note_ratio (clip (/ pitch 261.6256) 0.25 4.0))
(def tn (* note_ratio (semi (mod tune))))
(def vel (clip velocity 0 1))
; accum holds 0 for the trigger sample and the one after it, so it lags the
; fit's n/samplerate ramp by one sample; add the sample back (only sample 0
; differs, where every envelope is ~1 either way).
(def t (+ (accum (/ 1.0 samplerate) trigger 0 1000000) (/ 1.0 samplerate)))

; sources: one noise stream, filtered three ways
(def n (- (* (noise) 2.0) 1.0))
(def bp1 (biquad n (bq-hz (* fc1 tn)) q1 1.0 2))
(def bp2 (biquad n (bq-hz (* fc2 tn)) q2 1.0 2))
(def hpn (biquad n (bq-hz (* hp_fc tn)) 0.707 1.0 1))
(def src (+ bp1 (* g2 bp2)))
(def lp_src (biquad bp1 (bq-hz tail_lpf) 0.707 1.0 0))

; burst train: measured flam, each burst plus a short sub-burst
(def fl (clip (mod flam) 0.25 4))
(def t2 (* sp1 fl 0.001))
(def t3 (* (+ sp1 sp2) fl 0.001))
(def t4 (* (+ sp1 sp2 sp3) fl 0.001))
(def sd (* sub_delay 0.001))
(def env_b (+ (seg t 0.0 bdecay) (* sub_gain (seg t sd bdecay))
              (* l2 (+ (seg t t2 bdecay) (* sub_gain (seg t (+ t2 sd) bdecay))))
              (* l3 (+ (seg t t3 bdecay) (* sub_gain (seg t (+ t3 sd) bdecay))))
              (* l4 (+ (seg t t4 bdecay) (* sub_gain (seg t (+ t4 sd) bdecay))))))

; tail from the last burst: fast bright stage + slow dark stage
(def dk (/ 1.0 (clip (mod decay) 0.1 6)))
(def tail_fast (* tail_a1 (seg t t4 (* tail_d1 dk))))
(def tail_slow (* tail_a2 (seg t t4 (* tail_d2 dk))))

(def br (clip (mod bright) 0 6))
(def x (+ (* (+ src (* b_hp br hpn)) env_b burst_amp (clip (mod snap) 0 4))
          (* (+ src (* t_hp br hpn)) tail_fast (clip (mod body) 0 4))
          (* lp_src tail_slow (clip (mod body) 0 4))))
(def shaped (* (tanh (* x out_drive (clip (mod drive) 0.2 6))) out_gain))
; the R-8's output stage: nothing above its 13 kHz Nyquist leaves the machine
(def r8 (biquad (biquad shaped (bq-hz 12000) 0.707 1.0 0) (bq-hz 12000) 0.707 1.0 0))
(def voice (biquad r8 (bq-hz out_hp) 0.707 1.0 1))
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


; One clock event, with state passed explicitly so four sequential time
; steps can share one set of histories. F2 advances on divided F1 events.
(defmacro bank-clock-step (sig increment divisor gcoef kbase phase count lp1 bp1 xs1 lp2 bp2 xs2)
  (def phase_sum (+ phase increment))
  (def tick1 (gte phase_sum 1))
  (def phase_next (- phase_sum tick1))
  (def count_sum (+ count tick1))
  (def tick2 (* tick1 (gte count_sum divisor)))
  (def count_next (- count_sum (* divisor tick2)))
  (def x1 (mix xs1 sig tick1))
  (def k1 (+ kbase (* 1.2 bp1 bp1)))
  (def hp1 (- x1 (+ lp1 (* k1 bp1))))
  (def b1 (* 1.078 (- (tanh (+ bp1 (* gcoef hp1) 0.28)) (tanh 0.28))))
  (def l1 (+ lp1 (* gcoef b1)))
  (def x2 (mix xs2 (tanh (* 1.7 lp1)) tick2))
  (def k2 (+ kbase (* 1.2 bp2 bp2)))
  (def hp2 (- x2 (+ lp2 (* k2 bp2))))
  (def b2 (* 1.078 (- (tanh (+ bp2 (* gcoef hp2) 0.28)) (tanh 0.28))))
  (def l2 (+ lp2 (* gcoef b2)))
  (tuple phase_next count_next
         (mix lp1 l1 tick1) (mix bp1 b1 tick1) x1
         (mix lp2 l2 tick2) (mix bp2 b2 tick2) x2
         lp1 (+ (* 0.98 bp2) (* 0.02 hp2))))

(defmacro bank-stage (sig triggered wet_a env_a freq_a res_a note_in)
  ; Defaults are the exact settings the gesture was discovered with:
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
  ; with the note, in the log-cutoff domain, relative to the
  ; default 55 Hz fundamental. At that reference pitch the two modes are
  ; identical — so the discovered sound is unchanged there. 0 = free
  ; (fixed frequencies). Follows tune and glide; intermediates blend.
  (param bank_track @default 1 @min 0 @max 1)
  ; Reconstruction filter (the thing after the chip that Sherman barely
  ; has): a one-pole tracking the CLOCK at 0.35*fclk — above the passband
  ; (cutoff = fclk/ratio, tone untouched) but below the ZOH image bands,
  ; so it eats the staircase aliasing wherever the sweep sits. 0 = raw
  ; hardware grit, 1 = fully reconstructed (default).
  (param bank_recon @default 1 @min 0 @max 1)

  (def wet_amt (clip wet_a 0 1))
  ; note_in follows the clap filter tuning (clamped host note times Tune),
  ; so the bank and noise filters track together. Reference =
  ; C4 (261.6256 Hz), the identified clap reference: at that pitch key and
  ; free are identical.
  (def bk_note (max note_in 1.0))
  (def bk_key_off (* (clip bank_track 0 1) (/ (log (/ bk_note 261.6256)) 5.586)))
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
  (def bk_benv (+ bprev (* bcoef (- bmag bprev))))
  (write-history bk_biash bk_benv)
  (def dbias (* 0.22 (tanh bk_benv)))
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
  (def bk_dcy (+ (- de_drv (read-history bk_dcx)) (* dc_r (read-history bk_dcy))))
  (write-history bk_dcx de_drv)
  (write-history bk_dcy bk_dcy)
  (def x bk_dcy)
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
  (def fc (min (* 30 (exp (* 5.586 fpos))) (* 0.45 samplerate)))

  ; switched-cap clock: crunch morphs ratio 100:1 -> 25:1 (log)
  (def ratio (* 100 (exp (* bank_crunch (log 0.25)))))
  (def kbase (- (* 2.08 (- 1 (clip res_a 0 1))) 0.22))

  ; clock jitter, depth keyed to crunch
  (make-history bk_nzh)
  (def bk_nz (+ (read-history bk_nzh) (* 0.05 (- (noise) (read-history bk_nzh)))))
  (write-history bk_nzh bk_nz)
  ; Four integration steps per host sample keep the Chamberlin coefficient
  ; in its stable range at the top of the authored 30..8000 Hz cutoff range.
  ; If the requested chip clock exceeds that rate, preserve fc by deriving
  ; the coefficient from the actual clock. Crush still controls clock rate
  ; wherever it can be represented; it no longer imposes a cutoff ceiling.
  (def bank_rate (* 4 samplerate))
  (def clock_limit (* 0.99 bank_rate))
  (def requested_clock (* fc ratio))
  (def clock_jitter (+ 1 (* (* 0.012 (+ 0.3 bank_crunch)) bk_nz)))
  (def fclk (clip (* requested_clock clock_jitter) 200 clock_limit))
  (def represented_ratio (/ fclk (* fc clock_jitter)))
  (def gcoef (* 2 (sin (/ pi represented_ratio))))
  (def clock_increment (/ fclk bank_rate))

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
  ; sweep thump: charge injection puts a moving DC offset into the loop
  (def thump (* 60 fpos_diff (mix 0.0015 0.006 (> fpos_diff 0))))
  (def xin (+ x thump))

  ; Clock, held inputs and both filter states persist across host samples.
  (make-history bk_phase)
  (make-history bk_count)
  (make-history bk_lp1)
  (make-history bk_bp1)
  (make-history bk_xs1)
  (make-history bk_lp2)
  (make-history bk_bp2)
  (make-history bk_xs2)
  (def (phase_1 count_1 lp1_1 bp1_1 xs1_1 lp2_1 bp2_1 xs2_1 f1_1 f2_1)
    (bank-clock-step xin clock_increment divisor gcoef kbase (read-history bk_phase) (read-history bk_count) (read-history bk_lp1) (read-history bk_bp1) (read-history bk_xs1) (read-history bk_lp2) (read-history bk_bp2) (read-history bk_xs2)))
  (def (phase_2 count_2 lp1_2 bp1_2 xs1_2 lp2_2 bp2_2 xs2_2 f1_2 f2_2)
    (bank-clock-step xin clock_increment divisor gcoef kbase phase_1 count_1 lp1_1 bp1_1 xs1_1 lp2_1 bp2_1 xs2_1))
  (def (phase_3 count_3 lp1_3 bp1_3 xs1_3 lp2_3 bp2_3 xs2_3 f1_3 f2_3)
    (bank-clock-step xin clock_increment divisor gcoef kbase phase_2 count_2 lp1_2 bp1_2 xs1_2 lp2_2 bp2_2 xs2_2))
  (def (phase_4 count_4 lp1_4 bp1_4 xs1_4 lp2_4 bp2_4 xs2_4 f1_4 f2_4)
    (bank-clock-step xin clock_increment divisor gcoef kbase phase_3 count_3 lp1_3 bp1_3 xs1_3 lp2_3 bp2_3 xs2_3))
  (write-history bk_phase phase_4)
  (write-history bk_count count_4)
  (write-history bk_lp1 lp1_4)
  (write-history bk_bp1 bp1_4)
  (write-history bk_xs1 xs1_4)
  (write-history bk_lp2 lp2_4)
  (write-history bk_bp2 bp2_4)
  (write-history bk_xs2 xs2_4)
  (def f1 f1_4)
  (def f2 f2_4)
  (def ph1 phase_4)

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
  (def bk_pre (+ (* 0.5 (+ f1 f2)) bleed))
  (make-history bk_cmph)
  (def cmpa (abs bk_pre))
  (write-history bk_cmph (+ (read-history bk_cmph)
                            (* (mix 0.0004 0.02 (> cmpa (read-history bk_cmph)))
                               (- cmpa (read-history bk_cmph)))))
  (def cmp (/ 1 (+ 1 (* 3.2 (read-history bk_cmph)))))
  (def wet (* 0.85 (tanh (* 1.7 (* bk_pre cmp)))))

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


(def bank_s (onepole-param (mod bank) smoothing))
(def bank_env_s (onepole-param (mod bank_env) smoothing))
(def bank_freq_s (onepole-param (mod bank_freq) smoothing))
(def bank_res_s (onepole-param (mod bank_res) smoothing))
(def banked (bank-stage voice trigger bank_s bank_env_s bank_freq_s bank_res_s (* 261.6256 tn)))
(out (* banked vel (clip (mod level) 0 1.5)) 1 @name audio)
