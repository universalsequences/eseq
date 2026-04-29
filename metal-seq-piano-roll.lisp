;; metal-seq-piano-roll.lisp -- step-quantized piano roll for current track.
;; Renders to *piano-roll* buffer. Loaded by metal-seq-grid.lisp.

(defstate piano-roll-tool :pointer)
(defstate piano-roll-view-start 0)
(defstate piano-roll-view-duration 32)
(defstate piano-roll-lane-scroll 36)
(defstate piano-roll-lane-height 1)
(defstate piano-roll-cursor-time 0)
(defstate piano-roll-selection-rect nil)
(defstate piano-roll-status "piano roll")

(def piano-roll-native-action? (event)
  (or (= event.type :select)
      (= event.type :clear-selection)
      (= event.type :finish-marquee-select)
      (= event.type :delete-items)
      (= event.type :nudge-selection)
      (= event.type :move-items-absolute)
      (= event.type :resize-item-absolute)
      (= event.type :finish-create-item)))

(def piano-roll-event-num (event key fallback)
  (let ((value (get event key)))
    (if (= value nil) fallback value)))

(def piano-roll-action (event)
  (do
    (match event.type
      :scroll-view
      (do
        (set! piano-roll-view-start
          (max 0 (+ piano-roll-view-start (piano-roll-event-num event :delta-time 0))))
        (set! piano-roll-lane-scroll
          (max 0 (min 96 (+ piano-roll-lane-scroll (piano-roll-event-num event :delta-lanes 0))))))
      :zoom-view
      (let ((cur-duration piano-roll-view-duration)
            (factor (piano-roll-event-num event :factor 1)))
        (let ((next-duration (max 4 (min 128 (/ piano-roll-view-duration factor)))))
        (let ((anchor-ratio
                (if (<= cur-duration 0)
                  0.5
                  (max 0 (min 1 (/ (- (piano-roll-event-num event :anchor-time piano-roll-view-start) piano-roll-view-start) cur-duration))))))
          (do
            (set! piano-roll-view-duration next-duration)
            (set! piano-roll-view-start
              (max 0 (- (piano-roll-event-num event :anchor-time piano-roll-view-start) (* anchor-ratio next-duration)))))))))
      :zoom-lanes
      (let ((cur-height piano-roll-lane-height)
            (factor (piano-roll-event-num event :factor 1)))
        (let ((next-height (max 0.5 (min 6 (* piano-roll-lane-height factor)))))
        (let ((anchor-lane (piano-roll-event-num event :anchor-lane piano-roll-lane-scroll)))
        (let ((anchor-offset (- anchor-lane piano-roll-lane-scroll)))
          (do
            (set! piano-roll-lane-height next-height)
            (set! piano-roll-lane-scroll
              (max 0
                (min 96
                  (- anchor-lane
                    (* anchor-offset (/ cur-height next-height))))))))))
      :set-cursor
      (set! piano-roll-cursor-time event.time)
      :marquee-select
      (set! piano-roll-selection-rect event)
      :finish-marquee-select
      (set! piano-roll-selection-rect nil)
      :select
      (set! piano-roll-selection-rect nil)
      :clear-selection
      (set! piano-roll-selection-rect nil))
    (if (piano-roll-native-action? event)
      (set! piano-roll-status (seq-piano-roll-action event))
      (set! piano-roll-status "piano roll"))))

(effect-buffer "*piano-roll*"
  (timeline
    :height 35
    :focusable true
    :sidebar-width 5
    :time-ruler (dict :mode :bars-beats :beats-per-bar 4)
    :tool piano-roll-tool
    :playhead-time SEQ.playhead
    :lanes SEQ.piano-roll-lanes
    :items SEQ.piano-roll-items
    :selection SEQ.piano-roll-selection
    :selection-rect piano-roll-selection-rect
    :view-start piano-roll-view-start
    :view-duration piano-roll-view-duration
    :lane-scroll piano-roll-lane-scroll
    :lane-height piano-roll-lane-height
    :snap 1
    :resize-snap :grid
    :snap-mode :floor
    :resize-snap-mode :round
    :scroll-mode :smooth
    :on-action |event| (piano-roll-action event)))
