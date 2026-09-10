;; Shared controls and track-chain pages for the Monomachine-inspired synths.
(module eseq.effects.mnm-surface)
(export mnm-accent mnm-section mnm-bind mnm-value mnm-knob mnm-panel mnm-num mnm-write mnm-caption mnm-ahd mnm-release mnm-amp-page mnm-filter-band mnm-filter-curve mnm-filter-page mnm-color-page)
(def mnm-accent () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def mnm-section () eseq.vanilla/custom-ui-selected-section)
(def mnm-bind (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-binding
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
(def mnm-value (name) (reactive-value (mnm-bind name)))
(def mnm-knob (section name title width decimals taper)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title width 3.55 2.6
    (mnm-accent) decimals taper :widget-knob-track 8.5 8 :center))
(def mnm-panel (section title width body)
  (box :debug-name (str "mnm-panel-" section) :width width :height 4.8 :padding 0.2
    :background-color (if (= (mnm-section) section) :instrument-panel-bg :instrument-group-bg)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    (v-stack :gap 0.15
      (box :width 7 :height 0.7 :background-color (mnm-accent)
        (label title :width 7 :height 0.7 :font-size 8 :h-align :center :v-align :center
          :color :black :bg :transparent))
      body)))
(def mnm-ink (p)
  (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white :black))
(def mnm-num (name title decimals)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "mnm-mod-" name)
      (v-stack :width 8.4 :height 1.3 :gap 0
        (label title :height 0.65 :font-size 9 :v-align :center :color (mnm-ink p) :bg :transparent)
        (number-picker :debug-name (str "mnm-num-" name) :width 8.4 :height 0.65
          :noui true :font-size 10 :decimals decimals :step (pow 10 (- 0 decimals))
          :value (mnm-bind name)
          :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
          :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
          :text-align :left
          :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
            (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) (mnm-ink p))
          :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))))
(def mnm-write (scope name value)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope name)))
    (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
      (max (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
        (min (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p) value)))))
(def mnm-caption (text)
  (label text :height 0.55 :font-size 8.4 :v-align :center :color :black :bg :transparent))


(def mnm-ahd (attack hold decay height)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :mode :ahd :debug-name "mnm-ahd" :width 35.2 :height height
      :curve-color :black :point-color :black :grid-color :black :background-color (mnm-accent)
      :attack (mnm-bind attack) :attack-max (if hold 2000 4000)
      :hold (if hold (mnm-bind hold) 0) :hold-max 4000 :hold-editable (if hold true false)
      :decay (mnm-bind decay) :decay-max 8000 :decay-db 60
      :on-change (lambda (env)
        (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
          (if hold
            (list (list :attack attack) (list :hold hold) (list :decay decay))
            (list (list :attack attack) (list :decay decay))) env)))))
(def mnm-release ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :mode :decay :debug-name "mnm-release" :width 35.2 :height 2
      :curve-color :black :point-color :black :grid-color :black :background-color (mnm-accent)
      :initial 1 :initial-min 0 :initial-max 1 :initial-editable false
      :time (mnm-bind "amp_release_ms") :time-max 8000 :decay-db 60
      :on-change (lambda (env)
        (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
          (list (list :time "amp_release_ms")) env)))))
(def mnm-amp-page ()
  (v-stack :gap 0.15
      (mnm-ahd "amp_attack_ms" "amp_hold_ms" "amp_decay_ms" 2.5)
      (mnm-caption "Gate off / release from the current level")
      (mnm-release)
      (h-stack :gap 0.3
        (mnm-num "amp_attack_ms" "Attack ms" 1) (mnm-num "amp_hold_ms" "Hold ms" 1)
        (mnm-num "amp_decay_ms" "Decay ms" 1) (mnm-num "amp_release_ms" "Release ms" 1))
      (h-stack :gap 0.3
        (mnm-num "glide_ms" "Glide ms" 1)
        (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-base-note-param)))
          (v-stack :width 8.4 :height 1.3 :gap 0
            (label "Note" :height 0.65 :font-size 9 :v-align :center :color :black :bg :transparent)
            (number-picker :debug-name "mnm-base-note" :width 8.4 :height 0.65
              :noui true :font-size 10 :decimals 0 :step 1 :text-align :left :text-color :black
              :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
              :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
              :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
              :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p)))))))

;; The filter is a four-pole LP followed by a four-pole HP. The curve shows
;; its unmodulated passband; envelope and key tracking are edited below.
(def mnm-filter-band (id type freq q)
  (dict :id id :type type :freq freq :freq-min 20 :freq-max 16000
    :q q :q-min 0.5 :q-max 14 :q-taper "log"
    :gain 0 :enabled true :selected true))
(def mnm-filter-curve ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (response-curve-editor :debug-name "mnm-filter" :width 35.2 :height 2.2
      :mode :filter :freq-min 20 :freq-max 18000 :gain-min -24 :gain-max 18
      :background-color (mnm-accent) :stroke-color :black :point-color :black
      :grid-color (rgba 0 0 0 0.15) :stroke-width 3
      :bands (list
        (mnm-filter-band 0 "highpass" (mnm-bind "flt_base") (mnm-bind "flt_res_hi"))
        (mnm-filter-band 1 "lowpass"
          (min 16000 (max 30 (* (mnm-value "flt_base") (pow 2 (mnm-value "flt_width")))))
          (mnm-bind "flt_res_lo")))
      :on-action (lambda (event)
        (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
          (if (= (get event :id) 0)
            (do (mnm-write scope "flt_base" (get event :freq))
                (mnm-write scope "flt_res_hi" (get event :q)))
            (do (mnm-write scope "flt_width"
                  (/ (log (/ (max 30 (get event :freq))
                    (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding
                      (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope "flt_base"))))) (log 2)))
                (mnm-write scope "flt_res_lo" (get event :q)))) false)))))
(def mnm-filter-page ()
  (v-stack :gap 0.15
    (mnm-filter-curve)
    (mnm-caption "Filter envelope")
    (mnm-ahd "fenv_attack_ms" false "fenv_decay_ms" 2)
    (h-stack :gap 0.3
      (mnm-num "flt_res_hi" "HP Res" 2) (mnm-num "flt_res_lo" "LP Res" 2)
      (mnm-num "fenv_attack_ms" "Attack ms" 1) (mnm-num "fenv_decay_ms" "Decay ms" 1))
    (h-stack :gap 0.3
      (mnm-num "env_to_base" "Env > Base" 2) (mnm-num "env_to_width" "Env > Width" 2)
      (mnm-num "keytrack" "Keytrack" 2))))


(def mnm-color-page ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (v-stack :gap 0.2
      (response-curve-editor :debug-name "mnm-eq" :width 35.2 :height 3.5 :mode :eq
        :bands (list (dict :id 0 :type "bell" :freq (mnm-bind "eq_freq")
          :freq-min 40 :freq-max 11000 :gain (mnm-bind "eq_gain_db")
          :gain-min -30 :gain-max 30 :q (mnm-bind "eq_q") :q-min 0.5 :q-max 12 :enabled true :selected true))
        :freq-min 20 :freq-max 18000 :gain-min -30 :gain-max 30
        :background-color (mnm-accent) :stroke-color :black :point-color :black
        :grid-color (rgba 0 0 0 0.15) :stroke-width 3
        :on-action (lambda (event)
          (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
            (do (mnm-write scope "eq_freq" (get event :freq))
                (mnm-write scope "eq_gain_db" (get event :gain))
                (mnm-write scope "eq_q" (get event :q))) false)))
      (h-stack :gap 0.3
        (mnm-num "eq_freq" "EQ Hz" 0) (mnm-num "eq_gain_db" "EQ dB" 1)
        (mnm-num "eq_q" "EQ Q" 2))
      (h-stack :gap 0.3
        (mnm-num "am_rate" "AM Hz" 1) (mnm-num "am_depth" "AM Depth" 2))
      (mnm-caption "AM > Filter > Sample reduction > EQ > Drive"))))

(export mnm-option mnm-display mnm-source-preview)
(def mnm-option (name options)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (dropdown :debug-name (str "mnm-option-" name) :width 16 :height 0.8 :font-size 8
      :options options :value-index (mnm-bind name)
      :bg-color :instrument-control-bg :text-color (mnm-accent) :chevron-color (mnm-accent)
      :on-change (lambda (v)
        (mnm-write scope name (eseq.effects.param-controls/custom-ui-option-index options v))))))
(def mnm-display (title titles pages)
  (box :debug-name "mnm-display" :width 35.8 :height 9.8 :padding 0.3 :background-color (mnm-accent)
    (v-stack :gap 0.25
      (label (str title " / " (nth titles (mnm-section))) :width 35.2 :height 0.7
        :h-align :center :v-align :center :font-size 8 :color (mnm-accent) :bg :black)
      ((nth pages (mnm-section))))))
;; Ideal oscillator shape, before sync, ring modulation and the track chain.
;; Noise is intentionally omitted: there is no deterministic per-note waveform.
(defwidget mnm-source-wave
  :width 35.2 :height 4.2 :state (wave pw) :bindable (wave pw)
  :shader
  (let ((u (fract (* 2 (/ (+ (/ x aspect) 1) 2))))
        (saw (- (* u 2) 1))
        (pulse (if (< u pw) 1 -1))
        (tri (- (* 4 (abs (- u 0.5))) 1))
        (a (if (< wave 0.5) tri (if (< wave 1.5) saw (if (< wave 2.5) pulse (* saw pulse))))))
    (sdf/layer
      (sdf/region :width (sdf/rect width height) :yellow)
      (sdf/paint (sdf/rect aspect 0.007) (rgba 0 0 0 0.2))
      (sdf/paint (- (abs (+ y (* 0.7 a))) 0.022) :black)
      (if (< (abs (- wave 2)) 0.5)
        (sdf/paint (max (- (min (abs (- u pw)) (min u (- 1 u))) (/ 0.015 aspect))
          (- (abs y) 0.7)) :black)
        (rgba 0 0 0 0)))))
(def mnm-source-preview (wave width-param)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (gesture (dict :start nil)))
    (mnm-source-wave :debug-name "mnm-source-wave" :wave wave :pw (mnm-bind width-param)
      :on-mouse-down (lambda (x y region) (set! gesture.start (list x (mnm-value width-param))))
      :on-mouse-up (lambda (x y region) (set! gesture.start nil))
      :on-drag (lambda (x y region)
        (if gesture.start
          (mnm-write scope width-param (+ (nth gesture.start 1) (* 0.4 (- x (nth gesture.start 0))))) false)))))
