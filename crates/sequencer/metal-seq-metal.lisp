;; metal-seq-metal.lisp - Step grid UI for Metal Sequencer
;; Renders to *metal* buffer. Loaded by metal-seq-grid.lisp.

;; ── Main UI ──

(defstate metal-track-r 0.34)
(defstate metal-track-g 0.48)
(defstate metal-track-b 0.98)

(def metal-empty-track-fallback ()
  (box :width :fill :height :fill :padding 1 :h-align :center :v-align :center
    (v-stack :gap 0.35 :align :center
      (label "Select a sound to create a track"
        :font-size 14 :color :gray :bg :transparent)
      (label "Sampler, instruments, and projects are in the left browser."
        :font-size 10 :color :dark-gray :bg :transparent))))

(def metal-current-track-color ()
  (if (and (< SEQ.current-track (len SEQ.track-colors)) (>= SEQ.current-track 0))
    (nth SEQ.track-colors SEQ.current-track)
    (list 0.34 0.48 0.98)))

(def metal-track-color-r ()
  (nth (metal-current-track-color) 0))

(def metal-track-color-g ()
  (nth (metal-current-track-color) 1))

(def metal-track-color-b ()
  (nth (metal-current-track-color) 2))

(def metal-sync-track-color-state ()
  (do
    (set! metal-track-r (metal-track-color-r))
    (set! metal-track-g (metal-track-color-g))
    (set! metal-track-b (metal-track-color-b))))

(def metal-track-slider-fill ()
  (rgba (metal-track-color-r) (metal-track-color-g) (metal-track-color-b) 1.0))

(def metal-track-slider-muted-fill ()
  (rgba
    (+ (* (metal-track-color-r) 0.30) (* 0.08 0.70))
    (+ (* (metal-track-color-g) 0.30) (* 0.08 0.70))
    (+ (* (metal-track-color-b) 0.30) (* 0.12 0.70))
    0.50))

(def metal-track-slider-muted-dot ()
  (rgba
    (+ (* (metal-track-color-r) 0.28) (* 0.25 0.72))
    (+ (* (metal-track-color-g) 0.28) (* 0.25 0.72))
    (+ (* (metal-track-color-b) 0.28) (* 0.30 0.72))
    0.55))

(defwidget metal-track-tick
  :width 1.5 :height 1.5
  :state (active plocked selected track-r track-g track-b)
  :bindable (active plocked selected track-r track-g track-b)
  :shader
  (let ((sel-y (if (= selected 1) (* 0.1 (cos (* 3 itime))) 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (sdf/circle 1)
          (material
            :lighting (lighting :edge-min -0.35 :edge-max 0.5
              :light (vec3 0.0 -1.0 2.5) :shininess 32.0)
            :color
            (* (if (= active 1) 1 0.3)
               (aqua-color
                 (rgba (* track-r 0.82) (* track-g 0.82) (* track-b 0.82) 1.0)
                 (rgba track-r track-g track-b 1.0)))))))))

(def metal-bus-selection-panel ()
  (v-stack
    :padding 1
    :gap 0.1

    (h-stack :gap 0.5
      (box :width 8 :height 2
        :bg (if (= param-mode 0) :blue :dark-gray)
        :on-click |x y r| (set! param-mode 0)
        (label "gate" :font-size 12
          :color (if (= param-mode 0) :white :gray)
          :bg :transparent))
      (box :width 8 :height 2
        :bg (if (= param-mode 1) :green :dark-gray)
        :on-click |x y r| (set! param-mode 1)
        (label "dur" :font-size 12
          :color (if (= param-mode 1) :white :gray)
          :bg :transparent))
      (box :width 8 :height 2
        :bg (if (= param-mode 2) :magenta :dark-gray)
        :on-click |x y r| (set! param-mode 2)
        (label "syn" :font-size 12
          :color (if (= param-mode 2) :white :gray)
          :bg :transparent))
      (h-stack :align :center :gap 0.35
        (dropdown :value (bus-seq-timebase)
          :options seq-timebase-options
          :on-change (lambda (v) (do (cool-off-follow) (bus-set-sequencer-label "timebase" v)))
          :width 6 :height 1.45 :font-size 10)))

    (grid :cols 16 :col-width 4
      (each (range 0 page-size) |i|
        (let ((step (bus-step-index i))
              (visible (bus-step-visible? i))
              (bus-steps (bus-seq-list SEQ.bus-steps))
              (bus-plocks (bus-seq-list SEQ.bus-step-has-plocks)))
          (box :padding 0.25
            :background (if visible (if (= (bus-current-step) step) "cursor-highlight" nil) nil)
            :active true
            :selected true
            :on-click (lambda (evt)
              (if visible
                (do
                  (cool-off-follow)
                  (set! cursor-step step)
                  (if (selection-click? evt)
                    (bus-step-select-drag-start step evt)
                    (seq-clear-selection)))
                nil))
            :on-drag (lambda (evt)
              (if visible
                (bus-step-select-drag-over step evt)
                nil))
            (v-stack :align :center :gap 0.5
              (let ((step-on (and visible (nth bus-steps step))))
                (if step-on
                  (vslider :height 4
                    :width 2
                    :min (bus-seq-param-min) :max (bus-seq-param-max)
                    :origin (bus-seq-param-min)
                    :value (nth (bus-seq-param-values) step)
                    :items (if (= param-mode 2) SEQ.sync-labels '())
                    :font-size 11
                    :color :white
                    :fill (rgba 0.20 0.20 0.92 1.0)
                    :dot-color :dark-gray
                    :material (aqua-slider-material)
                    :on-change (lambda (v)
                      (if visible
                        (do
                          (cool-off-follow)
                          (set! cursor-step step)
                          (if (seq-has-selection?)
                            (bus-set-selected-step-param v)
                            (bus-set-step-param step v)))
                        nil)))
                  (vslider :height 4
                    :width 2
                    :min (bus-seq-param-min) :max (bus-seq-param-max)
                    :origin (bus-seq-param-min)
                    :value (nth (bus-seq-param-values) step)
                    :items (if (= param-mode 2) SEQ.sync-labels '())
                    :font-size 11
                    :color :dim
                    :fill (rgba 0.08 0.08 0.25 0.45)
                    :dot-color (rgba 0.25 0.25 0.30 0.45)
                    :material (aqua-slider-muted-material)
                    :on-change (lambda (v)
                      (if visible
                        (do
                          (cool-off-follow)
                          (set! cursor-step step)
                          (if (seq-has-selection?)
                            (bus-set-selected-step-param v)
                            (bus-set-step-param step v)))
                        nil)))))
              (box
                :active (if visible (if (nth bus-steps step) 1 0) 0)
                :plocked (if visible (if (nth bus-plocks step) 1 0) 0)
                :selected (if visible (bind-seq-nth "selected-steps" step) 0)
                :background "aqua-button"
                :align :center :width 3 :height 1.5
                :on-mouse-down (lambda (evt)
                  (if visible
                    (bus-step-pointer-down step evt)
                    nil))
                :on-drag (lambda (evt)
                  (if visible
                    (bus-step-select-drag-over step evt)
                    nil))
                :on-mouse-up (lambda (evt)
                  (if visible
                    (bus-step-pointer-up step evt)
                    nil))
                (tick :active (if visible (if (nth bus-steps step) 1 0) 0)
                      :plocked (if visible (if (nth bus-plocks step) 1 0) 0)
                      :selected (if visible (bind-seq-nth "selected-steps" step) 0)))
              (label (if visible (str (+ step 1)) "")
                :font-size 10 :bg :transparent
                :active (if visible (bind-seq-nth "selected-steps" step) 0)
                :active-color :yellow
                :color (if (and visible SEQ.playing (= (bus-seq-playhead) step)) :white :dim))
              (subtree :key (str "bus-step-playhead-probe-" step)
                (step-playhead-dot
                  :active (if (and visible SEQ.playing (= (bus-seq-playhead) step)) 1 0))))))))

    (h-stack :gap 1 :align :center
      (box :width 14 :height 1.3
        (label (fmt "Bus Step {}  {}" (+ (bus-current-step) 1) (bus-seq-param-name))
          :font-size 11 :width 14 :color :dim :bg :transparent))
      (number-picker :value (nth (bus-seq-param-values) (bus-current-step))
        :min (bus-seq-param-min) :max (bus-seq-param-max) :decimals (if (= param-mode 2) 0 2)
        :on-change (lambda (v)
          (do
            (cool-off-follow)
            (bus-set-step-param (bus-current-step) v)))
        :width 8 :height 1.3 :font-size 11)
      (box :background "transport-btn-bg" :padding 0.2 :height 1.4
        (h-stack :gap 0.1 :align :center
          (each (range 0 (bus-page-count)) |page|
            (box :width page-button-width :height 1.1
              :background "pattern-pill-bg"
              :active (if (= page (bus-current-page)) 1 0)
              :style pattern-control-style
              :on-click |x y r| (bus-goto-page page)
              (v-stack :align :center
                (label (fmt " {} " (+ page 1))
                  :font-size 11
                  :color (if (= page (bus-current-page)) :white :dim)
                  :bg :transparent)))))))

    ))

(effect-buffer "*metal*"
  (if (seq-has-selected-bus?)
    (metal-bus-selection-panel)
    (if (= SEQ.num-tracks 0)
    (metal-empty-track-fallback)
    (do
    (metal-sync-track-color-state)

    (box :background-color :mixer-strip-bg :corner-radius 10
    (v-stack
      :padding 1.5
      :gap 0.1
      
      ; Param mode selector
      (h-stack :gap 0.5
        (box :width 8 :height 2
          :bg (if (= param-mode 0) :blue :dark-gray)
          :on-click |x y r| (set! param-mode 0)
          (label "vel" :font-size 12
            :color (if (= param-mode 0) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 1) :green :dark-gray)
          :on-click |x y r| (set! param-mode 1)
          (label "dur" :font-size 12
            :color (if (= param-mode 1) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 2) :magenta :dark-gray)
          :on-click |x y r| (set! param-mode 2)
          (label "aux_a" :font-size 12
            :color (if (= param-mode 2) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 3) :yellow :dark-gray)
          :on-click |x y r| (set! param-mode 3)
          (label "xpose" :font-size 12
            :color (if (= param-mode 3) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 4) :red :dark-gray)
          :on-click |x y r| (set! param-mode 4)
          (label "pan" :font-size 12
            :color (if (= param-mode 4) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 5) :green :dark-gray)
          :on-click |x y r| (set! param-mode 5)
          (label "syn" :font-size 12
            :color (if (= param-mode 5) :white :gray)
            :bg :transparent))
        (h-stack :align :center :gap 0.35
          (dropdown :value SEQ.tp-timebase
            :options seq-timebase-options
            :on-change (lambda (v)
              (do
                (cool-off-follow)
                (if (seq-has-selection?)
                  (seq-plock-timebase v)
                  (seq-set-timebase v))))
            :width 6 :height 1.45 :font-size 10)))
    
    ; Step columns: vslider + aqua step toggle + step number
    (grid :cols 16 :col-width 4
      (each (range 0 page-size) |i|
        (let ((step (step-index i))
              (visible (step-visible? i)))
          (box :padding 0.25 :background (if visible (if (= (current-step) step) "cursor-highlight" nil) nil)
            :active true
            :selected true
            :on-click (lambda (evt)
              (if visible
                (do
                  (cool-off-follow)
                  (set-track-cursor-step step)
                  (if (selection-click? evt)
                    (step-select-drag-start step evt)
                    (seq-clear-selection)))
                nil))
            :on-drag (lambda (evt)
              (if visible
                (step-select-drag-over step evt)
                nil))
            (v-stack :align :center :gap 0.5
              (let ((step-on (and visible (nth SEQ.steps step))))
                (if step-on
                  (vslider :height 4
                    :width (if (= param-mode 5) 2 1)
                    :min (param-slider-min) :max (param-slider-max)
                    :origin (param-origin)
                    :value (param-slider-value step)
                    :haptic-value (nth (param-values) step)
                    :haptic-min (param-min)
                    :haptic-max (param-max)
                    :haptic-pivot-position (param-haptic-pivot-position)
                    :haptic-pivot-value (param-haptic-pivot-value)
                    :haptic-exponent (param-haptic-exponent)
                    :items (if (= param-mode 5) SEQ.sync-labels '())
                    :font-size 11
                    :color :white
                    :fill (metal-track-slider-fill)
                    :dot-color :dark-gray
                    :material (aqua-slider-track-material)
                    :on-change (lambda (v)
                      (if visible
                        (do
                          (cool-off-follow)
                          (set-track-cursor-step step)
                          (let ((value (step-slider-param-value v)))
                          (seq-set-step-param-from-step step (param-keyword) value)))
                        nil)))
                  (vslider :height 4
                    :width (if (= param-mode 5) 2 1)
                    :min (param-slider-min) :max (param-slider-max)
                    :origin (param-origin)
                    :value (param-slider-value step)
                    :haptic-value (nth (param-values) step)
                    :haptic-min (param-min)
                    :haptic-max (param-max)
                    :haptic-pivot-position (param-haptic-pivot-position)
                    :haptic-pivot-value (param-haptic-pivot-value)
                    :haptic-exponent (param-haptic-exponent)
                    :items (if (= param-mode 5) SEQ.sync-labels '())
                    :font-size 11
                    :color :dim
                    :fill (metal-track-slider-muted-fill)
                    :dot-color (metal-track-slider-muted-dot)
                    :material (aqua-slider-track-muted-material)
                    :on-change (lambda (v)
                      (if visible
                        (do
                          (cool-off-follow)
                          (set-track-cursor-step step)
                          (let ((value (step-slider-param-value v)))
                          (seq-set-step-param-from-step step (param-keyword) value)))
                        nil)))))
              (box
                :active (if visible (if (nth SEQ.steps step) 1 0) 0)
                :plocked (if visible (if (nth SEQ.step-has-plocks step) 1 0) 0)
                :selected (if visible (bind-seq-nth "selected-steps" step) 0)
                :background "aqua-button"
                :align :center :width 3 :height 1.5
                :on-mouse-down (lambda (evt)
                  (if visible
                    (step-pointer-down step evt)
                    nil))
                :on-drag (lambda (evt)
                  (if visible
                    (step-select-drag-over step evt)
                    nil))
                :on-mouse-up (lambda (evt)
                  (if visible
                    (step-pointer-up step evt)
                    nil))
                (metal-track-tick
                      :active (if visible (if (nth SEQ.steps step) 1 0) 0)
                      :plocked (if visible (if (nth SEQ.step-has-plocks step) 1 0) 0)
                      :selected (if visible (bind-seq-nth "selected-steps" step) 0)
                      :track-r (metal-track-color-r)
                      :track-g (metal-track-color-g)
                      :track-b (metal-track-color-b)))
              (label (if visible (str (+ step 1)) "")
                :font-size 10 :bg :transparent
                :active (if visible (bind-seq-nth "selected-steps" step) 0)
                :active-color :yellow
                :color :dim)
              (subtree :key (str "step-playhead-probe-" step)
                (step-playhead-dot
                  :active (bind-seq (str "playhead-active-" step)))))))))

    ; Step cursor info
    (h-stack :gap 1 :align :center
      (box :width 11.5 :height 1.3
        (label (fmt "Step {}  {}" (+ (current-step) 1) (param-name))
          :font-size 11 :width 11.5 :color :dim :bg :transparent))
      (if (= param-mode 5)
        (box :width 8 :height 1.3
          (label (sync-current-label)
            :font-size 11 :color :white :bg :transparent))
        (number-picker :key "metal-step-param-number-picker"
          :value (nth (param-values) (current-step))
          :min (param-min) :max (param-max) :decimals (param-decimals)
          :on-change (lambda (v)
            (do
              (cool-off-follow)
              (seq-set-step-param-from-step
                (current-step)
                (param-keyword)
                (step-param-value v))))
          :width 8 :height 1.3 :font-size 11))
      (h-stack :gap 0.4 :align :center
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :on-click |x y r| (halve-track-pattern)
          (v-stack :align :center
            (label "-"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :on-click |x y r| (double-track-pattern)
          (v-stack :align :center
            (label "+"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "transport-btn-bg" :padding 0.2 :height 1.4
          (h-stack :gap 0.1 :align :center
            (each (range 0 (page-count)) |page|
              (box :width page-button-width :height 1.1
                :background "pattern-pill-bg"
                :active (if (= page (visible-page)) 1 0)
                :style pattern-control-style
                :on-click |x y r| (goto-page page)
                (v-stack :align :center
                  (label (fmt " {} " (+ page 1))
                    :font-size 11
                    :color (if (= page (visible-page)) :white :dim)
                    :bg :transparent))))))))

    ))))))

; Set mode after buffer exists (effect-buffer creates it above)
(set-buffer-mode-for "*metal*" "seq-grid-mode")
