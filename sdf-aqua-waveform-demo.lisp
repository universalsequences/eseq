;; sdf-aqua-waveform-demo.lisp — Aqua chassis with embedded waveform

;; Load aqua widget definitions
(load "sdf-aqua-demo.lisp")

;; ── Waveform state ──────────────────────────────────────────────────────

(def wf-sample (sample-load-wav "sample.wav"))

(defstate wf-view-start 0.0)
(defstate wf-view-duration 1.0)
(defstate wf-cursor-time 0.0)
(defstate wf-selection-start nil)
(defstate wf-selection-end nil)

(def wf-clamp-start (next-start)
  (if wf-sample
    (max 0 (min next-start (max 0 (- wf-sample.duration wf-view-duration))))
    (max 0 next-start)))

(def wf-clamp-duration (next-duration)
  (if wf-sample
    (max 0.001 (min next-duration (max 0.001 wf-sample.duration)))
    (max 0.001 next-duration)))

(def wf-handle-action (event)
  (match event.type
    :set-cursor
    (set! wf-cursor-time event.time)
    :set-selection
    (do
      (set! wf-selection-start event.start)
      (set! wf-selection-end event.end))
    :clear-selection
    (do
      (set! wf-selection-start nil)
      (set! wf-selection-end nil))
    :scroll-view
    (set! wf-view-start (wf-clamp-start (+ wf-view-start event.delta-time)))
    :zoom-view
    (let ((anchor-ratio (/ (- event.anchor-time wf-view-start) wf-view-duration))
          (next-duration (wf-clamp-duration (/ wf-view-duration event.factor))))
      (let ((next-start (wf-clamp-start (- event.anchor-time (* anchor-ratio next-duration)))))
        (set! wf-view-duration next-duration)
        (set! wf-view-start next-start)))
    _
    nil))

;; ── Slider state ────────────────────────────────────────────────────────

(defstate wf-sliders (map |x| 0.5 (range 0 16)))
(defstate wf-toggles (map |x| 1 (range 0 16)))

(def wf-conv (y)
  (clamp (+ 0.5 (* -0.5 y)) 0 1))

;; ── Demo ───────────────────────────────────────────────────────────────

(effect-buffer "*aqua-wf*"
  (v-stack :padding 2 :gap 2
    (box :background "aqua-graphite"
      :padding 2
      (v-stack :gap 0.125
        ;; Waveform "screen" embedded in the chassis
        (if wf-sample
          (h-stack (box :width 40 :height 4 :padding 2
              ;; Labels row
              (h-stack :gap 1.5
                (label "vel" :color :black :bg :transparent)
                (label "pan" :color :white :bg :transparent)
                (label "env" :color :black :bg :transparent)
                (label "xps" :color :black :bg :transparent)
                )
              )
            (box :width 20 :height 3
              (waveform
                :height 3
                :ruler-font-size 8
                :ruler-color :gray
                :ruler-bg :black
                :grid-major-color :dim
                :grid-minor-color :dim
                :bg :black
                :header-height 0.5
                :focusable true
                :buffer wf-sample
                :view-start wf-view-start
                :view-duration wf-view-duration
                :cursor-time wf-cursor-time
                :selection-start wf-selection-start
                :selection-end wf-selection-end
                :time-ruler (dict :mode :seconds)
                :on-action |event| (wf-handle-action event))))
          (label "no sample.wav found" :color :dim))
        
        
        
        ;; Sliders + toggle buttons
        (h-stack :gap 0.25
          (each (zip wf-sliders wf-toggles (range 0 16)) |(v t i)|
            (v-stack :align :center :gap 0.5
              (aqua-vslider
                :value v
                :yo (if t 0 1)
                :on-drag |x y r| (set! wf-sliders (set-nth wf-sliders i (wf-conv y))))
              (box
                :on-click |x y r| (set! wf-toggles (set-nth wf-toggles i (if (> t 0.5) 0 1)))
                :active t
                :background "aqua-button" :align :center :padding 0.25 :width 4 :height 2
                (tick :active t))
              (label (+ i 1) :font-size 8 :color :white :bg :transparent))))))
    
    (h-stack :gap 3 :align :baseline
      (label "aqua waveform" :font-size 48)
      (label "sampler" :font-size 16 :color :dim))))

(delete-other-windows)
(split-window-right "*aqua-wf*")
