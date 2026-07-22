;; ui/piano-roll.lisp -- step-quantized piano roll for current track.
;; Renders to *piano-roll* buffer. Loaded by ui/main.lisp.

(defstate piano-roll-tool :pointer)
(defstate piano-roll-view-start 0)
(defstate piano-roll-view-duration 10.6667)
(defstate piano-roll-lane-scroll 36)
(defstate piano-roll-lane-height 0.5)
(defstate piano-roll-cursor-time 0)
(defstate piano-roll-selection-rect nil)
(defstate piano-roll-status "piano roll")
(defstate piano-roll-create-duration 1)
(def piano-roll-fit-pending false)
(def piano-roll-fit-track -1)

(def piano-roll-timeline-height 35)
(def piano-roll-header-height 2)
(def piano-roll-default-pane-height 11.5)
(def piano-roll-view-padding 1)
(def piano-roll-min-view-duration 4)
(def piano-roll-max-view-duration 256)

(def piano-roll-native-action? (event)
  (or (= event.type :select)
      (= event.type :clear-selection)
      (= event.type :finish-marquee-select)
      (= event.type :delete-items)
      (= event.type :copy-items)
      (= event.type :paste-items)
      (= event.type :nudge-selection)
      (= event.type :move-items-absolute)
      (= event.type :resize-item-absolute)
      (= event.type :finish-move-items)
      (= event.type :finish-resize-items)
      (= event.type :finish-create-item)))

(def piano-roll-event-num (event key fallback)
  (let ((value (get event key)))
    (if (= value nil) fallback value)))

(def piano-roll-lane-height-value ()
  (if (= piano-roll-lane-height nil) 1 piano-roll-lane-height))

(def piano-roll-lane-count ()
  (max 1 (len SEQ.piano-roll-lanes)))

(def piano-roll-content-height ()
  (max 1 (- piano-roll-default-pane-height piano-roll-header-height)))

(def piano-roll-visible-lane-count ()
  (/ (piano-roll-content-height) (piano-roll-lane-height-value)))

(def piano-roll-max-lane-scroll ()
  (max 0 (- (piano-roll-lane-count) (piano-roll-visible-lane-count))))

(def piano-roll-clamp-lane-scroll (scroll)
  (max 0 (min (piano-roll-max-lane-scroll) scroll)))

(def set-piano-roll-lane-scroll (scroll)
  (set! piano-roll-lane-scroll (piano-roll-clamp-lane-scroll scroll)))

(def piano-roll-action-duration (event)
  (max 0.03125
    (piano-roll-event-num event :duration
      (- (piano-roll-event-num event :end 1)
         (piano-roll-event-num event :start 0)))))

(def piano-roll-set-cursor-from-event (event)
  (let ((time (get event :time)))
    (if (= time nil)
      nil
      (set! piano-roll-cursor-time time))))

(def piano-roll-current-track-color ()
  (if (and (< SEQ.current-track (len SEQ.track-colors)) (>= SEQ.current-track 0))
    (nth SEQ.track-colors SEQ.current-track)
    (list 0.34 0.48 0.98)))

(def piano-roll-max-view-start (duration)
  (max 0 (- (+ SEQ.tp-num-steps 4) duration)))

(def set-piano-roll-view-start (start duration)
  (set! piano-roll-view-start
    (max 0 (min (piano-roll-max-view-start duration) start))))

(def piano-roll-zoom-view (event)
  (let ((cur-duration piano-roll-view-duration)
        (factor (piano-roll-event-num event :factor 1)))
    (let ((next-duration (max piano-roll-min-view-duration
                           (min piano-roll-max-view-duration
                             (/ piano-roll-view-duration factor)))))
      (let ((anchor-ratio
              (if (<= cur-duration 0)
                0.5
                (max 0 (min 1 (/ (- (piano-roll-event-num event :anchor-time piano-roll-view-start) piano-roll-view-start) cur-duration))))))
        (do
          (set! piano-roll-view-duration next-duration)
          (set-piano-roll-view-start
            (- (piano-roll-event-num event :anchor-time piano-roll-view-start) (* anchor-ratio next-duration))
            next-duration))))))

(def piano-roll-zoom-lanes (event)
  (let ((cur-height (piano-roll-lane-height-value))
        (factor (piano-roll-event-num event :factor 1)))
    (let ((next-height (max 0.5 (min 6 (* (piano-roll-lane-height-value) factor)))))
      (let ((anchor-lane (piano-roll-event-num event :anchor-lane piano-roll-lane-scroll)))
        (let ((anchor-offset (- anchor-lane piano-roll-lane-scroll)))
          (do
            (set! piano-roll-lane-height next-height)
            (set-piano-roll-lane-scroll
              (- anchor-lane
                (* anchor-offset (/ cur-height next-height))))))))))

(def piano-roll-has-items? ()
  (> (len SEQ.piano-roll-items) 0))

(def piano-roll-item-start (item)
  (piano-roll-event-num item :start 0))

(def piano-roll-item-end (item)
  (max (piano-roll-item-start item)
    (piano-roll-event-num item :end (piano-roll-item-start item))))

(def piano-roll-item-lane (item)
  (piano-roll-event-num item :lane 0))

(def piano-roll-fit-horizontal (min-start max-end)
  (let ((start (max 0 (- min-start piano-roll-view-padding)))
        (end (max min-start (+ max-end piano-roll-view-padding))))
    (let ((duration (max piano-roll-min-view-duration
                      (min piano-roll-max-view-duration
                        (- end start)))))
      (do
        (set! piano-roll-view-duration duration)
        (set-piano-roll-view-start start duration)))))

(def piano-roll-fit-vertical (min-lane max-lane)
  (let ((center (/ (+ min-lane max-lane 1) 2)))
    (set-piano-roll-lane-scroll
      (- center (/ (piano-roll-visible-lane-count) 2)))))

(def piano-roll-fit-empty-view ()
  (do
    (set! piano-roll-view-duration
      (max piano-roll-min-view-duration
        (min piano-roll-max-view-duration SEQ.tp-num-steps)))
    (set-piano-roll-view-start 0 piano-roll-view-duration)
    (piano-roll-fit-vertical
      (floor (/ (- (piano-roll-lane-count) 1) 2))
      (floor (/ (- (piano-roll-lane-count) 1) 2)))))

(def piano-roll-fit-notes-to-view ()
  (if (piano-roll-has-items?)
    (let ((first (nth SEQ.piano-roll-items 0)))
      (let ((min-start (reduce |acc item| (min acc (piano-roll-item-start item))
                         (piano-roll-item-start first)
                         SEQ.piano-roll-items))
            (max-end (reduce |acc item| (max acc (piano-roll-item-end item))
                       (piano-roll-item-end first)
                       SEQ.piano-roll-items))
            (min-lane (reduce |acc item| (min acc (piano-roll-item-lane item))
                        (piano-roll-item-lane first)
                        SEQ.piano-roll-items))
            (max-lane (reduce |acc item| (max acc (piano-roll-item-lane item))
                        (piano-roll-item-lane first)
                        SEQ.piano-roll-items)))
        (do
          (piano-roll-fit-horizontal min-start max-end)
          (piano-roll-fit-vertical min-lane max-lane))))
    (piano-roll-fit-empty-view)))

(def piano-roll-request-fit-for-track (track)
  (do
    (set! piano-roll-fit-pending true)
    (set! piano-roll-fit-track track)
    (piano-roll-apply-pending-fit)))

(def piano-roll-request-fit ()
  (piano-roll-request-fit-for-track SEQ.current-track))

(def piano-roll-apply-pending-fit ()
  (if (and piano-roll-fit-pending (= piano-roll-fit-track SEQ.current-track))
    (do
      (piano-roll-fit-notes-to-view)
      (set! piano-roll-fit-pending false))
    nil))

(def piano-roll-action (event)
  (do
    (match event.type
      :scroll-view
      (let ((view-start (get event :view-start))
            (lane-scroll (get event :lane-scroll)))
        (do
          (set-piano-roll-view-start
            (if (= view-start nil)
              (+ piano-roll-view-start (piano-roll-event-num event :delta-time 0))
              view-start)
            piano-roll-view-duration)
          (set-piano-roll-lane-scroll
            (if (= lane-scroll nil)
              (+ piano-roll-lane-scroll (piano-roll-event-num event :delta-lanes 0))
              lane-scroll))))
      :zoom-view
      (piano-roll-zoom-view event)
      :zoom-lanes
      (piano-roll-zoom-lanes event)
      :set-cursor
      (set! piano-roll-cursor-time event.time)
      :resize-content-length
      (do
        (cool-off-follow)
        (seq-set-track-param :num-steps event.length))
      :marquee-select
      (set! piano-roll-selection-rect event)
      :finish-marquee-select
      (set! piano-roll-selection-rect nil)
      :select
      (do
        (set! piano-roll-selection-rect nil)
        (piano-roll-set-cursor-from-event event))
      :clear-selection
      (do
        (set! piano-roll-selection-rect nil)
        (piano-roll-set-cursor-from-event event))
      :finish-create-item
      (set! piano-roll-create-duration (piano-roll-action-duration event))
      :resize-item-absolute
      (set! piano-roll-create-duration (piano-roll-action-duration event)))
    (if (piano-roll-native-action? event)
      (set! piano-roll-status (seq-piano-roll-action event))
      (set! piano-roll-status "piano roll"))))

(effect-buffer "*piano-roll*"
  (timeline
    :height piano-roll-timeline-height
    :focusable true
    :sidebar-width 5
    :sidebar-style :piano
    :header-height piano-roll-header-height
    :time-ruler (dict :mode :bars-beats :beats-per-bar 4)
    :item-color (piano-roll-current-track-color)
    :loop-color (piano-roll-current-track-color)
    :tool piano-roll-tool
    :playhead-time (bind-seq "playhead")
    :cursor-time piano-roll-cursor-time
    :lanes SEQ.piano-roll-lanes
    :items SEQ.piano-roll-items
    :selection SEQ.piano-roll-selection
    :selection-rect piano-roll-selection-rect
    :view-start piano-roll-view-start
    :view-duration piano-roll-view-duration
    :zoom-min-duration piano-roll-min-view-duration
    :zoom-max-duration piano-roll-max-view-duration
    :content-length SEQ.tp-num-steps
    :content-length-min 1
    :content-length-max 256
    :lane-scroll piano-roll-lane-scroll
    :lane-height (piano-roll-lane-height-value)
    :scroll-viewport-height piano-roll-default-pane-height
    :snap 1
    :min-duration 0.03125
    :create-duration piano-roll-create-duration
    :move-snap-mode :alignment-helper
    :resize-snap :grid
    :snap-mode :floor
    :resize-snap-mode :alignment-helper
    :scroll-mode :smooth
    :on-action |event| (piano-roll-action event)))
