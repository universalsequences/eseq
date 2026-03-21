;; waveform-demo.lisp -- repo-local waveform inspector demo
;;
;; Assumes `sample.wav` exists in the project root.

(def sample (sample-load-wav "sample.wav"))

(defstate view-start 0.0)
(defstate view-duration 1.0)
(defstate cursor-time 0.0)
(defstate selection-start nil)
(defstate selection-end nil)
(defstate last-action nil)

(def clamp-view-start (next-start)
  (if sample
    (max 0 (min next-start (max 0 (- sample.duration view-duration))))
    (max 0 next-start)))

(def clamp-view-duration (next-duration)
  (if sample
    (max 0.001 (min next-duration (max 0.001 sample.duration)))
    (max 0.001 next-duration)))

(def apply-scroll-view (event)
  (set! view-start (clamp-view-start (+ view-start event.delta-time))))

(def apply-zoom-view (event)
  (let ((anchor-ratio (/ (- event.anchor-time view-start) view-duration))
        (next-duration (clamp-view-duration (/ view-duration event.factor))))
    (let ((next-start (clamp-view-start (- event.anchor-time (* anchor-ratio next-duration)))))
      (set! view-duration next-duration)
      (set! view-start next-start))))

(def handle-waveform-action (event)
  (set! last-action event)
  (match event.type
    :set-cursor
    (set! cursor-time event.time)
    :set-selection
    (do
      (set! selection-start event.start)
      (set! selection-end event.end))
    :clear-selection
    (do
      (set! selection-start nil)
      (set! selection-end nil))
    :scroll-view
    (apply-scroll-view event)
    :zoom-view
    (apply-zoom-view event)
    _
    nil))

(effect
  (v-stack
    
    (if sample
      (v-stack
        :gap 2
        (label "Waveform" :font-size 32)
        (waveform
          :height 6
          :header-height 0.5
          :focusable true
          :buffer sample
          :view-start view-start
          :view-duration view-duration
          :cursor-time cursor-time
          :selection-start selection-start
          :selection-end selection-end
          :time-ruler (dict :mode :seconds)
          :on-action |event| (handle-waveform-action event)
          )
        (hslider :min 0 :max 100 :value 50)
        )
      (label "failed to load ./sample.wav"))))
