;; Factory ID909 — the identified 909 with a p-lock surface. Layout follows
;; the synthid-808 lego style (shared with id808): PITCH/BODY/OUT column, CLICK+NOISE column,
;; BANK+TONE column.

(def id909-track-options ()
  '("free" "key"))

(def id909-pitch-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :width 0.5)
      (v-stack :width 9.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "sweep" "SWEEP" 5.5 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "glide" "GLIDE" 4.6 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body_asymmetry" "ASYM" 4.6 3 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "tune" "TUNE" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "start_ratio" "RATIO" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 3)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "body_amp" "AMP" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 3)))))

(def id909-env-block ()
  (eseq.effects.custom-ui-lego/ui-detail-adsr-s 0 "AMP ENV" "attack" "decay" "sustain" "release"))

(def id909-output-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "IDENTIFIED OUTPUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "drive" "DRIVE" 3.6 3 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "out_gain" "GAIN" 3.6 3 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LVL" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fade" "FADE" 5.5 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "retrigger_fade" "XFD" 5.5 1 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def id909-click-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :height :fill :v-align :start :padding 0.5
      (v-stack :width 9.0 :gap 0.018 :align :start 
        (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "CLICK" 4.4 :fg)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "click_decay" "DEC" 6.0 0 "1/s" (eseq.effects.custom-ui-lego/ui-accent-blue)
          )))
      (h-stack :gap 0.10 :align :center
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "click_freq" "FREQ" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "click_amp" "AMP" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

(def id909-noise-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :height :fill :v-align :start :padding 0.5
        (v-stack :gap 0.18 :width 9.0 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "NOISE" 4.4 :fg)
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "noise_decay" "DEC" 6.0 2 "1/s" (eseq.effects.custom-ui-lego/ui-accent-blue)))
        )
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "noise_cutoff" "CUT" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "noise_amp" "AMP" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 6)))))

(def id909-bank-panel ()
  (eseq.effects.custom-ui-lego/ui-readout-panel-medium-s 0
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
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "bank_track" "TRK" 4.8 (id909-track-options) :fg)))))

(def id909-tone-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :height :fill :padding 0.5
      (v-stack :width 12 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TONE" 4.4 :fg)
        (h-stack :gap 0.14 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_curve" "CURV" 6.0 1 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body_harmonic" "ODD" 6.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))
        ))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 0 "lpf" "LPF" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "hpf" "HPF" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (id909-pitch-block)
      (id909-env-block))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (id909-click-block)
      (id909-noise-block)
      (id909-output-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (id909-tone-block)
      (id909-bank-panel))))
