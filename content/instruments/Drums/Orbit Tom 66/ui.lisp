;; Orbit Tom 66 — the identified E-mu Orbit-9090 tom with a p-lock surface.
;;
;; Left column: the departure knobs (all no-ops at their defaults, so the
;; instrument boots up AS the sample). Middle and right columns: the
;; identified scalars, grouped as the voice is built — SWEEP (carrier and
;; modulator pitch curves), BODY (envelope + output stage), SIDEBANDS (the
;; FM bank that makes this a tom and not a sine), TRANSIENT (click + noise).
;;
;; Field widths follow idvb23: ~0.6 cells per glyph at font-size 9.5, sized
;; from the widest string the parameter's range can print.

(def idtom66-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def idtom66-id-c () (eseq.effects.custom-ui-lego/ui-accent-blue))

;; Panel heights: a row of number fields is 1.64, the first row starts 1.0
;; below the panel's top edge.
(def idtom66-h2 () 4.90)   ; two rows of fields
(def idtom66-h3 () 6.60)   ; three rows of fields
(def idtom66-knob-h () 3.40)

(def idtom66-n (name title width decimals unit)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title width decimals unit (idtom66-id-c)))
(def idtom66-badge (title)
  (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 title 4.0 :fg))
(def idtom66-gap (width)
  (box :width width :height 1.64))
(def idtom66-knob (name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 4.6
    (idtom66-knob-h) (idtom66-knob-h) (idtom66-c) 2))

(def idtom66-panel (width height body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s width height 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start body)))

;; ---- column 1: the departure knobs ------------------------------------------

(def idtom66-play-panel ()
  (idtom66-panel 28.6 5.70
    (h-stack :width :fill :gap 0.15 :align :center
      (v-stack :width 4.6 :gap 0.18 :align :start
        (idtom66-badge "PLAY")
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.6 (idtom66-c)))
      (idtom66-knob "tune" "TUNE")
      (idtom66-knob "ratio" "RATIO")
      (idtom66-knob "sweep" "SWEEP")
      (idtom66-knob "drive" "DRIVE")
      (idtom66-knob "level" "LEVEL"))))

(def idtom66-shape-panel ()
  (idtom66-panel 28.6 (idtom66-h2)
    (h-stack :width :fill :gap 0.15 :align :center
      (v-stack :width 4.6 :gap 0.18 :align :start
        (idtom66-badge "SHAPE")
        (idtom66-gap 4.6))
      (idtom66-knob "attack" "ATTACK")
      (idtom66-knob "decay" "DECAY")
      (idtom66-knob "harm" "HARM")
      (idtom66-knob "bright" "BRIGHT")
      (idtom66-knob "noise" "NOISE"))))

;; ---- column 2: the pitch curves + the body ----------------------------------

;; CARRIER: two stacked exponentials fall onto END — a fast SNAP that HOLDs
;; for ~3 ms at ~2.9 kHz and then drops to 300 Hz in 5 ms, and a slower DROP
;; under it. MOD: the modulator is locked to the carrier at RATIO (an FM tom:
;; both operators ride one pitch envelope) plus its own small SNAP.
;; RATE is each fall, in 1/s.
(def idtom66-sweep-panel ()
  (idtom66-panel 34.0 (idtom66-h2)
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "CARRIER")
        (idtom66-n "c_end" "END" 5.4 1 "Hz")
        (idtom66-n "c_a1" "SNAP" 5.4 0 "Hz")
        (idtom66-n "c_r1" "RATE" 4.6 0 false)
        (idtom66-n "c_hold" "HOLD" 5.0 4 "s")
        (idtom66-n "c_a2" "DROP" 5.4 1 "Hz")
        (idtom66-n "c_r2" "RATE" 4.6 1 false))
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "MOD")
        (idtom66-n "m_ratio" "RATIO" 5.4 3 false)
        (idtom66-n "m_a1" "SNAP" 5.4 1 "Hz")
        (idtom66-n "m_r1" "RATE" 4.6 0 false)))))

;; BODY is the carrier's gain and the shape of its decay — FALL the linear
;; rate, BEND the curvature, ATK the attack time constant, FM ATK the rise
;; of the sidebands (the modulation index). OUT is the
;; gain-normalised saturator (SAT shapes, GAIN sets the level).
(def idtom66-body-panel ()
  (idtom66-panel 34.0 (idtom66-h2)
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "BODY")
        (idtom66-n "body_amp" "GAIN" 5.0 3 false)
        (idtom66-n "amp_decay" "FALL" 5.0 2 false)
        (idtom66-n "amp_curve" "BEND" 5.0 1 false)
        (idtom66-n "attack_time" "ATK" 5.4 4 "s")
        (idtom66-n "sb_attack" "FM ATK" 5.4 4 "s"))
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "OUT")
        (idtom66-n "out_drive" "SAT" 5.0 3 false)
        (idtom66-n "out_gain" "GAIN" 5.0 3 false)
        (idtom66-n "onset" "ONSET" 5.4 4 "s")))))

;; ---- column 3: the sideband bank + the transient layers ---------------------

;; The identified bank: partial -1 is the lower sideband (carrier − mod),
;; +1..+5 the upper ones (carrier + k × mod). Each has its LEVEL relative
;; to the carrier, an extra DECAY (1/s) on top of the body envelope, and a
;; PHASE offset (cycles). The tail is 68 / 155 / 242 / 329 / 415 / 502 Hz —
;; evenly spaced but not harmonic, the thing an 808 tom cannot do.
(def idtom66-sb (name title decimals)
  (idtom66-n name title 5.2 decimals false))

(def idtom66-sidebands-panel ()
  (idtom66-panel 39.0 (idtom66-h3)
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "LEVEL")
        (idtom66-sb "h0" "-1" 3) (idtom66-sb "h2" "+1" 3) (idtom66-sb "h3" "+2" 3)
        (idtom66-sb "h4" "+3" 3) (idtom66-sb "h5" "+4" 4) (idtom66-sb "h6" "+5" 4))
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "DECAY")
        (idtom66-sb "d0" "-1" 1) (idtom66-sb "d2" "+1" 1) (idtom66-sb "d3" "+2" 1)
        (idtom66-sb "d4" "+3" 1) (idtom66-sb "d5" "+4" 1) (idtom66-sb "d6" "+5" 1))
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "PHASE")
        (idtom66-sb "p0" "-1" 2) (idtom66-sb "p2" "+1" 2) (idtom66-sb "p3" "+2" 2)
        (idtom66-sb "p4" "+3" 2) (idtom66-sb "p5" "+4" 2) (idtom66-sb "p6" "+5" 2)))))

;; CLICK and NOISE carry whatever the sweep itself does not in the first
;; milliseconds (capped small: the onset IS the sweep).
(def idtom66-transient-panel ()
  (idtom66-panel 39.0 3.08
    (h-stack :gap 0.60 :align :start
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "CLICK")
        (idtom66-n "click_freq" "FREQ" 5.0 0 "Hz")
        (idtom66-n "click_amp" "LEVEL" 4.8 4 false)
        (idtom66-n "click_decay" "DECAY" 5.0 0 false))
      (h-stack :gap 0.18 :align :start
        (idtom66-badge "NOISE")
        (idtom66-n "noise_cutoff" "CUT" 5.0 0 "Hz")
        (idtom66-n "noise_amp" "LEVEL" 4.8 4 false)
        (idtom66-n "noise_decay" "DECAY" 5.0 0 false)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (v-stack :gap 0.10 :align :start
      (idtom66-play-panel)
      (idtom66-shape-panel))
    (v-stack :gap 0.10 :align :start
      (idtom66-sweep-panel)
      (idtom66-body-panel))
    (v-stack :gap 0.10 :align :start
      (idtom66-sidebands-panel)
      (idtom66-transient-panel))))
