;; metal-seq-metal.lisp - Step grid UI for Metal Sequencer
;; Renders to *metal* buffer. Loaded by metal-seq-grid.lisp.

;; ── Main UI ──

(def metal-empty-track-fallback ()
  (v-stack :width :fill :padding 1 :gap 0
    (box :flex 1)
    (h-stack :width :fill :align :center
      (box :flex 1)
      (v-stack :gap 0.35 :align :center
        (label "Select a sound to create a track"
          :font-size 14 :color :gray :bg :transparent)
        (label "Sampler, instruments, and projects are in the left browser."
          :font-size 10 :color :dark-gray :bg :transparent))
      (box :flex 1))
    (box :flex 1)))

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
          :options '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh")
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
                :plocked 1
                :selected (if visible (if (nth SEQ.selected-steps step) 1 0) 0)
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
                      :selected (if visible (if (nth SEQ.selected-steps step) 1 0) 0)))
              (label (if visible (str (+ step 1)) "")
                :font-size 10 :bg :transparent
                :color (if visible
                         (if (nth SEQ.selected-steps step) :yellow
                           (if (and SEQ.playing (= (bus-seq-playhead) step)) :white :dim))
                         :dim))
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
    (v-stack
      :padding 2
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
            :options '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh")
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
            :on-click (lambda (evt)
              (if visible
                (do
                  (cool-off-follow)
                  (set! cursor-step step)
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
                    :fill (rgba 0.20 0.20 0.92 1.0)
                    :dot-color :dark-gray
                    :material (aqua-slider-material)
                    :on-change (lambda (v)
                      (if visible
                        (do
                          (cool-off-follow)
                          (set! cursor-step step)
                          (let ((value (step-slider-param-value v)))
                          (if (seq-has-selection?)
                            (seq-set-step-param-plock (param-keyword) value)
                            (seq-set-step-param step (param-keyword) value))))
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
                    :fill (rgba 0.08 0.08 0.25 0.45)
                    :dot-color (rgba 0.25 0.25 0.30 0.45)
                    :material (aqua-slider-muted-material)
                    :on-change (lambda (v)
                      (if visible
                        (do
                          (cool-off-follow)
                          (set! cursor-step step)
                          (let ((value (step-slider-param-value v)))
                          (if (seq-has-selection?)
                            (seq-set-step-param-plock (param-keyword) value)
                            (seq-set-step-param step (param-keyword) value))))
                        nil)))))
              (box
                :active (if visible (if (nth SEQ.steps step) 1 0) 0)
                :plocked 1
                :selected (if visible (if (nth SEQ.selected-steps step) 1 0) 0)
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
                (tick :active (if visible (if (nth SEQ.steps step) 1 0) 0)
                      :plocked (if visible (if (nth SEQ.step-has-plocks step) 1 0) 0)
                      :selected (if visible (if (nth SEQ.selected-steps step) 1 0) 0)))
              (label (if visible (str (+ step 1)) "")
                :font-size 10 :bg :transparent
                :color (if visible
                        (if (nth SEQ.selected-steps step) :yellow
                          :dim)
                        :dim))
              (subtree :key (str "step-playhead-probe-" step)
                (step-playhead-dot
                  :active (if (reactive-get "SEQ" (str "playhead-active-" step)) 1 0)))))))) 

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
              (seq-set-step-param (current-step) (param-keyword) (step-param-value v))))
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

    ))))

; Set mode after buffer exists (effect-buffer creates it above)
(set-buffer-mode-for "*metal*" "seq-grid-mode")
