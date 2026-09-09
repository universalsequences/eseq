;; FM Formant: performance controls, operator mixer, contextual editing.
;; All edits use the host's scoped parameter/modulation/p-lock callbacks.
(def ff-accent () :control-on-bg)
(def ff-letter (i) (nth '("A" "B" "C" "D") (- i 1)))
(def ff-bound (name fallback)
  (eseq.effects.custom-ui-controls/ui-param-bound-value name fallback))
(def ff-knob (section name title taper)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title
    7.4 1.8 2.3 (ff-accent) 2 taper :widget-knob-track 9 8 :right))
(def ff-num (section name title) (ff-num-in section name title "detail"))
(def ff-num-in (section name title role)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "ff-num-mod-" role "-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "ff-num-" role "-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width 7.4 :height 1.05 :gap 0.08
          (label title :v-align :center :height 0.43 :font-size 8 :color :dim :bg :transparent)
          (number-picker :width 7.4 :height 0.5 :noui true :font-size 9 :decimals 2 :step 0.01
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p)
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)))))))
(def ff-option (section name title options)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "ff-option-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "ff-option-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (v-stack :width 7.4 :height 1.2 :gap 0.06
          (label title :v-align :center :height 0.4 :font-size 8 :color :dim :bg :transparent)
          (dropdown :width 7.4 :height 0.7 :font-size 8 :options options
            :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :value-index-offset (get p :min)
            :bg-color :instrument-control-bg :text-color (ff-accent)
            :chevron-color (ff-accent) :border-width 0 :badge-color :transparent
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (lambda (value)
              (do
                (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
                (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                  (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options value)))))))))))
(def ff-title (title)
  (label title :v-align :center :height 0.55 :font-size 8.5 :color :dim :bg :transparent))
(def ff-nav (section title width)
  (button title :width width :height 0.8 :padding 0.05 :font-size 8 :corner-radius 2
    :color (if (= eseq.vanilla/custom-ui-selected-section section) :control-on-fg :dim)
    :background-color (if (= eseq.vanilla/custom-ui-selected-section section) (ff-accent) :instrument-control-bg)
    :border-color :transparent
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))
(def ff-env-plot (section prefix width)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :width width :height 2.0 :debug-name (str "ff-env-" prefix)
        :attack (ff-bound (str prefix "attack") 0)
        :decay (ff-bound (str prefix "decay") 100)
        :sustain (ff-bound (str prefix "sustain") 0.5)
        :release (ff-bound (str prefix "release") 100)
        :attack-max 10000 :decay-max 10000 :release-max 20000
        :curve-color (ff-accent) :point-color (ff-accent)
        :background-color :instrument-control-bg :grid-color :border-inactive
        :on-change (lambda (env)
          (do
            (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
            (eseq.effects.custom-ui-sections/custom-ui-set-active-adsr scope section (get env :active))
            (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
              (str prefix "attack") (str prefix "decay")
              (str prefix "sustain") (str prefix "release") env))))))
(def ff-env (section prefix title)
  (v-stack :gap 0.1
    (if title (ff-title title) false)
    (ff-env-plot section prefix 30.2)
    (h-stack :gap 0.2
      (ff-num section (str prefix "attack") "Attack ms")
      (ff-num section (str prefix "decay") "Decay ms")
      (ff-num section (str prefix "sustain") "Sustain")
      (ff-num section (str prefix "release") "Release ms"))))
(def ff-env-half (section prefix title)
  (v-stack :width 15 :gap 0.15
    (ff-title title)
    (ff-env-plot section prefix 15)
    (h-stack :gap 0.2
      (ff-num section (str prefix "attack") "Attack ms")
      (ff-num section (str prefix "decay") "Decay ms"))
    (h-stack :gap 0.2
      (ff-num section (str prefix "sustain") "Sustain")
      (ff-num section (str prefix "release") "Release ms"))))
(def ff-performance ()
  (box :width 20 :height 9.4 :padding 0.35 :corner-radius 3 :background-color :instrument-group-bg
    :debug-name "ff-performance"
    (v-stack :gap 0.15
      (h-stack :gap 2.4 (ff-knob eseq.vanilla/custom-ui-selected-section "formant_shift" "Formant shift" "linear")
        (ff-knob eseq.vanilla/custom-ui-selected-section "fm_intensity" "FM intensity" "linear"))
      (h-stack :gap 2.4 (ff-knob eseq.vanilla/custom-ui-selected-section "width_scale" "Bandwidth" "log")
        (ff-knob eseq.vanilla/custom-ui-selected-section "breath" "Breath" "linear"))
      (h-stack :gap 2.4 (ff-knob eseq.vanilla/custom-ui-selected-section "motion_position" "Position" "linear")
        (ff-knob eseq.vanilla/custom-ui-selected-section "motion_amount" "Motion mix" "linear"))
      (h-stack :gap 2.4 (ff-knob eseq.vanilla/custom-ui-selected-section "skirt_offset" "Skirt offset" "linear")
        (ff-knob eseq.vanilla/custom-ui-selected-section "attack_scale" "Attack scale" "log"))
      (h-stack :gap 0.1 (ff-nav 0 "Algorithm" 4.6) (ff-nav 17 "Motion" 4.6) (ff-nav 18 "Keys" 4.6) (ff-nav 23 "Amp" 4.6)))))
(def ff-global ()
  (v-stack :gap 0.15
    (ff-env 23 "master_" false)
    (h-stack :gap 0.2
      (ff-knob 23 "volume_db" "Output dB" "linear"))
    (h-stack :gap 0.2
      (ff-option 23 "phase_reset" "Phase" '("Continue" "Reset"))
      (ff-option 23 "legato_retrigger" "Legato env" '("Hold" "Retrigger"))
      (ff-num 23 "noise_seed" "Noise seed"))))
(def ff-motion ()
  (v-stack :gap 0.3
    (ff-title "MOTION PLAYBACK")
    (h-stack :gap 0.2
      (ff-option 17 "motion_mode" "Playback" '("Manual" "One shot" "Loop" "Transport"))
      (ff-option 17 "motion_sync" "Note timing" '("Seconds" "Tempo"))
      (ff-knob 17 "motion_seconds" "Duration s" "log")
      (ff-knob 17 "motion_bars" "Duration bars" "log"))
    (h-stack :gap 0.2
      (ff-knob 17 "wheel_motion" "Wheel > position" "linear"))))
(def ff-keyboard ()
  (v-stack :gap 0.2
    (ff-env 18 "pitch_" "PITCH ENVELOPE")
    (h-stack :gap 0.2
      (ff-knob 18 "pitch_env_depth" "Depth oct" "linear")
      (ff-knob 18 "bend_range" "Bend semitones" "linear")
      (ff-knob 18 "pressure_fm" "Pressure > FM" "linear")
      (ff-knob 18 "wheel_motion" "Wheel > position" "linear"))))
(def ff-voice (s p)
  (v-stack :gap 0.12
    (h-stack :gap 0.2
      (ff-option s (str p "mode") "Wave" '("Sine FM" "Formant"))
      (ff-option s (str p "fixed") "Tuning" '("Ratio" "Fixed Hz"))
      (ff-num s (str p "fixed_hz") "Fixed Hz")
      (ff-num s (str p "cents") "Detune cents"))
    (h-stack :gap 0.2
      (ff-knob s (str p "ratio") "Ratio" "log")
      (ff-knob s (str p "center") "Formant Hz" "log")
      (ff-knob s (str p "width") "Width Hz" "log")
      (ff-knob s (str p "skirt") "Skirt" "linear"))
    (h-stack :gap 0.2
      (ff-num s (str p "level") "Level")
      (ff-num s (str p "output") "To output")
      (ff-num s (str p "pan") "Pan")
      (ff-num s (str p "tracking") "Key tracking"))
    (ff-env s p false)))
(def ff-noise (s p)
  (v-stack :gap 0.2
    (h-stack :gap 0.2
      (ff-knob s (str p "center") "Center Hz" "log")
      (ff-knob s (str p "width") "Width Hz" "log")
      (ff-knob s (str p "level") "Noise level" "linear")
      (ff-knob s (str p "pan") "Pan" "linear"))
    (ff-env s p false)
))
(def ff-frequency (s p)
  (v-stack :gap 0.2
    (ff-env s (str p "freq_") false)
    (ff-knob s (str p "freq_depth") "Depth oct" "linear")))
(def ff-flatten (lists) (reduce (lambda (acc xs) (append acc xs)) '() lists))
(def ff-concat3 (a b c) (append (append a b) c))
;; Quick routes change only PM, feedback and output sends in one host transaction.
(def ff-route-config (mode)
  (let ((span (nth '(1 2 4) mode)))
    (ff-concat3
      (ff-flatten (map (lambda (i)
        (map (lambda (j) (dict :name (str "pm_" j "_to_" i)
          :value (if (and (= j (- i 1)) (not (= (mod (- i 1) span) 0))) 1 0)))
          (range 1 i))) (range 1 5)))
      (ff-flatten (map (lambda (i)
        (map (lambda (j) (dict :name (str "fb_" j "_to_" i) :value 0)) (range 1 5))) (range 1 5)))
      (map (lambda (i) (dict :name (str "v" i "_output")
        :value (if (= (mod i span) 0) 1 0))) (range 1 5)))))
(def ff-route-selected (config)
  (= 0 (len (filter (lambda (p)
    (> (abs (- (reactive-value (ff-bound (get p :name) 0)) (get p :value))) 0.0001)) config))))
(def ff-route-apply (mode)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)) (config (ff-route-config mode)))
    (lambda (x y r)
      (let ((first (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope "v1_output"))
            (updates (map (lambda (p)
              (dict :param-idx (get (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope (get p :name)) :idx)
                    :value (get p :value))) config)))
        (if (eseq.effects.param-controls/instrument-rack-target? first)
          (host-command (if (seq-has-selection?) "set-rack-slot-instrument-plock-batch" "set-rack-slot-instrument-param-batch")
            (dict :track (get first :rack-track) :slot (get first :rack-slot)
              :updates updates :commit true :gesture "fm-routing" :label "FM routing"))
          (host-command (if (seq-has-selection?) "set-instrument-plock-batch" "set-instrument-param-batch")
            (dict :updates updates :commit true :gesture "fm-routing" :label "FM routing")))))))
(defmacro ff-alg-node (i)
  `(let ((xx (* width (- (* (/ (+ (floor (/ ,i span)) 0.5) (/ 4 span)) 1.75) 0.875)))
         (yy (if (= span 1) -0.25 (+ -0.72 (* (/ (mod ,i span) (- span 1)) 1.25)))))
    (sdf/layer
      (sdf/stroke (sdf/line xx yy xx
        (if (= (mod (+ ,i 1) span) 0) 0.82 (+ yy (/ 1.25 (- span 1))))) 0.025 ink)
      (sdf/fill (sdf/translate xx yy (sdf/rect 0.13 (if (= span 8) 0.055 0.09))) ink))))
(defwidget ff-algorithm
  :width 15 :height 2 :state (mode selected) :bindable (selected)
  :shader
  (let ((span (if (= mode 0) 1 (if (= mode 1) 2 4)))
        (ink (if (> selected 0.5) :control-on-bg :dim)))
    (sdf/layer
      (sdf/fill (sdf/rect width height) :instrument-control-bg)
      (sdf/stroke (sdf/line (* width -0.9) 0.82 (* width 0.9) 0.82) 0.035 ink)
      (ff-alg-node 0) (ff-alg-node 1) (ff-alg-node 2) (ff-alg-node 3)
      )))
(def ff-alg-choice (mode)
  (subtree :key (str "ff-alg-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" mode)
    (v-stack :width 15 :gap 0.1
      (label (nth '("Parallel" "2 stacks" "Chain") mode)
        :v-align :center :height 0.5 :font-size 8 :bg :transparent)
      (ff-algorithm :width 15 :height 2 :mode mode
        :selected (if (ff-route-selected (ff-route-config mode)) 1 0)
        :debug-name (str "ff-algorithm-" mode) :on-click (ff-route-apply mode)))))
(def ff-algorithms ()
  (v-stack :gap 0.2
    (ff-title (if (or (ff-route-selected (ff-route-config 0)) (ff-route-selected (ff-route-config 1))
                     (ff-route-selected (ff-route-config 2)))
      "Algorithm" "Custom algorithm"))
    (h-stack :gap 0.2 (ff-alg-choice 0) (ff-alg-choice 1))
    (h-stack :gap 0.2 (ff-alg-choice 2))
    (h-stack :gap 0.2
      (map (lambda (i) (ff-nav (+ 4 (* (- i 1) 4)) (ff-letter i) 3.6)) (range 1 5)))))
;; Eight direct uniforms: incoming FM and one-sample feedback from A-D.
;; Each node opens that oscillator's own incoming-route editor.
(defmacro ff-incoming-node (xx direct feedback identity)
  `(sdf/layer
    (if (> (abs ,direct) 0.000001)
      (sdf/stroke (sdf/line (* width ,xx) -0.45 -0.2 0.6) 0.025 :control-on-bg)
      (rgba 0 0 0 0))
    (if (> (abs ,feedback) 0.000001)
      (sdf/stroke (min
        (sdf/line (* width ,xx) -0.45 (* width ,xx) 0.05)
        (sdf/line (* width ,xx) 0.05 0.2 0.6)) 0.025 :dim)
      (rgba 0 0 0 0))
    (sdf/region ,identity (sdf/translate (* width ,xx) -0.5 (sdf/rect (* width 0.04) 0.2))
      (material :color (if hit/hover :control-on-bg :dim)))))
(defwidget ff-incoming
  :width 30.2 :height 1.3
  :state (d1 d2 d3 d4 f1 f2 f3 f4)
  :bindable (d1 d2 d3 d4 f1 f2 f3 f4)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rect width height) :instrument-control-bg)
    (ff-incoming-node -0.75 d1 f1 :a)
    (ff-incoming-node -0.25 d2 f2 :b)
    (ff-incoming-node 0.25 d3 f3 :c)
    (ff-incoming-node 0.75 d4 f4 :d)
    (sdf/stroke (sdf/translate 0 0.65 (sdf/rect 0.45 0.18)) 0.035 :control-on-bg)))
(def ff-incoming-view (i)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (v-stack :gap 0.02
      (h-stack :gap 0.2 (map (lambda (j)
        (label (ff-letter j) :width 7.4 :height 0.4 :font-size 8 :h-align :center :v-align :center :bg :transparent)) (range 1 5)))
      (ff-incoming :debug-name "ff-incoming"
        :d1 (if (> i 1) (ff-bound (str "pm_1_to_" i) 0) 0) :f1 (ff-bound (str "fb_1_to_" i) 0)
        :d2 (if (> i 2) (ff-bound (str "pm_2_to_" i) 0) 0) :f2 (ff-bound (str "fb_2_to_" i) 0)
        :d3 (if (> i 3) (ff-bound (str "pm_3_to_" i) 0) 0) :f3 (ff-bound (str "fb_3_to_" i) 0)
        :d4 (if (> i 4) (ff-bound (str "pm_4_to_" i) 0) 0) :f4 (ff-bound (str "fb_4_to_" i) 0)
        :on-click (lambda (x y region)
          (let ((j (get (dict :a 1 :b 2 :c 3 :d 4 ) region)))
            (if j (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope (+ 4 (* (- j 1) 4))) false)))))))
(def ff-routes (s pair)
  (v-stack :gap 0.2
    (ff-incoming-view pair)
    (ff-title (str "FM → " (ff-letter pair)))
    (h-stack :gap 0.2
      (map (lambda (j) (if (< j pair) (ff-num s (str "pm_" j "_to_" pair) (str "From " (ff-letter j)))
        (label "-" :v-align :center :width 7.4 :height 1 :color :dim))) '(1 2 3 4)))
    (ff-title "Feedback amount")
    (h-stack :gap 0.2 (map (lambda (j) (ff-num s (str "fb_" j "_to_" pair) (str "From " (ff-letter j)))) '(1 2 3 4)))))
(def ff-noise-selected? (s)
  (or (and (>= s 19) (< s 23)) (and (> s 0) (< s 17) (= (mod (- s 1) 4) 1))))
(def ff-selected-osc (s)
  (if (and (>= s 19) (< s 23)) (+ 1 (- s 19))
    (if (and (> s 0) (< s 17)) (+ 1 (floor (/ (- s 1) 4))) 1)))
(def ff-detail ()
  (let ((s eseq.vanilla/custom-ui-selected-section))
    (box :width 32 :height 9.4 :padding 0.35 :corner-radius 3
      :background-color :instrument-panel-bg :debug-name "ff-detail"
      (v-stack :gap 0.18
        (if (or (and (> s 0) (< s 17)) (and (>= s 19) (< s 23)))
          (let ((i (ff-selected-osc s)) (noise (ff-noise-selected? s)))
            (let ((base (+ 1 (* (- i 1) 4))) (v (str "v" i "_")) (n (str "n" i "_")))
              (v-stack :gap 0.15
                (if noise
                  (h-stack :gap 0.2
                    (ff-nav (+ base 1) (str "Noise " (ff-letter i)) 15)
                    (ff-nav (+ 18 i) "Frequency envelope" 15))
                  (h-stack :gap 0.2
                    (ff-nav base (str "Osc " (ff-letter i)) 10)
                    (ff-nav (+ base 2) "Frequency env" 10)
                    (ff-nav (+ base 3) "FM routing" 10)))
                (if noise
                  (if (>= s 19) (ff-frequency s n) (ff-noise s n))
                  (if (= s base) (ff-voice s v)
                    (if (= s (+ base 2)) (ff-frequency s v) (ff-routes s i)))))))
          (if (= s 0) (ff-algorithms) (if (= s 17) (ff-motion) (if (= s 18) (ff-keyboard) (ff-global)))))))))
(def ff-oscillator (i noise)
  (let ((s (+ 1 (* (- i 1) 4))) (prefix (str (if noise "n" "v") i "_")))
    (let ((page (+ s (if noise 1 0))))
      (box :width 16 :height 2.0 :padding 0.15 :corner-radius 3 :background-color :instrument-group-bg
        (v-stack :gap 0.05
          (ff-nav page (ff-letter i) 15.5)
          (h-stack :gap 0.15
            (ff-num-in page (str prefix (if noise "center" "ratio")) (if noise "Frequency Hz" "Ratio") "mixer")
            (ff-num-in page (str prefix "level") "Level" "mixer")))))))
(def ff-bank ()
  (let ((s eseq.vanilla/custom-ui-selected-section))
    (let ((noise (ff-noise-selected? s)) (base (+ 1 (* (- (ff-selected-osc s) 1) 4))))
      (v-stack :gap 0.15 :debug-name "ff-oscillators"
        (h-stack :gap 0.2
          (ff-nav base "Oscillators" 7.9)
          (ff-nav (+ base 1) "Noise" 7.9))
        (ff-oscillator 1 noise)
        (ff-oscillator 2 noise)
        (ff-oscillator 3 noise)
        (ff-oscillator 4 noise)))))
(defsynth-ui
  (h-stack :gap 0.2 :align :start
    (ff-bank)
    (ff-detail)
    (ff-performance)))
