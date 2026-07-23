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
;; Live-drag preview state (spec 9.1): live gesture actions update this ghost
;; only; the terminal :finish-* action lowers to exactly one song primitive
;; via seq-arrangement-action and clears it. A primitive rejection reports on
;; the status line and, because items derive from the committed song, the
;; view snaps back on its own.
(defstate arrangement-ghost nil)

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

(def arrangement-ghost-kind ()
  (if (= arrangement-ghost nil) nil (get arrangement-ghost :kind)))

(def arrangement-ghost-row? (kind row-id)
  (and (= (arrangement-ghost-kind) kind)
    (= (get arrangement-ghost :row-id) row-id)))

;; Ghost overlay for one scene span (spec 9.1 live preview): a move ghost
;; shifts its row's span; a resize ghost moves the boundary shared by the
;; resized row's end and the next row's start.
(def arrangement-scene-span-start (row index start)
  (if (arrangement-ghost-row? :move (get row :id))
    (get arrangement-ghost :start)
    (if (and (> index 0)
          (arrangement-ghost-row? :resize
            (get (nth SEQ.song-rows (- index 1)) :id)))
      (get arrangement-ghost :end)
      start)))

(def arrangement-scene-span-end (row start end)
  (if (arrangement-ghost-row? :move (get row :id))
    (+ (get arrangement-ghost :start) (- end start))
    (if (arrangement-ghost-row? :resize (get row :id))
      (get arrangement-ghost :end)
      end)))

;; Scene-lane items (spec 8): one span per song row covering
;; [start_beat, next start_beat) labeled with the row's scene name. Spans (not
;; markers) so Slice C gets move/resize edges for free.
(def arrangement-scene-row-items ()
  (map
    (lambda (index)
      (let ((row (nth SEQ.song-rows index)))
        (let ((start (get row :start-beat))
              (end (arrangement-row-end-beat index)))
          (dict
            :id (get row :id)
            :lane 0
            :start (arrangement-scene-span-start row index start)
            :end (arrangement-scene-span-end row start end)
            :label (arrangement-scene-name (get row :scene))
            :kind :scene
            :selected (arrangement-row-selected? (get row :id))
            :color (list 0.52 0.56 0.62)))))
    (range 0 (len SEQ.song-rows))))

(def arrangement-scene-items ()
  (if (= (arrangement-ghost-kind) :create)
    (append (arrangement-scene-row-items)
      (list (dict
              :id :ghost-create
              :lane 0
              :start (get arrangement-ghost :start)
              :end (get arrangement-ghost :end)
              :kind :scene
              :label (arrangement-scene-name (or SEQ.current-pattern 0))
              :color (list 0.72 0.76 0.82))))
    (arrangement-scene-row-items)))

;; Song end, with the content-length drag ghost applied so the end marker
;; previews in every lane while dragging (spec 9.3).
(def arrangement-content-length ()
  (if (= (arrangement-ghost-kind) :end)
    (get arrangement-ghost :length)
    SEQ.song-end-beat))

;; The model rejects an end at/before the last row's start (spec 9.3); the
;; widget clamp mirrors that boundary.
(def arrangement-content-length-min ()
  (let ((count (len SEQ.song-rows)))
    (if (= count 0)
      1
      (max 1 (get (nth SEQ.song-rows (- count 1)) :start-beat)))))

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

;; ── MIDI content flattening (spec 7.1) ─────────────────────────────────────
;; SEQ.song-lane-events carries, per track, raw (time transpose velocity)
;; events for every pool pattern the lane projection references. The view
;; flattens one pattern cycle into normalized (offset, value) dots — a
;; snapshot-time preview, deliberately impressionistic at arrangement zoom.

(def arrangement-dot-cap 256)

(def arrangement-lane-pattern-events (track pattern-id)
  (let ((entries (if (< track (len SEQ.song-lane-events))
                   (nth SEQ.song-lane-events track)
                   '())))
    (let ((matches (filter (lambda (entry) (= (get entry :pattern-id) pattern-id))
                     entries)))
      (if (> (len matches) 0) (nth matches 0) nil))))

;; Vertical placement: spread the pattern's own transpose range across the
;; item rect (single-pitch patterns sit mid-rect).
(def arrangement-dot-value (note lo hi)
  (if (= hi lo)
    0.5
    (+ 0.15 (* 0.7 (/ (- note lo) (- hi lo))))))

;; Cap dots per item at arrangement-dot-cap, densest-first: events collapse
;; into 1/cap-wide time buckets (one dot per bucket), so dense clusters thin
;; out first while isolated events always survive. Events arrive step-ordered
;; from the read surface.
(def arrangement-pattern-dots (entry)
  (let ((events (get entry :events))
        (num-steps (max 1 (get entry :num-steps))))
    (if (= (len events) 0)
      '()
      (let ((lo (reduce |acc event| (min acc (nth event 1))
                  (nth (nth events 0) 1) events))
            (hi (reduce |acc event| (max acc (nth event 1))
                  (nth (nth events 0) 1) events)))
        (get
          (reduce |acc event|
            (let ((offset (max 0 (min 0.999 (/ (nth event 0) num-steps)))))
              (let ((bucket (floor (* offset arrangement-dot-cap))))
                (if (= bucket (get acc :last))
                  acc
                  (dict :last bucket
                    :dots (append (get acc :dots)
                            (list (dict :offset offset
                                    :value (arrangement-dot-value (nth event 1) lo hi))))))))
            (dict :last -1 :dots '())
            events)
          :dots)))))

(def arrangement-clip-content (i clip)
  (let ((entry (arrangement-lane-pattern-events i (get clip :pattern-id))))
    (if (= entry nil)
      nil
      (let ((dots (arrangement-pattern-dots entry)))
        (if (= (len dots) 0) nil (dict :dots dots))))))

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
        :kind :midi
        :content (arrangement-clip-content i clip)
        :color (arrangement-clip-color i (get clip :from-override))))
    (filter (lambda (clip) (not (= (get clip :pattern-id) nil)))
      (arrangement-track-clips i))))

;; ── Action handlers ────────────────────────────────────────────────────────

;; Lower one finished gesture to song primitives through the Rust translator
;; (spec 9.1): exactly one primitive per gesture, validation/undo/rejection
;; reporting owned by the song host commands.
(def arrangement-edit-finish (payload)
  (do
    (set! arrangement-ghost nil)
    (set! arrangement-status (seq-arrangement-action payload))))

;; Scene lane (spec 9.2: the only editable lane). View actions route to the
;; shared axis; live edit actions update the ghost preview only; terminal
;; actions commit through arrangement-edit-finish.
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
      (set! arrangement-selection-rect nil)
      ;; Live drags: ghost only, never a primitive (spec 9.1).
      :move-items-absolute
      (set! arrangement-ghost
        (dict :kind :move
          :row-id (get event :anchor-id)
          :start (get event :start)))
      :resize-item-absolute
      (set! arrangement-ghost
        (dict :kind :resize
          :row-id (get event :id)
          :end (get event :time)))
      :create-item
      (set! arrangement-ghost
        (dict :kind :create
          :start (get event :start)
          :end (get event :end)))
      :resize-content-length
      (set! arrangement-ghost
        (dict :kind :end :length (get event :length)))
      ;; Terminal actions: one primitive each, from the ghost's final values
      ;; (the widget's finish actions carry ids, not times).
      :finish-move-items
      (if (= (arrangement-ghost-kind) :move)
        (arrangement-edit-finish
          (dict :type :finish-move-items
            :row-id (get arrangement-ghost :row-id)
            :start (get arrangement-ghost :start)))
        (set! arrangement-ghost nil))
      :finish-resize-items
      (if (= (arrangement-ghost-kind) :resize)
        (arrangement-edit-finish
          (dict :type :finish-resize-items
            :row-id (get arrangement-ghost :row-id)
            :end (get arrangement-ghost :end)))
        (set! arrangement-ghost nil))
      :finish-create-item
      (arrangement-edit-finish
        (dict :type :finish-create-item
          :start (get event :start)
          :scene (or SEQ.current-pattern 0)))
      :finish-resize-content-length
      (arrangement-edit-finish
        (dict :type :finish-resize-content-length
          :length (get event :length)))
      :delete-items
      (arrangement-edit-finish
        (dict :type :delete-items :ids (get event :ids))))))

;; Track lanes are read-only previews of the lane projection (spec 9.2):
;; only shared view actions are honored.
(def arrangement-track-action (event)
  (if (arrangement-view-action? event)
    (arrangement-view-action event)
    nil))

;; ── Lane instances (spec 4.1/4.2) ──────────────────────────────────────────

;; The scene lane is the ONLY instance with a header/time ruler; it doubles
;; as the arrangement's bar/beat ruler.
;; Every lane is a flex child (:width 0 :flex 1) of its row: it absorbs
;; exactly the width remaining after the fixed header column, so no row can
;; overflow the pane and drag the buffer viewport into horizontal scrolling.
(def arrangement-scene-lane ()
  (timeline
    :key "arrangement-scene-lane"
    :width 0 :flex 1
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
    :content-length (arrangement-content-length)
    :content-length-min (arrangement-content-length-min)
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
    :key (str "arrangement-track-lane-" i)
    :width 0 :flex 1
    :height arrangement-track-lane-height
    ;; Vertical scrolling belongs to the enclosing track scroll container;
    ;; horizontal deltas still pan the shared time axis.
    :scroll-passthrough :vertical
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
    :content-length (arrangement-content-length)
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

;; Rows wrap their h-stack in a :width :fill box (the sequencer.lisp track-row
;; idiom): the box stretches to the pane, which gives the inner h-stack a
;; bounded width for flex distribution — without it the row collapses to its
;; fixed content and the flexed lane measures ~zero wide.
(def arrangement-track-row (i)
  (box :width :fill
    (h-stack :width :fill :gap 0.6 :align :start
      (box :width arrangement-header-width
        (seqv-track-header i))
      (arrangement-track-lane i))))

;; Rows stack with :gap 0 so the timeline instances are vertically flush —
;; the pointer is always over a lane, keeping scroll/zoom gestures captured
;; by the timelines instead of leaking to the buffer viewport.
;;
;; The toolbar and scene lane (the arrangement's one ruler) sit OUTSIDE the
;; track scroll container, so they stay pinned while the track rows scroll
;; vertically inside it. Track lanes pass vertical scrolling through to the
;; container (:scroll-passthrough :vertical).
(effect-buffer "*arrangement*"
  (v-stack :padding 0.0 :gap 0.0
    (arrangement-toolbar)
    (if SEQ.song-exists
      (box :width 0 :height 0 :bg :transparent)
      (arrangement-empty-banner))
    (box :width :fill
      (h-stack :width :fill :gap 0.6 :align :start
        (box :key "arrangement-scene-header-spacer"
          :width arrangement-header-width :height arrangement-scene-lane-height
          :bg :transparent)
        (arrangement-scene-lane)))
    (scroll :key "arrangement-track-scroll" :width :fill :flex 1
      (v-stack :width :fill :gap 0.0
        (each (seq-visible-track-indices) |i|
          (subtree :key (str "arr-track-" (nth SEQ.track-ids i))
            (arrangement-track-row i)))))))
