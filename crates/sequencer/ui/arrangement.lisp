;; ui/arrangement.lisp -- arrangement timeline view over song mode
;; (docs/arrangement-timeline-ui-spec.md). Renders to *arrangement* buffer;
;; loaded by ui/main.lisp. One timeline widget instance per lane: the scene
;; lane (the only instance with a header/time ruler) plus one headerless,
;; sidebar-less instance per visible track, all driven by the same shared
;; time-axis state so every lane stays in sync by construction (spec 5).

(module eseq.arrangement)

;; Migration aliases (module spec §10 step 2) for the names unconverted
;; callers still spell flat.  Four are lisp-side — transport.lisp calls
;; `set-arrangement-cursor` when it parks the playhead, and the three
;; capture fixtures under ui/capture-fixtures/ set the view window and the
;; scene ghost directly — the rest are entry points the Rust arrangement
;; tests drive by name (ui/tests.rs and ui/state_values/tests.rs).  Deleted
;; as each consumer converts.

;; Declared dependencies (spec §4). `import` is a *runtime* form — it runs
;; after this file is compiled — so it guarantees only that the target is
;; evaluated before our body executes. That is exactly what the lane bodies
;; need: they call `seq-visible-track-indices` and `sound-palette-panel`
;; while the buffer tree is being built. These two edges are the ordering
;; ui/main.lisp used to encode by listing those files above us.
(import eseq.track-collapse)
(import eseq.sound-palette)

(defstate view-start 0)
(defstate view-duration 64)
(defstate cursor-time 0)
;; The edit cursor is track-specific (Ableton-style): clicking a time in a
;; track lane parks the cursor on that track; -1 is the scene lane. Later
;; edits ("paste clip at cursor") get both a time and a target track.
(defstate cursor-track -1)
(defstate selection '())
(defstate %selection-rect nil)
;; Track-clip selection (spec 9.2 extension): ids are the first row-id of a
;; merged clip, valid only within the owning track's lane. Selecting in any
;; lane clears the other kind so Backspace is never ambiguous.
(def track-selection '())
(def selected-track -1)
;; Live-drag preview state for the SCENE lane (spec 9.1): live gesture
;; actions update this ghost only; the terminal :finish-* action lowers to
;; exactly one song primitive via seq-arrangement-action and clears it. A
;; primitive rejection reports on the status line and, because items derive
;; from the committed song, the view snaps back on its own.
(defstate ghost nil)
;; TRACK-lane drag state is deliberately NOT reactive
;; (UI_PERFORMANCE_TUNING.md: gesture-only variables must not be defstate).
;; The commit data lives in this plain global; the live preview rides the
;; per-lane SEQV float channels below, which repaint only the affected
;; timeline widgets instead of rerunning any effect per pointer tick.
(def track-drag nil)
;; Live region-drag preview (region spec 4.4): a transient
;; {track-a track-b start end} updated per :marquee-select frame. The
;; COMMITTED region lives in Rust (SEQ.song-region) so it survives view
;; switches; this is only what the pointer is currently sweeping, and the
;; ghost wins over the committed region while it is set. Plain global for
;; the same reason as track-drag; lanes render it through the
;; SEQV region channels.
(def region-ghost nil)

;; ── Per-lane widget channels (UI_PERFORMANCE_TUNING.md boundaries) ─────────
;; Each track lane binds these SEQV floats as timeline props. reactive-set
;; marks only the bound widget dirty, so scroll/zoom, drag ghosts, the click
;; selection, the sound binding and the region highlight all update without
;; rerunning the arrangement effect. Ghost kinds match the widget decoder:
;; 0 none, 1 move, 2 resize-start, 3 resize-end, 4 marquee rect,
;; 5 region-move.
(def channel (name i) (str "arr-" name "-" i))

(def %publish-view ()
  (do
    (reactive-set "SEQV" "arr-view-start" view-start)
    (reactive-set "SEQV" "arr-view-duration" view-duration)))

(def %clear-lane-ghost (i)
  (reactive-set "SEQV" (channel "ghost-kind" i) 0))

(def %publish-lane-ghost (i kind clip-id time)
  (do
    (reactive-set "SEQV" (channel "ghost-id" i) clip-id)
    (reactive-set "SEQV" (channel "ghost-time" i) time)
    (reactive-set "SEQV" (channel "ghost-kind" i) kind)))

(def %publish-lane-region-ghost (i kind time-a time-b delta)
  (do
    (reactive-set "SEQV" (channel "ghost-region-a" i) time-a)
    (reactive-set "SEQV" (channel "ghost-region-b" i) time-b)
    (reactive-set "SEQV" (channel "ghost-time" i) delta)
    (reactive-set "SEQV" (channel "ghost-kind" i) kind)))

;; Publish a region ghost rect to every visible lane: kind over the covered
;; span, cleared elsewhere. `kind` is 4 for a marquee sweep, 5 for a region
;; move (delta shifts the covered clips too).
(def %publish-region-ghost (region kind delta)
  (map
    (lambda (i)
      (if (and (not (= region nil))
            (>= i (min (get region :track-a) (get region :track-b)))
            (<= i (max (get region :track-a) (get region :track-b))))
        (%publish-lane-region-ghost i kind
          (get region :start) (get region :end) delta)
        (%clear-lane-ghost i)))
    (eseq.track-collapse/visible-track-indices)))

(def %clear-region-ghost ()
  (%publish-region-ghost nil 0 0))

(def %publish-selection (track clip-id)
  (map
    (lambda (i)
      (reactive-set "SEQV" (channel "selected-clip" i)
        (if (= i track) clip-id -1)))
    (eseq.track-collapse/visible-track-indices)))

(def %publish-cursor (time track)
  (map
    (lambda (i)
      (reactive-set "SEQV" (channel "cursor" i)
        (if (= i track) time -1)))
    (eseq.track-collapse/visible-track-indices)))

;; Seed every channel a lane can bind before the first layout, so unbound
;; slots do not read as "clip 0 selected" or "cursor at beat 0".
(map
  (lambda (i)
    (do
      (reactive-set "SEQV" (channel "ghost-kind" i) 0)
      (reactive-set "SEQV" (channel "ghost-id" i) -1)
      (reactive-set "SEQV" (channel "ghost-time" i) -1)
      (reactive-set "SEQV" (channel "ghost-region-a" i) 0)
      (reactive-set "SEQV" (channel "ghost-region-b" i) 0)
      (reactive-set "SEQV" (channel "selected-clip" i) -1)
      (reactive-set "SEQV" (channel "bound-clip" i) -1)
      (reactive-set "SEQV" (channel "region-a" i) 0)
      (reactive-set "SEQV" (channel "region-b" i) 0)
      (reactive-set "SEQV" (channel "region-on" i) 0)
      (reactive-set "SEQV" (channel "cursor" i) -1)))
  (range 0 128))
(reactive-set "SEQV" "arr-view-start" 0)
(reactive-set "SEQV" "arr-view-duration" 64)
(reactive-set "SEQV" "arr-content-length" 64)

(def %min-view-duration 4)
(def %max-view-duration 1024)
(def %view-padding 8)
(def %beats-per-bar 4)
(def %snap %beats-per-bar)
(def %header-height 2.6)
;; One cell (~20 px at the default scale) between ruler/loop chrome and the
;; scene lane. The transport-start triangle lives in this gutter.
(def %cursor-gutter-height 1)
(def %scene-lane-height 4.6)
(def %track-lane-height 2.85)
;; Vertical distance in CELLS between one track row's top and the next.
;; Track rows stack in a :gap 0 v-stack (see the buffer composition below), so
;; the pitch is exactly the lane height — no gap and no per-row chrome to add.
;; A cross-track region drag converts the widget's `row-delta` (cells) into a
;; count of tracks by dividing by this, so it MUST track the lane height and
;; the v-stack gap; metal_seq_arrangement_region_row_pitch_matches_layout
;; measures the rendered rows and fails if they ever drift apart.
(def track-row-pitch %track-lane-height)
;; Clip title-bar height in cells (region spec 3.1): the move/resize strip
;; above each clip's body. Fixed rather than proportional so clips read the
;; same at any lane height; tune by eye against the Ableton reference.
(def %clip-title-bar-height 0.9)
(def %clip-label-font-size 9)
(def %clip-label-color '(rgba 0.2 0.2 0.2 1))
(def %timeline-background-color :buffer-bg)
(def %cursor-color '(rgba 0.32 0.78 0.94 1))
;; Clip corner radius in CELLS (GarageBand-style rounded clips), so it scales
;; with the UI zoom like the lane heights above. 0 gives the square clips
;; every other timeline host draws.
(def %clip-corner-radius 0.22)
;; Requests one finer candidate from the timeline's zoom-adaptive grid. The
;; widget promotes crowded candidates to a readable aligned interval, and that
;; one resolved interval drives lines, labels, cursor placement, marquee, and
;; resize snapping. Every arrangement lane passes the same value or the ruler
;; and lanes would quantize differently; the piano roll uses the stock density.
(def %grid-density 2)
;; Fixed width for the composed seqv-track-header column so every lane's time
;; axis starts at the same x; the scene lane leads with a spacer of the same
;; width (spec 4.2: the per-track sidebar role is played by the header).
(def %header-width 27.4)

(def %event-num (event key fallback)
  (let ((value (get event key)))
    (if (= value nil) fallback value)))

;; ── Shared time axis (spec 5.1) ────────────────────────────────────────────

;; The furthest beat there is anything to look at. While a capture runs the
;; committed song end can still be BEHIND the recording (it is zero for a
;; whole-song capture), and clamping the view to it pinned the arrangement at
;; bar 1 with no way to scroll after the playhead. The record head is the
;; honest extent for as long as the recording is the content; it reads 0 when
;; no capture is running, so nothing else changes.
(def scroll-extent ()
  (max SEQ.song-end-beat
    (max (%pending-head)
      ;; Open-ended playback runs PAST the end (jam room, unified-transport
      ;; spec 4.2); the playhead is the honest extent out there. Reads 0
      ;; while stopped, so the parked view is unchanged.
      (if (= SEQ.song-mode "stopped") 0 SEQ.song-position-beats))))

(def %max-view-start (duration)
  (max 0 (- (+ (scroll-extent) %view-padding) duration)))

(def set-view-start (start duration)
  (do
    (set! view-start
      (max 0 (min (%max-view-start duration) start)))
    (%publish-view)))

(def %set-zoom (event)
  (let ((cur-duration view-duration)
        (factor (%event-num event :factor 1)))
    (let ((next-duration (max %min-view-duration
                           (min %max-view-duration
                             (/ view-duration factor)))))
      (let ((anchor (%event-num event :anchor-time view-start)))
        (let ((anchor-ratio
                (if (<= cur-duration 0)
                  0.5
                  (max 0 (min 1 (/ (- anchor view-start) cur-duration))))))
          (do
            (set! view-duration next-duration)
            ;; set-view-start publishes both view channels.
            (set-view-start
              (- anchor (* anchor-ratio next-duration))
              next-duration)))))))

(def set-cursor (time track)
  (if (= time nil)
    nil
    (do
      (set! cursor-time (max 0 time))
      (set! cursor-track track)
      ;; Track lanes render the cursor through their bound channel; only the
      ;; owning lane repaints.
      (%publish-cursor (max 0 time) track)
      ;; Mirror into Rust (region spec 5.3): Cmd-V is handled Rust-side and
      ;; pastes at the cursor, so the paste target cannot live only here.
      (seq-song-set-arr-cursor (max 0 time) track))))

;; The lane that owns the cursor shows it; every other lane passes nil.
(def lane-cursor-time (track)
  (if (= cursor-track track) cursor-time nil))

;; Shared view-action routing (spec 5.2): every lane funnels scroll/zoom
;; into the one shared time axis, regardless of which lane the pointer is
;; over. `:lane-scroll`/`:delta-lanes` are ignored per spec 4.2 — vertical
;; navigation belongs to the track scroll container.
(def %view-action (event)
  (match event.type
    :scroll-view
    ;; `event-start` is deliberately NOT spelled `view-start`: dropping the
    ;; `arrangement-` prefix put the module global in the same namespace as
    ;; this local, and the nil branch below means the *global* start.
    (let ((event-start (get event :view-start)))
      (set-view-start
        (if (= event-start nil)
          (+ view-start (%event-num event :delta-time 0))
          event-start)
        view-duration))
    :zoom-view
    (%set-zoom event)))

(def %view-action? (event)
  (or (= event.type :scroll-view)
      (= event.type :zoom-view)))

;; ── Items from the song read surface (spec 6/8) ────────────────────────────

(def scene-name (scene)
  (if (< scene (len SEQ.scene-names))
    (nth SEQ.scene-names scene)
    (str "Scene " (+ scene 1))))

(def %ghost-kind ()
  (if (= ghost nil) nil (get ghost :kind)))

;; Scene-lane gestures address a scene EVENT, whose identity is its start
;; beat (lane spec 12: every span IS a scene event, so there is no ambiguity
;; left to resolve).
(def %ghost-event? (kind beat)
  (and (= (%ghost-kind) kind)
    (= (get ghost :beat) beat)))

;; Ghost overlay for one scene span (spec 9.1 live preview): a move ghost
;; shifts its event's span; a resize ghost moves the boundary shared by the
;; resized span's end and the next event's start.
(def %scene-span-start (index start)
  (if (%ghost-event? :move start)
    (get ghost :start)
    (if (and (> index 0)
          (%ghost-event? :resize
            (get (nth SEQ.scene-spans (- index 1)) :start-beat)))
      (get ghost :end)
      start)))

(def %scene-span-end (start end)
  (if (%ghost-event? :move start)
    (+ (get ghost :start) (- end start))
    (if (%ghost-event? :resize start)
      (get ghost :end)
      end)))

;; Scene-lane items (lane spec 12): ONE span per scene EVENT, running to the
;; next event (SEQ.scene-spans derives the end). A clip edge on any track can
;; no longer split this lane, so every span is labeled — the old
;; dedup-the-repeated-label hack that made a fragmented lane readable is gone
;; with the fragmentation.
(def %scene-row-items ()
  (map
    (lambda (index)
      (let ((span (nth SEQ.scene-spans index)))
        (let ((start (get span :start-beat))
              (end (get span :end-beat)))
          (dict
            :id start
            :lane 0
            :start (%scene-span-start index start)
            :end (%scene-span-end start end)
            :label (scene-name (get span :scene))
            :kind :scene
            :selected (%row-selected? start)
            ;; The scene identity is the performer's while scene-latched:
            ;; dim the lane like any overridden track (rev 4).
            :color (if SEQ.song-scene-latched
                     (%override-dim (list 0.52 0.56 0.62))
                     (list 0.52 0.56 0.62))))))
    (range 0 (len SEQ.scene-spans))))

(def scene-items ()
  (append
    (if (= (%ghost-kind) :create)
      (append (%scene-row-items)
        (list (dict
                :id :ghost-create
                :lane 0
                :start (get ghost :start)
                :end (get ghost :end)
                :kind :scene
                :label (scene-name (or SEQ.current-pattern 0))
                :color (list 0.72 0.76 0.82))))
      (%scene-row-items))
    ;; Launches captured so far, filling in the scene lane as you perform
    ;; (realtime feedback spec 3.1). Defined below with the rest of the
    ;; provisional surface.
    (%pending-scene-items)))

;; Song end, with the content-length drag ghost applied so the end marker
;; previews in every lane while dragging (spec 9.3).
;; While a capture runs past the old song end the marker follows the record
;; head: that IS where the song will end once the take commits (the splice
;; extends `end_beat` to the Stop beat), so leaving it behind would draw the
;; recording as if it fell outside the song.
(def content-length ()
  (if (= (%ghost-kind) :end)
    (get ghost :length)
    (scroll-extent)))


;; The furthest beat any stored clip runs to (0 when the lanes are empty).
(def %last-clip-end ()
  (reduce |acc lane|
    (reduce |lane-acc clip| (max lane-acc (get clip :end-beat)) acc lane)
    0
    SEQ.song-lanes))

;; App::arr_set_end rejects an end AT or before the last scene change, and an
;; end before the last clip's end (spec 9.3). The clamp has to stay strictly
;; inside both boundaries: the widget clamps inclusively, so a min that equals
;; the last scene start hands the primitive the one value it refuses and the
;; drag comes back as an error banner instead of a clamp.
(def content-length-min ()
  (let ((count (len SEQ.scene-spans))
        (clip-floor (%last-clip-end)))
    (max 1 clip-floor
      (if (= count 0)
        0
        (+ (get (nth SEQ.scene-spans (- count 1)) :start-beat) 1)))))

(def %row-selected? (id)
  (> (len (filter (lambda (candidate) (= candidate id)) selection)) 0))

(def %track-color (i)
  (if (and (>= i 0) (< i (len SEQ.track-colors)))
    (nth SEQ.track-colors i)
    (list 0.34 0.48 0.98)))

;; Every audible span is a clip now (lane spec 6.2), so there is exactly one
;; clip tint: the track color lifted slightly. The lift is multiplicative, not
;; a lerp toward white: mixing in white desaturated the track color into
;; pastel, so arrangement clips no longer read as the same color the session
;; grid and piano roll use for the track.
(def clip-color (i)
  (let ((color (%track-color i)))
    (list
      (min 1 (* 1.15 (nth color 0)))
      (min 1 (* 1.15 (nth color 1)))
      (min 1 (* 1.15 (nth color 2))))))

;; The STORED clips of one track lane (lane spec 12): already merged, with
;; real ids — the view derives nothing, and every clip has a source. A stretch
;; of lane with no clip is silence and draws as empty.
(def track-clips (i)
  (if (< i (len SEQ.song-lanes))
    (nth SEQ.song-lanes i)
    '()))

(def %find-track-clip (i clip-id)
  (let ((matches (filter (lambda (clip) (= (get clip :clip-id) clip-id))
                   (track-clips i))))
    (if (> (len matches) 0) (nth matches 0) nil)))

;; The ids from a :select that name a REAL stored clip. Provisional recording
;; items (realtime feedback spec 3.4) carry no id, so selecting one selects
;; nothing — and every other gesture already resolves through
;; %find-track-clip, which cannot match them either.
(def %real-clip-ids (i ids)
  (filter (lambda (id) (not (= (%find-track-clip i id) nil))) ids))

;; Same guard for the scene lane, whose item ids are scene-event start beats.
(def %real-scene-ids (ids)
  (filter (lambda (id)
            (> (len (filter (lambda (span) (= (get span :start-beat) id))
                      SEQ.scene-spans))
              0))
    ids))

;; ── MIDI content flattening (spec 7.1) ─────────────────────────────────────
;; SEQ.song-lane-events carries, per track, raw (time transpose velocity)
;; events for every pool pattern the lane projection references. The view
;; flattens one pattern cycle into normalized (offset, value) dots — a
;; snapshot-time preview, deliberately impressionistic at arrangement zoom.

(def %dot-cap 256)

(def lane-pattern-events (track pattern-id)
  (let ((entries (if (< track (len SEQ.song-lane-events))
                   (nth SEQ.song-lane-events track)
                   '())))
    (let ((matches (filter (lambda (entry) (= (get entry :pattern-id) pattern-id))
                     entries)))
      (if (> (len matches) 0) (nth matches 0) nil))))

;; Aggregated take content (takes spec 11.3): one entry per take, event
;; times continuous across chunk boundaries, :num-steps = take length.
(def %lane-take-events (track take-id)
  (let ((entries (if (< track (len SEQ.song-lane-events))
                   (nth SEQ.song-lane-events track)
                   '())))
    (let ((matches (filter (lambda (entry) (= (get entry :take-id) take-id))
                     entries)))
      (if (> (len matches) 0) (nth matches 0) nil))))

;; Vertical placement: spread the pattern's own transpose range across the
;; item rect (single-pitch patterns sit mid-rect).
(def %dot-value (note lo hi)
  (if (= hi lo)
    0.5
    (+ 0.15 (* 0.7 (/ (- note lo) (- hi lo))))))

;; Note length in steps (4th event element, region spec 3.2). Older/short
;; event rows degrade to a point dot rather than erroring.
(def %event-duration (event)
  (if (< (len event) 4)
    0
    (let ((duration (nth event 3)))
      (if (= duration nil) 0 (max 0 duration)))))

;; Cap dots per item at %dot-cap, densest-first: events collapse
;; into 1/cap-wide time buckets (one dot per bucket), so dense clusters thin
;; out first while isolated events always survive. Events arrive step-ordered
;; from the read surface. Only events inside the step window
;; [from, from + span) are shown, normalized to the window — a re-anchored
;; take clip (nonzero offset-steps) renders the slice it actually plays.
(def %windowed-dots (entry from span)
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
        ;; Accumulate with cons + one final reverse: `append` copies the
        ;; whole accumulated list per event, which is quadratic at the
        ;; 256-dot cap and runs once per clip per rebuild.
        (reverse
          (get
            (reduce |acc event|
              (let ((offset (max 0 (min 0.999 (/ (- (nth event 0) from) span)))))
                (let ((bucket (floor (* offset %dot-cap))))
                  (if (= bucket (get acc :last))
                    acc
                    (dict :last bucket
                      :dots (cons (dict :offset offset
                                    :value (%dot-value (nth event 1) lo hi)
                                    ;; Real note length normalized to the
                                    ;; drawn window, clamped so a note never
                                    ;; paints past the item's end.
                                    :width (max 0
                                             (min (- 1 offset)
                                               (/ (%event-duration event)
                                                 span))))
                              (get acc :dots))))))
              (dict :last -1 :dots '())
              events)
            :dots))))))

(def pattern-dots (entry)
  (%windowed-dots entry 0 (max 1 (get entry :num-steps))))

;; True drawn window of a take clip (takes spec 11.3): a take is finite and
;; never loops, so the item ends at min(row-span end, start + remaining
;; take length at this clip's offset). A row extending past the take's end
;; is the silent tail — it renders as empty lane, matching what plays.
(def %take-clip-window (i clip)
  (let ((entry (%lane-take-events i (get clip :take-id))))
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

;; One repetition's length relative to the clip span (widget :cycle key).
;; This is deliberately allowed above 1: a clip shorter than its pattern
;; shows only the source window it actually plays instead of squeezing the
;; whole pattern into the clip.
(def clip-cycle (entry clip)
  (let ((span (- (get clip :end-beat) (get clip :start-beat)))
        (length-beats (get entry :length-beats)))
    (if (and (> length-beats 0) (> span 0))
      (/ length-beats span)
      1)))

(def %clip-phase (entry clip)
  (if (= (get clip :take-id) nil)
    (if (<= (get entry :num-steps) 0)
      0
      (/ (or (get clip :offset-steps) 0) (get entry :num-steps)))
    ;; Take dots are already windowed and normalized to the exact played
    ;; range below, so they start at phase zero and never repeat.
    0))

(def %clip-content (i clip)
  (let ((entry (if (= (get clip :take-id) nil)
                 (lane-pattern-events i (get clip :pattern-id))
                 (%lane-take-events i (get clip :take-id)))))
    (if (= entry nil)
      nil
      (let ((dots
              (if (= (get clip :take-id) nil)
                (pattern-dots entry)
                ;; Take dots render the exact step window the clip plays
                ;; (offset..offset+span), normalized to the drawn item —
                ;; so timeline dots line up with the take's actual notes.
                (let ((window (%take-clip-window i clip)))
                  (if (= window nil)
                    (pattern-dots entry)
                    (%windowed-dots entry
                      (get window :offset-steps)
                      (get window :window-steps)))))))
        (if (= (len dots) 0)
          nil
          (dict :dots dots
            ;; Takes never loop (spec 11.3): one item, no repeat tiling.
            :cycle (if (= (get clip :take-id) nil)
                     (clip-cycle entry clip)
                     1)
            :phase (%clip-phase entry clip)
            ;; The widget's live start-edge ghost wraps a pattern's phase and
            ;; clamps a take's at zero (takes spec 8).
            :wrap (= (get clip :take-id) nil)))))))

;; Track-lane items (lane spec 12): the stored clips, and nothing else. A gap
;; between clips produces NO item — the lane really is silent there.
(def %track-clip-label (clip)
  (if (= (get clip :take-id) nil)
    (str "Pattern " (get clip :pattern-id))
    ;; Take ids are zero-based internally; their default user-facing names
    ;; and every other take badge are one-based.
    (str "Take " (+ (get clip :take-id) 1))))

;; Sound identity dot (takes spec 17.6, amended): SEQ.song-clip-sounds joins
;; each clip to its Patch's palette color. The dot marks patch IDENTITY —
;; same color = same sound — on every clip with a resolvable sound,
;; unconditionally. Divergence-vs-effective-refs and single-patch-lane
;; suppression were both rejected: each made dots appear/disappear for
;; reasons invisible to the user (the playhead moving; a take splice
;; crossing the patch-count threshold). A colored patch passes (r g b); a
;; name-only patch passes true (gray fallback, 17.11).
(def clip-sound-dot (i clip-id)
  (if (>= i (len SEQ.song-clip-sounds))
    nil
    (let ((matches (filter (lambda (entry) (= (get entry :clip-id) clip-id))
                     (nth SEQ.song-clip-sounds i))))
      (if (= (len matches) 0)
        nil
        (let ((entry (nth matches 0)))
          (if (get entry :dot)
            (if (= (get entry :color) nil)
              true
              ;; The dot dims with its lane (rev 4 override dim): a bright
              ;; sound square on a darkened overridden clip reads as lit.
              (let ((color (list (get entry :color-r)
                                 (get entry :color-g)
                                 (get entry :color-b))))
                (if (%lane-latched? i)
                  (%override-dim color)
                  color)))
            nil))))))

;; ── Per-lane override dim (unified-transport rev 4, Ableton-style) ────────
;; A latched lane's committed clips darken: the arrangement is NOT what you
;; hear on that lane until Back to Arrangement. Per-track, not global —
;; un-latched lanes keep playing (and showing) the arrangement at full color.
(def %lane-latched? (i)
  (= true (nth SEQ.song-track-latched i)))

(def %override-dim (color)
  (list (* 0.35 (nth color 0)) (* 0.35 (nth color 1)) (* 0.35 (nth color 2))))

(def %lane-clip-color (i)
  (if (%lane-latched? i)
    (%override-dim (clip-color i))
    (clip-color i)))

(def %track-clip-items (i)
  (map
    (lambda (clip)
      (let ((clip clip))
        (dict
          :id (get clip :clip-id)
          :lane 0
          :start (get clip :start-beat)
          ;; Take items clamp to the take's true remaining length (takes
          ;; spec 11.3) — the clip may extend past the take's end, but that
          ;; tail is silent and draws as empty lane.
          :end (if (= (get clip :take-id) nil)
                 (get clip :end-beat)
                 (let ((window (%take-clip-window i clip)))
                   (if (= window nil)
                     (get clip :end-beat)
                     (get window :end-beat))))
          :kind :midi
          :label (%track-clip-label clip)
          :content (%clip-content i clip)
          :color (%lane-clip-color i)
          :sound-dot (clip-sound-dot i (get clip :clip-id)))))
    (track-clips i)))

;; ── Provisional capture content (realtime feedback spec 3) ────────────────
;; SEQ.song-pending is the recording IN FLIGHT: the pending take lanes and
;; the launches captured so far, published only while arrangement capture is
;; running and cleared on stop, cancel and failure alike. Its items are drawn
;; and never edited — provisional content has no clip id yet, so it appears
;; nowhere in track-clips and no gesture can resolve one. That is
;; the same items-for-drawing vs clips-for-editing split the ghost preview
;; uses (spec 3.4).

;; Provisional items wear the SAME tint and labels as the committed clips
;; they are about to become: a capture preview whose whole job is to show
;; where the music is landing should not be a differently-coloured stand-in
;; for it. What marks them as in-flight is that they are growing under the
;; playhead, not a paint job.

(def %pending-lanes ()
  (if (= SEQ.song-pending nil) '() (get SEQ.song-pending :lanes)))

(def %pending-scene-events ()
  (if (= SEQ.song-pending nil) '() (get SEQ.song-pending :scene-events)))

;; The record head, which every provisional span grows toward.
(def %pending-head ()
  (if (= SEQ.song-pending nil) 0 (get SEQ.song-pending :head-beat)))

;; Provisional lanes carry RAW events in the same shape SEQ.song-lane-events
;; uses, so they go through the committed clips' dot pipeline unchanged.
;;
;; The window is the item's OWN span, not the recorded content's length: the
;; item grows to the record head while the notes stay put, so normalizing over
;; the content would stretch the same dots across an ever-wider rect (they
;; would snap back on every new note and creep apart again in between). A take
;; never loops, so this is one cycle at phase 0 and the span past the last
;; note is honestly empty.
(def %pending-span-steps (lane)
  (let ((num-steps (max 1 (get lane :num-steps)))
        (length-beats (get lane :length-beats)))
    (if (<= length-beats 0)
      num-steps
      (let ((step-beats (/ length-beats num-steps)))
        (max 1
          (/ (- (get lane :end-beat) (get lane :start-beat)) step-beats))))))

(def %pending-content (lane)
  (let ((dots (%windowed-dots lane 0
                (%pending-span-steps lane))))
    (if (= (len dots) 0)
      nil
      (dict :dots dots :cycle 1 :phase 0))))

;; No :id — the one thing that would make a provisional item addressable.
;; The take has no TakeId until the stop-commit registers it, so the label
;; cannot name a number yet.
(def %pending-track-items (i)
  (map
    (lambda (lane)
      (dict
        :lane 0
        :start (get lane :start-beat)
        :end (get lane :end-beat)
        :kind :midi
        :label "Take"
        :content (%pending-content lane)
        :color (clip-color i)))
    (filter (lambda (lane) (= (get lane :track) i))
      (%pending-lanes))))

;; ── Provisional launch clips ──────────────────────────────────────────────
;; What each captured launch put on a TRACK lane: a clip-launched pattern, or
;; the scene's cell pattern on every lane a captured scene change claimed.
;; These are the clips the stop-commit will write, so they preview the same
;; way — a looping pattern tiled over its span.

(def %pending-track-events (i)
  (filter (lambda (event) (= (get event :track) i))
    (if (= SEQ.song-pending nil) '() (get SEQ.song-pending :track-events))))

(def %pending-launch-content (event span)
  (let ((dots (pattern-dots event))
        (length-beats (get event :length-beats)))
    (if (= (len dots) 0)
      nil
      (dict :dots dots
        :cycle (if (and (> length-beats 0) (> span 0))
                 (/ length-beats span)
                 1)
        :phase 0))))

;; The span runs to the next launch on this lane, or to the record head while
;; it is still the last one.
(def %pending-launch-item (i index events)
  (let ((event (nth events index)))
    (let ((start (get event :start-beat))
          (end (if (< (+ index 1) (len events))
                 (get (nth events (+ index 1)) :start-beat)
                 (%pending-head))))
      (if (<= end start)
        nil
        (dict
          :lane 0
          :start start
          :end end
          :kind :midi
          :label (str "Pattern " (get event :pattern-id))
          :content (%pending-launch-content event (- end start))
          :color (clip-color i))))))

(def %pending-launch-items (i)
  (let ((events (%pending-track-events i)))
    (filter (lambda (item) (not (= item nil)))
      (map (lambda (index) (%pending-launch-item i index events))
        (range 0 (len events))))))

;; A captured launch's provisional span runs to the next captured launch, or
;; to the record head while it is still the last one. A launch the head has
;; not passed yet has nothing to draw.
(def %pending-scene-item (index events)
  (let ((event (nth events index)))
    (let ((start (get event :start-beat))
          (end (if (< (+ index 1) (len events))
                 (get (nth events (+ index 1)) :start-beat)
                 (%pending-head))))
      (if (<= end start)
        nil
        (dict
          :lane 0
          :start start
          :end end
          :kind :scene
          :label (scene-name (get event :scene))
          :color (list 0.52 0.56 0.62))))))

(def %pending-scene-items ()
  (let ((events (%pending-scene-events)))
    (filter (lambda (item) (not (= item nil)))
      (map (lambda (index) (%pending-scene-item index events))
        (range 0 (len events))))))

;; Committed clips first, provisional content on top (spec 3.4). Recorded
;; takes last of all: the stop-commit paints them OVER whatever the launches
;; put on the lane, so the preview stacks the same way.
(def track-items (i)
  (append (%track-clip-items i)
    (append (%pending-launch-items i)
      (%pending-track-items i))))

;; Selection prop for one track lane: only the owning track shows its ids.
;; The bound clip (takes spec 16.6) is Rust-side persistent timeline state,
;; so it carries the highlight after a view switch, when this buffer's own
;; selection defstate has been reset.
;; Selection surface for one lane, read back from the published channels
;; (the widgets render straight from the bound floats; this helper exists for
;; handlers and tests). Only the owning track shows its click selection; the
;; bound clip (takes spec 16.6) is Rust-side persistent timeline state, so it
;; carries the highlight after a view switch.
(def lane-selection (i)
  (let ((selected (reactive-get "SEQV" (channel "selected-clip" i)))
        (bound (reactive-get "SEQV" (channel "bound-clip" i))))
    (append
      (if (and (not (= selected nil)) (>= selected 0)) (list selected) '())
      (if (and (not (= bound nil)) (>= bound 0) (not (= bound selected)))
        (list bound)
        '()))))

;; ── Region selection (region spec 4) ───────────────────────────────────────

;; Drag capture is per widget instance, so a cross-track marquee never reaches
;; the lanes it sweeps over: the originating lane reports the pointer's
;; VERTICAL TRAVEL (`:row-delta`, cells, signed and unclamped) and the host
;; reconstructs the track span from it (spec 4.2).

;; Position of a model track index within the currently visible tracks, or -1
;; when it is collapsed away. Collapsed tracks are simply absent, so a region
;; drag always spans visible tracks and never lands on a hidden one.
(def visible-ordinal (visible track)
  (reduce |acc o| (if (= (nth visible o) track) o acc)
    -1
    (range 0 (len visible))))

;; Ordinal -> model track index, clamped to the visible range so a drag that
;; runs off the top or bottom of the arrangement selects out to the edge.
(def %track-at-ordinal (visible ordinal)
  (nth visible (max 0 (min (- (len visible) 1) ordinal))))

;; The far end of a region drag that started in track `i`: convert the
;; vertical travel to a count of track rows, step that many places through the
;; visible order, and map back to a model index.
(def region-other-track (i row-delta)
  (let ((visible (eseq.track-collapse/visible-track-indices)))
    (if (= (len visible) 0)
      i
      (let ((ordinal (visible-ordinal visible i)))
        (if (< ordinal 0)
          i
          (%track-at-ordinal visible
            (+ ordinal (round (/ (or row-delta 0) track-row-pitch)))))))))

(def %region-from-event (i event)
  (dict
    :track-a i
    :track-b (region-other-track i (get event :row-delta))
    :start (get event :time-a)
    :end (get event :time-b)))

;; Scene-lane marquees select the time span across EVERY visible track
;; (spec 4.2): the scene lane spans the whole arrangement, so its vertical
;; travel carries no track information.
(def %region-all-tracks (event)
  (let ((visible (eseq.track-collapse/visible-track-indices)))
    (if (= (len visible) 0)
      nil
      (dict
        :track-a (nth visible 0)
        :track-b (nth visible (- (len visible) 1))
        :start (get event :time-a)
        :end (get event :time-b)
        :scene-lane true))))

;; The fifth argument is the SCENE-LANE bit (lane spec 8): a marquee swept in
;; the scene lane copies/pastes/deletes the scene EVENTS inside it as well as
;; the clips, which a track-lane marquee covering the same rectangle never
;; does.
(def %region-commit (region)
  (do
    (set! region-ghost nil)
    (%clear-region-ghost)
    (if (= region nil)
      (seq-song-clear-region)
      (seq-song-set-region
        (get region :track-a) (get region :track-b)
        (get region :start) (get region :end)
        (= (get region :scene-lane) true)))))

;; Clicking a clip's title bar is BOTH gestures: it binds the track's sound to
;; the clip AND selects the clip's span as a one-track region, so the body
;; lights up exactly like a swept region does and copy/delete have a target
;; (Ableton). A free marquee, which names no single clip, still releases the
;; binding — and so does a click on a BACKDROP ghost (negative id, lane spec
;; 12): a gap is not a clip, so there is nothing to bind or select.
(def %select-clip (i clip-id)
  (let ((clip (%find-track-clip i clip-id)))
    (if (= clip nil)
      (do (seq-song-deselect-clip) (seq-song-clear-region))
      (seq-song-select-clip i clip-id
        (get clip :start-beat) (get clip :end-beat)))))

;; Any other selection gesture drops the region: the two are mutually
;; exclusive (spec 4.1). Clip selection additionally clears it Rust-side, so
;; this is really about the in-flight ghost and the scene-row path.
(def %region-clear ()
  (do
    (set! region-ghost nil)
    (%clear-region-ghost)
    (seq-song-clear-region)))

;; The scene lane draws its own marquee echo while a drag is live, then keeps
;; the committed rect lit for a SCENE-LANE region (region spec 4.4) — the
;; visible sign that the region carries the scene events, not just the clips.
(def %scene-region-rect ()
  (if (not (= %selection-rect nil))
    %selection-rect
    (if (and (not (= SEQ.song-region nil))
          (= (nth SEQ.song-region 4) true))
      (dict :time-a (nth SEQ.song-region 2) :time-b (nth SEQ.song-region 3)
        :lane-a 0 :lane-b 0)
      nil)))

;; ── Action handlers ────────────────────────────────────────────────────────

;; Lower one finished gesture to song primitives through the Rust translator
;; (spec 9.1): exactly one primitive per gesture, validation/undo/rejection
;; reporting owned by the song host commands.
(def %edit-finish (payload)
  (do
    (set! ghost nil)
    (seq-arrangement-action payload)))

;; Scene lane (spec 9.2: the only editable lane). View actions route to the
;; shared axis; live edit actions update the ghost preview only; terminal
;; actions commit through %edit-finish.
(def scene-action (event)
  (if (%view-action? event)
    (%view-action event)
    (match event.type
      :select
      (do
        (set! %selection-rect nil)
        (set! track-selection '())
        (set! selected-track -1)
        (%publish-selection -1 -1)
        ;; Provisional captured launches carry no id (realtime feedback spec
        ;; 3.4), so only real scene events can enter the selection.
        (set! selection (%real-scene-ids (get event :ids)))
        ;; A scene span covers every track and names no single clip, so it
        ;; releases the sound binding (takes spec 16.6 cause 2) — and, for the
        ;; same reason, drops the region (region spec 4.1).
        (%region-clear)
        (seq-song-deselect-clip)
        (set-cursor (get event :time) -1))
      :clear-selection
      (do
        (set! %selection-rect nil)
        (set! selection '())
        (%region-clear)
        (seq-song-deselect-clip)
        (set-cursor (get event :time) -1))
      :set-cursor
      (set-cursor (get event :time) -1)
      ;; A scene-lane marquee selects the time span across ALL visible tracks
      ;; (region spec 4.2): the lane has no per-track geometry to sweep. The
      ;; scene lane keeps its own dashed marquee echo while the drag is live.
      :marquee-select
      (do
        (set! %selection-rect event)
        (set! region-ghost (%region-all-tracks event))
        (%publish-region-ghost region-ghost 4 0))
      :finish-marquee-select
      (do
        (set! %selection-rect nil)
        (%region-commit (%region-all-tracks event)))
      ;; Live drags: ghost only, never a primitive (spec 9.1). A scene-lane
      ;; item's id IS its scene event's start beat (lane spec 12).
      :move-items-absolute
      (set! ghost
        (dict :kind :move
          :beat (get event :anchor-id)
          :start (get event :start)))
      ;; The scene lane is contiguous, so dragging an event's START edge IS
      ;; moving that event: the boundary it owns is the same one the previous
      ;; span's end handle drags. Lower it to the move ghost rather than the
      ;; resize one, or the drag would write the event's start into its end.
      :resize-item-absolute
      (set! ghost
        (if (= (get event :edge) :start)
          (dict :kind :move
            :beat (get event :id)
            :start (get event :time))
          (dict :kind :resize
            :beat (get event :id)
            :end (get event :time))))
      :create-item
      (set! ghost
        (dict :kind :create
          :start (get event :start)
          :end (get event :end)))
      :resize-content-length
      (set! ghost
        (dict :kind :end :length (get event :length)))
      ;; Terminal actions: one primitive each, from the ghost's final values
      ;; (the widget's finish actions carry ids, not times).
      :finish-move-items
      (if (= (%ghost-kind) :move)
        (%edit-finish
          (dict :type :finish-move-items
            :from-beat (get ghost :beat)
            :start (get ghost :start)))
        (set! ghost nil))
      :finish-resize-items
      (if (= (%ghost-kind) :resize)
        (%edit-finish
          (dict :type :finish-resize-items
            :from-beat (get ghost :beat)
            :end (get ghost :end)))
        ;; Start-edge drag: the ghost is a move (see above), so it commits as
        ;; one.
        (if (= (%ghost-kind) :move)
          (%edit-finish
            (dict :type :finish-move-items
              :from-beat (get ghost :beat)
              :start (get ghost :start)))
          (set! ghost nil)))
      :finish-create-item
      (%edit-finish
        (dict :type :finish-create-item
          :start (get event :start)
          :end (get event :end)
          :scene (or SEQ.current-pattern 0)))
      :finish-resize-content-length
      (%edit-finish
        (dict :type :finish-resize-content-length
          :length (get event :length)))
      :delete-items
      (do
        ;; The ids ARE the scene events' beats (lane spec 12). Removing a
        ;; scene change can never touch a clip, but the selection/region were
        ;; measured against the old lane — drop them with it.
        (set! selection '())
        (%region-clear)
        ;; A provisional captured launch has no id, so a delete aimed at one
        ;; resolves to nothing and never reaches a primitive.
        (let ((ids (%real-scene-ids (get event :ids))))
          (if (= (len ids) 0)
            nil
            (%edit-finish (dict :type :delete-items :ids ids)))))
      ;; A scene-lane marquee selects the span across every track, so its
      ;; clipboard keys drive the same region commands (region spec 5.3).
      :copy-items
      (seq-song-region-copy)
      :paste-items
      (seq-song-region-paste (get event :time)))))

;; Track-lane clip editing (Ableton-style): select a clip, Backspace deletes
;; it, dragging its end edge resizes it — fewer loops shortens it, more eats
;; into whatever follows. Every gesture lowers to ONE clip primitive
;; (arrangement-lane-model-spec 8/12: a clip is a first-class object, so
;; resize is a resize, not "move the next row").
;;
;; The view speaks STORED clip ids (lane spec 12), so each gesture names the
;; object it edits — no span-to-clip resolution anywhere.
(def %clip-edit (i clip payload)
  (%edit-finish
    (merge payload
      :track i
      :clip-id (get clip :clip-id))))

(def %track-resize-end-finish (i clip time)
  (let ((new-end (max 0 (min SEQ.song-end-beat time))))
    (if (= new-end (get clip :end-beat))
      nil
      (if (<= new-end (get clip :start-beat))
        ;; Dragged past its own start: the clip is gone.
        (%clip-edit i clip (dict :type :clip-delete))
        (%clip-edit i clip
          (dict :type :clip-resize
            :start (get clip :start-beat)
            :end new-end))))))

;; Start-edge resize is NOT a move: the span's left edge and the clip's phase
;; anchor move together, so the surviving music stays where it was (lane spec
;; 8 / takes spec 8 — `arrangement-clip-resize` re-stamps `offset-steps` by
;; the split rule, in both directions). Same primitive as the end edge, only
;; the other coordinate changes.
(def %track-resize-start-finish (i clip time)
  (let ((new-start (max 0 time)))
    (if (= new-start (get clip :start-beat))
      nil
      (if (>= new-start (get clip :end-beat))
        ;; Dragged past its own end: the clip is gone.
        (%clip-edit i clip (dict :type :clip-delete))
        (%clip-edit i clip
          (dict :type :clip-resize
            :start new-start
            :end (get clip :end-beat)))))))

(def %track-resize-finish (i)
  (let ((clip-id (get track-drag :clip-id))
        (edge (get track-drag :edge))
        (time (get track-drag :time)))
    (let ((clip (%find-track-clip i clip-id)))
      (do
        (set! track-drag nil)
        (%clear-lane-ghost i)
        (if (= clip nil)
          nil
          (if (= edge :start)
            (%track-resize-start-finish i clip time)
            (%track-resize-end-finish i clip time)))))))

;; ── Clip / region move (region spec 6) ─────────────────────────────────────

;; Does the committed region cover this clip?
(def %clip-in-region? (i clip)
  (if (or (= SEQ.song-region nil) (= clip nil))
    false
    (and (>= i (nth SEQ.song-region 0))
      (<= i (nth SEQ.song-region 1))
      (> (get clip :end-beat) (nth SEQ.song-region 2))
      (< (get clip :start-beat) (nth SEQ.song-region 3)))))

;; ...and does it reach BEYOND it — another track, or more time?
;;
;; This is what separates the two title-bar gestures. Selecting a clip makes
;; its own span a one-clip region (spec 4.1), and the widget selects before it
;; drags, so "the region covers this clip" is true of every single-clip drag:
;; testing only that would turn every move into a region move, previewed as a
;; bare rectangle instead of the clip itself. A rectangle that is exactly the
;; dragged clip IS the clip, so it moves as one.
(def %region-beyond-clip? (i clip)
  (if (or (= SEQ.song-region nil) (= clip nil))
    false
    (or (< (nth SEQ.song-region 0) i)
      (> (nth SEQ.song-region 1) i)
      (< (nth SEQ.song-region 2) (get clip :start-beat))
      (> (nth SEQ.song-region 3) (get clip :end-beat)))))

(def %clip-drags-region? (i clip)
  (and (%clip-in-region? i clip)
    (%region-beyond-clip? i clip)))

;; Live title-bar drag: ghost only, never a primitive (spec 9.1). Vertical
;; travel is ignored — cross-track moves are invalid for the same per-track
;; pattern-pool reason as cross-track paste (region spec 8), so the widget's
;; :lane is dropped here.
(def %track-move-ghost (i event)
  (let ((clip (%find-track-clip i (get event :anchor-id))))
    (if (= clip nil)
      (set! track-drag nil)
      (if (%clip-drags-region? i clip)
        ;; Region move: the widget shifts every covered clip and the region
        ;; rect by the published delta, clamped so the rectangle can never
        ;; run before beat 0 (the primitive rejects that rather than
        ;; truncating the leading clips).
        (let ((delta (max (- 0 (nth SEQ.song-region 2))
                       (- (get event :start) (get clip :start-beat)))))
          (do
            (set! track-drag
              (dict :kind :region-move :track i :delta delta
                :track-a (nth SEQ.song-region 0)
                :track-b (nth SEQ.song-region 1)
                :start (nth SEQ.song-region 2)
                :end (nth SEQ.song-region 3)))
            (set! region-ghost
              (dict :track-a (nth SEQ.song-region 0)
                :track-b (nth SEQ.song-region 1)
                :start (+ (nth SEQ.song-region 2) delta)
                :end (+ (nth SEQ.song-region 3) delta)
                :scene-lane (nth SEQ.song-region 4)))
            (%publish-region-ghost
              (dict :track-a (nth SEQ.song-region 0)
                :track-b (nth SEQ.song-region 1)
                :start (nth SEQ.song-region 2)
                :end (nth SEQ.song-region 3))
              5 delta)))
        (do
          (set! track-drag
            (dict :kind :track-move :track i
              :clip-id (get clip :clip-id)
              :start (max 0 (get event :start))))
          (%publish-lane-ghost i 1
            (get clip :clip-id) (max 0 (get event :start))))))))

;; Release: one primitive from the ghost's final values, and never a stale
;; ghost on any path (including the guard failures).
;; Test/introspection surface: the region rect one lane currently renders,
;; reconstructed from its published channels the same way the widget does
;; (the in-flight ghost wins over the committed region).
(def lane-region-rect (i)
  (let ((kind (reactive-get "SEQV" (channel "ghost-kind" i))))
    (if (and (not (= kind nil)) (>= kind 4))
      (let ((delta (if (>= kind 5)
                     (reactive-get "SEQV" (channel "ghost-time" i))
                     0)))
        (dict
          :time-a (+ (reactive-get "SEQV" (channel "ghost-region-a" i)) delta)
          :time-b (+ (reactive-get "SEQV" (channel "ghost-region-b" i)) delta)))
      (if (= (reactive-get "SEQV" (channel "region-on" i)) 1)
        (dict
          :time-a (reactive-get "SEQV" (channel "region-a" i))
          :time-b (reactive-get "SEQV" (channel "region-b" i)))
        nil))))

(def track-drag-kind ()
  (if (= track-drag nil) nil (get track-drag :kind)))

(def %track-move-finish (i)
  (let ((kind (track-drag-kind))
        (track (get track-drag :track)))
    (if (and (= kind :region-move) (= track i))
      (let ((delta (get track-drag :delta)))
        (do
          (set! region-ghost nil)
          (set! track-drag nil)
          (%clear-region-ghost)
          (if (= delta 0)
            nil
            (%edit-finish (dict :type :region-move :delta delta)))))
      (if (and (= kind :track-move) (= track i))
        (let ((clip (%find-track-clip i (get track-drag :clip-id)))
              (start (get track-drag :start)))
          (do
            (set! track-drag nil)
            (%clear-lane-ghost i)
            (if (or (= clip nil) (= start (get clip :start-beat)))
              nil
              (%clip-edit i clip
                (dict :type :clip-move :start start)))))
        (do
          (set! region-ghost nil)
          (set! track-drag nil)
          (%clear-region-ghost)
          (%clear-lane-ghost i))))))

(def %track-delete (i ids)
  (do
    (set! track-selection '())
    (%publish-selection -1 -1)
    ;; Deleting the clip deletes what the selection pointed at: the region
    ;; goes with it, or the highlight would stay lit over empty lane (region
    ;; spec 4.1 — a clip selection IS its region).
    (%region-clear)
    (seq-song-deselect-clip)
    (map
      (lambda (clip-id)
        (let ((clip (%find-track-clip i clip-id)))
          (if (= clip nil)
            nil
            (%clip-edit i clip (dict :type :clip-delete)))))
      ids)))

;; The single-click select body, shared with :double-click-item (clip-edit
;; target spec 4.2: a double-click is "bind + ensure the editor is open", so
;; it must run the exact same bind/region/track-select path first).
(def %track-select (i event)
  ;; Only ids that name a stored clip survive: a provisional recording
  ;; item has none, so clicking one selects nothing (realtime feedback
  ;; spec 3.4).
  (let ((ids (%real-clip-ids i (get event :ids))))
    (let ((clip (if (= (len ids) 0)
                  nil
                  (%find-track-clip i (nth ids 0)))))
      (do
        (eseq.sequencer/select-track-for-edit i)
        (set! selection '())
        (set! %selection-rect nil)
        ;; A clip and a region are mutually exclusive (region spec 4.1);
        ;; the Rust side drops the region too, this clears the in-flight
        ;; ghost.
        (set! region-ghost nil)
        (%clear-region-ghost)
        (set! selected-track i)
        (set! track-selection ids)
        (%publish-selection i
          (if (= (len ids) 0) -1 (nth ids 0)))
        ;; Selecting a clip is the explicit sound-binding gesture (takes
        ;; spec 16.2/16.6): it re-binds this track's device panel, monitor
        ;; sound and take punch-in template. The binding lives in Rust so
        ;; it survives view switches and transport.
        (if (= clip nil)
          (do (seq-song-deselect-clip) (seq-song-clear-region))
          (%select-clip i (nth ids 0)))
        ;; A clip click also parks the transport start at the clip's
        ;; beginning, independent of where in its title bar was hit.
        (set-cursor
          (if (= clip nil) (get event :time) (get clip :start-beat))
          i)
        clip))))

(def %track-clear-selection (i event)
  (do
    (eseq.sequencer/select-track-for-edit i)
    (set! track-selection '())
    (%publish-selection -1 -1)
    (%region-clear)
    (seq-song-deselect-clip)
    (set-cursor (get event :time) i)))

(def track-action (i event)
  (if (%view-action? event)
    (%view-action event)
    (match event.type
      :select
      (%track-select i event)
      ;; Title-bar double-click (clip-edit-target spec 4): bind exactly like a
      ;; single click, then open the piano roll on the bound source. The
      ;; piano-roll targeting comes from the binding itself, so the open call
      ;; stays track-shaped.
      :double-click-item
      (let ((clip (%track-select i event)))
        (if (= clip nil)
          nil
          (eseq.seq-panels/seq-open-arrangement-piano-roll-bottom-for-track i)))
      ;; Background double-click: follow the same empty-space selection path
      ;; as the first click, then atomically mint a real silent take + clip.
      ;; The host command selects the new clip when it commits, so the
      ;; track-shaped open call resolves onto that take on the next UI tick.
      :finish-create-item
      (let ((start (get event :start))
            (end (get event :end)))
        (do
          (%track-select i (dict :ids '() :time start))
          (seq-arrangement-empty-take-create i start end)
          (eseq.seq-panels/seq-open-arrangement-piano-roll-bottom-for-track i)))
      ;; Degenerate zero-movement release, or a click on empty lane space:
      ;; in FX mode a clip BODY remains a region/cursor surface. In the
      ;; arrangement piano-roll mode the widget includes the body clip id,
      ;; and the same press retargets the editor instead. A true background
      ;; click carries no real id, so it clears focus and the piano roll shows
      ;; "No clip selected".
      :clear-selection
      (let ((ids (%real-clip-ids i (or (get event :ids) '()))))
        (if (and (eseq.piano-roll/piano-roll-arrangement-mode?)
              (= eseq.seq-step-tabs/lower-panel-buffer "*piano-roll*")
              (> (len ids) 0))
          (%track-select i event)
          (%track-clear-selection i event)))
      :set-cursor
      (do
        (eseq.sequencer/select-track-for-edit i)
        (set-cursor (get event :time) i))
      ;; Cross-track region sweep (region spec 4.2/4.4): live frames update
      ;; the ghost only; the release commits the Rust-owned region.
      :marquee-select
      (do
        (set! region-ghost (%region-from-event i event))
        (%publish-region-ghost region-ghost 4 0))
      :finish-marquee-select
      (%region-commit (%region-from-event i event))
      ;; Live edge drag: ghost preview only (spec 9.1). Either edge; the
      ;; ghost carries which one so the preview and the commit agree.
      :resize-item-absolute
      (do
        (set! track-drag
          (dict :kind :track-resize :track i
            :clip-id (get event :id)
            :edge (if (= (get event :edge) :start) :start :end)
            :time (get event :time)))
        (%publish-lane-ghost i
          (if (= (get event :edge) :start) 2 3)
          (get event :id) (get event :time)))
      :finish-resize-items
      (if (and (= (track-drag-kind) :track-resize)
            (= (get track-drag :track) i))
        (%track-resize-finish i)
        (do
          (set! track-drag nil)
          (%clear-lane-ghost i)))
      :delete-items
      (%track-delete i (get event :ids))
      ;; Clipboard (region spec 5.3): the widget emits these when a lane has
      ;; keyboard focus; the ui/input.rs seam emits the same commands when it
      ;; does not. Both converge on the region primitives, which read the
      ;; Rust-owned region — a clip click already made that a one-clip region.
      :copy-items
      (seq-song-region-copy)
      :paste-items
      (seq-song-region-paste (get event :time))
      ;; Title-bar drag (region spec 6): one rigid clip move, or a move of the
      ;; whole region when the dragged clip lies inside it.
      :move-items-absolute
      (%track-move-ghost i event)
      :finish-move-items
      (%track-move-finish i))))

;; ── Scene drag-and-drop (Ableton-style, replaces the draw tool) ────────────
;; The transport scene pills are drag sources (:drag-type "transport-scene");
;; dropping one on any lane inserts a row launching that scene at the drop
;; beat, snapped to the bar grid. The drop event's :sx is the normalized
;; (-1..1) position within the lane, which maps straight onto the shared view
;; span because lanes have no sidebar.
(def %drop-time (event)
  (let ((ratio (max 0 (min 1 (/ (+ (%event-num event :sx -1) 1) 2)))))
    (let ((time (+ view-start (* ratio view-duration))))
      (max 0 (* %snap (floor (/ time %snap)))))))

(def drop-scene (event)
  (let ((scene (get (get event :payload) :scene)))
    (if (= scene nil)
      nil
      (let ((start (%drop-time event)))
        (%edit-finish
          (dict :type :finish-create-item
            :start start
            ;; Dropping at/past the song end extends it by four bars from
            ;; the drop point (the translator only uses :end in that case).
            :end (+ start (* %beats-per-bar 4))
            :scene scene))))))

;; ── Lane instances (spec 4.1/4.2) ──────────────────────────────────────────

;; The scene lane is the ONLY instance with a header/time ruler; it doubles
;; as the arrangement's bar/beat ruler.
;; Every lane is a flex child (:width 0 :flex 1) of its row: it absorbs
;; exactly the width remaining after the fixed header column, so no row can
;; overflow the pane and drag the buffer viewport into horizontal scrolling.
(def %scene-lane ()
  (timeline
    :key "scene-lane"
    :width 0 :flex 1
    :height %scene-lane-height
    :focusable true
    :sidebar-width 0
    :header-height %header-height
    :header-bottom-gutter %cursor-gutter-height
    :time-ruler (dict :mode :bars-beats :beats-per-bar %beats-per-bar)
    :grid-density %grid-density
    :background-color %timeline-background-color
    :title-bar-height %clip-title-bar-height
    :item-label-font-size %clip-label-font-size
    :item-label-color %clip-label-color
    :item-corner-radius %clip-corner-radius
    :item-color (list 0.52 0.56 0.62)
    :loop-color (list 0.92 0.72 0.25)
    :playhead-time (bind-seq "song-position-beats")
    ;; The ruler always owns the transport-start triangle, while the
    ;; track-specific cursor line remains in the lane the user clicked.
    :cursor-time cursor-time
    :cursor-marker-visible true
    :cursor-marker-scale 1.6
    :cursor-marker-width-scale 1.5
    :cursor-marker-height-scale 0.7
    :cursor-line-visible false
    :cursor-color %cursor-color
    :drop-types (list "transport-scene")
    :on-drop (lambda (event) (drop-scene event))
    :items (scene-items)
    :selection selection
    :selection-rect (%scene-region-rect)
    :view-start (bind "SEQV" "arr-view-start")
    :view-duration (bind "SEQV" "arr-view-duration")
    :zoom-min-duration %min-view-duration
    :zoom-max-duration %max-view-duration
    :content-length (bind "SEQV" "arr-content-length")
    :content-length-min (content-length-min)
    :content-length-max 8192
    :lane-scroll 0
    :snap %snap
    :min-duration 1
    :create-duration (* %beats-per-bar 4)
    :move-snap-mode :alignment-helper
    :resize-snap :grid
    ;; Region drags quantize to the zoom-adaptive grid, min down / max up
    ;; (region spec 4.3), so "grab exactly 4 bars" is a sloppy drag.
    :marquee-snap :grid
    :snap-mode :floor
    :resize-snap-mode :alignment-helper
    :scroll-mode :smooth
    :on-action |event| (scene-action event)))


(def %bottom-lane ()
  (timeline
    :key "scene-lane"
    :width 0 :flex 1
    :height 0
    :focusable true
    :sidebar-width 0
    :header-height 1
    :header-bottom-gutter %cursor-gutter-height
    :time-ruler (dict :mode :bars-beats :beats-per-bar %beats-per-bar)
    :grid-density %grid-density
    :background-color %timeline-background-color
    :title-bar-height %clip-title-bar-height
    :item-label-font-size %clip-label-font-size
    :item-label-color %clip-label-color
    :item-corner-radius %clip-corner-radius
    :item-color (list 0.52 0.56 0.62)
    :loop-color (list 0.92 0.72 0.25)
    :playhead-time (bind-seq "song-position-beats")
    ;; The ruler always owns the transport-start triangle, while the
    ;; track-specific cursor line remains in the lane the user clicked.
    :cursor-time cursor-time
    :cursor-marker-visible true
    :cursor-marker-scale 1.6
    :cursor-marker-width-scale 1.5
    :cursor-marker-height-scale 0.7
    :cursor-line-visible false
    :cursor-color %cursor-color
    :drop-types (list "transport-scene")
    :on-drop (lambda (event) (drop-scene event))
    :items '()
    :selection selection
    :selection-rect (%scene-region-rect)
    :view-start (bind "SEQV" "arr-view-start")
    :view-duration (bind "SEQV" "arr-view-duration")
    :zoom-min-duration %min-view-duration
    :zoom-max-duration %max-view-duration
    ;:content-length (bind "SEQV" "arr-content-length")
    ;:content-length-min (content-length-min)
    :content-length-max 8192
    :lane-scroll 0
    :snap %snap
    :min-duration 1
    :create-duration (* %beats-per-bar 4)
    :move-snap-mode :alignment-helper
    :resize-snap :grid
    ;; Region drags quantize to the zoom-adaptive grid, min down / max up
    ;; (region spec 4.3), so "grab exactly 4 bars" is a sloppy drag.
    :marquee-snap :grid
    :snap-mode :floor
    :resize-snap-mode :alignment-helper
    :scroll-mode :smooth
    :on-action |event| (scene-action event)))
;; Headerless, sidebar-less, single-lane track instance (spec 4.2). Lane
;; scrolling is inert; the outer buffer viewport owns vertical navigation.
(def %track-lane (i)
  (timeline
    :key (str "track-lane-" i)
    :width 0 :flex 1
    :height %track-lane-height
    ;; Vertical scrolling belongs to the enclosing track scroll container;
    ;; horizontal deltas still pan the shared time axis.
    :scroll-passthrough :vertical
    :background-color %timeline-background-color
    :title-bar-height %clip-title-bar-height
    ;; Title-bar double-click opens an existing clip; background double-click
    ;; uses the timeline's standard :finish-create-item action to create an
    ;; empty take clip. The scene lane deliberately does NOT opt into item
    ;; double-clicks.
    :double-click-items true
    :item-label-font-size %clip-label-font-size
    :item-label-color %clip-label-color
    :item-corner-radius %clip-corner-radius
    :sidebar-width 0
    :header-height 0
    ;; Track lanes draw NO ruler (:header-height 0 gates every bit of ruler
    ;; chrome), but they still need the bar/beat time base: it is what picks
    ;; the zoom-adaptive grid ladder that both the lane's grid lines and the
    ;; :grid snapping (region marquee, clip resize) quantize to. Without it a
    ;; lane falls back to the SECONDS ladder — steps like 5 or 10 beats — so
    ;; its grid lines miss bar lines the ruler above is labelling, and a
    ;; region drag snaps to those same wrong positions.
    :time-ruler (dict :mode :bars-beats :beats-per-bar %beats-per-bar)
    :grid-density %grid-density
    :focusable true
    :playhead-time (bind-seq "song-position-beats")
    ;; Every fast-changing surface below is a BOUND channel: the cursor, the
    ;; live drag ghost, the click selection, the sound binding and the region
    ;; highlight update by repainting this one widget, never by rerunning the
    ;; arrangement effect (UI_PERFORMANCE_TUNING.md ownership boundaries).
    :cursor-time (bind "SEQV" (channel "cursor" i))
    :cursor-marker-visible false
    :cursor-line-visible true
    :cursor-color %cursor-color
    :drop-types (list "transport-scene")
    :on-drop (lambda (event) (drop-scene event))
    :items (track-items i)
    :selected-id (bind "SEQV" (channel "selected-clip" i))
    :bound-id (bind "SEQV" (channel "bound-clip" i))
    :ghost-kind (bind "SEQV" (channel "ghost-kind" i))
    :ghost-id (bind "SEQV" (channel "ghost-id" i))
    :ghost-time (bind "SEQV" (channel "ghost-time" i))
    :ghost-region-a (bind "SEQV" (channel "ghost-region-a" i))
    :ghost-region-b (bind "SEQV" (channel "ghost-region-b" i))
    ;; Region highlight (region spec 4.4): the ghost channel while a drag is
    ;; live, else the committed region bridged from Rust. Empty lanes light
    ;; up too — the region is a rectangle over time, not over clips.
    :region-a (bind "SEQV" (channel "region-a" i))
    :region-b (bind "SEQV" (channel "region-b" i))
    :region-on (bind "SEQV" (channel "region-on" i))
    :selection-rect-style :region
    :view-start (bind "SEQV" "arr-view-start")
    :view-duration (bind "SEQV" "arr-view-duration")
    :zoom-min-duration %min-view-duration
    :zoom-max-duration %max-view-duration
    :content-length (bind "SEQV" "arr-content-length")
    :lane-scroll 0
    :snap %snap
    :min-duration 1
    ;; A background double-click creates an editable take clip. Give that
    ;; discrete gesture a musically useful span instead of falling back to
    ;; the timeline's one-beat resize minimum.
    :create-duration %beats-per-bar
    :resize-snap :grid
    :marquee-snap :grid
    :snap-mode :floor
    ;; A title-bar drag snaps the same way an edge drag does (region spec 6.3):
    ;; the zoom-adaptive grid ladder plus neighbouring clip edges.
    :move-snap-mode :alignment-helper
    :resize-snap-mode :alignment-helper
    :scroll-mode :smooth
    :on-action |event| (track-action i event)))

;; ── Buffer composition (spec 4.1) ──────────────────────────────────────────

;; Rows wrap their h-stack in a :width :fill box (the sequencer.lisp track-row
;; idiom): the box stretches to the pane, which gives the inner h-stack a
;; bounded width for flex distribution — without it the row collapses to its
;; fixed content and the flexed lane measures ~zero wide.
(def %track-row (i)
  (box :width :fill :border-color :bg :border-width 2
    (h-stack :width :fill :align :start
      (box
        :key (str "track-header-" i)
        :height :fill :width %header-width
        :selected (eseq.sequencer/track-selected-binding i)
        :background-color :buffer-bg
        :selected-background-color :mixer-strip-selected-bg
        (eseq.sequencer/track-header i true))
      (%track-lane i))))

;; Rows stack with :gap 0 so the timeline instances are vertically flush —
;; the pointer is always over a lane, keeping scroll/zoom gestures captured
;; by the timelines instead of leaking to the buffer viewport.
;;
;; No mode toolbar (Ableton-style): pointer gestures + scene drag-and-drop
;; and double-click cover editing; Backspace deletes the selection. The scene
;; lane (the arrangement's one ruler) sits OUTSIDE the track scroll container
;; so it stays pinned while track rows scroll vertically inside it.
;; The step tile hides the global status line, so song-primitive rejections
;; (including "song editing is unavailable during arrangement capture")
;; surface here; the strip disappears on the next successful edit.
(def %error-banner ()
  (if (= SEQ.song-edit-error nil)
    (box :width 0 :height 0 :bg :transparent)
    (box :width :fill :height 1.5 :padding 0.3
      :background-color (rgba 0.32 0.13 0.12 1.0)
      (label (str "Edit rejected: " SEQ.song-edit-error)
        :key "edit-error-label"
        :font-size 10.5 :color (rgba 1.0 0.78 0.72 1.0) :bg :transparent))))

;; ── Channel bridge: Rust-owned song state -> per-lane channels ─────────────
;; Reruns cost microseconds (a handful of float publishes); the lanes that
;; bind the channels just repaint. Without this bridge every lane subtree
;; would read SEQ.song-bound-clip / SEQ.song-region / SEQ.song-end-beat
;; directly and a clip click would rebuild every lane's item list. It lives
;; as an invisible subtree INSIDE the buffer (not a bare top-level effect):
;; subtree reruns ride the widget-flush machinery and cannot disturb the
;; active buffer's layout.
(def %publish-bridge ()
  (do
    (let ((bound SEQ.song-bound-clip))
      (map
        (lambda (i)
          (reactive-set "SEQV" (channel "bound-clip" i)
            (if (and (not (= bound nil)) (= (nth bound 0) i))
              (nth bound 1)
              -1)))
        (eseq.track-collapse/visible-track-indices)))
    (let ((region SEQ.song-region))
      (map
        (lambda (i)
          (if (and (not (= region nil))
                (>= i (nth region 0))
                (<= i (nth region 1)))
            (do
              (reactive-set "SEQV" (channel "region-a" i)
                (nth region 2))
              (reactive-set "SEQV" (channel "region-b" i)
                (nth region 3))
              (reactive-set "SEQV" (channel "region-on" i) 1))
            (reactive-set "SEQV" (channel "region-on" i) 0)))
        (eseq.track-collapse/visible-track-indices)))
    (reactive-set "SEQV" "arr-content-length" (content-length))))

(effect-buffer "*arrangement*"
  (v-stack :padding 0.0 :gap 0.0
    ;; Every root-level read lives inside its own subtree: a whole-list read
    ;; (like the scene lane's content-length-min reduce over SEQ.song-lanes)
    ;; at the root would turn each song publish into a full-buffer rerun and
    ;; defeat the per-track index-aware invalidation of the lane subtrees.
    (subtree :key "arr-channel-bridge"
      (do
        (%publish-bridge)
        (box :width 0 :height 0 :bg :transparent)))
    ;; The "No song yet" banner is gone (empty-arrangement spec 8): the
    ;; arrangement always exists, so there is no mode to explain.
    (subtree :key "arr-error-banner"
      (%error-banner))
    ;; Sound palette overlay (takes spec 17.6): opened from a clip via the
    ;; toolbar/badge gestures; renders as a centered modal over the whole
    ;; frame (modal spec §4) with zero footprint here while closed.
    (subtree :key "arr-sound-palette"
      (eseq.sound-palette/panel))
    (subtree :key "arr-scene-row"
      (box :width :fill
        (h-stack :width :fill :align :start
          (box :key "scene-header-spacer"
            :width %header-width :height %scene-lane-height
            )
          (%scene-lane))))
    (box :width :fill :height 0.1 :background-color :bg)
    (scroll :key "track-scroll" :width :fill :flex 1
      (v-stack :width :fill :gap 0.0
        (each (eseq.track-collapse/visible-track-indices) |i|
          (subtree :key (str "arr-track-" (nth SEQ.track-ids i))
            (%track-row i)))))
    
      (subtree :key "bottom-arr-ucene-row"
      (box :width :fill :height 1
        (h-stack :width :fill :align :start
          (box :key "bottom-scene-header-spacer"
            :width %header-width :height %scene-lane-height
            )
          (%bottom-lane))))    
    ))
