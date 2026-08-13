(def md-engine-options () '("TRX-BD" "TRX-B2" "EFM-BD" "PI-BD" "GND-SN"))

(def md-engine-index ()
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine")))
    (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1)))
      (if (= e 2) 2 (if (= e 3) 3 (if (= e 4) 4 (if (= e 5) 5 1)))))))

(def md-core-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "MD KICK" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "machine" 7.8 (md-engine-options) (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 3.5 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "hmnz" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "lvl" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dec" "DEC" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ramp" "RAMP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)))))

(def md-kick-engine-block ()
  (let ((e (md-engine-index)))
    (if (= e 1)
      (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.6 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRX-BD" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clip" "CLIP" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "srr" "SRR" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
          (h-stack :gap 0.10 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "strt" "STRT" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "nois" "NOIS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "harm" "HARM" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2))))
      (if (= e 2)
        (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
          (h-stack :width :fill :height :fill :gap 0.30 :align :center
            (v-stack :width 10.6 :gap 0.18 :align :start
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRX-B2" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clip" "CLIP" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "nois" "NOIS" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
            (h-stack :gap 0.10 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hold" "HOLD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tick" "TICK" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dirt" "DIRT" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2))))
        (if (= e 3)
          (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
            (h-stack :width :fill :height :fill :gap 0.30 :align :center
              (v-stack :width 10.6 :gap 0.18 :align :start
                (h-stack :gap 0.18 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "EFM-BD" 4.4 (eseq.effects.custom-ui-lego/ui-accent-blue))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MDEC" 3.6 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
                (h-stack :gap 0.18 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clip" "CLIP" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
              (h-stack :gap 0.10 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mod_amt" "MOD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mfb" "MFB" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dist" "DIST" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))
          (if (= e 4)
            (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
              (h-stack :width :fill :height :fill :gap 0.30 :align :center
                (v-stack :width 10.6 :gap 0.18 :align :start
                  (h-stack :gap 0.18 :align :start
                    (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "PI-BD" 4.4 (eseq.effects.custom-ui-lego/ui-accent-green))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hamr" "HAMR" 3.4 1 "ms" (eseq.effects.custom-ui-lego/ui-accent-green))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clip" "CLIP" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
                  (h-stack :gap 0.18 :align :start
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
                (h-stack :gap 0.10 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hard" "HARD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tens" "TENS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "damp" "DAMP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))
            (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
              (h-stack :width :fill :height :fill :gap 0.30 :align :center
                (v-stack :width 10.6 :gap 0.18 :align :start
                  (h-stack :gap 0.18 :align :start
                    (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "GND-SN" 4.4 (eseq.effects.custom-ui-lego/ui-accent-cyan))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
                  (h-stack :gap 0.18 :align :start
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "srr" "SRR" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
                    (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEV" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
                (h-stack :gap 0.10 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dec" "DEC" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
                  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ramp" "RAMP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 0))))))))))

(def md-kick-engine-small ()
  (let ((e (md-engine-index)))
    (if (= e 1)
      (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TRX-BD DETAIL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "strt" "STRT" 3.2 0 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "harm" "HARM" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (if (= e 2)
        (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TRX-B2 DETAIL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tick" "TICK" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dirt" "DIRT" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
        (if (= e 3)
          (eseq.effects.custom-ui-lego/ui-readout-block-small-s "EFM DETAIL" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
            (h-stack :gap 0.24 :align :end
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.5 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MDEC" 3.5 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfb" "MFB" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))))
          (if (= e 4)
            (eseq.effects.custom-ui-lego/ui-readout-block-small-s "PI DETAIL" (eseq.effects.custom-ui-lego/ui-accent-green) 0
              (h-stack :gap 0.24 :align :end
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hamr" "HAMR" 3.4 1 "ms" (eseq.effects.custom-ui-lego/ui-accent-green))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tens" "TENS" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "damp" "DAMP" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))))
            (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GND DETAIL" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
              (h-stack :gap 0.24 :align :end
                (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 (eseq.effects.custom-ui-lego/ui-accent-orange))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ramp" "RAMP" 3.3 0 false (eseq.effects.custom-ui-lego/ui-accent-violet))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rdec" "RDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet)))))))))

(def md-fx-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRACK FX" 4.4 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amd" "AMD" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amf" "AMF" 3.2 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "eqf" "EQF" 3.2 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "eqg" "EQG" 3.2 1 "dB" (eseq.effects.custom-ui-lego/ui-accent-green))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "clip" "CLIP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dist" "DIST" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "srr" "SRR" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

(def md-filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "FLT" 4.4 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fltq" "Q" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEV" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fltf" "FLTF" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fltw" "FLTW" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "level" "LEV" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

)

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (md-core-block)
      (subtree :key (str "md-kick-engine-block-" (md-engine-index)) (md-kick-engine-block))
      (subtree :key (str "md-kick-engine-small-" (md-engine-index)) (md-kick-engine-small)))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (md-fx-block)
      (md-filter-block)
      (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "srr" "SRR" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dist" "DIST" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))))
