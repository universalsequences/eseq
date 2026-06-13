(def md-engine-options () '("TRX-SD" "EFM-SD" "PI-SD" "TRX-RS" "EFM-RS" "PI-RS"))

(def block-a ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "MD SNARE" 4.7 (ui-accent-orange))
          (ui-lego-micro-option-s 0 "engine" "machine" 8.0 (md-engine-options) (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "humanize" "hmnz" 3.5 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "level" "lvl" 3.2 2 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "dec" "DEC" 3.9 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "snap" "SNAP" 3.9 (ui-accent-cyan) 2)))))
(def block-b ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "TRX" 4.7 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "benv" "BENV" 3.4 0 "ms" (ui-accent-violet))
          (ui-lego-micro-num-s 0 "tune" "TUNE" 3.4 2 false (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "noise" "NOIS" 3.4 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "ndec" "NDEC" 3.4 0 "ms" (ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "bump" "BUMP" 3.9 (ui-accent-violet) 0)
        (ui-lego-knob-s 0 "tone" "TONE" 3.9 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "clip" "CLIP" 3.9 (ui-accent-orange) 2)))))
(def block-c ()
  (ui-readout-block-small-s "EFM / PI" (ui-accent-blue) 0
    (h-stack :gap 0.22 :align :end
      (ui-lego-micro-num-s 0 "mod_amt" "MOD" 3.2 2 false (ui-accent-blue))
      (ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.4 0 "Hz" (ui-accent-blue))
      (ui-lego-micro-num-s 0 "mdec" "MDEC" 3.4 0 "ms" (ui-accent-blue))
      (ui-lego-micro-num-s 0 "rvol" "RVOL" 3.2 2 false (ui-accent-green))
      (ui-lego-micro-num-s 0 "ring" "RING" 3.2 2 false (ui-accent-violet)))))
(def block-fx ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "TRACK FX" 4.7 (ui-accent-green))
          (ui-lego-micro-num-s 0 "eqf" "EQF" 3.4 0 "Hz" (ui-accent-green))
          (ui-lego-micro-num-s 0 "eqg" "EQG" 3.4 1 "dB" (ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "fltq" "FLTQ" 3.4 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "amf" "AMF" 3.4 0 "Hz" (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "fltf" "FLTF" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "fltw" "FLTW" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "dist" "DIST" 3.9 (ui-accent-orange) 2)))))
(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column (block-a) (block-b) (block-c))
    (ui-lego-column
      (block-fx)
      (ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.8 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (ui-lego-badge-s 0 "WIRE" 4.7 (ui-accent-green))
              (ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (ui-accent-green))
              (ui-lego-micro-num-s 0 "hpf" "HPF" 3.4 0 "Hz" (ui-accent-cyan)))
            (h-stack :gap 0.18 :align :start
              (ui-lego-micro-num-s 0 "noise" "NOIS" 3.4 2 false (ui-accent-cyan))
              (ui-lego-micro-num-s 0 "ndec" "NDEC" 3.4 0 "ms" (ui-accent-cyan))))
          (h-stack :gap 0.10 :align :start
            (ui-lego-knob-s 0 "hard" "HARD" 3.9 (ui-accent-orange) 2)
            (ui-lego-knob-s 0 "tens" "TENS" 3.9 (ui-accent-green) 2)
            (ui-lego-knob-s 0 "rvol" "RVOL" 3.9 (ui-accent-green) 2))))
      (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (ui-lego-micro-base-note-s 0 4.0 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "level" "LEV" 3.2 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "srr" "SRR" 3.2 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "dist" "DIST" 3.2 2 false (ui-accent-orange)))))))
