(def md-engine-options () '("TRX-HH" "EFM-HH" "PI-HH"))
(def block-a ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "MD HAT" 4.4 (ui-accent-cyan))
          (ui-lego-micro-option-s 0 "engine" "machine" 7.6 (md-engine-options) (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "hpf" "HPF" 3.6 0 "Hz" (ui-accent-green))
          (ui-lego-micro-num-s 0 "lpf" "LPF" 3.6 0 "Hz" (ui-accent-green))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "gap" "GAP" 3.9 (ui-accent-orange) 2)
        (ui-lego-knob-s 0 "dec" "DEC" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "mtal" "MTAL" 3.9 (ui-accent-violet) 2)))))
(def block-b ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "EFM" 4.4 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.6 0 "Hz" (ui-accent-blue))
          (ui-lego-micro-num-s 0 "mdec" "MDEC" 3.6 0 "ms" (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "ptch" "PTCH" 3.6 0 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "tfrq" "TFRQ" 3.6 0 "Hz" (ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "mod_amt" "MOD" 3.9 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "fb" "FB" 3.9 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "trem" "TREM" 3.9 (ui-accent-violet) 2)))))
(def block-c ()
  (ui-readout-block-small-s "PI / FX" (ui-accent-green) 0
    (h-stack :gap 0.22 :align :end
      (ui-lego-micro-num-s 0 "clsn" "CLSN" 3.2 2 false (ui-accent-green))
      (ui-lego-micro-num-s 0 "ring" "RING" 3.2 2 false (ui-accent-green))
      (ui-lego-micro-num-s 0 "clos" "CLOS" 3.2 0 false (ui-accent-green))
      (ui-lego-micro-num-s 0 "dist" "DIST" 3.2 2 false (ui-accent-orange))
      (ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.2 2 false (ui-accent-blue)))))
(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column (block-a) (block-b) (block-c))
    (ui-lego-column
      (ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.6 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (ui-lego-badge-s 0 "FILTER" 4.4 (ui-accent-cyan))
              (ui-lego-micro-num-s 0 "fltq" "FLTQ" 3.4 2 false (ui-accent-cyan))
              (ui-lego-micro-num-s 0 "eqg" "EQG" 3.4 1 "dB" (ui-accent-green)))
            (h-stack :gap 0.18 :align :start
              (ui-lego-micro-num-s 0 "eqf" "EQF" 3.4 0 "Hz" (ui-accent-green))
              (ui-lego-micro-num-s 0 "amf" "AMF" 3.4 0 "Hz" (ui-accent-violet))))
          (h-stack :gap 0.10 :align :start
            (ui-lego-knob-s 0 "fltf" "FLTF" 3.9 (ui-accent-cyan) 0)
            (ui-lego-knob-s 0 "fltw" "FLTW" 3.9 (ui-accent-cyan) 0)
            (ui-lego-knob-s 0 "level" "LEV" 3.9 (ui-accent-orange) 2))))
      (ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.6 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (ui-lego-badge-s 0 "COLOR" 4.4 (ui-accent-violet))
              (ui-lego-micro-num-s 0 "ag" "AG" 3.1 2 false (ui-accent-green))
              (ui-lego-micro-num-s 0 "au" "AU" 3.1 2 false (ui-accent-green)))
            (h-stack :gap 0.18 :align :start
              (ui-lego-micro-num-s 0 "br" "BR" 3.1 2 false (ui-accent-green))
              (ui-lego-micro-num-s 0 "amd" "AMD" 3.1 2 false (ui-accent-violet))))
          (h-stack :gap 0.10 :align :start
            (ui-lego-knob-s 0 "clsn" "CLSN" 3.9 (ui-accent-green) 2)
            (ui-lego-knob-s 0 "ring" "RING" 3.9 (ui-accent-green) 2)
            (ui-lego-knob-s 0 "dist" "DIST" 3.9 (ui-accent-orange) 2))))
      (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (ui-lego-micro-base-note-s 0 4.0 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "srr" "SRR" 3.2 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "level" "LEV" 3.2 2 false (ui-accent-orange)))))))
