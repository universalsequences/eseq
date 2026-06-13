(def md-engine-options () '("TRX-CP" "TRX-CB" "TRX-CL" "TRX-MA" "EFM-CB" "PI-ML" "PI-MA"))
(def block-a ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "MD PERC" 4.8 (ui-accent-orange))
          (ui-lego-micro-option-s 0 "engine" "machine" 8.0 (md-engine-options) (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "ptch" "PTCH" 3.5 0 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "level" "LEV" 3.5 2 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "dec" "DEC" 3.9 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "tone" "TONE" 3.9 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "hard" "HARD" 3.9 (ui-accent-violet) 2)))))
(def block-clap ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "CLAP" 4.8 (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "rate" "RATE" 3.5 0 "ms" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "clpy" "CLPY" 3.5 0 false (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "rsiz" "RSIZ" 3.5 0 "ms" (ui-accent-green))
          (ui-lego-micro-num-s 0 "rtun" "RTUN" 3.5 0 "Hz" (ui-accent-green))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "rich" "RICH" 3.9 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "room" "ROOM" 3.9 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "enh" "ENH" 3.9 (ui-accent-orange) 2)))))
(def block-body ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "BODY" 4.8 (ui-accent-blue))
          (ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.5 0 "Hz" (ui-accent-blue))
          (ui-lego-micro-num-s 0 "mdec" "MDEC" 3.5 0 "ms" (ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "glen" "GLEN" 3.5 2 false (ui-accent-green))
          (ui-lego-micro-num-s 0 "grns" "GRNS" 3.5 2 false (ui-accent-green))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "bump" "BUMP" 3.9 (ui-accent-violet) 0)
        (ui-lego-knob-s 0 "mod_amt" "MOD" 3.9 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "rattl" "RATL" 3.9 (ui-accent-green) 2)))))
(def block-body-small ()
  (ui-readout-block-small-s "BODY" (ui-accent-blue) 0
    (h-stack :gap 0.22 :align :end
      (ui-lego-micro-num-s 0 "bump" "BUMP" 3.2 0 false (ui-accent-violet))
      (ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.4 0 "Hz" (ui-accent-blue))
      (ui-lego-micro-num-s 0 "mdec" "MDEC" 3.4 0 "ms" (ui-accent-blue))
      (ui-lego-micro-num-s 0 "rattl" "RATL" 3.2 2 false (ui-accent-green))
      (ui-lego-micro-num-s 0 "grns" "GRNS" 3.2 2 false (ui-accent-green)))))
(def block-fx ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "FX" 4.8 (ui-accent-green))
          (ui-lego-micro-num-s 0 "fltf" "FLTF" 3.5 0 "Hz" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "fltw" "FLTW" 3.5 0 "Hz" (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "eqf" "EQF" 3.5 0 "Hz" (ui-accent-green))
          (ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.5 2 false (ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "size" "SIZE" 3.9 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "tens" "TENS" 3.9 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "dist" "DIST" 3.9 (ui-accent-orange) 2)))))
(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column (block-a) (block-clap) (block-body-small))
    (ui-lego-column
      (block-body)
      (block-fx)
      (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (ui-lego-micro-base-note-s 0 4.0 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "dual" "DUAL" 3.2 2 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "rev" "REV" 3.2 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "fb" "FB" 3.2 2 false (ui-accent-blue)))))))
