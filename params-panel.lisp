;; params-panel.lisp — demo of the params panel UI
;; Evaluate this buffer to see the panel.

(defstate active-tab 1)

;; PARAMS state
(defstate trig-prob 50)
(defstate micro-shift 2)
(defstate gate-len 30)
(defstate density 85)
(defstate scatter 25)
(defstate low-cut 100)

;; NODES state
(defstate osc-level 72)
(defstate osc-detune 5)
(defstate filter-cutoff 80)
(defstate filter-res 40)
(defstate lfo-rate 35)
(defstate lfo-depth 60)
(defstate reverb-mix 45)
(defstate reverb-decay 70)

;; MATRIX state
(defstate m-a1 80)
(defstate m-a2 0)
(defstate m-a3 50)
(defstate m-b1 0)
(defstate m-b2 65)
(defstate m-b3 30)
(defstate m-c1 40)
(defstate m-c2 20)
(defstate m-c3 90)

(effect
  (tabs :items (list "NODES" "PARAMS" "MATRIX")
        :padding 1
        :bind active-tab

    ;; ── Tab 0: NODES ──
    (v-stack :gap 0.5 :padding 1

      (label "OSCILLATOR" :color :dim)

      (h-stack :align :center :gap 1
        (label "level" :width 10)
        (box :flex 1 (hslider :fill :primary :min 0 :max 100 :bind osc-level))
        (label (fmt "{:.0}" osc-level) :width 5))

      (h-stack :align :center :gap 1
        (label "detune" :width 10)
        (box :flex 1 (hslider :fill :yellow :min -50 :max 50 :bind osc-detune))
        (label (fmt "{:.0}" osc-detune) :width 5))

      (label "FILTER" :color :dim)

      (h-stack :align :center :gap 1
        (label "cutoff" :width 10)
        (box :flex 1 (hslider :fill :primary :min 0 :max 100 :bind filter-cutoff))
        (label (fmt "{:.0}" filter-cutoff) :width 5))

      (h-stack :align :center :gap 1
        (label "resonance" :width 10)
        (box :flex 1 (hslider :fill :primary :min 0 :max 100 :bind filter-res))
        (label (fmt "{:.0}" filter-res) :width 5))

      (label "MODULATION" :color :dim)

      (h-stack :align :center :gap 1
        (label "lfo_rate" :width 10)
        (box :flex 1 (hslider :fill :blue :min 0 :max 100 :bind lfo-rate))
        (label (fmt "{:.0}" lfo-rate) :width 5))

      (h-stack :align :center :gap 1
        (label "lfo_depth" :width 10)
        (box :flex 1 (hslider :fill :blue :min 0 :max 100 :bind lfo-depth))
        (label (fmt "{:.0}" lfo-depth) :width 5))

      (label "EFFECTS" :color :dim)

      (h-stack :align :center :gap 1
        (label "rev_mix" :width 10)
        (box :flex 1 (hslider :fill :gray :min 0 :max 100 :bind reverb-mix))
        (label (fmt "{:.0}" reverb-mix) :width 5))

      (h-stack :align :center :gap 1
        (label "rev_decay" :width 10)
        (box :flex 1 (hslider :fill :gray :min 0 :max 100 :bind reverb-decay))
        (label (fmt "{:.0}" reverb-decay) :width 5)))

    ;; ── Tab 1: PARAMS ──
    (v-stack :gap 0.5 :padding 1

      (h-stack :justify :space-between :align :center :gap 1
        (label "target:" :color :dim)
        (label "REV. BASS" :color :primary))

      (label "TIMING_DYNAMICS" :color :dim)

      (h-stack :align :center :gap 1
        (label "trig_prob" :width 10)
        (box :flex 1 (hslider :fill :primary :min 0 :max 100 :bind trig-prob))
        (label (fmt "{:.0}%" trig-prob) :width 5))

      (h-stack :align :center :gap 1
        (label "micro_shift" :width 10)
        (box :flex 1 (hslider :fill :yellow :min -10 :max 10 :bind micro-shift))
        (label (fmt "{:.0}" micro-shift) :width 5))

      (h-stack :align :center :gap 1
        (label "gate_len" :width 10)
        (box :flex 1 (hslider :fill :gray :min 0 :max 100 :bind gate-len))
        (label (fmt "{:.0}" gate-len) :width 5))

      (label "FOG_CLOUD_SEND" :color :dim)

      (h-stack :align :center :gap 1
        (label "density" :width 10)
        (box :flex 1 (hslider :fill :red :min 0 :max 100 :bind density))
        (label (fmt ".{:.0}" density) :width 5 :color :red))

      (h-stack :align :center :gap 1
        (label "scatter" :width 10)
        (box :flex 1 (hslider :fill :gray :min 0 :max 100 :bind scatter))
        (label (fmt ".{:.0}" scatter) :width 5))

      (h-stack :align :center :gap 1
        (label "low_cut" :width 10)
        (box :flex 1 (hslider :fill :primary :min 0 :max 100 :bind low-cut))
        (label "MAX" :width 5)))

    ;; ── Tab 2: MATRIX ──
    (v-stack :gap 0.5 :padding 1

      (label "ROUTING MATRIX" :color :dim)

      ;; Column headers
      (h-stack :align :center :gap 1
        (label "" :width 10)
        (label "OSC" :width 8 :color :dim)
        (label "FILTER" :width 8 :color :dim)
        (label "AMP" :width 8 :color :dim))

      ;; Row A: LFO 1
      (h-stack :align :center :gap 1
        (label "LFO_1" :width 10 :color :primary)
        (box :width 8 (knob :size 3 :min 0 :max 100 :bind m-a1))
        (box :width 8 (knob :size 3 :min 0 :max 100 :bind m-a2))
        (box :width 8 (knob :size 3 :min 0 :max 100 :bind m-a3)))

      ;; Row B: LFO 2
      (h-stack :align :center :gap 1
        (label "LFO_2" :width 10 :color :primary)
        (box :width 8 (knob :size 3 :min 0 :max 100 :bind m-b1))
        (box :width 8 (knob :size 3 :min 0 :max 100 :bind m-b2))
        (box :width 8 (knob :size 3 :min 0 :max 100 :bind m-b3)))

      ;; Row C: ENV
      (h-stack :align :center :gap 1
        (label "ENV" :width 10 :color :red)
        (box :width 8 (knob :size 3 :min 0 :max 100 :color :red :bind m-c1))
        (box :width 8 (knob :size 3 :min 0 :max 100 :color :red :bind m-c2))
        (box :width 8 (knob :size 3 :min 0 :max 100 :color :red :bind m-c3))))))
