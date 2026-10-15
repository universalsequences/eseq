;; Beads — the module's granular processor laid out as four surfaces: the
;; buffer (TIME and its scatter, FREEZE, the recording media), the grain
;; (SIZE / SHAPE / PITCH and their per-grain randomization), the seed
;; (DENSITY, FEEDBACK, the SEED button and its latch) and the mix (DRY/WET,
;; REVERB, input and output level). Every control appears exactly once.

(def bfx-knob () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def bfx-head () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def bfx-text () :fg)

(def bfx-surf () :instrument-group-bg)
(def bfx-bord () :border-inactive)

(def bfx-panel (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (bfx-surf) (bfx-bord) false body))

;; The module's four media (QUALITY): recording rate, bit depth, buffer time.
(def bfx-quality-options ()
  '("dirty tape" "clean tape" "vintage" "hi-fi"))

;; Header, then a row of small controls, beside three full knobs.
(def bfx-block (section title small-row k1 k2 k3)
  (bfx-panel section
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s section title 4.0 (bfx-head))
        small-row)
      (h-stack :gap 0.08 :align :start k1 k2 k3))))

(def bfx-buffer-block ()
  (bfx-block 0 "BUFFER"
    (h-stack :gap 0.18 :align :start
      (v-stack :gap 0.05 :align :start
        (label "media" :font-size 8.2 :width 6.4 :color :dim :bg :transparent)
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "quality" "media" 6.4 (bfx-quality-options) (bfx-text)))
      (v-stack :gap 0.05 :align :start
        (label "freeze" :font-size 8.2 :width 3.4 :color :dim :bg :transparent)
        (eseq.effects.custom-ui-lego/ui-lego-micro-toggle-s 0 "freeze" 3.4 (bfx-knob))))
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "time" "time" 3.7 (bfx-knob) 2)
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "time_amt" "scatter" 3.7 (bfx-knob) 2)
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "density" "density" 3.7 (bfx-knob) 2)))

;; Per-grain randomization: positive amounts scatter every grain, negative
;; ones scatter only some of them (the module's attenuverter halves). With
;; SIZE fully clockwise (delay mode) sz ± is the LP/HP filter and pt ± a
;; vibrato.
(def bfx-grain-block ()
  (bfx-block 1 "GRAIN"
    (h-stack :gap 0.18 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "size_amt" "sz ±" 3.2 2 false (bfx-text))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "shape_amt" "sh ±" 3.2 2 false (bfx-text))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "pitch_amt" "pt ±" 3.2 2 false (bfx-text)))
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "size" "size" 3.7 (bfx-knob) 2)
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "shape" "shape" 3.7 (bfx-knob) 2)
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "pitch" "pitch" 3.7 (bfx-knob) 1)))

;; SEED fires a grain (or taps the delay tempo); latch keeps grains coming
;; without it, as the module's held SEED button does.
(def bfx-seed-block ()
  (bfx-block 2 "SEED"
    (h-stack :gap 0.30 :align :start
      (v-stack :gap 0.05 :align :start
        (label "seed" :font-size 8.2 :width 3.4 :color :dim :bg :transparent)
        (eseq.effects.custom-ui-lego/ui-lego-micro-toggle-s 2 "seed" 3.4 (bfx-knob)))
      (v-stack :gap 0.05 :align :start
        (label "latch" :font-size 8.2 :width 3.4 :color :dim :bg :transparent)
        (eseq.effects.custom-ui-lego/ui-lego-micro-toggle-s 2 "latch" 3.4 (bfx-knob))))
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "feedback" "fdbk" 3.7 (bfx-knob) 2)
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "dry_wet" "dry/wet" 3.7 (bfx-knob) 2)
    (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "reverb" "reverb" 3.7 (bfx-knob) 2)))

(def bfx-level-block ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 3 (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (bfx-surf) (bfx-bord) false
    (v-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-header-s 3 "LEVEL" 4.0 (bfx-head))
      (h-stack :gap 0.30 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "input_db" "input" 5.0 1 "dB" (bfx-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "level_db" "output" 5.0 1 "dB" (bfx-text))))))

(defeffect-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (bfx-buffer-block)
      (bfx-grain-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (bfx-seed-block)
      (bfx-level-block))))
