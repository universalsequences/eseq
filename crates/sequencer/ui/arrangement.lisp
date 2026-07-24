;; ui/arrangement.lisp -- arrangement timeline view over song mode
;; (docs/arrangement-timeline-ui-spec.md). Renders to *arrangement* buffer;
;; loaded by ui/main.lisp. One timeline widget instance per lane: the scene
;; lane (the only instance with a header/time ruler) plus one headerless,
;; sidebar-less instance per visible track, all driven by the same shared
;; time-axis state so every lane stays in sync by construction (spec 5).

(defstate arrangement-view-start 0)
(defstate arrangement-view-duration 64)
(defstate arrangement-cursor-time 0)
;; The edit cursor is track-specific (Ableton-style): clicking a time in a
;; track lane parks the cursor on that track; -1 is the scene lane. Later
;; edits ("paste clip at cursor") get both a time and a target track.
(defstate arrangement-cursor-track -1)
(defstate arrangement-selection '())
(defstate arrangement-selection-rect nil)
;; Track-clip selection (spec 9.2 extension): ids are the first row-id of a
;; merged clip, valid only within the owning track's lane. Selecting in any
;; lane clears the other kind so Backspace is never ambiguous.
(defstate arrangement-track-selection '())
(defstate arrangement-selected-track -1)
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
;; Clip title-bar height in cells (region spec 3.1): the move/resize strip
;; above each clip's body. Fixed rather than proportional so clips read the
;; same at any lane height; tune by eye against the Ableton reference.
(def arrangement-clip-title-bar-height 0.9)
;; Clip corner radius in CELLS (GarageBand-style rounded clips), so it scales
;; with the UI zoom like the lane heights above. 0 gives the square clips
;; every other timeline host draws.
(def arrangement-clip-corner-radius 0.22)
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

(def set-arrangement-cursor (time track)
  (if (= time nil)
    nil
    (do
      (set! arrangement-cursor-time (max 0 time))
      (set! arrangement-cursor-track track))))

;; The lane that owns the cursor shows it; every other lane passes nil.
(def arrangement-lane-cursor-time (track)
  (if (= arrangement-cursor-track track) arrangement-cursor-time nil))

;; Shared view-action routing (spec 5.2): every lane funnels scroll/zoom
;; into the one shared time axis, regardless of which lane the pointer is
;; over. `:lane-scroll`/`:delta-lanes` are ignored per spec 4.2 — vertical
;; navigation belongs to the track scroll container.
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
    (set-arrangement-zoom event)))

(def arrangement-view-action? (event)
  (or (= event.type :scroll-view)
      (= event.type :zoom-view)))

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
;; markers) so Slice C gets move/resize edges for free. Rows created by
;; track-clip surgery keep the same base scene as their predecessor; labeling
;; only scene CHANGES keeps the lane readable ("Scene 4 | | |" reads as one
;; region with row splits, not four different states).
(def arrangement-scene-row-label (row index)
  (if (and (> index 0)
        (= (get row :scene) (get (nth SEQ.song-rows (- index 1)) :scene)))
    nil
    (arrangement-scene-name (get row :scene))))

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
            :label (arrangement-scene-row-label row index)
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
;; The lift is multiplicative, not a lerp toward white: mixing in white
;; desaturated the track color into pastel, so arrangement clips no longer
;; read as the same color the session grid and piano roll use for the track.
(def arrangement-clip-color (i from-override)
  (let ((color (arrangement-track-color i)))
    (if from-override
      (list
        (min 1 (* 1.15 (nth color 0)))
        (min 1 (* 1.15 (nth color 1)))
        (min 1 (* 1.15 (nth color 2))))
      color)))

(def arrangement-track-clips (i)
  (if (< i (len SEQ.song-lanes))
    (nth SEQ.song-lanes i)
    '()))

;; Steps-per-beat of a pattern from the lane-events read surface; nil when
;; the entry is missing (merge falls back to pattern identity alone).
(def arrangement-pattern-steps-per-beat (track pattern-id)
  (let ((entry (arrangement-lane-pattern-events track pattern-id)))
    (if (or (= entry nil) (<= (get entry :length-beats) 0))
      nil
      (/ (get entry :num-steps) (get entry :length-beats)))))

;; Phase continuity between two adjacent same-pattern spans (takes spec 7):
;; the later span continues the earlier clip iff its stored offset equals the
;; earlier anchor's offset advanced by the elapsed beats, modulo the pattern
;; length. Discontinuous spans are separate clips (the later one re-anchors).
;; Take spans (spec 6.1) are linear — continuity is exact offset advance, no
;; modulo.
(def arrangement-offsets-continuous? (track cur clip)
  (if (not (= (get cur :take-id) nil))
    (let ((entry (arrangement-lane-take-events track (get cur :take-id))))
      (if (or (= entry nil) (<= (get entry :length-beats) 0))
        true
        (let ((spb (/ (get entry :num-steps) (get entry :length-beats))))
          (let ((expected (+ (or (get cur :offset-steps) 0)
                            (* spb (- (get clip :start-beat) (get cur :start-beat))))))
            (let ((err (- expected (or (get clip :offset-steps) 0))))
              (< (max err (- 0 err)) 0.0001))))))
    (let ((spb (arrangement-pattern-steps-per-beat track (get cur :pattern-id)))
          (entry (arrangement-lane-pattern-events track (get cur :pattern-id))))
      (if (or (= spb nil) (= entry nil))
        true
        (let ((num-steps (max 1 (get entry :num-steps)))
              (expected (+ (or (get cur :offset-steps) 0)
                          (* spb (- (get clip :start-beat) (get cur :start-beat))))))
          (let ((cycles (/ (- expected (or (get clip :offset-steps) 0)) num-steps)))
            (let ((wrap-error (- cycles (floor (+ cycles 0.5)))))
              (< (max wrap-error (- 0 wrap-error)) 0.0001))))))))

;; Merge adjacent same-pattern, phase-continuous spans into one clip (the
;; spec's "merging is a view concern"): a row split made to edit ANOTHER
;; track must not visually fragment this track's clip. Empty spans never
;; merge — they are the gaps — and a re-anchored span (offset discontinuity)
;; starts a new clip. The merged clip keeps the FIRST row's id as its stable
;; gesture identity and the first span's start/offset as its phase anchor.
(def arrangement-merge-clip-fold (i acc clip)
  (let ((cur (get acc :cur)))
    (if (= cur nil)
      (dict :done (get acc :done) :cur clip)
      (if (and (= (get cur :pattern-id) (get clip :pattern-id))
            (= (get cur :take-id) (get clip :take-id))
            (= (get cur :end-beat) (get clip :start-beat))
            (arrangement-offsets-continuous? i cur clip))
        (dict :done (get acc :done)
          :cur (dict
                 :row-id (get cur :row-id)
                 :start-beat (get cur :start-beat)
                 :end-beat (get clip :end-beat)
                 :pattern-id (get cur :pattern-id)
                 :take-id (get cur :take-id)
                 :offset-steps (get cur :offset-steps)
                 :from-override (or (get cur :from-override)
                                  (get clip :from-override))))
        (dict :done (append (get acc :done) (list cur)) :cur clip)))))

(def arrangement-merged-track-clips (i)
  (let ((folded (reduce |acc clip| (arrangement-merge-clip-fold i acc clip)
                  (dict :done '() :cur nil)
                  (filter (lambda (clip)
                            (not (and (= (get clip :pattern-id) nil)
                                   (= (get clip :take-id) nil))))
                    (arrangement-track-clips i)))))
    (if (= (get folded :cur) nil)
      (get folded :done)
      (append (get folded :done) (list (get folded :cur))))))

(def arrangement-find-track-clip (i row-id)
  (let ((matches (filter (lambda (clip) (= (get clip :row-id) row-id))
                   (arrangement-merged-track-clips i))))
    (if (> (len matches) 0) (nth matches 0) nil)))

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

;; Aggregated take content (takes spec 11.3): one entry per take, event
;; times continuous across chunk boundaries, :num-steps = take length.
(def arrangement-lane-take-events (track take-id)
  (let ((entries (if (< track (len SEQ.song-lane-events))
                   (nth SEQ.song-lane-events track)
                   '())))
    (let ((matches (filter (lambda (entry) (= (get entry :take-id) take-id))
                     entries)))
      (if (> (len matches) 0) (nth matches 0) nil))))

;; Vertical placement: spread the pattern's own transpose range across the
;; item rect (single-pitch patterns sit mid-rect).
(def arrangement-dot-value (note lo hi)
  (if (= hi lo)
    0.5
    (+ 0.15 (* 0.7 (/ (- note lo) (- hi lo))))))

;; Note length in steps (4th event element, region spec 3.2). Older/short
;; event rows degrade to a point dot rather than erroring.
(def arrangement-event-duration (event)
  (if (< (len event) 4)
    0
    (let ((duration (nth event 3)))
      (if (= duration nil) 0 (max 0 duration)))))

;; Cap dots per item at arrangement-dot-cap, densest-first: events collapse
;; into 1/cap-wide time buckets (one dot per bucket), so dense clusters thin
;; out first while isolated events always survive. Events arrive step-ordered
;; from the read surface. Only events inside the step window
;; [from, from + span) are shown, normalized to the window — a re-anchored
;; take clip (nonzero offset-steps) renders the slice it actually plays.
(def arrangement-windowed-dots (entry from span)
  (let ((events (filter (lambda (event)
                          (and (>= (nth event 0) from)
                            (< (nth event 0) (+ from span))))
                  (get entry :events))))
    (if (= (len events) 0)
      '()
      (let ((lo (reduce |acc event| (min acc (nth event 1))
                  (nth (nth events 0) 1) events))
            (hi (reduce |acc event| (max acc (nth event 1))
                  (nth (nth events 0) 1) events)))
        (get
          (reduce |acc event|
            (let ((offset (max 0 (min 0.999 (/ (- (nth event 0) from) span)))))
              (let ((bucket (floor (* offset arrangement-dot-cap))))
                (if (= bucket (get acc :last))
                  acc
                  (dict :last bucket
                    :dots (append (get acc :dots)
                            (list (dict :offset offset
                                    :value (arrangement-dot-value (nth event 1) lo hi)
                                    ;; Real note length normalized to the
                                    ;; drawn window, clamped so a note never
                                    ;; paints past the item's end.
                                    :width (max 0
                                             (min (- 1 offset)
                                               (/ (arrangement-event-duration event)
                                                 span))))))))))
            (dict :last -1 :dots '())
            events)
          :dots)))))

(def arrangement-pattern-dots (entry)
  (arrangement-windowed-dots entry 0 (max 1 (get entry :num-steps))))

;; True drawn window of a take clip (takes spec 11.3): a take is finite and
;; never loops, so the item ends at min(row-span end, start + remaining
;; take length at this clip's offset). A row extending past the take's end
;; is the silent tail — it renders as empty lane, matching what plays.
(def arrangement-take-clip-window (i clip)
  (let ((entry (arrangement-lane-take-events i (get clip :take-id))))
    (if (or (= entry nil)
          (<= (get entry :length-beats) 0)
          (<= (get entry :num-steps) 0))
      nil
      (let ((num-steps (get entry :num-steps))
            (length-beats (get entry :length-beats))
            (offset (or (get clip :offset-steps) 0)))
        (let ((step-beats (/ length-beats num-steps)))
          (let ((remaining-steps (max 0 (- num-steps offset))))
            (let ((end (min (get clip :end-beat)
                          (+ (get clip :start-beat)
                            (* remaining-steps step-beats)))))
              (dict
                :offset-steps offset
                :window-steps
                (max 0.000001
                  (/ (- end (get clip :start-beat)) step-beats))
                :end-beat end))))))))

;; One repetition's fraction of the clip span (widget :cycle key): a clip
;; longer than the pattern tiles the preview per cycle with separator lines,
;; DAW-style; a clip at or shorter than one cycle spans the whole item.
(def arrangement-clip-cycle (entry clip)
  (let ((span (- (get clip :end-beat) (get clip :start-beat)))
        (length-beats (get entry :length-beats)))
    (if (and (> length-beats 0) (> span length-beats))
      (/ length-beats span)
      1)))

(def arrangement-clip-content (i clip)
  (let ((entry (if (= (get clip :take-id) nil)
                 (arrangement-lane-pattern-events i (get clip :pattern-id))
                 (arrangement-lane-take-events i (get clip :take-id)))))
    (if (= entry nil)
      nil
      (let ((dots
              (if (= (get clip :take-id) nil)
                (arrangement-pattern-dots entry)
                ;; Take dots render the exact step window the clip plays
                ;; (offset..offset+span), normalized to the drawn item —
                ;; so timeline dots line up with the take's actual notes.
                (let ((window (arrangement-take-clip-window i clip)))
                  (if (= window nil)
                    (arrangement-pattern-dots entry)
                    (arrangement-windowed-dots entry
                      (get window :offset-steps)
                      (get window :window-steps)))))))
        (if (= (len dots) 0)
          nil
          (dict :dots dots
            ;; Takes never loop (spec 11.3): one item, no repeat tiling.
            :cycle (if (= (get clip :take-id) nil)
                     (arrangement-clip-cycle entry clip)
                     1)))))))

;; Live resize ghost for a track clip: while the edge drag is in flight the
;; clip previews its new end; the finish action lowers to one song-track-paint.
(def arrangement-track-ghost-clip (i clip)
  (if (and (= (arrangement-ghost-kind) :track-resize)
        (= (get arrangement-ghost :track) i)
        (= (get arrangement-ghost :row-id) (get clip :row-id)))
    (dict
      :row-id (get clip :row-id)
      :start-beat (get clip :start-beat)
      :end-beat (max (+ (get clip :start-beat) 1)
                  (min SEQ.song-end-beat (get arrangement-ghost :end)))
      :pattern-id (get clip :pattern-id)
      :take-id (get clip :take-id)
      :from-override (get clip :from-override))
    clip))

;; Track-lane items (spec 6): merged clips; spans whose resolved pattern is
;; nil produce NO item — a track with nothing playing renders as an empty
;; lane, and empty gaps are exactly where merged clips end.
(def arrangement-track-items (i)
  (map
    (lambda (raw)
      (let ((clip (arrangement-track-ghost-clip i raw)))
        (dict
          :id (get clip :row-id)
          :lane 0
          :start (get clip :start-beat)
          ;; Take items clamp to the take's true remaining length (takes
          ;; spec 11.3) — the row may extend past the take's end, but that
          ;; tail is silent and draws as empty lane.
          :end (if (= (get clip :take-id) nil)
                 (get clip :end-beat)
                 (let ((window (arrangement-take-clip-window i clip)))
                   (if (= window nil)
                     (get clip :end-beat)
                     (get window :end-beat))))
          :kind :midi
          :content (arrangement-clip-content i clip)
          :color (arrangement-clip-color i (get clip :from-override)))))
    (arrangement-merged-track-clips i)))

;; Selection prop for one track lane: only the owning track shows its ids.
;; The bound clip (takes spec 16.6) is Rust-side persistent timeline state,
;; so it carries the highlight after a view switch, when this buffer's own
;; selection defstate has been reset.
(def arrangement-lane-selection (i)
  (if (= arrangement-selected-track i)
    arrangement-track-selection
    (if (and (not (= SEQ.song-bound-clip nil))
          (= (nth SEQ.song-bound-clip 0) i))
      (list (nth SEQ.song-bound-clip 1))
      '())))

;; ── Action handlers ────────────────────────────────────────────────────────

;; Lower one finished gesture to song primitives through the Rust translator
;; (spec 9.1): exactly one primitive per gesture, validation/undo/rejection
;; reporting owned by the song host commands.
(def arrangement-edit-finish (payload)
  (do
    (set! arrangement-ghost nil)
    (seq-arrangement-action payload)))

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
        (set! arrangement-track-selection '())
        (set! arrangement-selected-track -1)
        (set! arrangement-selection (get event :ids))
        ;; A scene row spans every track and names no single clip, so it
        ;; releases the sound binding (takes spec 16.6 cause 2).
        (seq-song-deselect-clip)
        (set-arrangement-cursor (get event :time) -1))
      :clear-selection
      (do
        (set! arrangement-selection-rect nil)
        (set! arrangement-selection '())
        (seq-song-deselect-clip)
        (set-arrangement-cursor (get event :time) -1))
      :set-cursor
      (set-arrangement-cursor (get event :time) -1)
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
          :end (get event :end)
          :scene (or SEQ.current-pattern 0)))
      :finish-resize-content-length
      (arrangement-edit-finish
        (dict :type :finish-resize-content-length
          :length (get event :length)))
      :delete-items
      (arrangement-edit-finish
        (dict :type :delete-items :ids (get event :ids))))))

;; Track-lane clip editing (Ableton-style): select a clip, Backspace deletes
;; it, dragging its end edge resizes it — fewer loops leaves silence, more
;; eats into whatever follows. Every gesture lowers to ONE song-track-paint
;; primitive; the paint's row surgery (split + per-row override set) is owned
;; by the primitive, never composed here.
(def arrangement-paint-track (i start end pattern-id)
  (arrangement-edit-finish
    (dict :type :track-paint
      :track i :start start :end end :pattern-id pattern-id)))

;; Paint continuing an existing clip's phase (takes spec 7.4): the anchor is
;; the clip's start beat + stored offset, so a grown region carries the loop
;; forward instead of re-starting it at the paint start.
(def arrangement-paint-track-anchored (i start end pattern-id anchor anchor-offset)
  (arrangement-edit-finish
    (dict :type :track-paint
      :track i :start start :end end :pattern-id pattern-id
      :anchor-beat anchor
      :anchor-offset-steps (or anchor-offset 0))))

(def arrangement-track-resize-finish (i)
  (let ((row-id (get arrangement-ghost :row-id))
        (new-end (max 0 (min SEQ.song-end-beat (get arrangement-ghost :end)))))
    (let ((clip (arrangement-find-track-clip i row-id)))
      (do
        (set! arrangement-ghost nil)
        (if (= clip nil)
          nil
          (let ((old-end (get clip :end-beat)))
            (if (< new-end old-end)
              ;; Shrink: silence the released tail (shrinking to the clip
              ;; start deletes it outright).
              (arrangement-paint-track i
                (max (get clip :start-beat) new-end) old-end nil)
              (if (> new-end old-end)
                ;; Grow: the clip's pattern eats into whatever follows,
                ;; continuing the clip's own loop phase (takes spec 7.4).
                (arrangement-paint-track-anchored i old-end new-end
                  (get clip :pattern-id)
                  (get clip :start-beat) (get clip :offset-steps))
                nil))))))))

(def arrangement-track-delete (i ids)
  (do
    (set! arrangement-track-selection '())
    (seq-song-deselect-clip)
    (map
      (lambda (row-id)
        (let ((clip (arrangement-find-track-clip i row-id)))
          (if (= clip nil)
            nil
            (arrangement-paint-track i
              (get clip :start-beat) (get clip :end-beat) nil))))
      ids)))

(def arrangement-track-action (i event)
  (if (arrangement-view-action? event)
    (arrangement-view-action event)
    (match event.type
      :select
      (do
        (set! arrangement-selection '())
        (set! arrangement-selection-rect nil)
        (set! arrangement-selected-track i)
        (set! arrangement-track-selection (get event :ids))
        ;; Selecting a clip is the explicit sound-binding gesture (takes
        ;; spec 16.2/16.6): it re-binds this track's device panel, monitor
        ;; sound and take punch-in template. The binding lives in Rust so it
        ;; survives view switches and transport.
        (if (= (len (get event :ids)) 0)
          (seq-song-deselect-clip)
          (seq-song-select-clip i (nth (get event :ids) 0)))
        (set-arrangement-cursor (get event :time) i))
      :clear-selection
      (do
        (set! arrangement-track-selection '())
        (seq-song-deselect-clip)
        (set-arrangement-cursor (get event :time) i))
      :set-cursor
      (set-arrangement-cursor (get event :time) i)
      ;; Live edge drag: ghost preview only (spec 9.1).
      :resize-item-absolute
      (set! arrangement-ghost
        (dict :kind :track-resize :track i
          :row-id (get event :id) :end (get event :time)))
      :finish-resize-items
      (if (and (= (arrangement-ghost-kind) :track-resize)
            (= (get arrangement-ghost :track) i))
        (arrangement-track-resize-finish i)
        (set! arrangement-ghost nil))
      :delete-items
      (arrangement-track-delete i (get event :ids))
      ;; Whole-clip moves are not lowered yet: never leave a stale ghost.
      :finish-move-items
      (set! arrangement-ghost nil))))

;; ── Scene drag-and-drop (Ableton-style, replaces the draw tool) ────────────
;; The transport scene pills are drag sources (:drag-type "transport-scene");
;; dropping one on any lane inserts a row launching that scene at the drop
;; beat, snapped to the bar grid. The drop event's :sx is the normalized
;; (-1..1) position within the lane, which maps straight onto the shared view
;; span because lanes have no sidebar.
(def arrangement-drop-time (event)
  (let ((ratio (max 0 (min 1 (/ (+ (arrangement-event-num event :sx -1) 1) 2)))))
    (let ((time (+ arrangement-view-start (* ratio arrangement-view-duration))))
      (max 0 (* arrangement-snap (floor (/ time arrangement-snap)))))))

(def arrangement-drop-scene (event)
  (let ((scene (get (get event :payload) :scene)))
    (if (= scene nil)
      nil
      (let ((start (arrangement-drop-time event)))
        (arrangement-edit-finish
          (dict :type :finish-create-item
            :start start
            ;; Dropping at/past the song end extends it by four bars from
            ;; the drop point (the translator only uses :end in that case).
            :end (+ start (* arrangement-beats-per-bar 4))
            :scene scene))))))

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
    :title-bar-height arrangement-clip-title-bar-height
    :item-corner-radius arrangement-clip-corner-radius
    :item-color (list 0.52 0.56 0.62)
    :loop-color (list 0.92 0.72 0.25)
    :playhead-time (bind-seq "song-position-beats")
    :cursor-time (arrangement-lane-cursor-time -1)
    :drop-types (list "transport-scene")
    :on-drop (lambda (event) (arrangement-drop-scene event))
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
    :title-bar-height arrangement-clip-title-bar-height
    :item-corner-radius arrangement-clip-corner-radius
    :sidebar-width 0
    :header-height 0
    :focusable true
    :playhead-time (bind-seq "song-position-beats")
    :cursor-time (arrangement-lane-cursor-time i)
    :drop-types (list "transport-scene")
    :on-drop (lambda (event) (arrangement-drop-scene event))
    :items (arrangement-track-items i)
    :selection (arrangement-lane-selection i)
    :view-start arrangement-view-start
    :view-duration arrangement-view-duration
    :zoom-min-duration arrangement-min-view-duration
    :zoom-max-duration arrangement-max-view-duration
    :content-length (arrangement-content-length)
    :lane-scroll 0
    :snap arrangement-snap
    :min-duration 1
    :resize-snap :grid
    :snap-mode :floor
    :resize-snap-mode :alignment-helper
    :scroll-mode :smooth
    :on-action |event| (arrangement-track-action i event)))

;; ── Buffer composition (spec 4.1) ──────────────────────────────────────────

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
  (box :width :fill :border-color :bg :border-width 2
    (h-stack :width :fill :gap 0.6 :align :start
      (box :height :fill :width arrangement-header-width
        (seqv-track-header i))
      (arrangement-track-lane i))))

;; Rows stack with :gap 0 so the timeline instances are vertically flush —
;; the pointer is always over a lane, keeping scroll/zoom gestures captured
;; by the timelines instead of leaking to the buffer viewport.
;;
;; No mode toolbar (Ableton-style): pointer gestures + scene drag-and-drop
;; and double-click cover editing; Backspace deletes the selection. The scene
;; lane (the arrangement's one ruler) sits OUTSIDE the track scroll container
;; so it stays pinned while track rows scroll vertically inside it.
;; The step tile hides the global status line, so song-primitive rejections
;; (including "song editing is unavailable during song playback/capture")
;; surface here; the strip disappears on the next successful edit.
(def arrangement-error-banner ()
  (if (= SEQ.song-edit-error nil)
    (box :width 0 :height 0 :bg :transparent)
    (box :width :fill :height 1.5 :padding 0.3
      :background-color (rgba 0.32 0.13 0.12 1.0)
      (label (str "Edit rejected: " SEQ.song-edit-error)
        :key "arrangement-edit-error-label"
        :font-size 10.5 :color (rgba 1.0 0.78 0.72 1.0) :bg :transparent))))

(effect-buffer "*arrangement*"
  (v-stack :padding 0.0 :gap 0.0
    (if SEQ.song-exists
      (box :width 0 :height 0 :bg :transparent)
      (arrangement-empty-banner))
    (arrangement-error-banner)
    (box :width :fill
      (h-stack :width :fill :gap 0.6 :align :start
        (box :key "arrangement-scene-header-spacer"
          :width arrangement-header-width :height arrangement-scene-lane-height
          :bg :transparent)
        (arrangement-scene-lane)))
    (box :width :fill :height 0.1 :background-color :bg)
    (scroll :key "arrangement-track-scroll" :width :fill :flex 1
      (v-stack :width :fill :gap 0.0
        (each (seq-visible-track-indices) |i|
          (subtree :key (str "arr-track-" (nth SEQ.track-ids i))
            (arrangement-track-row i)))))))
