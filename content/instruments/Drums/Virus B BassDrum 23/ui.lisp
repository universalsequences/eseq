;; Access Virus B BassDrum_23.
;;
;; Four columns. PLAY / SHAPE are the performance macros; AMP ENV and the FB2
;; bank are the blocks shared with factory/id808 and factory/id909; the middle
;; column is the identified voice — its harmonic ladder in ONE panel, and the
;; layers that sit under it.
;;
;; Naming follows id808: the badge on the left of a row carries the noun and
;; the fields carry the plain role, so a field reads FREQ / LEVEL / DECAY under
;; CLICK rather than CFREQ / CAMP / CDEC. Field widths are set from the widest
;; string the parameter's range can print (~0.6 cells per glyph at font-size
;; 9.5) so no number runs into its neighbour.

(def idvb23-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def idvb23-id-c () (eseq.effects.custom-ui-lego/ui-accent-blue))

;; Panel heights. The stock ui-detail-adsr-s / ui-readout-panel-medium-s boxes
;; are a fixed 5.58 that their own contents overflow by ~0.9, which is fine as
;; the last panel in a column but not in a grid; these panels are sized to
;; their content instead. A row of number fields measures 1.64, and a panel's
;; first row starts 1.0 below its top edge.
(def idvb23-h1 () 3.08)   ; one row of fields
(def idvb23-h2 () 4.90)   ; two rows of fields
(def idvb23-knob-h () 3.40)

(def idvb23-n (name title width decimals unit)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title width decimals unit (idvb23-id-c)))
(def idvb23-badge (title)
  (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 title 4.0 :fg))
(def idvb23-gap (width)
  (box :width width :height 1.64))
(def idvb23-knob (name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 4.6
    (idvb23-knob-h) (idvb23-knob-h) (idvb23-c) 2))

(def idvb23-panel (width height body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s width height 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start body)))

;; ---- column 1: the macro knobs ----------------------------------------------

(def idvb23-play-panel ()
  (idvb23-panel 24 5.70
    (h-stack :width :fill :gap 0.15 :align :center
      (v-stack :width 4.6 :gap 0.18 :align :start
        (idvb23-badge "PLAY")
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.6 (idvb23-c)))
      (idvb23-knob "tune" "TUNE")
      (idvb23-knob "sweep" "SWEEP")
      (idvb23-knob "drive" "DRIVE")
      (idvb23-knob "level" "LEVEL"))))

(def idvb23-shape-panel ()
  (idvb23-panel 24 4.90
    (h-stack :width :fill :gap 0.15 :align :center
      (v-stack :width 4.6 :gap 0.18 :align :start
        (idvb23-badge "SHAPE")
        (idvb23-gap 4.6))
      (idvb23-knob "harm" "HARM")
      (idvb23-knob "bright" "BRIGHT")
      (idvb23-knob "noise" "NOISE")
      (idvb23-knob "hiss" "AIR"))))

;; ---- column 2: AMP ENV + the pitch sweep ------------------------------------

;; ui-detail-adsr-s in a box sized to this grid: same editor widget, same
;; stage-linked readouts, the plot simply flexes into whatever height is left.
(def idvb23-env-panel (width height)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (box :width width :height height
         :background-color :instrument-control-bg
         :border-width 1 :corner-radius 12 :padding 0.18
         :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback 0)
      (v-stack :width :fill :height :fill :gap 0.18 :align :stretch
        (box :width :fill :height 0.34 :h-align :start :v-align :center
          (h-stack (box :width 0.5)
            (label "AMP ENV" :font-size 7.8 :color :dim :bg :transparent)))
        (adsr-editor
          :attack (eseq.effects.custom-ui-controls/ui-param-bound-value "attack" 5)
          :decay (eseq.effects.custom-ui-controls/ui-param-bound-value "decay" 120)
          :sustain (eseq.effects.custom-ui-controls/ui-param-bound-value "sustain" 0.7)
          :release (eseq.effects.custom-ui-controls/ui-param-bound-value "release" 120)
          :width :fill :height 2.40
          :background-color :instrument-control-bg
          :on-change (lambda (env)
            (do
              (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope 0)
              (eseq.effects.custom-ui-sections/custom-ui-set-active-adsr scope 0 (get env :active))
              (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope "attack" "decay" "sustain" "release" env))))
        (h-stack :width :fill :height 1.30 :gap 0.24 :align :start
          (box :width 1)
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-stage-s 0 :attack "attack" "atk" 5.1 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-stage-s 0 :decay "decay" "dec" 5.1 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-stage-s 0 :sustain "sustain" "sus" 5.1 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-stage-s 0 :release "release" "rel" 5.1 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan)))))))

;; Two stacked exponentials fall onto END: a fast SNAP over the first
;; milliseconds and a slower DROP under it. RATE is each fall, in 1/s.
(def idvb23-pitch-panel ()
  (idvb23-panel 24 (idvb23-h2)
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.18 :align :start
        (idvb23-badge "PITCH")
        (idvb23-n "f_end" "END" 5.4 2 "Hz")
        (idvb23-n "sweep_a1" "SNAP" 6.0 1 "Hz")
        (idvb23-n "sweep_r1" "RATE" 4.6 1 false))
      (h-stack :gap 0.18 :align :start
        (idvb23-gap 4.0)
        (idvb23-gap 5.4)
        (idvb23-n "sweep_a2" "DROP" 6.0 1 "Hz")
        (idvb23-n "sweep_r2" "RATE" 4.6 1 false)))))

;; ---- column 3: the identified voice -----------------------------------------

;; The identified ladder, one panel: partial 2..10 across, its LEVEL over its
;; extra DECAY (1/s) on top of the shared body envelope. H2 ~ -25 dB, H3 ~ -40,
;; H4 ~ -43, H7 ~ -47 is the character of this kick.
(def idvb23-h (name title)
  (idvb23-n name title 4.3 4 false))
(def idvb23-d (name title)
  (idvb23-n name title 4.3 1 false))

(def idvb23-harmonics-panel ()
  (idvb23-panel 46 5.70
    (v-stack :width :fill :gap 0.18 :align :start
      (box :width :fill :height 0.60 :h-align :start :v-align :center
        (h-stack (box :width 0.5)
          (label "HARMONIC LADDER — PARTIAL 2…10 OF THE SWEEP" :font-size 7.8 :color :dim :bg :transparent)))
      (h-stack :gap 0.18 :align :start
        (idvb23-badge "LEVEL")
        (idvb23-h "h2" "2")  (idvb23-h "h3" "3")  (idvb23-h "h4" "4")
        (idvb23-h "h5" "5")  (idvb23-h "h6" "6")  (idvb23-h "h7" "7")
        (idvb23-h "h8" "8")  (idvb23-h "h9" "9")  (idvb23-h "h10" "10"))
      (h-stack :gap 0.18 :align :start
        (idvb23-badge "DECAY")
        (idvb23-d "d2" "2")  (idvb23-d "d3" "3")  (idvb23-d "d4" "4")
        (idvb23-d "d5" "5")  (idvb23-d "d6" "6")  (idvb23-d "d7" "7")
        (idvb23-d "d8" "8")  (idvb23-d "d9" "9")  (idvb23-d "d10" "10")))))

;; The layers under the ladder. CLICK and NOISE carry the first 20 ms of
;; non-harmonic transient; AIR is the recording / machine hiss (-70 dBFS, the
;; sample's texture); BODY is the fundamental's gain and the shape of the decay
;; curve AMP ENV drives — FALL is its linear rate, BEND its curvature.
(def idvb23-layer-row (badge a-name a-title a-w a-dec a-unit
                             b-name b-title b-w b-dec
                             c-name c-title c-w c-dec)
  (h-stack :gap 0.18 :align :start
    (idvb23-badge badge)
    (idvb23-n a-name a-title a-w a-dec a-unit)
    (idvb23-n b-name b-title b-w b-dec false)
    (idvb23-n c-name c-title c-w c-dec false)))

(def idvb23-layers-panel ()
  (idvb23-panel 46 (idvb23-h2)
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.60 :align :start
        (idvb23-layer-row "CLICK" "click_freq" "FREQ" 5.0 0 "Hz"
                                  "click_amp" "LEVEL" 4.8 4
                                  "click_decay" "DECAY" 5.0 1)
        (idvb23-layer-row "NOISE" "noise_cutoff" "CUT" 5.0 0 "Hz"
                                  "noise_amp" "LEVEL" 4.8 4
                                  "noise_decay" "DECAY" 5.0 2))
      (h-stack :gap 0.60 :align :start
        (idvb23-layer-row "AIR" "hiss_cutoff" "CUT" 5.0 0 "Hz"
                                "hiss_amp" "LEVEL" 4.8 5
                                "hiss_decay" "DECAY" 5.0 1)
        (idvb23-layer-row "BODY" "body_amp" "GAIN" 5.0 3 false
                                 "amp_decay" "FALL" 4.8 2
                                 "amp_curve" "BEND" 5.0 2)))))

;; ---- column 4: the FB2 bank (the id808 / id909 panel) + output --------------

(def idvb23-track-options ()
  '("free" "key"))

(def idvb23-bank-panel ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s 24 6.60 0 :instrument-control-bg
    (v-stack :width :fill :height :fill :gap 0.06 :align :stretch
      (h-stack :width :fill :gap 0.30 :align :start
        (box :width 0.2)
        (v-stack :width 12.2 :gap 0.08 :align :start
          (box :height 0.2)
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "BANK" 4.4 :fg)
            (box :width 1.3)
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_time" "TIME" 6.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_freq" "FLTR" 6.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_res" "RES" 6.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
        (h-stack :gap 0.10 :align :start
          (box :width 0.5)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "bank" "AMT" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "bank_env" "ENV" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)))
      (h-stack :gap 0.24 :align :end
        (box :width 0.5)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-step-s 0 "bank_harm" "HARM" 4.2 1 0.5 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_crunch" "CRUSH" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_drive" "DRIVE" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_recon" "SMTH" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "bank_track" "TRK" 4.8 (idvb23-track-options) :fg)))))

;; SAT / GAIN are the identified saturator; SMOOTH is the control-smoothing
;; time constant behind every field on this page.
(def idvb23-out-panel ()
  (idvb23-panel 24 4.10
    (h-stack :gap 0.18 :align :start
      (idvb23-badge "OUT")
      (idvb23-n "out_drive" "SAT" 4.6 3 false)
      (idvb23-n "out_gain" "GAIN" 4.6 3 false)
      (idvb23-n "smoothing" "SMOOTH" 4.6 0 "ms"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (v-stack :gap 0.10 :align :start
      (idvb23-play-panel)
      (idvb23-shape-panel))
    (v-stack :gap 0.10 :align :start
      (idvb23-env-panel 24 5.70)
      (idvb23-pitch-panel))
    (v-stack :gap 0.10 :align :start
      (idvb23-harmonics-panel)
      (idvb23-layers-panel))
    (v-stack :gap 0.10 :align :start
      (idvb23-bank-panel)
      (idvb23-out-panel))))
