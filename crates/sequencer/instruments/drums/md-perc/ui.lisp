(def md-engine-options () '("TRX-CP" "TRX-CB" "TRX-CL" "TRX-MA" "EFM-CB" "PI-ML" "PI-MA"))
(def md-engine-index () (let ((p (custom-ui-current-param "engine"))) (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (custom-ui-param-value p)))) 1))) (if (= e 2) 2 (if (= e 3) 3 (if (= e 4) 4 (if (= e 5) 5 (if (= e 6) 6 (if (= e 7) 7 1)))))))))

(def core ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "MD PERC" 4.8 (ui-accent-orange))
          (ui-lego-micro-option-s 0 "engine" "machine" 8.0 (md-engine-options) (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-base-note-s 0 3.5 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "ptch" "PTCH" 3.5 0 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "level" "LEV" 3.5 2 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "dec" "DEC" 3.9 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "tone" "TONE" 3.9 (ui-accent-green) 2)
        (ui-lego-knob-s 0 "hard" "HARD" 3.9 (ui-accent-violet) 2)))))

(def engine-block ()
  (let ((e (md-engine-index)))
    (if (= e 1)
      (ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.8 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (ui-lego-badge-s 0 "TRX-CP" 4.8 (ui-accent-cyan))
              (ui-lego-micro-num-s 0 "rate" "RATE" 3.5 0 "ms" (ui-accent-cyan))
              (ui-lego-micro-num-s 0 "clpy" "CLPY" 3.5 0 false (ui-accent-cyan)))
            (h-stack :gap 0.18 :align :start
              (ui-lego-micro-num-s 0 "rsiz" "RSIZ" 3.5 0 "ms" (ui-accent-green))
              (ui-lego-micro-num-s 0 "rtun" "RTUN" 3.5 0 "Hz" (ui-accent-green))))
          (h-stack :gap 0.10 :align :start
            (ui-lego-knob-s 0 "rich" "RICH" 3.9 (ui-accent-cyan) 2)
            (ui-lego-knob-s 0 "room" "ROOM" 3.9 (ui-accent-green) 2)
            (ui-lego-knob-s 0 "enh" "ENH" 3.9 (ui-accent-orange) 2))))
      (if (= e 2)
        (ui-control-panel-dense-s 0
          (h-stack :width :fill :height :fill :gap 0.30 :align :center
            (v-stack :width 10.8 :gap 0.18 :align :start
              (h-stack :gap 0.18 :align :start
                (ui-lego-badge-s 0 "TRX-CB" 4.8 (ui-accent-orange))
                (ui-lego-micro-num-s 0 "enh" "ENH" 3.5 2 false (ui-accent-orange))
                (ui-lego-micro-num-s 0 "damp" "DAMP" 3.5 2 false (ui-accent-green)))
              (h-stack :gap 0.18 :align :start
                (ui-lego-micro-num-s 0 "bump" "BUMP" 3.5 0 false (ui-accent-violet))
                (ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.5 2 false (ui-accent-blue))))
            (h-stack :gap 0.10 :align :start
              (ui-lego-knob-s 0 "enh" "ENH" 3.9 (ui-accent-orange) 2)
              (ui-lego-knob-s 0 "bump" "BUMP" 3.9 (ui-accent-violet) 0)
              (ui-lego-knob-s 0 "damp" "DAMP" 3.9 (ui-accent-green) 2))))
        (if (= e 3)
          (ui-control-panel-dense-s 0
            (h-stack :width :fill :height :fill :gap 0.30 :align :center
              (v-stack :width 10.8 :gap 0.18 :align :start
                (h-stack :gap 0.18 :align :start
                  (ui-lego-badge-s 0 "TRX-CL" 4.8 (ui-accent-orange))
                  (ui-lego-micro-num-s 0 "dual" "DUAL" 3.5 2 false (ui-accent-cyan))
                  (ui-lego-micro-num-s 0 "clic" "CLIC" 3.5 2 false (ui-accent-cyan)))
                (h-stack :gap 0.18 :align :start
                  (ui-lego-micro-num-s 0 "damp" "DAMP" 3.5 2 false (ui-accent-green))
                  (ui-lego-micro-num-s 0 "bump" "BUMP" 3.5 0 false (ui-accent-violet))))
              (h-stack :gap 0.10 :align :start
                (ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (ui-accent-blue) 0)
                (ui-lego-knob-s 0 "dual" "DUAL" 3.9 (ui-accent-cyan) 2)
                (ui-lego-knob-s 0 "clic" "CLIC" 3.9 (ui-accent-cyan) 2))))
          (if (= e 4)
            (ui-control-panel-dense-s 0
              (h-stack :width :fill :height :fill :gap 0.30 :align :center
                (v-stack :width 10.8 :gap 0.18 :align :start
                  (h-stack :gap 0.18 :align :start
                    (ui-lego-badge-s 0 "TRX-MA" 4.8 (ui-accent-green))
                    (ui-lego-micro-num-s 0 "rattl" "RATL" 3.5 2 false (ui-accent-green))
                    (ui-lego-micro-num-s 0 "rev" "REV" 3.5 2 false (ui-accent-violet)))
                  (h-stack :gap 0.18 :align :start
                    (ui-lego-micro-num-s 0 "glen" "GLEN" 3.5 2 false (ui-accent-green))
                    (ui-lego-micro-num-s 0 "grns" "GRNS" 3.5 2 false (ui-accent-green))))
                (h-stack :gap 0.10 :align :start
                  (ui-lego-knob-s 0 "rattl" "RATL" 3.9 (ui-accent-green) 2)
                  (ui-lego-knob-s 0 "rev" "REV" 3.9 (ui-accent-violet) 2)
                  (ui-lego-knob-s 0 "hard" "HARD" 3.9 (ui-accent-violet) 2))))
            (if (= e 5)
              (ui-control-panel-dense-s 0
                (h-stack :width :fill :height :fill :gap 0.30 :align :center
                  (v-stack :width 10.8 :gap 0.18 :align :start
                    (h-stack :gap 0.18 :align :start
                      (ui-lego-badge-s 0 "EFM-CB" 4.8 (ui-accent-blue))
                      (ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.5 0 "Hz" (ui-accent-blue))
                      (ui-lego-micro-num-s 0 "mdec" "MDEC" 3.5 0 "ms" (ui-accent-blue)))
                    (h-stack :gap 0.18 :align :start
                      (ui-lego-micro-num-s 0 "fb" "FB" 3.5 2 false (ui-accent-blue))
                      (ui-lego-micro-num-s 0 "bump" "BUMP" 3.5 0 false (ui-accent-violet))))
                  (h-stack :gap 0.10 :align :start
                    (ui-lego-knob-s 0 "mod_amt" "MOD" 3.9 (ui-accent-blue) 2)
                    (ui-lego-knob-s 0 "fb" "FB" 3.9 (ui-accent-violet) 2)
                    (ui-lego-knob-s 0 "mfrq" "MFRQ" 3.9 (ui-accent-blue) 0))))
              (if (= e 6)
                (ui-control-panel-dense-s 0
                  (h-stack :width :fill :height :fill :gap 0.30 :align :center
                    (v-stack :width 10.8 :gap 0.18 :align :start
                      (h-stack :gap 0.18 :align :start
                        (ui-lego-badge-s 0 "PI-ML" 4.8 (ui-accent-green))
                        (ui-lego-micro-num-s 0 "size" "SIZE" 3.5 2 false (ui-accent-green))
                        (ui-lego-micro-num-s 0 "tens" "TENS" 3.5 2 false (ui-accent-green)))
                      (h-stack :gap 0.18 :align :start
                        (ui-lego-micro-num-s 0 "bump" "BUMP" 3.5 0 false (ui-accent-violet))
                        (ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.5 2 false (ui-accent-blue))))
                    (h-stack :gap 0.10 :align :start
                      (ui-lego-knob-s 0 "size" "SIZE" 3.9 (ui-accent-green) 2)
                      (ui-lego-knob-s 0 "tens" "TENS" 3.9 (ui-accent-green) 2)
                      (ui-lego-knob-s 0 "hard" "HARD" 3.9 (ui-accent-violet) 2))))
                (ui-control-panel-dense-s 0
                  (h-stack :width :fill :height :fill :gap 0.30 :align :center
                    (v-stack :width 10.8 :gap 0.18 :align :start
                      (h-stack :gap 0.18 :align :start
                        (ui-lego-badge-s 0 "PI-MA" 4.8 (ui-accent-green))
                        (ui-lego-micro-num-s 0 "grns" "GRNS" 3.5 2 false (ui-accent-green))
                        (ui-lego-micro-num-s 0 "glen" "GLEN" 3.5 2 false (ui-accent-green)))
                      (h-stack :gap 0.18 :align :start
                        (ui-lego-micro-num-s 0 "size" "SIZE" 3.5 2 false (ui-accent-green))
                        (ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.5 2 false (ui-accent-blue))))
                    (h-stack :gap 0.10 :align :start
                      (ui-lego-knob-s 0 "grns" "GRNS" 3.9 (ui-accent-green) 2)
                      (ui-lego-knob-s 0 "glen" "GLEN" 3.9 (ui-accent-green) 2)
                      (ui-lego-knob-s 0 "size" "SIZE" 3.9 (ui-accent-green) 2))))))))))))

(def engine-small ()
  (let ((e (md-engine-index)))
    (if (= e 1)
      (ui-readout-block-small-s "CLAP DETAIL" (ui-accent-cyan) 0
        (h-stack :gap 0.24 :align :end
          (ui-lego-micro-num-s 0 "clpy" "CLPY" 3.1 0 false (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "rate" "RATE" 3.4 0 "ms" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "room" "ROOM" 3.2 2 false (ui-accent-green))))
      (if (= e 5)
        (ui-readout-block-small-s "EFM DETAIL" (ui-accent-blue) 0
          (h-stack :gap 0.24 :align :end
            (ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.5 0 "Hz" (ui-accent-blue))
            (ui-lego-micro-num-s 0 "mdec" "MDEC" 3.5 0 "ms" (ui-accent-blue))
            (ui-lego-micro-num-s 0 "fb" "FB" 3.2 2 false (ui-accent-violet))))
        (ui-readout-block-small-s "DETAIL" (ui-accent-green) 0
          (h-stack :gap 0.24 :align :end
            (ui-lego-micro-num-s 0 "size" "SIZE" 3.2 2 false (ui-accent-green))
            (ui-lego-micro-num-s 0 "hard" "HARD" 3.2 2 false (ui-accent-violet))
            (ui-lego-micro-num-s 0 "dist" "DIST" 3.2 2 false (ui-accent-orange))))))))

(def fx-block ()
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
        (ui-lego-knob-s 0 "fltf" "FLTF" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "fltw" "FLTW" 3.9 (ui-accent-cyan) 0)
        (ui-lego-knob-s 0 "dist" "DIST" 3.9 (ui-accent-orange) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (core)
      (subtree :key (str "md-perc-engine-block-" (md-engine-index)) (engine-block))
      (subtree :key (str "md-perc-engine-small-" (md-engine-index)) (engine-small)))
    (ui-lego-column
      (fx-block)
      (ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.8 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (ui-lego-badge-s 0 "GLOBAL" 4.8 (ui-accent-orange))
              (ui-lego-micro-num-s 0 "amd" "AMD" 3.2 2 false (ui-accent-violet))
              (ui-lego-micro-num-s 0 "amf" "AMF" 3.2 0 "Hz" (ui-accent-violet)))
            (h-stack :gap 0.18 :align :start
              (ui-lego-micro-num-s 0 "eqg" "EQG" 3.2 1 "dB" (ui-accent-green))
              (ui-lego-micro-num-s 0 "srr" "SRR" 3.2 2 false (ui-accent-blue))))
          (h-stack :gap 0.10 :align :start
            (ui-lego-knob-s 0 "level" "LEV" 3.9 (ui-accent-orange) 2)
            (ui-lego-knob-s 0 "dist" "DIST" 3.9 (ui-accent-orange) 2)
            (ui-lego-knob-s 0 "srr" "SRR" 3.9 (ui-accent-blue) 2))))
      (ui-readout-block-small-s "GLOBAL" (ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (ui-lego-micro-base-note-s 0 4.0 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "level" "LEV" 3.2 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "dist" "DIST" 3.2 2 false (ui-accent-orange)))))))
