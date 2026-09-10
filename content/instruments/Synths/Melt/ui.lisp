;; Melt: moving-ratio FM, with one contextual display and persistent macro knobs.
(def melt-accent () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def melt-section () eseq.vanilla/custom-ui-selected-section)
(def melt-bind (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-binding
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
(def melt-value (name) (reactive-value (melt-bind name)))
(def melt-knob (section name title width decimals taper)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title width 3.55 2.6
    (melt-accent) decimals taper :widget-knob-track 10 9.5 :center))
(def melt-panel (section title width body)
  (box :debug-name (str "melt-panel-" section) :width width :height 4.8 :padding 0.2
    :background-color (if (= (melt-section) section) :instrument-panel-bg :instrument-group-bg)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    (v-stack :gap 0.15
      (box :width 7 :height 0.7 :background-color (melt-accent)
        (label title :width 7 :height 0.7 :font-size 8 :h-align :center :v-align :center
          :color :black :bg :transparent))
      body)))
(def melt-num (name title decimals)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "melt-mod-" name)
      (v-stack :width 8.4 :height 1.15 :gap 0.08
        (label title :height 0.5 :font-size 8 :v-align :center :color :black :bg :transparent)
        (number-picker :debug-name (str "melt-num-" name) :width 8.4 :height 0.55
          :noui true :font-size 8.5 :decimals decimals :step (pow 10 (- 0 decimals))
          :value (melt-bind name)
          :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
          :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
          :text-align :left
          :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
            (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) :black)
          :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))
(def melt-write (scope name value)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope name)))
    (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
      (max (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
        (min (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p) value)))))
(def melt-caption (text)
  (label text :height 0.55 :font-size 7.4 :v-align :center :color :black :bg :transparent))

(def melt-op-envelope (prefix)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :debug-name "melt-operator-envelope" :width 35.2 :height 4.65
      :curve-color :black :point-color :black :grid-color :black :background-color (melt-accent)
      :attack (melt-bind (str prefix "_attack_ms")) :attack-max 4000
      :decay (melt-bind (str prefix "_decay_ms")) :decay-max 8000
      :sustain (melt-bind (str prefix "_sustain"))
      :release (melt-bind (str prefix "_release_ms")) :release-max 8000
      :on-change (lambda (env)
        (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
          (str prefix "_attack_ms") (str prefix "_decay_ms")
          (str prefix "_sustain") (str prefix "_release_ms") env)))))
(def melt-op-page (prefix)
  (v-stack :gap 0.2
    (melt-op-envelope prefix)
    (h-stack :gap 0.3
      (melt-num (str prefix "_attack_ms") "Attack ms" 1)
      (melt-num (str prefix "_decay_ms") "Decay ms" 1)
      (melt-num (str prefix "_sustain") "Sustain" 2)
      (melt-num (str prefix "_release_ms") "Release ms" 1))
    (melt-caption "Envelope moves both FM depth and ratio.")))

(def melt-ahd (attack hold decay height)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :mode :ahd :debug-name "melt-ahd" :width 35.2 :height height
      :curve-color :black :point-color :black :grid-color :black :background-color (melt-accent)
      :attack (melt-bind attack) :attack-max (if hold 2000 4000)
      :hold (if hold (melt-bind hold) 0) :hold-max 4000 :hold-editable (if hold true false)
      :decay (melt-bind decay) :decay-max 8000 :decay-db 60
      :on-change (lambda (env)
        (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
          (if hold
            (list (list :attack attack) (list :hold hold) (list :decay decay))
            (list (list :attack attack) (list :decay decay))) env)))))
(def melt-release ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :mode :decay :debug-name "melt-release" :width 35.2 :height 2
      :curve-color :black :point-color :black :grid-color :black :background-color (melt-accent)
      :initial 1 :initial-min 0 :initial-max 1 :initial-editable false
      :time (melt-bind "amp_release_ms") :time-max 8000 :decay-db 60
      :on-change (lambda (env)
        (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
          (list (list :time "amp_release_ms")) env)))))
(def melt-amp-page ()
  (v-stack :gap 0.15
      (melt-ahd "amp_attack_ms" "amp_hold_ms" "amp_decay_ms" 2.5)
      (melt-caption "Gate off / release from the current level")
      (melt-release)
      (h-stack :gap 0.3
        (melt-num "amp_attack_ms" "Attack ms" 1) (melt-num "amp_hold_ms" "Hold ms" 1)
        (melt-num "amp_decay_ms" "Decay ms" 1) (melt-num "amp_release_ms" "Release ms" 1))
      (h-stack :gap 0.3
        (melt-num "glide_ms" "Glide ms" 1)
        (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-base-note-param)))
          (v-stack :width 8.4 :height 1.15 :gap 0.08
            (label "Note" :height 0.5 :font-size 8 :v-align :center :color :black :bg :transparent)
            (number-picker :debug-name "melt-base-note" :width 8.4 :height 0.55
              :noui true :font-size 8.5 :decimals 0 :step 1 :text-align :left :text-color :black
              :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
              :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
              :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
              :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p)))))))

;; The filter is a four-pole LP followed by a four-pole HP. The curve shows
;; its unmodulated passband; envelope and key tracking are edited below.
(def melt-filter-band (id type freq q)
  (dict :id id :type type :freq freq :freq-min 20 :freq-max 16000
    :q q :q-min 0.5 :q-max 14 :q-taper "log"
    :gain 0 :enabled true :selected true))
(def melt-filter-curve ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (response-curve-editor :debug-name "melt-filter" :width 35.2 :height 2.2
      :mode :filter :freq-min 20 :freq-max 18000 :gain-min -24 :gain-max 18
      :background-color (melt-accent) :stroke-color :black :point-color :black
      :grid-color (rgba 0 0 0 0.15) :stroke-width 3
      :bands (list
        (melt-filter-band 0 "highpass" (melt-bind "flt_base") (melt-bind "flt_res_hi"))
        (melt-filter-band 1 "lowpass"
          (min 16000 (max 30 (* (melt-value "flt_base") (pow 2 (melt-value "flt_width")))))
          (melt-bind "flt_res_lo")))
      :on-action (lambda (event)
        (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
          (if (= (get event :id) 0)
            (do (melt-write scope "flt_base" (get event :freq))
                (melt-write scope "flt_res_hi" (get event :q)))
            (do (melt-write scope "flt_width"
                  (/ (log (/ (max 30 (get event :freq))
                    (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding
                      (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope "flt_base"))))) (log 2)))
                (melt-write scope "flt_res_lo" (get event :q)))) false)))))
(def melt-filter-page ()
  (v-stack :gap 0.15
    (melt-filter-curve)
    (melt-caption "Filter envelope")
    (melt-ahd "fenv_attack_ms" false "fenv_decay_ms" 2)
    (h-stack :gap 0.3
      (melt-num "flt_res_hi" "HP Res" 2) (melt-num "flt_res_lo" "LP Res" 2)
      (melt-num "fenv_attack_ms" "Attack ms" 1) (melt-num "fenv_decay_ms" "Decay ms" 1))
    (h-stack :gap 0.3
      (melt-num "env_to_base" "Env > Base" 2) (melt-num "env_to_width" "Env > Width" 2)
      (melt-num "keytrack" "Keytrack" 2))))

(defwidget melt-routing
  :width 35.2 :height 3.8 :state (stack feedback) :bindable (stack feedback)
  :shader
  (let ((left (* aspect -0.65)) (right (* aspect 0.65))
        (serial (rgba 0 0 0 (+ 0.15 (* 0.85 stack))))
        (parallel (rgba 0 0 0 (- 1 (* 0.85 stack))))
        (fb (rgba 0 0 0 (+ 0.15 (* 0.7 feedback)))))
    (sdf/layer
      (sdf/region :routing (sdf/rect width height) :yellow)
      (sdf/stroke (sdf/line left 0 0 0) 0.035 serial)
      (sdf/stroke (sdf/line 0 0 right 0) 0.035 :black)
      (sdf/stroke (sdf/line left 0 left 0.65) 0.035 parallel)
      (sdf/stroke (sdf/line left 0.65 right 0.65) 0.035 parallel)
      (sdf/stroke (sdf/line right 0.65 right 0) 0.035 parallel)
      (sdf/stroke (sdf/line -0.35 0 -0.35 -0.65) 0.035 fb)
      (sdf/stroke (sdf/line -0.35 -0.65 0.35 -0.65) 0.035 fb)
      (sdf/stroke (sdf/line 0.35 -0.65 0.35 0) 0.035 fb)
      (sdf/paint (sdf/translate left 0 (sdf/rect 0.22 0.22)) :black)
      (sdf/paint (sdf/rect 0.22 0.22) :black)
      (sdf/paint (sdf/translate right 0 (sdf/rect 0.22 0.22)) :black))))
(def melt-fm-page ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (gesture (dict :start nil)))
    (v-stack :gap 0.3
      (melt-routing :debug-name "melt-routing" :stack (melt-bind "stack") :feedback (melt-bind "feedback")
        :on-mouse-down (lambda (x y region)
          (set! gesture.start (list x y (melt-value "stack") (melt-value "feedback"))))
        :on-mouse-up (lambda (x y region) (set! gesture.start nil))
        :on-drag (lambda (x y region)
          (if gesture.start
            (do (melt-write scope "stack" (+ (nth gesture.start 2) (* 0.5 (- x (nth gesture.start 0)))))
                (melt-write scope "feedback" (+ (nth gesture.start 3) (* 0.6 (- (nth gesture.start 1) y)))))
            false)))
      (h-stack :gap 0.3
        (label "OP 2" :width 11.5 :height 0.6 :h-align :center :color :black :bg :transparent)
        (label "OP 1" :width 11.5 :height 0.6 :h-align :center :color :black :bg :transparent)
        (label "CARRIER" :width 11.5 :height 0.6 :h-align :center :color :black :bg :transparent))
      (melt-caption "Drag X: parallel / serial blend. Y: OP 1 feedback.")
      (melt-caption "Ratio quantization")
      (dropdown :debug-name "melt-snap" :width 16 :height 0.8 :font-size 8
        :options '("Free" "Half-step ratios") :value-index (melt-bind "ratio_snap")
        :bg-color :instrument-control-bg :text-color (melt-accent) :chevron-color (melt-accent)
        :on-change (lambda (v)
          (melt-write scope "ratio_snap"
            (eseq.effects.param-controls/custom-ui-option-index '("Free" "Half-step ratios") v)))))))
(def melt-color-page ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (v-stack :gap 0.2
      (response-curve-editor :debug-name "melt-eq" :width 35.2 :height 3.5 :mode :eq
        :bands (list (dict :id 0 :type "bell" :freq (melt-bind "eq_freq")
          :freq-min 40 :freq-max 11000 :gain (melt-bind "eq_gain_db")
          :gain-min -30 :gain-max 30 :q (melt-bind "eq_q") :q-min 0.5 :q-max 12 :enabled true :selected true))
        :freq-min 20 :freq-max 18000 :gain-min -30 :gain-max 30
        :background-color (melt-accent) :stroke-color :black :point-color :black
        :grid-color (rgba 0 0 0 0.15) :stroke-width 3
        :on-action (lambda (event)
          (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
            (do (melt-write scope "eq_freq" (get event :freq))
                (melt-write scope "eq_gain_db" (get event :gain))
                (melt-write scope "eq_q" (get event :q))) false)))
      (h-stack :gap 0.3
        (melt-num "eq_freq" "EQ Hz" 0) (melt-num "eq_gain_db" "EQ dB" 1)
        (melt-num "eq_q" "EQ Q" 2))
      (h-stack :gap 0.3
        (melt-num "am_rate" "AM Hz" 1) (melt-num "am_depth" "AM Depth" 2))
      (melt-caption "AM > Filter > Sample reduction > EQ > Drive"))))

(def melt-display ()
  (box :debug-name "melt-display" :width 35.8 :height 9.8 :padding 0.3 :background-color (melt-accent)
    (v-stack :gap 0.25
      (label (str (nth '("OPERATOR 1" "OPERATOR 2" "FILTER" "AMPLITUDE" "FM ROUTING" "COLOR") (melt-section)))
        :width 35.2 :height 0.7 :h-align :center :v-align :center :font-size 8
        :color (melt-accent) :bg :black)
      (if (= (melt-section) 0) (melt-op-page "op1")
        (if (= (melt-section) 1) (melt-op-page "op2")
        (if (= (melt-section) 2) (melt-filter-page)
        (if (= (melt-section) 3) (melt-amp-page)
        (if (= (melt-section) 4) (melt-fm-page) (melt-color-page)))))))))
(defsynth-ui
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2
      (melt-panel 0 "OP 1" 21.4
        (h-stack :gap 0.2
          (melt-knob 0 "ratio1" "Ratio" 6.8 2 :log)
          (melt-knob 0 "idx1" "Index" 6.8 2 :linear)
          (melt-knob 0 "sweep1" "Sweep oct" 6.8 2 :linear)))
      (melt-panel 1 "OP 2" 21.4
        (h-stack :gap 0.2
          (melt-knob 1 "ratio2" "Ratio" 6.8 2 :log)
          (melt-knob 1 "idx2" "Index" 6.8 2 :linear)
          (melt-knob 1 "sweep2" "Sweep oct" 6.8 2 :linear))))
    (melt-display)
    (v-stack :gap 0.2
      (melt-panel 2 "FILTER" 14.6
        (h-stack :gap 0.2
          (melt-knob 2 "flt_base" "Base Hz" 7 0 :log)
          (melt-knob 2 "flt_width" "Width oct" 7 2 :linear)))
      (melt-panel 3 "AMP" 14.6
        (h-stack :gap 0.2
          (melt-knob 3 "gain" "Level" 7 2 :linear)
          (melt-knob 3 "pan_width" "Spread" 7 2 :linear))))
    (v-stack :gap 0.2
      (melt-panel 4 "FM" 14.6
        (h-stack :gap 0.2
          (melt-knob 4 "feedback" "Feedback" 7 2 :linear)
          (melt-knob 4 "stack" "Stack" 7 2 :linear)))
      (melt-panel 5 "COLOR" 14.6
        (h-stack :gap 0.2
          (melt-knob 5 "drive" "Drive" 7 2 :linear)
          (melt-knob 5 "srr" "Reduction" 7 2 :linear))))))
