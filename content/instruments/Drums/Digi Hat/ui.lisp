;; MD-HAT — Machinedrum-style hat with three machines (TRX/EFM/PI). Layout
;; follows the factory id808 lego style: monochrome accent, :fg badges,
;; full-height knobs. CORE + machine panel column, FILTER/COLOR/OUT column.

(def md-hat-engine-options () '("TRX-HH" "EFM-HH" "PI-HH"))
(def md-hat-engine-index () (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine"))) (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1))) (if (= e 2) 2 (if (= e 3) 3 1)))))

(def md-hat-core-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :height :fill :v-align :start :padding 0.5
        (v-stack :width 9.0 :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "MACHINE" 9.0 (md-hat-engine-options) :fg)
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "HUMAN" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "tune" "TUNE" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "dec" "DEC" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "level" "LEVEL" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)))))

;; Machine panel: same shape as the id808 BANK panel — badge + micro-nums top
;; left, two big knobs top right, a micro-num row along the bottom.
(def md-hat-machine-panel (badge head rows knob-a knob-b bottom)
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

(def md-hat-engine-panel ()
  (let ((e (md-hat-engine-index)))
    (if (= e 1)
      (md-hat-machine-panel "TRX-HH"
        (box :width 6.0)
        (box :height 1.18)
        (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "gap" "GAP" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
        (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "mtal" "METAL" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
        (h-stack :gap 0.24 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 6.0 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "lpf" "LPF" 6.0 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (if (= e 2)
        (md-hat-machine-panel "EFM-HH"
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ptch" "PITCH" 6.0 0 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mfrq" "MOD HZ" 6.0 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mdec" "MOD DEC" 6.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "mod_amt" "MOD AMT" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "fb" "FDBK" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "trem" "TREM" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tfrq" "TREM HZ" 5.5 1 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))))
        (md-hat-machine-panel "PI-HH"
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "clos" "CLOSED" 6.0 0 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ptch" "PITCH" 6.0 0 false (eseq.effects.custom-ui-lego/ui-accent-blue)))
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "clsn" "COLLIDE" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "ring" "RING" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ag" "AIR" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "au" "DAMP" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "br" "BODY" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))))))

(def md-hat-filter-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :height :fill :v-align :start :padding 0.5
        (v-stack :width 9.0 :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "FILTER" 4.4 :fg)
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "fltq" "Q" 6.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 0 "fltf" "FREQ" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 0 "fltw" "WIDTH" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)))))

(def md-hat-color-block ()
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

(def md-hat-output-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "OUTPUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.0 :fg)
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amd" "AM AMT" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "amf" "AM HZ" 5.5 1 "Hz" (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEVEL" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (md-hat-core-block)
      (subtree :key (str "md-hat-engine-panel-" (md-hat-engine-index)) (md-hat-engine-panel)))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (md-hat-filter-block)
      (md-hat-color-block)
      (md-hat-output-block))))
