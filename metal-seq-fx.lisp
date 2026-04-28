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

(def fx-param-row (p fx subtree-key)
  (subtree :key subtree-key
    (box :height 1.25 :no-clamp-width true
      (h-stack :gap 0.45 :align :center :no-clamp-width true
        (box :width 13.2 :height 1.25 :no-clamp-width true
          (h-stack :gap 0.25 :align :baseline :no-clamp-width true
            (label (substring (get p :name) 0 9) :font-size 12 :width 7
                   :color :gray :bg :transparent)
            (if (get p :boolean)
              (box :width 5.5 :height 1.25 :align :center
                   :bg :transparent
                   :on-click |x y r|
                     (if fx
                       (fx-set-effect-value fx p (if (> (get p :value) 0.5) 0 1))
                       (fx-set-instrument-value p (if (> (get p :value) 0.5) 0 1)))
                (label (if (> (get p :value) 0.5) "ON" "OFF")
                       :font-size 13 :width 5.5
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
                :width 5.8 :height 1.2 :font-size 13)
              (number-picker :value (get p :value)
                :min (get p :min) :max (get p :max) :decimals 2
                :noui true :font-size 12 :text-color :gray
                :on-change (lambda (v)
                  (if fx
                    (fx-set-effect-value fx p v)
                    (fx-set-instrument-value p v)))
                :width 5.2 :height 1.1)))))
        (if (or (get p :options) (get p :boolean))
          (label "" :width 7.8 :bg :transparent)
          (hslider :width 7.8 :min (get p :min) :max (get p :max)
                   :value (get p :value)
                   :material (aqua-slider-material)
                   :on-change (lambda (v)
                     (if fx
                       (fx-set-effect-value fx p v)
                       (fx-set-instrument-value p v)))))))))

(def fx-param-grid (params fx)
  (h-stack :gap 1.5 :no-clamp-width true
    (each (chunks params 6) |chunk ci|
      (v-stack :gap 0.25 :no-clamp-width true
        (each chunk |p pi|
          (fx-param-row p fx
            (if fx
              (str "fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))
              (str "instrument-tab-" instrument-panel-tab "-chunk-" ci "-param-" (get p :idx)))))))))

(def instrument-sources-grid (sections)
  (h-stack :gap 2 :no-clamp-width true
    (each sections |section si|
      (v-stack :gap 0.25 :no-clamp-width true
        (label (get section :name) :font-size 14 :color :white :bg :transparent)
        (each (get section :params) |p pi|
          (fx-param-row p false
            (str "instrument-source-" si "-param-" (get p :idx))))))))

(def fx-panel (title params fx)
  (box :background "fx-panel-bg" :padding 1.5 :no-clamp-width true
    (v-stack :gap 0.5 :no-clamp-width true
      (h-stack :gap 0.5 :align :center
        (label title :font-size 15 :color :white :bg :transparent)
        ;; Only show edit button for custom dgenlisp effects (not built-in Filter/Delay)
        (if (and (not (= title "Filter")) (not (= title "Delay")))
          (box :bg :dark-gray :width 4 :height 1.2 :align :center
            :on-click |x y r|
              (host-command "enter-edit-effect"
                (dict :name title :slot (get fx :slot-idx)))
            (label "edit" :font-size 8 :color :gray :bg :transparent))
          (box)))
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

(defstate sampler-view-start 0.0)
(defstate sampler-view-duration 0)
(defstate sampler-cursor-time 0.0)

(def sampler-set-start-end (start-seconds end-seconds duration)
  (if (> duration 0)
    (do
      (fx-set-instrument-value (dict :idx 2 :control "param") (* 100 (/ start-seconds duration)))
      (fx-set-instrument-value (dict :idx 3 :control "param") (* 100 (/ end-seconds duration))))))

(def sampler-clamp-start (next-start duration)
  (max 0 (min next-start (max 0 (- duration sampler-view-duration)))))

(def sampler-clamp-duration (next-duration duration)
  (max 0.001 (min next-duration (max 0.001 duration))))

(def handle-sampler-waveform-action (event duration)
  (match event.type
    :set-cursor
    (set! sampler-cursor-time event.time)
    :set-selection
    (sampler-set-start-end event.start event.end duration)
    :clear-selection
    (sampler-set-start-end 0 duration duration)
    :scroll-view
    (set! sampler-view-start (sampler-clamp-start (+ sampler-view-start event.delta-time) duration))
    :zoom-view
    (let ((cur-duration (if (= sampler-view-duration 0) duration sampler-view-duration)))
      (let ((anchor-ratio (/ (- event.anchor-time sampler-view-start) cur-duration))
            (next-duration (sampler-clamp-duration (/ cur-duration event.factor) duration)))
        (set! sampler-view-duration next-duration)
        (set! sampler-view-start (sampler-clamp-start (- event.anchor-time (* anchor-ratio next-duration)) duration))))
    _
    nil))

(def sampler-panel (inst)
  (box :background "fx-panel-bg" :padding 1.5 :no-clamp-width true
    (v-stack :gap 0.8 :no-clamp-width true
      (label "Sampler" :font-size 15 :color :white :bg :transparent)
      (if (get inst :buffer)
        (subtree :key (str "sampler-waveform-" (get inst :buffer))
          (box :width 25 :height 2.5
            (waveform
              :height 2.5
              :header-height 0.3
              :ruler-font-size 8
              :ruler-color :gray
              :ruler-bg :black
              :grid-major-color (rgba 0.15 0.15 0.15 1)
              :grid-minor-color (rgba 0.10 0.10 0.10 1)
              :bg :black
              :focusable true
              :buffer (get inst :buffer)
              :view-start sampler-view-start
              :view-duration (if (= sampler-view-duration 0) (get inst :duration) sampler-view-duration)
              :cursor-time sampler-cursor-time
              :playhead-time SEQ.sampler-playhead
              :selection-start (get inst :start-time)
              :selection-end (get inst :end-time)
              :time-ruler (dict :mode :seconds)
              :on-action |event| (handle-sampler-waveform-action event (get inst :duration)))))
        (label "No sample" :font-size 12 :color :gray :bg :transparent))
      (fx-param-grid (get inst :params) false))))

(def instrument-panel (inst)
  (if (= (get inst :type) "sampler")
    (sampler-panel inst)
    (box :background "fx-panel-bg" :padding 1.5 :no-clamp-width true
      (v-stack :gap 0.6 :no-clamp-width true
        (h-stack :gap 0.5 :no-clamp-width true
          (tabs :items (list "synth" "mod" "sources")
                :bind instrument-panel-tab
                :compact true
                :no-clamp-width true
                :gap 0.75
                :tab-padding 0.5
                :header-height 1.2
            (fx-param-grid (get inst :synth) false)
            (fx-param-grid (get inst :mod) false)
            (instrument-sources-grid (get inst :sources)))
          (box :bg :dark-gray :width 4 :height 1.2 :align :center
            :on-click |x y r|
              (host-command "enter-edit-instrument"
                (dict :name SEQ.sidebar-instrument-name))
            (label "edit" :font-size 8 :color :gray :bg :transparent))
          (save-icon
            :on-click |x y r| (sbrowser-enter-preset-save)
            :active 0))))))

(effect-buffer "*fx*"
  (v-stack :padding 0.5 :gap 1 :no-clamp-width true
    (h-stack :gap 1 :no-clamp-width true
      (each SEQ.instrument-panel |inst inst-idx|
        (instrument-panel inst))
      (each (filter |fx| (> (len (get fx :params)) 0) SEQ.effects) |fx slot-idx|
        (fx-panel (get fx :name) (get fx :params) fx))
      ;; Add effect
      (box :background "fx-panel-bg" :padding 1.5
        (v-stack :gap 0.5 :align :center
          (label "+" :font-size 15 :color :gray :bg :transparent)
          (dropdown :value ""
            :options SEQ.available-effects
            :placeholder "Add Effect"
            :on-change (lambda (v)
              (if (= v "+ New Effect")
                (do
                  (set! sbrowser-editor-name "")
                  (host-command "enter-new-effect-editor" (dict)))
                (host-command "add-effect" (dict :name v))))
            :width 12 :height 1.5 :font-size 14)
          (compile-progress
            :active (if SEQ.compiling 1 0)
            :width 12 :height 0.3))))))
