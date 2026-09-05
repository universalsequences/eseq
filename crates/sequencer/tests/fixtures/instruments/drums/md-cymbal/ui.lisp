;; MD-CYMBAL — Machinedrum-style cymbal with four machines (TRX/EFM/PI-RC/PI-CC).
;; Layout follows the factory id808 lego style: monochrome accent, :fg badges,
;; full-height knobs. CORE + machine panel column, FILTER/COLOR/OUT column.

(def md-cymbal-engine-options () '("TRX-CY" "EFM-CY" "PI-RC" "PI-CC"))
(def md-cymbal-engine-index () (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine"))) (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1))) (if (= e 2) 2 (if (= e 3) 3 (if (= e 4) 4 1))))))

(def md-cymbal-core-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :height :fill :v-align :start :padding 0.5
        (v-stack :width 9.0 :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "MACHINE" 9.0 (md-cymbal-engine-options) :fg)
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "HUMAN" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "dec" "DEC" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "size" "SIZE" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "level" "LEVEL" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

;; Machine panel: same shape as the id808 BANK panel — badge + micro-nums top
;; left, two big knobs top right, a micro-num row along the bottom.
(def md-cymbal-machine-panel (badge head rows knob-a knob-b bottom)
  (eseq.effects.custom-ui-lego/ui-readout-panel-medium-s 0
    (v-stack :width :fill :height :fill :gap 0.06 :align :stretch
      (h-stack :width :fill :gap 0.30 :align :start
        (box :width 0.2)
        (v-stack :width 12.2 :gap 0.08 :align :start
          (box :height 0.2)
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 badge 4.4 :fg)
            (box :width 1.3)
            head)
          rows)
        (h-stack :gap 0.10 :align :start
          (box :width 0.5)
          knob-a
          knob-b))
      (h-stack :gap 0.24 :align :end
        (box :width 0.5)
        bottom))))

(def md-cymbal-engine-panel ()
  (let ((e (md-cymbal-engine-index)))
    (if (= e 1)
      (md-cymbal-machine-panel "TRX-CY"
        (box :width 6.0)
        (box :height 1.18)
        (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "rich" "RICH" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
        (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "top" "TOP" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
        (h-stack :gap 0.24 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ttun" "TOP HZ" 6.0 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "peak" "PEAK" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (if (= e 2)
        (md-cymbal-machine-panel "EFM-CY"
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ptch" "PITCH" 6.0 0 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MOD HZ" 6.0 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MOD DEC" 6.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "mod_amt" "MOD AMT" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "fb" "FDBK" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 6.0 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))))
        (md-cymbal-machine-panel (if (= e 3) "PI-RC" "PI-CC")
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ptch" "PITCH" 6.0 0 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (box :height 1.18)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "ring" "RING" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "grab" "GRAB" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hard" "HARD" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clsn" "COLLIDE" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))))))

(def md-cymbal-filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :height :fill :v-align :start :padding 0.5
        (v-stack :width 9.0 :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "FILTER" 4.4 :fg)
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fltq" "Q" 6.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 0 "fltf" "FREQ" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 0 "fltw" "WIDTH" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)))))

(def md-cymbal-color-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :width 0.5)
      (v-stack :width 9.6 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "COLOR" 4.4 :fg)
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "eqf" "EQ FREQ" 4.7 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "eqg" "EQ GAIN" 4.7 1 "dB" (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "dist" "DIST" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "srr" "CRUSH" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

(def md-cymbal-output-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "OUTPUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 :fg)
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amd" "AM AMT" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amf" "AM HZ" 5.5 1 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEVEL" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (md-cymbal-core-block)
      (subtree :key (str "md-cymbal-engine-panel-" (md-cymbal-engine-index)) (md-cymbal-engine-panel)))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (md-cymbal-filter-block)
      (md-cymbal-color-block)
      (md-cymbal-output-block))))
