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
      (box :width 11 :height 2
        (label (selected-bus-name)
          :font-size 12 :color :white :bg :transparent)))

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
              (vslider :height 4
                :width 2
                :min (bus-seq-param-min) :max (bus-seq-param-max)
                :origin (bus-seq-param-min)
                :value (nth (bus-seq-param-values) step)
                :items (if (= param-mode 2) SEQ.sync-labels '())
                :font-size 11
                :color (if visible
                         (if (nth bus-steps step) :white :gray)
                         :gray)
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
                           (if (and SEQ.playing (= (bus-seq-playhead) step)) :white :gray))
                         :gray))
              (subtree :key (str "bus-step-playhead-probe-" step)
                (step-playhead-dot
                  :active (if (and visible SEQ.playing (= (bus-seq-playhead) step)) 1 0))))))))

    (h-stack :gap 1 :align :center
      (box :width 14 :height 1.3
        (label (fmt "Bus Step {}  {}" (+ (bus-current-step) 1) (bus-seq-param-name))
          :font-size 11 :width 14 :color :gray :bg :transparent))
      (number-picker :value (nth (bus-seq-param-values) (bus-current-step))
        :min (bus-seq-param-min) :max (bus-seq-param-max) :decimals (if (= param-mode 2) 0 2)
        :on-change (lambda (v)
          (do
            (cool-off-follow)
            (bus-set-step-param (bus-current-step) v)))
        :width 8 :height 1.3 :font-size 11)
      (box :background "transport-btn-bg" :padding 0 :height 1.8
        (box :width (bus-page-panel-width) :height 1.7 :padding 0.0525
          (h-stack :gap 0.4 :padding 0.3
            (h-stack :gap 0.4
              (each (range 0 (bus-page-count)) |page|
                (box :width page-button-width :height 1.25 :align :center
                    :bg (if (= page (bus-current-page)) :blue :dark-gray)
                    :on-click |x y r| (bus-goto-page page)
                    (v-stack :gap 0.02 :align :center
                      (label (str (+ page 1))
                        :font-size 10
                        :color (if (= page (bus-current-page)) :white :gray)
                        :bg :transparent)))))))))

    (h-stack :gap 1.5
      (v-stack :align :center :gap 0.25
        (label "timebase" :font-size 9 :color :gray :bg :transparent)
        (dropdown :value (bus-seq-timebase)
          :options '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh")
          :on-change (lambda (v) (do (cool-off-follow) (bus-set-sequencer-label "timebase" v)))
          :width 6 :height 1.5 :font-size 11))
      (v-stack :align :center :gap 0.25
        (h-stack :gap 0.25 :align :baseline
          (label "swg" :font-size 9 :color :gray :bg :transparent)
          (number-picker :value (bus-seq-swing) :min 50 :max 75 :decimals 1
            :noui true :font-size 9 :text-color :gray
            :on-change (lambda (v) (do (cool-off-follow) (bus-set-sequencer-param "swing" v)))
            :width 4 :height 1))
        (box :width 8 :height 2
          (hslider :min 50 :max 75
            :value (bus-seq-swing)
            :material (aqua-slider-material)
            :on-change (lambda (v) (do (cool-off-follow) (bus-set-sequencer-param "swing" v))))))
      (v-stack :align :center :gap 0.25
        (label "swg resolution" :font-size 9 :color :gray :bg :transparent)
        (dropdown :value (bus-seq-swing-resolution)
          :options '("1/16" "1/8" "1/4" "1/2")
          :on-change (lambda (v) (do (cool-off-follow) (bus-set-sequencer-label "swing-resolution" v)))
          :width 5 :height 1.5 :font-size 11))
      (v-stack :align :center :gap 0.25
        (h-stack :gap 0.25 :align :baseline
          (label "steps" :font-size 9 :color :gray :bg :transparent)
          (number-picker :value (bus-seq-num-steps) :min 1 :max 256 :decimals 0
            :noui true :font-size 9 :text-color :gray
            :on-change (lambda (v) (do (cool-off-follow) (bus-set-sequencer-param "num-steps" v)))
            :width 3 :height 1))
        (box :width 8 :height 2
          (hslider :min 1 :max 256
            :value (bus-seq-num-steps)
            :material (aqua-slider-material)
            :on-change (lambda (v) (do (cool-off-follow) (bus-set-sequencer-param "num-steps" v)))))))))

(effect-buffer "*metal*"
  (if (seq-has-selected-bus?)
    (metal-bus-selection-panel)
    (if (= SEQ.num-tracks 0)
    (metal-empty-track-fallback)
    (v-stack
      :padding 1
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
            :bg :transparent)))
    
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
              (vslider :height 4
                :width (if (= param-mode 5) 3 2)
                :min (param-min) :max (param-max)
                :origin (param-origin)
                :value (nth (param-values) step)
                :items (if (= param-mode 5) SEQ.sync-labels '())
                :font-size 11
                :color (if visible
                         (if (nth SEQ.steps step) :white :gray)
                         :gray)
                :material (aqua-slider-material)
                :on-change (lambda (v)
                  (if visible
                    (do
                      (cool-off-follow)
                      (set! cursor-step step)
                      (let ((value (step-param-value v)))
                      (if (seq-has-selection?)
                        (seq-set-step-param-plock (param-keyword) value)
                        (seq-set-step-param step (param-keyword) value))))
                    nil)))
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
                          :gray)
                        :gray))
              (subtree :key (str "step-playhead-probe-" step)
                (step-playhead-dot
                  :active (if (reactive-get "SEQ" (str "playhead-active-" step)) 1 0)))))))) 

    ; Step cursor info
    (h-stack :gap 1 :align :center
      (box :width 11.5 :height 1.3
        (label (fmt "Step {}  {}" (+ (current-step) 1) (param-name))
          :font-size 11 :width 11.5 :color :gray :bg :transparent))
      (if (= param-mode 5)
        (box :width 8 :height 1.3
          (label (sync-current-label)
            :font-size 11 :color :white :bg :transparent))
        (number-picker :value (nth (param-values) (current-step))
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
        (box :background "transport-btn-bg" :padding 0 :height 1.8
          (box :width (page-panel-width) :height 1.7 :padding 0.0525
            (h-stack :gap 0.4 :padding 0.3
              (h-stack :gap 0.4
                (each (range 0 (page-count)) |page|
                  (box :width page-button-width :height 1.25 :align :center
                      :bg (if (= page (visible-page)) :blue :dark-gray)
                      :on-click |x y r| (goto-page page)
                      (v-stack :gap 0.02 :align :center
                        (label (str (+ page 1))
                          :font-size 10
                          :color (if (= page (visible-page)) :white :gray)
                          :bg :transparent)
                        (page-playhead-dot :active (if (= page (playhead-page)) 1 0)))))))))))

    ))))

; Set mode after buffer exists (effect-buffer creates it above)
(set-buffer-mode-for "*metal*" "seq-grid-mode")
