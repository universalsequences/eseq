;; Factory ID909 — the identified 909 with a p-lock surface. Layout follows
;; the synthid-808 lego style (shared with id808): PITCH/BODY/OUT column, CLICK+NOISE column,
;; BANK+TONE column.

(def id909-track-options ()
  '("free" "key"))

(def id909-pitch-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "ID909" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "sweep" "SWP" 3.8 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "glide" "GLIDE" 3.6 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body_asymmetry" "ASYM" 3.6 3 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
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
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fade" "FADE" 3.8 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "retrigger_fade" "XFD" 3.4 1 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(def id909-click-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "CLICK" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "click_decay" "DEC" 4.2 0 "1/s" (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "click_freq" "FREQ" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "click_amp" "AMP" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

(def id909-noise-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "NOISE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "noise_decay" "DEC" 4.2 2 "1/s" (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "noise_cutoff" "CUT" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "noise_amp" "AMP" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 6)))))

(def id909-bank-panel ()
  (eseq.effects.custom-ui-lego/ui-readout-panel-medium-s 0
    (v-stack :width :fill :height :fill :gap 0.26 :align :stretch
      (h-stack :width :fill :gap 0.30 :align :center
        (v-stack :width 10.2 :gap 0.18 :align :start
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "BANK" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_time" "TIME" 3.8 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_freq" "FLR" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_res" "RES" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
        (h-stack :gap 0.10 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "bank" "AMT" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "bank_env" "ENV" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)))
      (h-stack :gap 0.24 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-step-s 0 "bank_harm" "HARM" 3.6 1 0.5 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_crunch" "CRN" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_drive" "DRV" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_recon" "SMTH" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "bank_track" "TRK" 4.8 (id909-track-options) :fg)))))

(def id909-tone-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 6.6 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TONE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
        (h-stack :gap 0.14 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amp_curve" "CURV" 3.2 1 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body_harmonic" "ODD" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 0 "lpf" "LPF" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "hpf" "HPF" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
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
