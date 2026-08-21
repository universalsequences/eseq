(def md-engine-options () '("TRX-SD" "EFM-SD" "PI-SD" "TRX-RS" "EFM-RS" "PI-RS"))

(def md-engine-index ()
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine")))
    (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1)))
      (if (= e 2) 2 (if (= e 3) 3 (if (= e 4) 4 (if (= e 5) 5 (if (= e 6) 6 1))))))))

(def md-core-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "MD SNARE" 4.7 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "machine" 8.0 (md-engine-options) (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 3.5 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "hmnz" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "lvl" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dec" "DEC" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "snap" "SNAP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)))))

(def md-snare-engine-block ()
  (let ((e (md-engine-index)))
    (if (= e 1)
      (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.8 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRX-SD" 4.7 (eseq.effects.custom-ui-lego/ui-accent-orange))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "benv" "BENV" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tune" "TUNE" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "noise" "NOIS" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ndec" "NDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))))
          (h-stack :gap 0.10 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bump" "BUMP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "TONE" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "clip" "CLIP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))
      (if (= e 2)
        (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
          (h-stack :width :fill :height :fill :gap 0.30 :align :center
            (v-stack :width 10.8 :gap 0.18 :align :start
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "EFM-SD" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ndec" "NDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))))
            (h-stack :gap 0.10 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mod_amt" "MOD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "noise" "NOIS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "clip" "CLIP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))
        (if (= e 3)
          (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
            (h-stack :width :fill :height :fill :gap 0.30 :align :center
              (v-stack :width 10.8 :gap 0.18 :align :start
                (h-stack :gap 0.18 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "PI-SD" 4.7 (eseq.effects.custom-ui-lego/ui-accent-green))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-green))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ring" "RING" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))
                (h-stack :gap 0.18 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clip" "CLIP" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
              (h-stack :gap 0.10 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hard" "HARD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tens" "TENS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rvol" "RVOL" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))
          (if (= e 4)
            (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
              (h-stack :width :fill :height :fill :gap 0.30 :align :center
                (v-stack :width 10.8 :gap 0.18 :align :start
                  (h-stack :gap 0.18 :align :start
                    (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRX-RS" 4.7 (eseq.effects.custom-ui-lego/ui-accent-orange))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clip" "CLIP" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
                  (h-stack :gap 0.18 :align :start
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tone" "TONE" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "snap" "SNAP" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
                (h-stack :gap 0.10 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dec" "DEC" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "clip" "CLIP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))
            (if (= e 5)
              (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
                (h-stack :width :fill :height :fill :gap 0.30 :align :center
                  (v-stack :width 10.8 :gap 0.18 :align :start
                    (h-stack :gap 0.18 :align :start
                      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "EFM-RS" 4.7 (eseq.effects.custom-ui-lego/ui-accent-blue))
                      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
                      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
                    (h-stack :gap 0.18 :align :start
                      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))
                      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
                  (h-stack :gap 0.10 :align :start
                    (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mod_amt" "MOD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
                    (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dec" "DEC" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
                    (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "clip" "CLIP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))
              (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
                (h-stack :width :fill :height :fill :gap 0.30 :align :center
                  (v-stack :width 10.8 :gap 0.18 :align :start
                    (h-stack :gap 0.18 :align :start
                      (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "PI-RS" 4.7 (eseq.effects.custom-ui-lego/ui-accent-green))
                      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-green))
                      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ring" "RING" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))
                    (h-stack :gap 0.18 :align :start
                      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clip" "CLIP" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
                      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
                  (h-stack :gap 0.10 :align :start
                    (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rvol" "RVOL" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
                    (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ring" "RING" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
                    (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "clip" "CLIP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))))))))

(def md-snare-engine-small ()
  (let ((e (md-engine-index)))
    (if (= e 1)
      (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TRX DETAIL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bump" "BUMP" 3.3 0 false (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "benv" "BENV" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tune" "TUNE" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (if (= e 2)
        (eseq.effects.custom-ui-lego/ui-readout-block-small-s "EFM DETAIL" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.5 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MDEC" 3.5 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 3.5 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))))
        (if (= e 3)
          (eseq.effects.custom-ui-lego/ui-readout-block-small-s "PI WIRE" (eseq.effects.custom-ui-lego/ui-accent-green) 0
            (h-stack :gap 0.24 :align :end
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rvol" "RVOL" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-green))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ring" "RING" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))))
          (eseq.effects.custom-ui-lego/ui-readout-block-small-s "RIM DETAIL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
            (h-stack :gap 0.24 :align :end
              (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 (eseq.effects.custom-ui-lego/ui-accent-orange))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dec" "DEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-orange))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))))))

(def md-fx-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.8 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRACK FX" 4.7 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "eqf" "EQF" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "eqg" "EQG" 3.4 1 "dB" (eseq.effects.custom-ui-lego/ui-accent-green)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fltq" "FLTQ" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amf" "AMF" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fltf" "FLTF" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fltw" "FLTW" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dist" "DIST" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(def md-global-small ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEV" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "srr" "SRR" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

)

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (md-core-block)
      (subtree :key (str "md-snare-engine-block-" (md-engine-index)) (md-snare-engine-block))
      (subtree :key (str "md-snare-engine-small-" (md-engine-index)) (md-snare-engine-small)))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (md-fx-block)
      (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.8 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "NOISE" 4.7 (eseq.effects.custom-ui-lego/ui-accent-cyan))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "noise" "NOIS" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ndec" "NDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan)))
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tone" "TONE" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-green))))
          (h-stack :gap 0.10 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "noise" "NOIS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tone" "TONE" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "clip" "CLIP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))
      (md-global-small))))
