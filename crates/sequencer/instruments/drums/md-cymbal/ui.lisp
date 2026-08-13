(def md-engine-options () '("TRX-CY" "EFM-CY" "PI-RC" "PI-CC"))
(def md-engine-index () (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine"))) (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1))) (if (= e 2) 2 (if (= e 3) 3 (if (= e 4) 4 1))))))

(def core ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.7 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "MD CYMB" 4.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "machine" 8.0 (md-engine-options) (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 3.5 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEV" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dec" "DEC" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "size" "SIZE" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dist" "DIST" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(def engine-block ()
  (let ((e (md-engine-index)))
    (if (= e 1)
      (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.7 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRX-CY" 4.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ttun" "TTUN" 3.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "peak" "PEAK" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan)))
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "srr" "SRR" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
          (h-stack :gap 0.10 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "rich" "RICH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "top" "TOP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "peak" "PEAK" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))
      (if (= e 2)
        (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
          (h-stack :width :fill :height :fill :gap 0.30 :align :center
            (v-stack :width 10.7 :gap 0.18 :align :start
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "EFM-CY" 4.6 (eseq.effects.custom-ui-lego/ui-accent-blue))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MDEC" 3.6 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 3.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ptch" "PTCH" 3.4 0 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
            (h-stack :gap 0.10 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "mod_amt" "MOD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fb" "FB" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hpf" "HPF" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 0))))
        (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
          (h-stack :width :fill :height :fill :gap 0.30 :align :center
            (v-stack :width 10.7 :gap 0.18 :align :start
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 (if (= e 3) "PI-RC" "PI-CC") 4.6 (eseq.effects.custom-ui-lego/ui-accent-green))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hard" "HARD" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clsn" "CLSN" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEV" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
            (h-stack :gap 0.10 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ring" "RING" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "grab" "GRAB" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "hard" "HARD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))))))

(def engine-small ()
  (let ((e (md-engine-index)))
    (if (= e 1)
      (eseq.effects.custom-ui-lego/ui-readout-block-small-s "TRX DETAIL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "rich" "RICH" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "top" "TOP" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ttun" "TTUN" 3.5 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (if (= e 2)
        (eseq.effects.custom-ui-lego/ui-readout-block-small-s "EFM DETAIL" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MFRQ" 3.5 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MDEC" 3.5 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fb" "FB" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))))
        (eseq.effects.custom-ui-lego/ui-readout-block-small-s "PI DETAIL" (eseq.effects.custom-ui-lego/ui-accent-green) 0
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clsn" "CLSN" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ring" "RING" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "grab" "GRAB" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))))))))

(def fx-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.7 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "FILTER" 4.6 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fltq" "FLTQ" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "srr" "SRR" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "eqf" "EQF" 3.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "eqg" "EQG" 3.6 1 "dB" (eseq.effects.custom-ui-lego/ui-accent-green))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fltf" "FLTF" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "fltw" "FLTW" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dist" "DIST" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (core)
      (subtree :key (str "md-cymbal-engine-block-" (md-engine-index)) (engine-block))
      (subtree :key (str "md-cymbal-engine-small-" (md-engine-index)) (engine-small)))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (fx-block)
      (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.7 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRACK FX" 4.6 (eseq.effects.custom-ui-lego/ui-accent-green))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amd" "AMD" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amf" "AMF" 3.5 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-violet)))
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "HMNZ" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEV" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
          (h-stack :gap 0.10 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "size" "SIZE" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "grab" "GRAB" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "level" "LEV" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))
      (eseq.effects.custom-ui-lego/ui-readout-block-small-s "GLOBAL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEV" 3.3 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))))
