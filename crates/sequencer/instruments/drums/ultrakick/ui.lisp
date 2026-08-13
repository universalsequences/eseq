(def uk-engine-options () '("SINE" "FM" "MODAL" "FOLD"))

(def uk-engine-index ()
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "engine")))
    (let ((e (if p (round (if (get p :value-field) (reactive-get "SEQ" (get p :value-field)) (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-value p)))) 1)))
      (if (= e 2) 2 (if (= e 3) 3 (if (= e 4) 4 1))))))

(def uk-ttyp-options () '("TICK" "KNOCK" "CLICK" "SNAP"))

(def uk-core-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "ULTRAKICK" 5.6 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "engine" "body" 6.6 (uk-engine-options) (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 3.5 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "humanize" "hmnz" 3.5 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "lvl" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "ptch" "PTCH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dec" "DEC" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sweep" "SWEEP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)))))

(def uk-engine-block ()
  (let ((e (uk-engine-index)))
    (if (= e 1)
      (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
        (h-stack :width :fill :height :fill :gap 0.30 :align :center
          (v-stack :width 10.6 :gap 0.18 :align :start
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "SINE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "swpt" "SWPT" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan)))
            (h-stack :gap 0.18 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "driv" "DRIV" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
          (h-stack :gap 0.10 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bx" "HARM" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "by" "SAT" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
            (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bz" "OCT" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))
      (if (= e 2)
        (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
          (h-stack :width :fill :height :fill :gap 0.30 :align :center
            (v-stack :width 10.6 :gap 0.18 :align :start
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "FM" 4.4 (eseq.effects.custom-ui-lego/ui-accent-blue))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "swpt" "SWPT" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan)))
              (h-stack :gap 0.18 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "driv" "DRIV" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
                (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
            (h-stack :gap 0.10 :align :start
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bx" "MOD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "by" "MFRQ" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
              (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bz" "MFB" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2))))
        (if (= e 3)
          (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
            (h-stack :width :fill :height :fill :gap 0.30 :align :center
              (v-stack :width 10.6 :gap 0.18 :align :start
                (h-stack :gap 0.18 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "MODAL" 4.4 (eseq.effects.custom-ui-lego/ui-accent-green))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "swpt" "SWPT" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan)))
                (h-stack :gap 0.18 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "driv" "DRIV" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
              (h-stack :gap 0.10 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bx" "HARD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "by" "TENS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 2)
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bz" "DAMP" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2))))
          (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
            (h-stack :width :fill :height :fill :gap 0.30 :align :center
              (v-stack :width 10.6 :gap 0.18 :align :start
                (h-stack :gap 0.18 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "FOLD" 4.4 (eseq.effects.custom-ui-lego/ui-accent-violet))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "swpt" "SWPT" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hold" "HOLD" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan)))
                (h-stack :gap 0.18 :align :start
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "driv" "DRIV" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
                  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "dirt" "DIRT" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
              (h-stack :gap 0.10 :align :start
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bx" "FOLD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "by" "SYM" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
                (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "bz" "CHEW" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))))))

(def uk-engine-small ()
  (let ((e (uk-engine-index)))
    (if (= e 1)
      (eseq.effects.custom-ui-lego/ui-readout-block-small-s "SINE DETAIL" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
        (h-stack :gap 0.24 :align :end
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bx" "HARM" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "by" "SAT" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bz" "OCT" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (if (= e 2)
        (eseq.effects.custom-ui-lego/ui-readout-block-small-s "FM DETAIL" (eseq.effects.custom-ui-lego/ui-accent-blue) 0
          (h-stack :gap 0.24 :align :end
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bx" "MOD" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "by" "MFRQ" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bz" "MFB" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
        (if (= e 3)
          (eseq.effects.custom-ui-lego/ui-readout-block-small-s "MODAL DETAIL" (eseq.effects.custom-ui-lego/ui-accent-green) 0
            (h-stack :gap 0.24 :align :end
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bx" "HARD" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "by" "TENS" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bz" "DAMP" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
          (eseq.effects.custom-ui-lego/ui-readout-block-small-s "FOLD DETAIL" (eseq.effects.custom-ui-lego/ui-accent-violet) 0
            (h-stack :gap 0.24 :align :end
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bx" "FOLD" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "by" "SYM" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
              (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bz" "CHEW" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))))))

(def uk-transient-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TRANS" 4.4 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "ttyp" "type" 6.6 (uk-ttyp-options) (eseq.effects.custom-ui-lego/ui-accent-cyan)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ttun" "TTUN" 3.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tdec" "TDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tamt" "TAMT" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "pnch" "PNCH" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "driv" "DRIV" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(def uk-layers-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "LAYERS" 4.4 (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "stun" "STUN" 3.4 0 "st" (eseq.effects.custom-ui-lego/ui-accent-violet))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "sdec" "SDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ncol" "NCOL" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "ndec" "NDEC" 3.4 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "sub" "SUB" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "nois" "NOIS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "dirt" "DIRT" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(def uk-out-small ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "OUTPUT" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "hpf" "HPF" 3.4 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "lpf" "LPF" 3.6 0 "Hz" (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LEV" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (uk-core-block)
      (subtree :key (str "uk-engine-block-" (uk-engine-index)) (uk-engine-block))
      (subtree :key (str "uk-engine-small-" (uk-engine-index)) (uk-engine-small)))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (uk-transient-block)
      (uk-layers-block)
      (uk-out-small))))
