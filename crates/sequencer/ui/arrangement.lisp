;; ui/arrangement.lisp -- arrangement timeline view over song mode
;; (docs/arrangement-timeline-ui-spec.md). Renders to *arrangement* buffer;
;; loaded by ui/main.lisp. One timeline widget instance per lane: the scene
;; lane (the only instance with a header/time ruler) plus one headerless,
;; sidebar-less instance per visible track, all driven by the same shared
;; time-axis state so every lane stays in sync by construction (spec 5).

(defstate arrangement-tool :pointer)
(defstate arrangement-view-start 0)
(defstate arrangement-view-duration 64)
(defstate arrangement-cursor-time 0)
(defstate arrangement-selection '())
(defstate arrangement-selection-rect nil)
(defstate arrangement-status "arrangement")

(def arrangement-min-view-duration 4)
(def arrangement-max-view-duration 1024)
(def arrangement-view-padding 8)
(def arrangement-beats-per-bar 4)
(def arrangement-snap arrangement-beats-per-bar)
(def arrangement-header-height 1.6)
(def arrangement-scene-lane-height 3.6)
(def arrangement-track-lane-height 2.85)
;; Fixed width for the composed seqv-track-header column so every lane's time
;; axis starts at the same x; the scene lane leads with a spacer of the same
;; width (spec 4.2: the per-track sidebar role is played by the header).
(def arrangement-header-width 26)

(def arrangement-event-num (event key fallback)
  (let ((value (get event key)))
    (if (= value nil) fallback value)))

;; ── Shared time axis (spec 5.1) ────────────────────────────────────────────

(def arrangement-max-view-start (duration)
  (max 0 (- (+ SEQ.song-end-beat arrangement-view-padding) duration)))

(def set-arrangement-view-start (start duration)
  (set! arrangement-view-start
    (max 0 (min (arrangement-max-view-start duration) start))))

(def set-arrangement-zoom (event)
  (let ((cur-duration arrangement-view-duration)
        (factor (arrangement-event-num event :factor 1)))
    (let ((next-duration (max arrangement-min-view-duration
                           (min arrangement-max-view-duration
                             (/ arrangement-view-duration factor)))))
      (let ((anchor (arrangement-event-num event :anchor-time arrangement-view-start)))
        (let ((anchor-ratio
                (if (<= cur-duration 0)
                  0.5
                  (max 0 (min 1 (/ (- anchor arrangement-view-start) cur-duration))))))
          (do
            (set! arrangement-view-duration next-duration)
            (set-arrangement-view-start
              (- anchor (* anchor-ratio next-duration))
              next-duration)))))))

(def set-arrangement-cursor-time (time)
  (if (= time nil) nil (set! arrangement-cursor-time (max 0 time))))

;; Shared view-action routing (spec 5.2): every lane funnels scroll/zoom/
;; cursor/tool changes into the one shared axis, regardless of which lane the
;; pointer is over. `:lane-scroll`/`:delta-lanes` are ignored per spec 4.2 —
;; vertical navigation belongs to the buffer viewport, not the lanes.
(def arrangement-view-action (event)
  (match event.type
    :scroll-view
    (let ((view-start (get event :view-start)))
      (set-arrangement-view-start
        (if (= view-start nil)
          (+ arrangement-view-start (arrangement-event-num event :delta-time 0))
          view-start)
        arrangement-view-duration))
    :zoom-view
    (set-arrangement-zoom event)
    :set-cursor
    (set-arrangement-cursor-time (get event :time))
    :set-tool
    (set! arrangement-tool event.tool)))

(def arrangement-view-action? (event)
  (or (= event.type :scroll-view)
      (= event.type :zoom-view)
      (= event.type :set-cursor)
      (= event.type :set-tool)))

;; ── Items from the song read surface (spec 6/8) ────────────────────────────

(def arrangement-scene-name (scene)
  (if (< scene (len SEQ.scene-names))
    (nth SEQ.scene-names scene)
    (str "Scene " (+ scene 1))))

(def arrangement-row-end-beat (index)
  (if (< (+ index 1) (len SEQ.song-rows))
    (get (nth SEQ.song-rows (+ index 1)) :start-beat)
    SEQ.song-end-beat))

;; Scene-lane items (spec 8): one span per song row covering
;; [start_beat, next start_beat) labeled with the row's scene name. Spans (not
;; markers) so Slice C gets move/resize edges for free.
(def arrangement-scene-items ()
  (map
    (lambda (index)
      (let ((row (nth SEQ.song-rows index)))
        (dict
          :id (get row :id)
          :lane 0
          :start (get row :start-beat)
          :end (arrangement-row-end-beat index)
          :label (arrangement-scene-name (get row :scene))
          :selected (arrangement-row-selected? (get row :id))
          :color (list 0.52 0.56 0.62))))
    (range 0 (len SEQ.song-rows))))

(def arrangement-row-selected? (row-id)
  (> (len (filter (lambda (id) (= id row-id)) arrangement-selection)) 0))

(def arrangement-track-color (i)
  (if (and (>= i 0) (< i (len SEQ.track-colors)))
    (nth SEQ.track-colors i)
    (list 0.34 0.48 0.98)))

;; Override spans are tinted brighter than scene-provided spans — the
;; `from-override` render hint from the lane projection (song-mode-spec 5.5).
(def arrangement-clip-color (i from-override)
  (let ((color (arrangement-track-color i)))
    (if from-override
      (list
        (+ 0.4 (* 0.6 (nth color 0)))
        (+ 0.4 (* 0.6 (nth color 1)))
        (+ 0.4 (* 0.6 (nth color 2))))
      color)))

(def arrangement-track-clips (i)
  (if (< i (len SEQ.song-lanes))
    (nth SEQ.song-lanes i)
    '()))

;; Track-lane items (spec 6): spans whose resolved pattern is nil produce NO
;; item — a track with nothing playing renders as an empty lane.
(def arrangement-track-items (i)
  (map
    (lambda (clip)
      (dict
        :id (get clip :row-id)
        :lane 0
        :start (get clip :start-beat)
        :end (get clip :end-beat)
        :color (arrangement-clip-color i (get clip :from-override))))
    (filter (lambda (clip) (not (= (get clip :pattern-id) nil)))
      (arrangement-track-clips i))))

;; ── Action handlers ────────────────────────────────────────────────────────

;; Scene lane: view actions route to the shared axis; editing gestures land
;; in Slice C (spec 9). Until then edit actions are inert.
(def arrangement-scene-action (event)
  (if (arrangement-view-action? event)
    (arrangement-view-action event)
    (match event.type
      :select
      (do
        (set! arrangement-selection-rect nil)
        (set! arrangement-selection (get event :ids))
        (set-arrangement-cursor-time (get event :time)))
      :clear-selection
      (do
        (set! arrangement-selection-rect nil)
        (set! arrangement-selection '())
        (set-arrangement-cursor-time (get event :time)))
      :marquee-select
      (set! arrangement-selection-rect event)
      :finish-marquee-select
      (set! arrangement-selection-rect nil))))

;; Track lanes are read-only previews of the lane projection (spec 9.2):
;; only shared view actions are honored.
(def arrangement-track-action (event)
  (if (arrangement-view-action? event)
    (arrangement-view-action event)
    nil))

;; ── Lane instances (spec 4.1/4.2) ──────────────────────────────────────────

;; The scene lane is the ONLY instance with a header/time ruler; it doubles
;; as the arrangement's bar/beat ruler.
(def arrangement-scene-lane ()
  (timeline
    :height arrangement-scene-lane-height
    :focusable true
    :sidebar-width 0
    :header-height arrangement-header-height
    :time-ruler (dict :mode :bars-beats :beats-per-bar arrangement-beats-per-bar)
    :item-color (list 0.52 0.56 0.62)
    :loop-color (list 0.92 0.72 0.25)
    :tool arrangement-tool
    :playhead-time (bind-seq "song-position-beats")
    :cursor-time arrangement-cursor-time
    :items (arrangement-scene-items)
    :selection arrangement-selection
    :selection-rect arrangement-selection-rect
    :view-start arrangement-view-start
    :view-duration arrangement-view-duration
    :zoom-min-duration arrangement-min-view-duration
    :zoom-max-duration arrangement-max-view-duration
    :content-length SEQ.song-end-beat
    :content-length-min 1
    :content-length-max 8192
    :lane-scroll 0
    :snap arrangement-snap
    :min-duration 1
    :create-duration (* arrangement-beats-per-bar 4)
    :move-snap-mode :alignment-helper
    :resize-snap :grid
    :snap-mode :floor
    :resize-snap-mode :alignment-helper
    :scroll-mode :smooth
    :on-action |event| (arrangement-scene-action event)))

;; Headerless, sidebar-less, single-lane track instance (spec 4.2). Lane
;; scrolling is inert; the outer buffer viewport owns vertical navigation.
(def arrangement-track-lane (i)
  (timeline
    :height arrangement-track-lane-height
    :sidebar-width 0
    :header-height 0
    :tool arrangement-tool
    :playhead-time (bind-seq "song-position-beats")
    :cursor-time arrangement-cursor-time
    :items (arrangement-track-items i)
    :view-start arrangement-view-start
    :view-duration arrangement-view-duration
    :zoom-min-duration arrangement-min-view-duration
    :zoom-max-duration arrangement-max-view-duration
    :content-length SEQ.song-end-beat
    :lane-scroll 0
    :snap arrangement-snap
    :scroll-mode :smooth
    :on-action |event| (arrangement-track-action event)))

;; ── Buffer composition (spec 4.1) ──────────────────────────────────────────

(def arrangement-tool-button (label tool)
  (button label
    :key (str "arrangement-tool-" tool)
    :width 3.4 :height 1.2 :padding 0 :font-size 10
    :background-color (if (= arrangement-tool tool)
      (rgba 0.30 0.44 0.80 1.0)
      (rgba 0.10 0.11 0.13 1.0))
    :color (if (= arrangement-tool tool) :white :dim)
    :on-click |x y r| (set! arrangement-tool tool)))

(def arrangement-toolbar ()
  (h-stack :gap 0.4 :align :center :padding 0.3
    (arrangement-tool-button "Sel" :pointer)
    (arrangement-tool-button "Draw" :draw)
    (arrangement-tool-button "Erase" :erase)
    (arrangement-tool-button "Pan" :pan)
    (box :width 1 :height 0.1 :bg :transparent)
    (badge arrangement-status
      :key "arrangement-status-badge"
      :font-size 10 :height 1.2 :padding 0.2
      :h-align :left
      :background-color :transparent
      :border-color :transparent
      :color :dim
      :bg :transparent)))

(def arrangement-empty-banner ()
  (box :width :fill :height 2.2 :padding 0.4
    :background-color (rgba 0.10 0.11 0.13 1.0)
    (label "No song yet — record an arrangement (ARR REC) or define one with def-song."
      :key "arrangement-empty-banner-label"
      :font-size 11 :color :dim :bg :transparent)))

(def arrangement-track-row (i)
  (h-stack :width :fill :gap 0.6 :align :start
    (box :width arrangement-header-width
      (seqv-track-header i))
    (arrangement-track-lane i)))

(effect-buffer "*arrangement*"
  (v-stack :padding 0.0 :gap 0.2
    (arrangement-toolbar)
    (if SEQ.song-exists
      (box :width 0 :height 0 :bg :transparent)
      (arrangement-empty-banner))
    (h-stack :width :fill :gap 0.6 :align :start
      (box :key "arrangement-scene-header-spacer"
        :width arrangement-header-width :height arrangement-scene-lane-height
        :bg :transparent)
      (box :width :fill :key "arrangement-scene-lane"
        (arrangement-scene-lane)))
    (each (seq-visible-track-indices) |i|
      (subtree :key (str "arr-track-" (nth SEQ.track-ids i))
        (arrangement-track-row i)))))
