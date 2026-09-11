;; Shared identified-drum surface. Config carries only voice differences;
;; every parameter binding and callback retains the current instrument scope.
(module eseq.effects.identified-drum)
(export panel)
(def drum-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def drum-section () eseq.vanilla/custom-ui-selected-section)
(def drum-p (name) (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
(def drum-bind (name) (eseq.effects.custom-ui-runtime/custom-ui-param-binding (drum-p name)))
(def drum-value (name) (reactive-value (drum-bind name)))
(def drum-knob (section name title width decimals taper)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title width 3.45 2.15
    (drum-c) decimals taper :widget-knob-track 8.5 8 :center))
(def drum-label (title width)
  (box :width width :height 0.72 :background-color (drum-c)
    (label title :width width :height 0.72 :h-align :center :v-align :center
      :font-size 8.2 :color :black :bg :transparent)))
(def drum-panel (section width body)
  (box :width width :height 4.8 :padding 0.18
    :background-color (if (= (drum-section) section) :instrument-panel-bg :instrument-group-bg)
    :debug-name (str "kick-panel-" section)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section) body))
(def drum-num (section name title width decimals)
  (let ((p (drum-p name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white :black)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "kick-mod-" name)
      (subtree :key (str "kick-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width width :height 1.15 :gap 0.08
          (label title :height 0.5 :v-align :center :font-size 8 :color ink :bg :transparent)
          (number-picker :width width :height 0.55 :noui true :font-size 8.5 :decimals decimals
            :step (if (= name "bank_harm") 0.5 (pow 10 (- 0 decimals)))
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) ink)
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)))))))

;; Closed-form integrated exponential pitch sweep, at the identified
;; reference pitch. This illustrates the editable source components, not the
;; bank's audio output. The 909's fixed learned Fourier corrections and the
;; voices' saturation, parameter slew and retrigger history are not modeled.
;; Shader macros use qualified calls because defwidget expands shaders outside
;; the authoring module; widget constructors themselves use the flat registry.
(defmacro drum-source-paint (overlay)
  `(let ((u (clamp (/ (+ (/ x aspect) 0.94) 1.88) 0 1))
        (duration (if (< mode 0.5) 1.2 (if (< mode 1.5) 0.035 0.25)))
        (t (* duration u u))
        (rate (/ -6.9077553 (* 0.001 (max sweep 50))))
        (freq (* reference (pow 2 (/ (+ tune note) 12))))
        (phase (* 6.2831853 (+ (* freq t) (* (/ (* freq (- ratio 1)) rate) (- (exp (* rate t)) 1)))))
        (env (+ (clamp sustain 0 1) (* (- 1 (clamp sustain 0 1)) (exp (/ (* -6907.7553 t) (max decay 1))))))
        (ramp (if (< attack 0.5) 1 (clamp (/ (* 1000 t) (max attack 0.5)) 0 1)))
        (body (* amp env ramp (exp (* curvature t t))
          (+ (sin phase) (* asym (sin (- (* 2 phase) 0.62)) (exp (* -17 t)))
            (* odd (+ (/ (sin (* 3 phase)) 9) (/ (sin (* 5 phase)) 25))))))
        (transient (* (exp (* trate t)) (if (< mode 1.5)
          (* amp (sin (* 6.2831853 tfreq t)))
          (+ (* 0.5 (sin (* tfreq 6.2831853 t))) (* 0.3 (sin (+ 1 (* tfreq 3.771 t)))) (* 0.2 (sin (+ 2 (* tfreq 1.713 t))))))))
        (signal (* 0.65 (clamp (if (< mode 0.5) body transient) -1.1 1.1)))
        (silhouette (max (- (min 0 signal) y) (- y (max 0 signal))))
        (hx (* aspect (- (* 1.88 (sqrt (/ sweep 1200))) 0.94))))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :cyan)
      (sdf/paint (sdf/rect (* aspect 0.94) 0.006) (rgba 0 0 0 0.35))
      (sdf/paint (max silhouette (- (abs x) (* aspect 0.94))) :black)
      ,overlay)))

(defwidget eseq-identified-drum-source
  :width 35.2 :height 4.65
  :state (tune note ratio sweep decay attack sustain amp asym mode tfreq trate reference odd curvature)
  :bindable (tune note ratio sweep decay attack sustain amp asym mode tfreq trate reference odd curvature)
  :shader
  (eseq.effects.identified-drum/drum-source-paint
    (sdf/layer
      (sdf/region :sweep
        (sdf/translate hx -0.82 (sdf/rect 0.045 0.045))
        (material :color (if hit/hover :white :black))
        (sdf/translate hx -0.82 (sdf/rect 0.11 0.12)))
      (sdf/paint (sdf/translate hx -0.45 (sdf/rect 0.005 0.32)) (rgba 0 0 0 0.4)))))

(defwidget eseq-identified-drum-transient
  :width 35.2 :height 4.65
  :state (tune note ratio sweep decay attack sustain amp asym mode tfreq trate reference odd curvature)
  :bindable (tune note ratio sweep decay attack sustain amp asym mode tfreq trate reference odd curvature)
  :shader (eseq.effects.identified-drum/drum-source-paint (rgba 0 0 0 0)))

(def drum-source (config mode)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (gesture (eseq.effects.drum-surface/parameter-gesture
          (if (= mode 2) "noise_decay" "click_decay") (if (= mode 1) "click_amp" false)))
        (draw (if (= mode 0) eseq.vanilla/eseq-identified-drum-source eseq.vanilla/eseq-identified-drum-transient)))
    (subtree :key (str "kick-source-" mode "-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
      (draw :debug-name "kick-source" :width 35.2 :height 4.65
        :note (eseq.effects.custom-ui-runtime/custom-ui-param-binding (eseq.effects.custom-ui-runtime/custom-ui-current-base-note-param))
        :tune (drum-bind "tune") :ratio (drum-bind "start_ratio") :sweep (drum-bind "sweep")
        :decay (drum-bind "decay") :attack (drum-bind "attack") :sustain (drum-bind "sustain")
        :amp (drum-bind (if (= mode 0) "body_amp" "click_amp")) :asym (drum-bind "body_asymmetry")
        :reference (get config :reference)
        :odd (if (get config :odd) (drum-bind "body_harmonic") 0)
        :curvature (if (get config :curvature) (drum-bind "amp_curve") 0)
        :mode mode :tfreq (drum-bind (if (= mode 2) "noise_cutoff" "click_freq"))
        :trate (drum-bind (if (= mode 2) "noise_decay" "click_decay"))
        :on-mouse-down (get gesture :down) :on-mouse-up (get gesture :up)
        :on-drag (lambda (sx sy region)
          (if (= region :sweep)
            (let ((u (max 0 (min 1 (/ (+ sx 0.94) 1.88)))))
              (eseq.effects.custom-ui-runtime/custom-ui-set-param-by-name-in-scope scope "sweep"
                (max 50 (min 900 (* 1200 u u)))))
            (if (> mode 0) ((get gesture :drag) sx sy region) false)))))))

(defmacro drum-bank-handle (name hx hy)
  `(sdf/region ,name
     (sdf/translate ,hx ,hy (sdf/rect 0.045 0.045))
     (material :color (if hit/hover :white :black))
     (sdf/translate ,hx ,hy (sdf/rect 0.12 0.12))))

(defwidget eseq-identified-drum-bank-sweep
  :width 35.2 :height 2.95
  :state (floor depth duration tune note track)
  :bindable (floor depth duration tune note track)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.94) 1.88) 0 1))
        (key-offset (* (clamp track 0 1) (/ (* (+ tune note) 0.057762265) 5.586)))
        (base (+ floor key-offset))
        ;; Two seconds, with extra space for short sweeps. The time handle
        ;; sits at one time constant (37% depth), not on the nearly-flat T60.
        (v (clamp (+ base (* depth (exp (/ (* -13815.5106 u u) (max duration 20))))) 0 1))
        (line (- (abs (- y (- 0.72 (* 1.44 v)))) 0.018))
        (tx (* aspect (- (* 1.88 (sqrt (/ duration 13815.5106))) 0.94)))
        (ty (- 0.72 (* 1.44 (clamp (+ base (* depth 0.36787944)) 0 1)))))
    (sdf/layer
      (sdf/paint (sdf/rect width height) :cyan)
      (sdf/paint (sdf/translate 0 0.72 (sdf/rect (* aspect 0.94) 0.006)) (rgba 0 0 0 0.3))
      (sdf/paint (max line (- (abs x) (* aspect 0.94))) :black)
      (eseq.effects.identified-drum/drum-bank-handle :depth (* aspect -0.94) (- 0.72 (* 1.44 (clamp (+ base depth) 0 1))))
      (eseq.effects.identified-drum/drum-bank-handle :floor (* aspect 0.94) (- 0.72 (* 1.44 (clamp base 0 1))))
      (eseq.effects.identified-drum/drum-bank-handle :time tx ty))))

;; Small diagrams explain the controls, not an audio analysis of the bank.
(defmacro drum-bank-divisor (h)
  `(if (< ,h 1) 1 (if (< ,h 2) 1.2 (if (< ,h 3) 1.5
     (if (< ,h 4) 2 (if (< ,h 5) 3 (if (< ,h 6) 4 (if (< ,h 7) 5 7))))))))
(defmacro drum-tanh (v) `(- (/ 2 (+ 1 (exp (* -2 ,v)))) 1))
(defwidget eseq-identified-bank-shape
  :width 8.65 :height 1.0
  :state (mode value)
  :bindable (value)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.9) 1.8) 0 1))
        (h (* 0.5 (round (* value 2))))
        (divisor (mix (eseq.effects.identified-drum/drum-bank-divisor (floor h)) (eseq.effects.identified-drum/drum-bank-divisor (+ 1 (floor h))) (fract h)))
        ;; Clock pulses across 0.2 cutoff periods: the DSP ratio is
        ;; 100 * 0.25^crunch before sample-rate clipping and jitter.
        (clock (if (< (fract (* u 20 (pow 0.25 value))) 0.2) -0.6 0.6))
        ;; Reconstruction is a dry / cascaded-RC crossfade. Show its
        ;; actual weights, rather than inventing a smoothed waveform.
        (weights (min
          (max (- (abs (+ y 0.35)) 0.12) (- u (- 1 value)))
          (max (- (abs (- y 0.35)) 0.12) (- u value))))
        (input (* (- (* 2 u) 1) (+ 1 (* 24 value))))
        (tube (max input -2.4))
        (diode (if (< input 0) (* 1.2 (eseq.effects.identified-drum/drum-tanh (/ input 1.2)))
          (if (< input 0.35) input (+ 0.35 (/ (- 1 (exp (* -3 (- input 0.35)))) 3)))))
        (transfer (+ (* 0.55 (eseq.effects.identified-drum/drum-tanh (+ tube (* 0.2 tube tube)))) (* 0.45 diode)))
        (curve (if (< mode 0.5)
          (min (- (abs (- y (if (< (fract (* u 8)) 0.2) -0.65 -0.15))) 0.025)
               (- (abs (- y (if (< (fract (* u 8)) 0.2)
                 (if (> (floor (/ (+ (floor (* u 8)) 1) divisor)) (floor (/ (floor (* u 8)) divisor))) 0.15 0.65) 0.65))) 0.025))
          (if (> mode 2.5) weights
            (- (abs (+ y (if (< mode 1.5) clock (* 0.65 transfer)))) 0.025)))))
    (sdf/layer
      (sdf/region :value (sdf/rect width height) :cyan)
      (sdf/paint (max curve (- (abs x) (* aspect 0.9))) :black))))

(def drum-bank-mini (name title mode maximum)
  (let ((p (drum-p name)) (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((change (lambda (sx sy region)
            (let ((v (* maximum (max 0 (min 1 (/ (+ sx 0.9) 1.8))))))
              (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                (if (= mode 0) (* 0.5 (round (* v 2))) v))))))
      (v-stack :width 8.65 :height 1.55 :gap 0.05
        (label title :height 0.45 :h-align :center :v-align :center :font-size 7.6 :color :black :bg :transparent)
        (eseq-identified-bank-shape :debug-name (str "kick-bank-mini-" name) :width 8.65 :height 1.0
          :mode mode :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
          :on-click change :on-drag change)))))

(def drum-bank-visual ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (gesture (dict :start nil)))
    (let ((read (lambda (name)
            (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding
              (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope name))))))
      (v-stack :width 35.2 :height 4.65 :gap 0.15
        (eseq-identified-drum-bank-sweep :debug-name "kick-bank-sweep" :width 35.2 :height 2.95
          :tune (drum-bind "tune") :track (drum-bind "bank_track")
          :note (eseq.effects.custom-ui-runtime/custom-ui-param-binding (eseq.effects.custom-ui-runtime/custom-ui-current-base-note-param))
          :floor (drum-bind "bank_freq") :depth (drum-bind "bank_env") :duration (drum-bind "bank_time")
          ;; Gesture-local snapshot preserves grab offset and permits editing
          ;; depth even when the cutoff clips at the top of the display.
          :on-mouse-down (lambda (sx sy region)
            (set! gesture.start (list sx sy (read "bank_freq") (read "bank_env") (read "bank_time"))))
          :on-mouse-up (lambda (sx sy region) (set! gesture.start nil))
          :on-drag (lambda (sx sy region)
            (if gesture.start
              (let ((dy (/ (- (nth gesture.start 1) sy) 1.44))
                    (u (+ (sqrt (/ (nth gesture.start 4) 13815.5106)) (/ (- sx (nth gesture.start 0)) 1.88))))
                (if (= region :time)
                  (eseq.effects.custom-ui-runtime/custom-ui-set-param-by-name-in-scope scope "bank_time"
                    (max 20 (min 2000 (* 13815.5106 (max 0 u) (max 0 u)))))
                  (if (= region :floor)
                    (eseq.effects.custom-ui-runtime/custom-ui-set-param-by-name-in-scope scope "bank_freq"
                      (max 0 (min 1 (+ (nth gesture.start 2) dy))))
                    (if (= region :depth)
                      (eseq.effects.custom-ui-runtime/custom-ui-set-param-by-name-in-scope scope "bank_env"
                        (max 0 (min 1 (+ (nth gesture.start 3) dy)))) false)))) false)))
        (h-stack :gap 0.2
          (drum-bank-mini "bank_harm" "Harmonic" 0 7)
          (drum-bank-mini "bank_crunch" "Crush" 1 1)
          (drum-bank-mini "bank_drive" "Static drive" 2 1)
          (drum-bank-mini "bank_recon" "Dry / 2×RC mix" 3 1))))))

(def drum-env ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :debug-name "kick-envelope" :width 35.2 :height 4.65
      :curve-color :black :point-color :black :grid-color :black :background-color (drum-c)
      :attack (drum-bind "attack") :decay (drum-bind "decay")
      :sustain (drum-bind "sustain") :release (drum-bind "release")
      :attack-max 1000 :decay-max 8000 :release-max 8000
      :on-change (lambda (env)
        (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope "attack" "decay" "sustain" "release" env)))))

(def drum-track-option ()
  (let ((p (drum-p "bank_track")) (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p "kick-track-mod"
      (v-stack :width 8.4 :height 1.15 :gap 0.08
        (label "Tracking" :height 0.5 :v-align :center :font-size 8 :color :black :bg :transparent)
        (dropdown :debug-name "kick-tracking" :width 8.4 :height 0.55 :font-size 8 :options '("free" "key")
          :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
          :text-color (drum-c) :chevron-color (drum-c) :bg-color :instrument-control-bg :border-color :transparent
          :on-change (lambda (v)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
              (eseq.effects.param-controls/custom-ui-option-index '("free" "key") v))))))))
(def drum-tone-curve ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (response-curve-editor :debug-name "kick-tone-curve" :width 35.2 :height 4.65
      :mode :filter :freq-min 20 :freq-max 20000 :gain-min -24 :gain-max 6
      :stroke-width 3 :stroke-color :black :point-color :black :background-color (drum-c) :grid-color (rgba 0 0 0 0.18)
      :bands (list
        (dict :id 0 :type "lowpass" :freq (drum-bind "lpf") :freq-min 200 :freq-max 18000
          :q 0.707 :gain 0 :lock-y true :enabled true :selected true)
        (dict :id 1 :type "highpass" :freq (drum-bind "hpf") :freq-min 20 :freq-max 500
          :q 0.707 :gain 0 :lock-y true :enabled true :selected true))
      :on-action (lambda (event)
        (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-by-name-in-scope scope
            (if (= (get event :id) 0) "lpf" "hpf") (get event :freq)) false)))))
(def drum-body-panel ()
  (drum-panel 0 20
    (v-stack :gap 0.14
      (drum-label "BODY" 6)
      (h-stack :gap 0.15
        (drum-knob 0 "tune" "Tune" 6.3 1 :linear)
        (drum-knob 0 "start_ratio" "Ratio" 6.3 3 :linear)
        (drum-knob 0 "body_amp" "Amp" 6.3 3 :linear)
      ))))
(def drum-amp-panel ()
  (drum-panel 1 20
    (v-stack :gap 0.14
      (drum-label "AMP" 6)
      (h-stack :gap 0.15
        (drum-knob 1 "decay" "Decay" 6.3 0 :log)
        (drum-knob 1 "release" "Release" 6.3 0 :log)
        (drum-knob 1 "level" "Level" 6.3 2 :linear)
      ))))
(def drum-click-panel ()
  (drum-panel 2 13
    (v-stack :gap 0.14
      (drum-label "CLICK" 6)
      (h-stack :gap 0.15
        (drum-knob 2 "click_freq" "Freq" 6.0 0 :log)
        (drum-knob 2 "click_amp" "Amp" 6.0 2 :linear)
      ))))
(def drum-noise-panel ()
  (drum-panel 3 13
    (v-stack :gap 0.14
      (drum-label "NOISE" 6)
      (h-stack :gap 0.15
        (drum-knob 3 "noise_cutoff" "Cutoff" 6.0 0 :log)
        (drum-knob 3 "noise_amp" "Amp" 6.0 6 :linear)
      ))))
(def drum-bank-panel ()
  (drum-panel 4 19
    (v-stack :gap 0.14
      (drum-label "BANK" 6)
      (h-stack :gap 0.15
        (drum-knob 4 "bank" "Amount" 6.0 2 :linear)
        (drum-knob 4 "bank_env" "Env" 6.0 2 :linear)
        (drum-knob 4 "bank_res" "Res" 6.0 2 :linear)
      ))))
(def drum-tone-panel ()
  (drum-panel 5 19
    (v-stack :gap 0.14
      (drum-label "TONE" 6)
      (h-stack :gap 0.15
        (drum-knob 5 "lpf" "LPF" 6.0 0 :log)
        (drum-knob 5 "hpf" "HPF" 6.0 0 :log)
        (drum-knob 5 "drive" "Drive" 6.0 2 :linear)
      ))))
(def drum-page-0 (config)
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (drum-num 0 "sweep" "Sweep ms" 8.4 1)
      (drum-num 0 "glide" "Glide ms" 8.4 0)
      (drum-num 0 "body_asymmetry" "Asymmetry" 8.4 3)
      (drum-num 0 "smoothing" "Smooth ms" 8.4 1)
    )
    (h-stack :gap 0.35
      (if (get config :odd) (drum-num 0 "body_harmonic" "Odd harmonics" 8.4 2) (box :width 8.4))
      (if (get config :curvature) (drum-num 0 "amp_curve" "Amp curve" 8.4 1) (box :width 8.4))
      (v-stack :gap 0.08
      (label "Note" :height 0.5 :font-size 8 :v-align :center :color :black :bg :transparent)
      (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-base-note-param)))
        (number-picker :width 5 :height 0.6 :noui true :decimals 0 :step 1 :font-size 8 :text-color :black
          :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
          :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
          :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
          :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p)))))
  ))
(def drum-page-1 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (drum-num 1 "attack" "Attack ms" 8.4 0)
      (drum-num 1 "sustain" "Sustain" 8.4 2)
    )
    (h-stack :gap 0.35
      (drum-num 1 "fade" "Fade ms" 8.4 0)
      (drum-num 1 "retrigger_fade" "Retrig ms" 8.4 1)
      (drum-num 1 "out_gain" "Gain" 8.4 3)
    )
  ))
(def drum-page-2 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (drum-num 2 "click_decay" "Decay 1/s" 8.4 0)
    )
  ))
(def drum-page-3 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (drum-num 3 "noise_decay" "Decay 1/s" 8.4 2)
    )
  ))
(def drum-page-4 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (drum-num 4 "bank_time" "Time ms" 8.4 0)
      (drum-num 4 "bank_freq" "Cutoff" 8.4 3)
      (drum-num 4 "bank_harm" "Harmonic" 8.4 1)
      (drum-num 4 "bank_crunch" "Crush" 8.4 2)
    )
    (h-stack :gap 0.35
      (drum-num 4 "bank_drive" "Drive" 8.4 2)
      (drum-num 4 "bank_recon" "Smooth" 8.4 2)
      (drum-track-option)
    )
  ))
(def drum-page-5 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (drum-num 5 "out_gain" "Gain" 8.4 3)
    )
    (h-stack :gap 0.35
      (drum-num 5 "fade" "Fade ms" 8.4 0)
      (drum-num 5 "retrigger_fade" "Retrig ms" 8.4 1)
    )
  ))
(def drum-page-button (section title)
  (button title :width 5.7 :height 0.8 :padding 0 :font-size 8
    :color (if (= (drum-section) section) (drum-c) :black)
    :border-color :transparent
    :background-color (if (= (drum-section) section) :black (drum-c))
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))
(def drum-display (config)
  (box :debug-name "kick-display" :width 35.8 :height 9.8 :padding 0.3 :background-color (drum-c)
    (v-stack :gap 0.15
      (label (str (get config :title) " / " (nth '("BODY SCHEMATIC" "AMP" "CLICK" "NOISE SHAPE" "BANK SWEEP" "TONE") (drum-section)))
        :width 35.2 :height 0.7 :h-align :center :v-align :center :font-size 8 :color (drum-c) :bg :black)
      (if (= (drum-section) 1) (drum-env)
        (if (= (drum-section) 4)
          (drum-bank-visual)
          (if (= (drum-section) 5) (drum-tone-curve)
            (drum-source config (if (= (drum-section) 2) 1 (if (= (drum-section) 3) 2 0))))))
      (box :width 35.2 :height 2.6
        (if (= (drum-section) 0) (drum-page-0 config)
          (if (= (drum-section) 1) (drum-page-1)
            (if (= (drum-section) 2) (drum-page-2)
              (if (= (drum-section) 3) (drum-page-3)
                (if (= (drum-section) 4) (drum-page-4) (drum-page-5)))))))
      (h-stack :gap 0.18
        (drum-page-button 0 "Body") (drum-page-button 1 "Amp")
        (drum-page-button 2 "Click") (drum-page-button 3 "Noise")
        (drum-page-button 4 "Bank") (drum-page-button 5 "Tone")))))
(def panel (config)
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2 (drum-body-panel) (drum-amp-panel))
    (drum-display config)
    (v-stack :gap 0.2 (drum-click-panel) (drum-noise-panel))
    (v-stack :gap 0.2 (drum-bank-panel) (drum-tone-panel))))
