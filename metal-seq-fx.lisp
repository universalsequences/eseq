;; metal-seq-fx.lisp — Effect chain UI for Metal Sequencer
;; Renders to *fx* buffer. Loaded by metal-seq-grid.lisp.

(defstate instrument-panel-tab 0)

(defwidget fx-panel-bg
  :width 1 :height 1
  :shader (sdf/layer
    (sdf/fill (+
        (* 0.05 (smoothstep 0 0.1 (* x y)))
        (sdf/rounded-rect (* 1 width) (* 1 height) 0.12))

      (material
        :color
        (mix
          :gray
          (rgba 0.10 0.10 0.11 1)
          (smoothstep 0 0.005 (- (abs d) 0.008))
          ) ))))

(defwidget compile-progress
  :width 12 :height 0.3
  :state (active)
  :shader
  (if (= active 0)
    (rgba 0 0 0 0)
    (let ((bar-w 0.3)
          (pos (fract (* 0.5 itime)))
          (bar-x (- (* pos (+ 1 bar-w)) (/ bar-w 2)))
          (d-bar (- (abs (- x bar-x)) (/ bar-w 2)))
          (bg (sdf/rounded-rect width height 0.06))
          (mask (max bg (- d-bar))))
      (sdf/layer
        (sdf/fill bg
          (material :color (rgba 0.15 0.15 0.17 1)))
          (sdf/fill mask
          (material :color
            (mix
              (rgba 0.3 0.5 1.0 1)
              (rgba 0.2 0.35 0.8 1)
              (smoothstep -0.02 0.02 d-bar))))))))

(def fx-set-instrument-value (p v)
  (if (= (get p :control) "base-note")
    (host-command "set-instrument-base-note" (dict :value v))
    (host-command
      (if (seq-has-selection?) "set-instrument-plock" "set-instrument-param")
      (dict :param-idx (get p :idx) :value v))))

(def fx-set-instrument-option (p label)
  (host-command
    (if (seq-has-selection?) "set-instrument-plock-option" "set-instrument-param-option")
    (dict :param-idx (get p :idx) :label label)))

(def fx-set-effect-value (fx p v)
  (if (seq-has-selection?)
    (seq-set-effect-plock (get fx :slot-idx) (get p :idx) v)
    (seq-set-effect-param (get fx :slot-idx) (get p :idx) v)))

(def fx-param-row (p fx)
  (h-stack :gap 0.5 :align :center
    (label (substring (get p :name) 0 12) :font-size 9 :width 9
           :color :gray :bg :transparent)
    (if (get p :boolean)
      (box :width 8 :height 1.3
           :bg :transparent
           :on-click |x y r|
             (if fx
               (fx-set-effect-value fx p (if (> (get p :value) 0.5) 0 1))
               (fx-set-instrument-value p (if (> (get p :value) 0.5) 0 1)))
        (label (if (> (get p :value) 0.5) "ON" "OFF")
               :font-size 11 :width 8
               :color :white :bg :transparent))
      (if (get p :options)
      (dropdown :value (get p :text-value)
        :options (get p :options)
        :on-change (lambda (v)
          (if fx
            (host-command
              (if (seq-has-selection?) "set-effect-plock-option" "set-effect-param-option")
              (dict :slot-idx (get fx :slot-idx) :param-idx (get p :idx) :label v))
            (fx-set-instrument-option p v)))
        :width 8 :height 1.3 :font-size 11)
      (number-picker :value (get p :value)
        :min (get p :min) :max (get p :max) :decimals 2
        :noui true :font-size 9 :text-color :gray
        :on-change (lambda (v)
          (if fx
            (fx-set-effect-value fx p v)
            (fx-set-instrument-value p v)))
        :width 8 :height 1.3)))
    (if (or (get p :options) (get p :boolean))
      (label "" :width 10 :bg :transparent)
      (hslider :width 10 :min (get p :min) :max (get p :max)
               :value (get p :value)
               :material (aqua-slider-material)
               :on-change (lambda (v)
                 (if fx
                   (fx-set-effect-value fx p v)
                   (fx-set-instrument-value p v)))))))

(def fx-param-grid (params fx)
  (h-stack :gap 1.5 :no-clamp-width true
    (each (chunks params 6) |chunk ci|
      (v-stack :gap 0.25 :no-clamp-width true
        (each chunk |p pi|
          (fx-param-row p fx))))))

(def instrument-sources-grid (sections)
  (h-stack :gap 2 :no-clamp-width true
    (each sections |section si|
      (v-stack :gap 0.25 :no-clamp-width true
        (label (get section :name) :font-size 11 :color :white :bg :transparent)
        (each (get section :params) |p pi|
          (fx-param-row p false))))))

(def fx-panel (title params fx)
  (box :background "fx-panel-bg" :padding 1.5 :no-clamp-width true
    (v-stack :gap 0.5 :no-clamp-width true
      (label title :font-size 12 :color :white :bg :transparent)
      (fx-param-grid params fx))))

(def instrument-tab-button (label idx width)
  (box :width width :height 1.6
    :bg (if (= instrument-panel-tab idx) :dark-gray :transparent)
    :on-click |x y r| (set! instrument-panel-tab idx)
    (label label :font-size 11
      :color (if (= instrument-panel-tab idx) :white :gray)
      :bg :transparent)))

(def instrument-panel-body (inst)
  (if (= instrument-panel-tab 0)
    (fx-param-grid (get inst :synth) false)
    (if (= instrument-panel-tab 1)
      (fx-param-grid (get inst :mod) false)
      (instrument-sources-grid (get inst :sources)))))

(def instrument-panel (inst)
  (box :background "fx-panel-bg" :padding 1.5 :no-clamp-width true
    (v-stack :gap 0.6 :no-clamp-width true
      (tabs :items (list "synth" "mod" "sources")
            :bind instrument-panel-tab
            :compact true
            :no-clamp-width true
            :gap 0.75
            :tab-padding 0.5
            :header-height 1.2
        (fx-param-grid (get inst :synth) false)
        (fx-param-grid (get inst :mod) false)
        (instrument-sources-grid (get inst :sources))))))

(effect-buffer "*fx*"
  (v-stack :padding 1 :gap 1 :no-clamp-width true
    (h-stack :gap 1 :no-clamp-width true
      (each SEQ.instrument-panel |inst inst-idx|
        (instrument-panel inst))
      (each (filter |fx| (> (len (get fx :params)) 0) SEQ.effects) |fx slot-idx|
        (fx-panel (get fx :name) (get fx :params) fx))
      ;; Add effect
      (box :background "fx-panel-bg" :padding 1.5
        (v-stack :gap 0.5 :align :center
          (label "+" :font-size 12 :color :gray :bg :transparent)
          (dropdown :value ""
            :options SEQ.available-effects
            :placeholder "Add Effect"
            :on-change (lambda (v)
              (host-command "add-effect" (dict :name v)))
            :width 12 :height 1.5 :font-size 11)
          (compile-progress
            :active (if SEQ.compiling 1 0)
            :width 12 :height 0.3))))))
