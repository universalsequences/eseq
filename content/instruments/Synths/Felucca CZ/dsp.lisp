; Felucca CZ: a phase-distortion voice ported from the PHASE engine of the
; Felucca firmware for the FM-1 (firmware/src/eng_phase.c,
; https://github.com/hugelton/Felucca). Its waveforms are Leo Kuroshita's
; clean-room CZ-style oscillator from CrispyZebra
; (https://github.com/hugelton/CrispyZebra).
; Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments.
; GPL-3.0-only, the same licence as eseq.
;
; A -cos table read through a bent phase whose bend is DCW. WAVE2 alternates
; with WAVE every other cycle, as the CZ's paired waveforms do. A second line
; detuned by DTN cents is mixed in or ring-modulated, and SUB adds a sine an
; octave down. One ADSR is both the amplitude and, scaled by ENV, the DCW
; sweep; a velocity accent (above 110 of 127) opens DCW further with it.
;
; Changes from the firmware: the fixed-point maths are written in floats
; with the same break points; the envelope takes times in ms (the firmware's
; 0..127 values map to 10000^(v/127) ms); the phases reset when a note starts
; from silence rather than running free, so every note opens the same way.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def clock (in 5 @name clock))
(def mod1 (in 6 @name mod1 @modulator 1))
(def mod2 (in 7 @name mod2 @modulator 2))
(def mod3 (in 8 @name mod3 @modulator 3))
(def mod4 (in 9 @name mod4 @modulator 4))

(param wave @default 0 @min 0 @max 7)
(param wave2 @default 0 @min 0 @max 8)
(param dcw @default 0.47 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param env_amt @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param dtn @default 0 @min 0 @max 127 @unit cents @mod true @mod-mode additive @mod-depth-min -127 @mod-depth-max 127 @mod-unit cents)
(param line @default 0 @min 0 @max 1)
(param sub @default 0 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)
(param amp_attack_ms @default 1 @min 1 @max 10000 @unit ms)
(param amp_decay_ms @default 1000 @min 1 @max 10000 @unit ms)
(param amp_sustain @default 1 @min 0 @max 1)
(param amp_release_ms @default 70 @min 1 @max 10000 @unit ms)
(param gain @default 0.5 @min 0 @max 1 @mod true @mod-mode additive @mod-depth-min -1 @mod-depth-max 1)

; The firmware's ADSR: a linear attack, then decay and release that cover
; ~99 % of the way (4.6 time constants) in their time. A new note attacks
; from wherever the level is; the voice is silent below 2^-12.
(defmacro cz-adsr (gate_sig trig attack_ms decay_ms sustain release_ms)
  (make-history level_h)
  (make-history attacking_h)
  (make-history gate_h)
  (def prev (read-history level_h))
  (def held (gt gate_sig 0.5))
  (def onset (max trig (* held (lte (read-history gate_h) 0.5))))
  (def attacking (gswitch onset 1 (read-history attacking_h)))
  (def atk_step (/ 1000 (* (max attack_ms 1) samplerate)))
  (def dec_k (- 1 (exp (/ -4600 (* (max decay_ms 1) samplerate)))))
  (def rel_k (- 1 (exp (/ -4600 (* (max release_ms 1) samplerate)))))
  (def rising (min 1 (+ prev atk_step)))
  (def sustaining (+ prev (* (- sustain prev) dec_k)))
  (def released (* prev (- 1 rel_k)))
  (def next
    (gswitch held
      (gswitch attacking rising sustaining)
      (gswitch (lt released 0.000244) 0 released)))
  (write-history attacking_h (* held attacking (lt next 1)))
  (write-history gate_h gate_sig)
  (write-history level_h next))

; One phase-distortion sample of wave w (0..7) at phase ph (0..1) and bend d.
; Waves 0..4 read -cos through a bent phase; 5..7 are the resonant waves, a
; sine at (1 + 7 rez) times the pitch under a saw, triangle or trapezoid
; window, so the core restarts each cycle as the CZ's resonance does.
(defmacro cz-wave (w ph d)
  ; SAW: the first half cycle squeezed into [0, x)
  (def saw_x (+ 0.5 (* 0.5 d)))
  (def saw_pd (gswitch (lt ph saw_x)
    (/ (* 0.5 ph) saw_x)
    (+ 0.5 (/ (* 0.5 (- ph saw_x)) (max (- 1 saw_x) 0.000001)))))
  ; SQUARE: each half rises in x, then holds
  (def sqr_x (max (- 0.5 (* 0.47485 d)) 0.000001))
  (def sqr_pd (gswitch (lt ph sqr_x) (/ (* 0.5 ph) sqr_x)
    (gswitch (lt ph 0.5) 0.5
      (gswitch (lt ph (+ 0.5 sqr_x)) (+ 0.5 (/ (* 0.5 (- ph 0.5)) sqr_x)) 1))))
  ; PULSE: the whole cycle in x, then held at the bottom
  (def pls_x (max (- 1 (* 0.96874 d)) 0.000001))
  (def pls_pd (gswitch (lt ph pls_x) (/ ph pls_x) 1))
  ; DOUBLE SINE: two cycles, the first in x
  (def dsn_x (- 1 (* 0.75 d)))
  (def dsn_pd (gswitch (lt ph dsn_x) (/ ph dsn_x) (/ (- ph dsn_x) (max (- 1 dsn_x) 0.000001))))
  ; SAW-PULSE: a saw bend inside x, then held
  (def spl_x (- 1 (* 0.5 d)))
  (def spl_peak (max (* spl_x (+ 0.5 (* 0.45 d))) 0.000001))
  (def spl_pd (gswitch (gte ph spl_x) 1
    (gswitch (lt ph spl_peak) (/ (* 0.5 ph) spl_peak)
      (+ 0.5 (/ (* 0.5 (- ph spl_peak)) (max (- spl_x spl_peak) 0.000001))))))
  ; RESONANCE: the core's phase, and the window of each resonant wave
  (def rez_pd (wrap (* ph (+ 1 (* 7 0.67139 d))) 0 1))
  (def win_saw (- 1 ph))
  (def win_tri (gswitch (lt ph 0.5) (* 2 ph) (* 2 (- 1 ph))))
  (def win_trap (min 1 (min (* 4 ph) (* 4 (- 1 ph)))))
  (def wi (+ (clip w 0 7) 1))
  (def bent (selector wi saw_pd sqr_pd pls_pd dsn_pd spl_pd rez_pd rez_pd rez_pd))
  (def level (selector wi 1 1 1 1 1 (* 0.625 win_saw) (* 0.625 win_tri) (* 0.625 win_trap)))
  (* -1 (cos (* twopi bent)) level))

; A line: a phasor at half the pitch runs two cycles per period, so its upper
; half is every other cycle and plays WAVE2 there. The half-rate phase is
; also the sub octave's.
(defmacro cz-line (hz reset w1 w2 d)
  (def half (phasor (* hz 0.5) reset))
  (def ph (wrap (* half 2) 0 1))
  (def w (gswitch (gte half 0.5) w2 w1))
  (tuple (cz-wave w ph d) half))

(def wave1 (floor wave))
(def wave_alt (gswitch (gte wave2 0.5) (- (floor wave2) 1) wave1))

(def amp_env (cz-adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
; Accent: a velocity above 110 of 127 opens DCW by another 24/127 of the envelope.
(def accent (* (gt velocity 0.866) amp_env 0.189))
(def depth (clip (+ (mod dcw) (* amp_env (mod env_amt)) accent) 0 1))
; The classic range of the bend: full DCW is 56000/65536 of the way.
(def bend (* depth 0.8545))

; Restart the phases only for a note that starts from silence.
(make-history env_prev_h)
(def fresh (* trigger (lt (read-history env_prev_h) 0.000244)))
(write-history env_prev_h amp_env)

(def detune (clip (mod dtn) 0 127))
(def ring (gte line 0.5))
(def second_on (max ring (gt detune 0.001)))

(def (s1 half1) (cz-line pitch fresh wave1 wave_alt bend))
(def (s2 half2) (cz-line (* pitch (exp (* detune 0.000577623))) fresh wave1 wave_alt bend))
(def lines (gswitch second_on (gswitch ring (* s1 s2) (* 0.5 (+ s1 s2))) s1))
(def sub_wave (* (clip (mod sub) 0 1) 0.775 (sin (* twopi half1))))

; 0.25 sits the default level with the other factory synths.
(def voice_out (* (+ lines sub_wave) amp_env velocity (clip (mod gain) 0 1) 0.25))
(out voice_out 1 @name left)
(out voice_out 2 @name right)
(use-defmacro voice-amp)
(out (voice-amp amp_env voice_out voice_out) 3 @name amp @amp true)
