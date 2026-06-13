(def md-engine-options () '("TRX-BD" "TRX-B2" "EFM-BD" "PI-BD" "GND-SN"))

(def md-block-main ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "MD KICK" 4.4 (ui-accent-orange))
          (ui-lego-micro-option-s 0 "engine" "machine" 7.8 (md-engine-options) (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "humanize" "hmnz" 3.5 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "level" "lvl" 3.2 2 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "dec" "DEC" 3.9 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "ramp" "RAMP" 3.9 (ui-accent-violet) 0)))))

(def md-block-trx ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "TRX" 4.4 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (ui-accent-violet))
          (ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "tick" "TICK" 3.4 2 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "strt" "STRT" 3.9 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "nois" "NOIS" 3.9 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "harm" "HARM" 3.9 (ui-accent-violet) 2)))))

(def md-block-fm-pi ()
  (ui-readout-block-small-s "EFM / PI" (ui-accent-blue) 0
    (h-stack :gap 0.22 :align :end
      (ui-lego-micro-num-s 0 "mod_amt" "MOD" 3.2 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "mfb" "MFB" 3.0 2 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.4 0 "Hz" (ui-accent-blue))
      (ui-lego-micro-num-s 0 "mdec" "MDEC" 3.4 0 "ms" (ui-accent-blue))
      (ui-lego-micro-num-s 0 "tens" "TENS" 3.2 2 false (ui-accent-green)))))

(def md-block-fx ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "TRACK FX" 4.4 (ui-accent-green))
          (ui-lego-micro-num-s 0 "amd" "AMD" 3.2 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "amf" "AMF" 3.2 0 "Hz" (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "eqf" "EQF" 3.2 0 "Hz" (ui-accent-green))
          (ui-lego-micro-num-s 0 "eqg" "EQG" 3.2 1 "dB" (ui-accent-green))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "clip" "CLIP" 3.9 (ui-accent-orange) 2)
        (ui-lego-knob-s 0 "dist" "DIST" 3.9 (ui-accent-orange) 2)
        (ui-lego-knob-s 0 "srr" "SRR" 3.9 (ui-accent-blue) 2)))))

(def md-block-filter ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "FLT" 4.4 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "fltq" "Q" 3.2 2 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-base-note-s 0 4.2 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "dist" "DIST" 3.2 2 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "fltf" "FLTF" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "fltw" "FLTW" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "level" "LEV" 3.9 (ui-accent-orange) 2)))))

(def env-column ()
  (ui-lego-column-full
    (box :width (ui-lego-col-w) :height (ui-lego-full-h)
      (ui-lego-adsr-s 0 "BODY" "strt" "dec" "harm" "rdec"))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (md-block-main)
      (md-block-trx)
      (md-block-fm-pi))
    (ui-lego-column
      (md-block-fx)
      (md-block-filter)
      (ui-readout-block-small-s "GND / BODY" (ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (ui-lego-micro-base-note-s 0 4.0 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "hamr" "HAMR" 3.3 1 "ms" (ui-accent-green))
          (ui-lego-micro-num-s 0 "damp" "DAMP" 3.3 2 false (ui-accent-green))
          (ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.3 2 false (ui-accent-blue)))))
    (env-column)))
