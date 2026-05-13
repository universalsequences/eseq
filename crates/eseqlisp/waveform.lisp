;; waveform.lisp -- single-buffer waveform inspector demo
;;
;; Edit `sample-path` below to point at a local WAV file, then evaluate this
;; buffer. The widget uses `sample-load-wav` to load the file synchronously and
;; renders a zoomable waveform inspector.

(defstate sample-path "/tmp/sample.wav")
(def sample (sample-load-wav sample-path))

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
    (label (fmt "sample path: {}" sample-path))
    (label (fmt "last action: {}" last-action))
    (if sample
      (v-stack
        (label
          (fmt
            "loaded {} frames @ {} Hz, duration {:.3}s"
            sample.frames
            sample.sample-rate
            sample.duration))
        (waveform
          :height 14
          :focusable true
          :buffer sample
          :view-start view-start
          :view-duration view-duration
          :cursor-time cursor-time
          :selection-start selection-start
          :selection-end selection-end
          :time-ruler (dict :mode :seconds)
          :on-action |event| (handle-waveform-action event)))
      (label "sample-load-wav failed; edit sample-path to point at a WAV file"))))
