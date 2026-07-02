(def uk-engine-options () '("SINE" "FM" "MODAL" "FOLD"))

(def uk-engine-index ()
  (let ((p (custom-ui-current-param "engine")))
    (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (custom-ui-param-value p)))) 1)))
      (if (= e 2) 2 (if (= e 3) 3 (if (= e 4) 4 1))))))

(def uk-ttyp-options () '("TICK" "KNOCK" "CLICK" "SNAP"))

(def uk-core-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "ULTRAKICK" 5.6 (ui-accent-orange))
          (ui-lego-micro-option-s 0 "engine" "body" 6.6 (uk-engine-options) (ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-base-note-s 0 3.5 (ui-accent-orange))
          (ui-lego-micro-num-s 0 "humanize" "hmnz" 3.5 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "level" "lvl" 3.2 2 false (ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (ui-accent-blue) 0)
        (ui-lego-knob-s 0 "dec" "DEC" 3.9 (ui-accent-orange) 0)
        (ui-lego-knob-s 0 "sweep" "SWEEP" 3.9 (ui-accent-violet) 0)))))

(def uk-engine-block ()
  (let ((e (uk-engine-index)))
    (if (= e 1)
      (ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.6 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (ui-lego-badge-s 0 "SINE" 4.4 (ui-accent-orange))
              (ui-lego-micro-num-s 0 "swpt" "SWPT" 3.4 0 "ms" (ui-accent-violet))
              (ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (ui-accent-cyan)))
            (h-stack :gap 0.18 :align :start
              (ui-lego-micro-num-s 0 "driv" "DRIV" 3.4 2 false (ui-accent-orange))
              (ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (ui-accent-blue))))
          (h-stack :gap 0.10 :align :start
            (ui-lego-knob-s 0 "bx" "HARM" 3.9 (ui-accent-violet) 2)
            (ui-lego-knob-s 0 "by" "SAT" 3.9 (ui-accent-orange) 2)
            (ui-lego-knob-s 0 "bz" "OCT" 3.9 (ui-accent-cyan) 2))))
      (if (= e 2)
        (ui-control-panel-dense-s 0
          (h-stack :width :fill :height :fill :gap 0.30 :align :center
            (v-stack :width 10.6 :gap 0.18 :align :start
              (h-stack :gap 0.18 :align :start
                (ui-lego-badge-s 0 "FM" 4.4 (ui-accent-blue))
                (ui-lego-micro-num-s 0 "swpt" "SWPT" 3.4 0 "ms" (ui-accent-violet))
                (ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (ui-accent-cyan)))
              (h-stack :gap 0.18 :align :start
                (ui-lego-micro-num-s 0 "driv" "DRIV" 3.4 2 false (ui-accent-orange))
                (ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (ui-accent-blue))))
            (h-stack :gap 0.10 :align :start
              (ui-lego-knob-s 0 "bx" "MOD" 3.9 (ui-accent-blue) 2)
              (ui-lego-knob-s 0 "by" "MFRQ" 3.9 (ui-accent-violet) 2)
              (ui-lego-knob-s 0 "bz" "MFB" 3.9 (ui-accent-orange) 2))))
        (if (= e 3)
          (ui-control-panel-dense-s 0
            (h-stack :width :fill :height :fill :gap 0.30 :align :center
              (v-stack :width 10.6 :gap 0.18 :align :start
                (h-stack :gap 0.18 :align :start
                  (ui-lego-badge-s 0 "MODAL" 4.4 (ui-accent-green))
                  (ui-lego-micro-num-s 0 "swpt" "SWPT" 3.4 0 "ms" (ui-accent-violet))
                  (ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (ui-accent-cyan)))
                (h-stack :gap 0.18 :align :start
                  (ui-lego-micro-num-s 0 "driv" "DRIV" 3.4 2 false (ui-accent-orange))
                  (ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (ui-accent-blue))))
              (h-stack :gap 0.10 :align :start
                (ui-lego-knob-s 0 "bx" "HARD" 3.9 (ui-accent-green) 2)
                (ui-lego-knob-s 0 "by" "TENS" 3.9 (ui-accent-green) 2)
                (ui-lego-knob-s 0 "bz" "DAMP" 3.9 (ui-accent-cyan) 2))))
          (ui-control-panel-dense-s 0
            (h-stack :width :fill :height :fill :gap 0.30 :align :center
              (v-stack :width 10.6 :gap 0.18 :align :start
                (h-stack :gap 0.18 :align :start
                  (ui-lego-badge-s 0 "FOLD" 4.4 (ui-accent-violet))
                  (ui-lego-micro-num-s 0 "swpt" "SWPT" 3.4 0 "ms" (ui-accent-violet))
                  (ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (ui-accent-cyan)))
                (h-stack :gap 0.18 :align :start
                  (ui-lego-micro-num-s 0 "driv" "DRIV" 3.4 2 false (ui-accent-orange))
                  (ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (ui-accent-blue))))
              (h-stack :gap 0.10 :align :start
                (ui-lego-knob-s 0 "bx" "FOLD" 3.9 (ui-accent-violet) 2)
                (ui-lego-knob-s 0 "by" "SYM" 3.9 (ui-accent-blue) 2)
                (ui-lego-knob-s 0 "bz" "CHEW" 3.9 (ui-accent-orange) 2)))))))))

(def uk-engine-small ()
  (let ((e (uk-engine-index)))
    (if (= e 1)
      (ui-readout-block-small-s "SINE DETAIL" (ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (ui-lego-micro-num-s 0 "bx" "HARM" 3.2 2 false (ui-accent-violet))
          (ui-lego-micro-num-s 0 "by" "SAT" 3.2 2 false (ui-accent-orange))
          (ui-lego-micro-num-s 0 "bz" "OCT" 3.2 2 false (ui-accent-cyan))))
      (if (= e 2)
        (ui-readout-block-small-s "FM DETAIL" (ui-accent-blue) 0
          (h-stack :gap 0.24 :align :end
            (ui-lego-micro-num-s 0 "bx" "MOD" 3.2 2 false (ui-accent-blue))
            (ui-lego-micro-num-s 0 "by" "MFRQ" 3.2 2 false (ui-accent-violet))
            (ui-lego-micro-num-s 0 "bz" "MFB" 3.2 2 false (ui-accent-orange))))
        (if (= e 3)
          (ui-readout-block-small-s "MODAL DETAIL" (ui-accent-green) 0
            (h-stack :gap 0.24 :align :end
              (ui-lego-micro-num-s 0 "bx" "HARD" 3.2 2 false (ui-accent-green))
              (ui-lego-micro-num-s 0 "by" "TENS" 3.2 2 false (ui-accent-green))
              (ui-lego-micro-num-s 0 "bz" "DAMP" 3.2 2 false (ui-accent-cyan))))
          (ui-readout-block-small-s "FOLD DETAIL" (ui-accent-violet) 0
            (h-stack :gap 0.24 :align :end
              (ui-lego-micro-num-s 0 "bx" "FOLD" 3.2 2 false (ui-accent-violet))
              (ui-lego-micro-num-s 0 "by" "SYM" 3.2 2 false (ui-accent-blue))
              (ui-lego-micro-num-s 0 "bz" "CHEW" 3.2 2 false (ui-accent-orange)))))))))

(def uk-transient-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "TRANS" 4.4 (ui-accent-cyan))
          (ui-lego-micro-option-s 0 "ttyp" "type" 6.6 (uk-ttyp-options) (ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "ttun" "TTUN" 3.6 0 "Hz" (ui-accent-cyan))
          (ui-lego-micro-num-s 0 "tdec" "TDEC" 3.4 0 "ms" (ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "tamt" "TAMT" 3.9 (ui-accent-cyan) 2)
        (ui-lego-knob-s 0 "pnch" "PNCH" 3.9 (ui-accent-orange) 2)
        (ui-lego-knob-s 0 "driv" "DRIV" 3.9 (ui-accent-orange) 2)))))

(def uk-layers-block ()
  (ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (ui-lego-badge-s 0 "LAYERS" 4.4 (ui-accent-violet))
          (ui-lego-micro-num-s 0 "stun" "STUN" 3.4 0 "st" (ui-accent-violet))
          (ui-lego-micro-num-s 0 "sdec" "SDEC" 3.4 0 "ms" (ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (ui-lego-micro-num-s 0 "ncol" "NCOL" 3.4 2 false (ui-accent-blue))
          (ui-lego-micro-num-s 0 "ndec" "NDEC" 3.4 0 "ms" (ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (ui-lego-knob-s 0 "sub" "SUB" 3.9 (ui-accent-violet) 2)
        (ui-lego-knob-s 0 "nois" "NOIS" 3.9 (ui-accent-blue) 2)
        (ui-lego-knob-s 0 "dirt" "DIRT" 3.9 (ui-accent-orange) 2)))))

(def uk-out-small ()
  (ui-readout-block-small-s "OUTPUT" (ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (ui-lego-micro-num-s 0 "hpf" "HPF" 3.4 0 "Hz" (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "lpf" "LPF" 3.6 0 "Hz" (ui-accent-cyan))
      (ui-lego-micro-num-s 0 "level" "LEV" 3.2 2 false (ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (uk-core-block)
      (subtree :key (str "uk-engine-block-" (uk-engine-index)) (uk-engine-block))
      (subtree :key (str "uk-engine-small-" (uk-engine-index)) (uk-engine-small)))
    (ui-lego-column
      (uk-transient-block)
      (uk-layers-block)
      (uk-out-small))))
