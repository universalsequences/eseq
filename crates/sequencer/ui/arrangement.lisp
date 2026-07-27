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
;; Live region-drag preview (region spec 4.4): a transient
;; {track-a track-b start end} updated per :marquee-select frame. The
;; COMMITTED region lives in Rust (SEQ.song-region) so it survives view
;; switches; this is only what the pointer is currently sweeping, and the
;; ghost wins over the committed region while it is set.
(defstate arrangement-region-ghost nil)

(def arrangement-min-view-duration 4)
(def arrangement-max-view-duration 1024)
(def arrangement-view-padding 8)
(def arrangement-beats-per-bar 4)
(def arrangement-snap arrangement-beats-per-bar)
(def arrangement-header-height 1.6)
(def arrangement-scene-lane-height 3.6)
(def arrangement-track-lane-height 2.85)
;; Vertical distance in CELLS between one track row's top and the next.
;; Track rows stack in a :gap 0 v-stack (see the buffer composition below), so
;; the pitch is exactly the lane height — no gap and no per-row chrome to add.
;; A cross-track region drag converts the widget's `row-delta` (cells) into a
;; count of tracks by dividing by this, so it MUST track the lane height and
;; the v-stack gap; metal_seq_arrangement_region_row_pitch_matches_layout
;; measures the rendered rows and fails if they ever drift apart.
(def arrangement-track-row-pitch arrangement-track-lane-height)
;; Clip title-bar height in cells (region spec 3.1): the move/resize strip
;; above each clip's body. Fixed rather than proportional so clips read the
;; same at any lane height; tune by eye against the Ableton reference.
(def arrangement-clip-title-bar-height 0.9)
(def arrangement-clip-label-font-size 9)
(def arrangement-clip-label-color '(rgba 0.2 0.2 0.2 1))
(def arrangement-timeline-background-color :buffer-bg)
;; Clip corner radius in CELLS (GarageBand-style rounded clips), so it scales
;; with the UI zoom like the lane heights above. 0 gives the square clips
;; every other timeline host draws.
(def arrangement-clip-corner-radius 0.22)
;; Divides the timeline's zoom-adaptive grid step: 2 means one ladder rung
;; finer than the default at every zoom, so bar lines stay put and a line
;; appears halfway between them. Both the drawn grid and :grid snapping
;; (region marquee, clip resize) follow it, which is what lets you grab a
;; half-bar without zooming all the way in. Every arrangement lane passes the
;; same value or the ruler and the lanes would quantize differently; hosts
;; that want the stock ladder (the piano roll) pass nothing.
(def arrangement-grid-density 2)
;; Fixed width for the composed seqv-track-header column so every lane's time
;; axis starts at the same x; the scene lane leads with a spacer of the same
;; width (spec 4.2: the per-track sidebar role is played by the header).
(def arrangement-header-width 26.5)

(def arrangement-event-num (event key fallback)
  (let ((value (get event key)))
    (if (= value nil) fallback value)))

;; ── Shared time axis (spec 5.1) ────────────────────────────────────────────

;; The furthest beat there is anything to look at. While a capture runs the
;; committed song end can still be BEHIND the recording (it is zero for a
;; whole-song capture), and clamping the view to it pinned the arrangement at
;; bar 1 with no way to scroll after the playhead. The record head is the
;; honest extent for as long as the recording is the content; it reads 0 when
;; no capture is running, so nothing else changes.
(def arrangement-scroll-extent ()
  (max SEQ.song-end-beat (arrangement-pending-head)))

(def arrangement-max-view-start (duration)
  (max 0 (- (+ (arrangement-scroll-extent) arrangement-view-padding) duration)))

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
      (set! arrangement-cursor-track track)
      ;; Mirror into Rust (region spec 5.3): Cmd-V is handled Rust-side and
      ;; pastes at the cursor, so the paste target cannot live only here.
      (seq-song-set-arr-cursor (max 0 time) track))))

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

(def arrangement-ghost-kind ()
  (if (= arrangement-ghost nil) nil (get arrangement-ghost :kind)))

;; Scene-lane gestures address a scene EVENT, whose identity is its start
;; beat (lane spec 12: every span IS a scene event, so there is no ambiguity
;; left to resolve).
(def arrangement-ghost-event? (kind beat)
  (and (= (arrangement-ghost-kind) kind)
    (= (get arrangement-ghost :beat) beat)))

;; Ghost overlay for one scene span (spec 9.1 live preview): a move ghost
;; shifts its event's span; a resize ghost moves the boundary shared by the
;; resized span's end and the next event's start.
(def arrangement-scene-span-start (index start)
  (if (arrangement-ghost-event? :move start)
    (get arrangement-ghost :start)
    (if (and (> index 0)
          (arrangement-ghost-event? :resize
            (get (nth SEQ.scene-spans (- index 1)) :start-beat)))
      (get arrangement-ghost :end)
      start)))

(def arrangement-scene-span-end (start end)
  (if (arrangement-ghost-event? :move start)
    (+ (get arrangement-ghost :start) (- end start))
    (if (arrangement-ghost-event? :resize start)
      (get arrangement-ghost :end)
      end)))

;; Scene-lane items (lane spec 12): ONE span per scene EVENT, running to the
;; next event (SEQ.scene-spans derives the end). A clip edge on any track can
;; no longer split this lane, so every span is labeled — the old
;; dedup-the-repeated-label hack that made a fragmented lane readable is gone
;; with the fragmentation.
(def arrangement-scene-row-items ()
  (map
    (lambda (index)
      (let ((span (nth SEQ.scene-spans index)))
        (let ((start (get span :start-beat))
              (end (get span :end-beat)))
          (dict
            :id start
            :lane 0
            :start (arrangement-scene-span-start index start)
            :end (arrangement-scene-span-end start end)
            :label (arrangement-scene-name (get span :scene))
            :kind :scene
            :selected (arrangement-row-selected? start)
            :color (list 0.52 0.56 0.62)))))
    (range 0 (len SEQ.scene-spans))))

(def arrangement-scene-items ()
  (append
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
      (arrangement-scene-row-items))
    ;; Launches captured so far, filling in the scene lane as you perform
    ;; (realtime feedback spec 3.1). Defined below with the rest of the
    ;; provisional surface.
    (arrangement-pending-scene-items)))

;; Song end, with the content-length drag ghost applied so the end marker
;; previews in every lane while dragging (spec 9.3).
;; While a capture runs past the old song end the marker follows the record
;; head: that IS where the song will end once the take commits (the splice
;; extends `end_beat` to the Stop beat), so leaving it behind would draw the
;; recording as if it fell outside the song.
(def arrangement-content-length ()
  (if (= (arrangement-ghost-kind) :end)
    (get arrangement-ghost :length)
    (arrangement-scroll-extent)))

;; The model rejects an end at/before the last scene change (spec 9.3); the
;; widget clamp mirrors that boundary.
(def arrangement-content-length-min ()
  (let ((count (len SEQ.scene-spans)))
    (if (= count 0)
      1
      (max 1 (get (nth SEQ.scene-spans (- count 1)) :start-beat)))))

(def arrangement-row-selected? (id)
  (> (len (filter (lambda (candidate) (= candidate id)) arrangement-selection)) 0))

(def arrangement-track-color (i)
  (if (and (>= i 0) (< i (len SEQ.track-colors)))
    (nth SEQ.track-colors i)
    (list 0.34 0.48 0.98)))

;; Every audible span is a clip now (lane spec 6.2), so there is exactly one
;; clip tint: the track color lifted slightly. The lift is multiplicative, not
;; a lerp toward white: mixing in white desaturated the track color into
;; pastel, so arrangement clips no longer read as the same color the session
;; grid and piano roll use for the track.
(def arrangement-clip-color (i)
  (let ((color (arrangement-track-color i)))
    (list
      (min 1 (* 1.15 (nth color 0)))
      (min 1 (* 1.15 (nth color 1)))
      (min 1 (* 1.15 (nth color 2))))))

;; The STORED clips of one track lane (lane spec 12): already merged, with
;; real ids — the view derives nothing, and every clip has a source. A stretch
;; of lane with no clip is silence and draws as empty.
(def arrangement-track-clips (i)
  (if (< i (len SEQ.song-lanes))
    (nth SEQ.song-lanes i)
    '()))

(def arrangement-find-track-clip (i clip-id)
  (let ((matches (filter (lambda (clip) (= (get clip :clip-id) clip-id))
                   (arrangement-track-clips i))))
    (if (> (len matches) 0) (nth matches 0) nil)))

;; The ids from a :select that name a REAL stored clip. Provisional recording
;; items (realtime feedback spec 3.4) carry no id, so selecting one selects
;; nothing — and every other gesture already resolves through
;; arrangement-find-track-clip, which cannot match them either.
(def arrangement-real-clip-ids (i ids)
  (filter (lambda (id) (not (= (arrangement-find-track-clip i id) nil))) ids))

;; Same guard for the scene lane, whose item ids are scene-event start beats.
(def arrangement-real-scene-ids (ids)
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

;; One repetition's length relative to the clip span (widget :cycle key).
;; This is deliberately allowed above 1: a clip shorter than its pattern
;; shows only the source window it actually plays instead of squeezing the
;; whole pattern into the clip.
(def arrangement-clip-cycle (entry clip)
  (let ((span (- (get clip :end-beat) (get clip :start-beat)))
        (length-beats (get entry :length-beats)))
    (if (and (> length-beats 0) (> span 0))
      (/ length-beats span)
      1)))

(def arrangement-clip-phase (entry clip)
  (if (= (get clip :take-id) nil)
    (if (<= (get entry :num-steps) 0)
      0
      (/ (or (get clip :offset-steps) 0) (get entry :num-steps)))
    ;; Take dots are already windowed and normalized to the exact played
    ;; range below, so they start at phase zero and never repeat.
    0))

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
                     1)
            :phase (arrangement-clip-phase entry clip)))))))

;; Start-edge preview offset (spec 8): the commit re-stamps `offset-steps` so
;; the surviving part keeps playing what it played there. Pattern offsets
;; wrap; take offsets advance linearly and clamp at zero. The live preview
;; must use the same rules or its dots jump when the gesture commits.
(def arrangement-ghost-offset-steps (i clip new-start)
  (let ((entry (if (= (get clip :take-id) nil)
                 (arrangement-lane-pattern-events i (get clip :pattern-id))
                 (arrangement-lane-take-events i (get clip :take-id)))))
    (if (or (= entry nil)
          (<= (get entry :num-steps) 0)
          (<= (get entry :length-beats) 0))
      (get clip :offset-steps)
      (let ((step-beats (/ (get entry :length-beats) (get entry :num-steps))))
        (let ((shifted (+ (or (get clip :offset-steps) 0)
                         (/ (- new-start (get clip :start-beat)) step-beats))))
          (if (= (get clip :take-id) nil)
            ;; Euclidean wrap expressed with floor because Lisp `mod` is a
            ;; signed remainder and a left-edge grow can make `shifted`
            ;; negative.
            (- shifted (* (get entry :num-steps)
                         (floor (/ shifted (get entry :num-steps)))))
            (max 0 shifted)))))))

;; Live move/resize ghost for a track clip. A move is RIGID (region spec 6.1 /
;; takes spec 7.4): the span slides, its length and `offset-steps` do not
;; change, so the preview draws the same music somewhere else.
(def arrangement-track-move-ghost-clip (i clip)
  (let ((new-start (max 0 (get arrangement-ghost :start))))
    (dict
      :clip-id (get clip :clip-id)
      :start-beat new-start
      :end-beat (+ new-start (- (get clip :end-beat) (get clip :start-beat)))
      :pattern-id (get clip :pattern-id)
      :take-id (get clip :take-id)
      :offset-steps (get clip :offset-steps))))

;; Region move: every clip the dragged RECTANGLE covers slides with it, in
;; every lane the rectangle spans — the rect ghost alone shows where the music
;; will land but not what lands there.
(def arrangement-region-ghost-covers? (i clip)
  (and (= (arrangement-ghost-kind) :region-move)
    (>= i (get arrangement-ghost :track-a))
    (<= i (get arrangement-ghost :track-b))
    (> (get clip :end-beat) (get arrangement-ghost :start))
    (< (get clip :start-beat) (get arrangement-ghost :end))))

(def arrangement-region-ghost-clip (i clip)
  (let ((delta (get arrangement-ghost :delta)))
    (dict
      :clip-id (get clip :clip-id)
      :start-beat (+ (get clip :start-beat) delta)
      :end-beat (+ (get clip :end-beat) delta)
      :pattern-id (get clip :pattern-id)
      :take-id (get clip :take-id)
      :offset-steps (get clip :offset-steps))))

(def arrangement-track-ghost-clip (i clip)
  (if (and (= (arrangement-ghost-kind) :track-move)
        (= (get arrangement-ghost :track) i)
        (= (get arrangement-ghost :clip-id) (get clip :clip-id)))
    (arrangement-track-move-ghost-clip i clip)
  (if (arrangement-region-ghost-covers? i clip)
    (arrangement-region-ghost-clip i clip)
  (if (and (= (arrangement-ghost-kind) :track-resize)
        (= (get arrangement-ghost :track) i)
        (= (get arrangement-ghost :clip-id) (get clip :clip-id)))
    (if (= (get arrangement-ghost :edge) :start)
      (let ((new-start (max 0
                         (min (- (get clip :end-beat) 1)
                           (get arrangement-ghost :time)))))
        (dict
          :clip-id (get clip :clip-id)
          :start-beat new-start
          :end-beat (get clip :end-beat)
          :pattern-id (get clip :pattern-id)
          :take-id (get clip :take-id)
          :offset-steps (arrangement-ghost-offset-steps i clip new-start)))
      (dict
        :clip-id (get clip :clip-id)
        :start-beat (get clip :start-beat)
        :end-beat (max (+ (get clip :start-beat) 1)
                    (min SEQ.song-end-beat (get arrangement-ghost :time)))
        :pattern-id (get clip :pattern-id)
        :take-id (get clip :take-id)
        :offset-steps (get clip :offset-steps)))
    clip))))

;; Track-lane items (lane spec 12): the stored clips, and nothing else. A gap
;; between clips produces NO item — the lane really is silent there.
(def arrangement-track-clip-label (clip)
  (if (= (get clip :take-id) nil)
    (str "Pattern " (get clip :pattern-id))
    ;; Take ids are zero-based internally; their default user-facing names
    ;; and every other take badge are one-based.
    (str "Take " (+ (get clip :take-id) 1))))

(def arrangement-track-clip-items (i)
  (map
    (lambda (raw)
      (let ((clip (arrangement-track-ghost-clip i raw)))
        (dict
          :id (get clip :clip-id)
          :lane 0
          :start (get clip :start-beat)
          ;; Take items clamp to the take's true remaining length (takes
          ;; spec 11.3) — the clip may extend past the take's end, but that
          ;; tail is silent and draws as empty lane.
          :end (if (= (get clip :take-id) nil)
                 (get clip :end-beat)
                 (let ((window (arrangement-take-clip-window i clip)))
                   (if (= window nil)
                     (get clip :end-beat)
                     (get window :end-beat))))
          :kind :midi
          :label (arrangement-track-clip-label clip)
          :content (arrangement-clip-content i clip)
          :color (arrangement-clip-color i))))
    (arrangement-track-clips i)))

;; ── Provisional capture content (realtime feedback spec 3) ────────────────
;; SEQ.song-pending is the recording IN FLIGHT: the pending take lanes and
;; the launches captured so far, published only while arrangement capture is
;; running and cleared on stop, cancel and failure alike. Its items are drawn
;; and never edited — provisional content has no clip id yet, so it appears
;; nowhere in arrangement-track-clips and no gesture can resolve one. That is
;; the same items-for-drawing vs clips-for-editing split the ghost preview
;; uses (spec 3.4).

;; Provisional items wear the SAME tint and labels as the committed clips
;; they are about to become: a capture preview whose whole job is to show
;; where the music is landing should not be a differently-coloured stand-in
;; for it. What marks them as in-flight is that they are growing under the
;; playhead, not a paint job.

(def arrangement-pending-lanes ()
  (if (= SEQ.song-pending nil) '() (get SEQ.song-pending :lanes)))

(def arrangement-pending-scene-events ()
  (if (= SEQ.song-pending nil) '() (get SEQ.song-pending :scene-events)))

;; The record head, which every provisional span grows toward.
(def arrangement-pending-head ()
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
(def arrangement-pending-span-steps (lane)
  (let ((num-steps (max 1 (get lane :num-steps)))
        (length-beats (get lane :length-beats)))
    (if (<= length-beats 0)
      num-steps
      (let ((step-beats (/ length-beats num-steps)))
        (max 1
          (/ (- (get lane :end-beat) (get lane :start-beat)) step-beats))))))

(def arrangement-pending-content (lane)
  (let ((dots (arrangement-windowed-dots lane 0
                (arrangement-pending-span-steps lane))))
    (if (= (len dots) 0)
      nil
      (dict :dots dots :cycle 1 :phase 0))))

;; No :id — the one thing that would make a provisional item addressable.
;; The take has no TakeId until the stop-commit registers it, so the label
;; cannot name a number yet.
(def arrangement-pending-track-items (i)
  (map
    (lambda (lane)
      (dict
        :lane 0
        :start (get lane :start-beat)
        :end (get lane :end-beat)
        :kind :midi
        :label "Take"
        :content (arrangement-pending-content lane)
        :color (arrangement-clip-color i)))
    (filter (lambda (lane) (= (get lane :track) i))
      (arrangement-pending-lanes))))

;; ── Provisional launch clips ──────────────────────────────────────────────
;; What each captured launch put on a TRACK lane: a clip-launched pattern, or
;; the scene's cell pattern on every lane a captured scene change claimed.
;; These are the clips the stop-commit will write, so they preview the same
;; way — a looping pattern tiled over its span.

(def arrangement-pending-track-events (i)
  (filter (lambda (event) (= (get event :track) i))
    (if (= SEQ.song-pending nil) '() (get SEQ.song-pending :track-events))))

(def arrangement-pending-launch-content (event span)
  (let ((dots (arrangement-pattern-dots event))
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
(def arrangement-pending-launch-item (i index events)
  (let ((event (nth events index)))
    (let ((start (get event :start-beat))
          (end (if (< (+ index 1) (len events))
                 (get (nth events (+ index 1)) :start-beat)
                 (arrangement-pending-head))))
      (if (<= end start)
        nil
        (dict
          :lane 0
          :start start
          :end end
          :kind :midi
          :label (str "Pattern " (get event :pattern-id))
          :content (arrangement-pending-launch-content event (- end start))
          :color (arrangement-clip-color i))))))

(def arrangement-pending-launch-items (i)
  (let ((events (arrangement-pending-track-events i)))
    (filter (lambda (item) (not (= item nil)))
      (map (lambda (index) (arrangement-pending-launch-item i index events))
        (range 0 (len events))))))

;; A captured launch's provisional span runs to the next captured launch, or
;; to the record head while it is still the last one. A launch the head has
;; not passed yet has nothing to draw.
(def arrangement-pending-scene-item (index events)
  (let ((event (nth events index)))
    (let ((start (get event :start-beat))
          (end (if (< (+ index 1) (len events))
                 (get (nth events (+ index 1)) :start-beat)
                 (arrangement-pending-head))))
      (if (<= end start)
        nil
        (dict
          :lane 0
          :start start
          :end end
          :kind :scene
          :label (arrangement-scene-name (get event :scene))
          :color (list 0.52 0.56 0.62))))))

(def arrangement-pending-scene-items ()
  (let ((events (arrangement-pending-scene-events)))
    (filter (lambda (item) (not (= item nil)))
      (map (lambda (index) (arrangement-pending-scene-item index events))
        (range 0 (len events))))))

;; Committed clips first, provisional content on top (spec 3.4). Recorded
;; takes last of all: the stop-commit paints them OVER whatever the launches
;; put on the lane, so the preview stacks the same way.
(def arrangement-track-items (i)
  (append (arrangement-track-clip-items i)
    (append (arrangement-pending-launch-items i)
      (arrangement-pending-track-items i))))

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

;; ── Region selection (region spec 4) ───────────────────────────────────────

;; Drag capture is per widget instance, so a cross-track marquee never reaches
;; the lanes it sweeps over: the originating lane reports the pointer's
;; VERTICAL TRAVEL (`:row-delta`, cells, signed and unclamped) and the host
;; reconstructs the track span from it (spec 4.2).

;; Position of a model track index within the currently visible tracks, or -1
;; when it is collapsed away. Collapsed tracks are simply absent, so a region
;; drag always spans visible tracks and never lands on a hidden one.
(def arrangement-visible-ordinal (visible track)
  (reduce |acc o| (if (= (nth visible o) track) o acc)
    -1
    (range 0 (len visible))))

;; Ordinal -> model track index, clamped to the visible range so a drag that
;; runs off the top or bottom of the arrangement selects out to the edge.
(def arrangement-track-at-ordinal (visible ordinal)
  (nth visible (max 0 (min (- (len visible) 1) ordinal))))

;; The far end of a region drag that started in track `i`: convert the
;; vertical travel to a count of track rows, step that many places through the
;; visible order, and map back to a model index.
(def arrangement-region-other-track (i row-delta)
  (let ((visible (seq-visible-track-indices)))
    (if (= (len visible) 0)
      i
      (let ((ordinal (arrangement-visible-ordinal visible i)))
        (if (< ordinal 0)
          i
          (arrangement-track-at-ordinal visible
            (+ ordinal (round (/ (or row-delta 0) arrangement-track-row-pitch)))))))))

(def arrangement-region-from-event (i event)
  (dict
    :track-a i
    :track-b (arrangement-region-other-track i (get event :row-delta))
    :start (get event :time-a)
    :end (get event :time-b)))

;; Scene-lane marquees select the time span across EVERY visible track
;; (spec 4.2): the scene lane spans the whole arrangement, so its vertical
;; travel carries no track information.
(def arrangement-region-all-tracks (event)
  (let ((visible (seq-visible-track-indices)))
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
(def arrangement-region-commit (region)
  (do
    (set! arrangement-region-ghost nil)
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
(def arrangement-select-clip (i clip-id)
  (let ((clip (arrangement-find-track-clip i clip-id)))
    (if (= clip nil)
      (do (seq-song-deselect-clip) (seq-song-clear-region))
      (seq-song-select-clip i clip-id
        (get clip :start-beat) (get clip :end-beat)))))

;; Any other selection gesture drops the region: the two are mutually
;; exclusive (spec 4.1). Clip selection additionally clears it Rust-side, so
;; this is really about the in-flight ghost and the scene-row path.
(def arrangement-region-clear ()
  (do
    (set! arrangement-region-ghost nil)
    (seq-song-clear-region)))

;; The scene lane draws its own marquee echo while a drag is live, then keeps
;; the committed rect lit for a SCENE-LANE region (region spec 4.4) — the
;; visible sign that the region carries the scene events, not just the clips.
(def arrangement-scene-region-rect ()
  (if (not (= arrangement-selection-rect nil))
    arrangement-selection-rect
    (if (and (not (= SEQ.song-region nil))
          (= (nth SEQ.song-region 4) true))
      (dict :time-a (nth SEQ.song-region 2) :time-b (nth SEQ.song-region 3)
        :lane-a 0 :lane-b 0)
      nil)))

;; The rect one lane draws: the ghost while a drag is live, else the committed
;; Rust-owned region, else nothing. Full lane height over [start, end) — the
;; lane is single-lane, so lane-a/lane-b are always 0 (spec 4.4).
(def arrangement-region-for-track (i)
  (let ((ghost arrangement-region-ghost))
    (if (not (= ghost nil))
      (if (and (>= i (min (get ghost :track-a) (get ghost :track-b)))
            (<= i (max (get ghost :track-a) (get ghost :track-b))))
        (dict :time-a (get ghost :start) :time-b (get ghost :end)
          :lane-a 0 :lane-b 0)
        nil)
      (if (= SEQ.song-region nil)
        nil
        (if (and (>= i (nth SEQ.song-region 0)) (<= i (nth SEQ.song-region 1)))
          (dict :time-a (nth SEQ.song-region 2) :time-b (nth SEQ.song-region 3)
            :lane-a 0 :lane-b 0)
          nil)))))

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
        ;; Provisional captured launches carry no id (realtime feedback spec
        ;; 3.4), so only real scene events can enter the selection.
        (set! arrangement-selection (arrangement-real-scene-ids (get event :ids)))
        ;; A scene span covers every track and names no single clip, so it
        ;; releases the sound binding (takes spec 16.6 cause 2) — and, for the
        ;; same reason, drops the region (region spec 4.1).
        (arrangement-region-clear)
        (seq-song-deselect-clip)
        (set-arrangement-cursor (get event :time) -1))
      :clear-selection
      (do
        (set! arrangement-selection-rect nil)
        (set! arrangement-selection '())
        (arrangement-region-clear)
        (seq-song-deselect-clip)
        (set-arrangement-cursor (get event :time) -1))
      :set-cursor
      (set-arrangement-cursor (get event :time) -1)
      ;; A scene-lane marquee selects the time span across ALL visible tracks
      ;; (region spec 4.2): the lane has no per-track geometry to sweep. The
      ;; scene lane keeps its own dashed marquee echo while the drag is live.
      :marquee-select
      (do
        (set! arrangement-selection-rect event)
        (set! arrangement-region-ghost (arrangement-region-all-tracks event)))
      :finish-marquee-select
      (do
        (set! arrangement-selection-rect nil)
        (arrangement-region-commit (arrangement-region-all-tracks event)))
      ;; Live drags: ghost only, never a primitive (spec 9.1). A scene-lane
      ;; item's id IS its scene event's start beat (lane spec 12).
      :move-items-absolute
      (set! arrangement-ghost
        (dict :kind :move
          :beat (get event :anchor-id)
          :start (get event :start)))
      ;; The scene lane is contiguous, so dragging an event's START edge IS
      ;; moving that event: the boundary it owns is the same one the previous
      ;; span's end handle drags. Lower it to the move ghost rather than the
      ;; resize one, or the drag would write the event's start into its end.
      :resize-item-absolute
      (set! arrangement-ghost
        (if (= (get event :edge) :start)
          (dict :kind :move
            :beat (get event :id)
            :start (get event :time))
          (dict :kind :resize
            :beat (get event :id)
            :end (get event :time))))
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
            :from-beat (get arrangement-ghost :beat)
            :start (get arrangement-ghost :start)))
        (set! arrangement-ghost nil))
      :finish-resize-items
      (if (= (arrangement-ghost-kind) :resize)
        (arrangement-edit-finish
          (dict :type :finish-resize-items
            :from-beat (get arrangement-ghost :beat)
            :end (get arrangement-ghost :end)))
        ;; Start-edge drag: the ghost is a move (see above), so it commits as
        ;; one.
        (if (= (arrangement-ghost-kind) :move)
          (arrangement-edit-finish
            (dict :type :finish-move-items
              :from-beat (get arrangement-ghost :beat)
              :start (get arrangement-ghost :start)))
          (set! arrangement-ghost nil)))
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
      (do
        ;; The ids ARE the scene events' beats (lane spec 12). Removing a
        ;; scene change can never touch a clip, but the selection/region were
        ;; measured against the old lane — drop them with it.
        (set! arrangement-selection '())
        (arrangement-region-clear)
        ;; A provisional captured launch has no id, so a delete aimed at one
        ;; resolves to nothing and never reaches a primitive.
        (let ((ids (arrangement-real-scene-ids (get event :ids))))
          (if (= (len ids) 0)
            nil
            (arrangement-edit-finish (dict :type :delete-items :ids ids)))))
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
(def arrangement-clip-edit (i clip payload)
  (arrangement-edit-finish
    (merge payload
      :track i
      :clip-id (get clip :clip-id))))

(def arrangement-track-resize-end-finish (i clip time)
  (let ((new-end (max 0 (min SEQ.song-end-beat time))))
    (if (= new-end (get clip :end-beat))
      nil
      (if (<= new-end (get clip :start-beat))
        ;; Dragged past its own start: the clip is gone.
        (arrangement-clip-edit i clip (dict :type :clip-delete))
        (arrangement-clip-edit i clip
          (dict :type :clip-resize
            :start (get clip :start-beat)
            :end new-end))))))

;; Start-edge resize is NOT a move: the span's left edge and the clip's phase
;; anchor move together, so the surviving music stays where it was (lane spec
;; 8 / takes spec 8 — `arrangement-clip-resize` re-stamps `offset-steps` by
;; the split rule, in both directions). Same primitive as the end edge, only
;; the other coordinate changes.
(def arrangement-track-resize-start-finish (i clip time)
  (let ((new-start (max 0 time)))
    (if (= new-start (get clip :start-beat))
      nil
      (if (>= new-start (get clip :end-beat))
        ;; Dragged past its own end: the clip is gone.
        (arrangement-clip-edit i clip (dict :type :clip-delete))
        (arrangement-clip-edit i clip
          (dict :type :clip-resize
            :start new-start
            :end (get clip :end-beat)))))))

(def arrangement-track-resize-finish (i)
  (let ((clip-id (get arrangement-ghost :clip-id))
        (edge (get arrangement-ghost :edge))
        (time (get arrangement-ghost :time)))
    (let ((clip (arrangement-find-track-clip i clip-id)))
      (do
        (set! arrangement-ghost nil)
        (if (= clip nil)
          nil
          (if (= edge :start)
            (arrangement-track-resize-start-finish i clip time)
            (arrangement-track-resize-end-finish i clip time)))))))

;; ── Clip / region move (region spec 6) ─────────────────────────────────────

;; Does the committed region cover this clip?
(def arrangement-clip-in-region? (i clip)
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
(def arrangement-region-beyond-clip? (i clip)
  (if (or (= SEQ.song-region nil) (= clip nil))
    false
    (or (< (nth SEQ.song-region 0) i)
      (> (nth SEQ.song-region 1) i)
      (< (nth SEQ.song-region 2) (get clip :start-beat))
      (> (nth SEQ.song-region 3) (get clip :end-beat)))))

(def arrangement-clip-drags-region? (i clip)
  (and (arrangement-clip-in-region? i clip)
    (arrangement-region-beyond-clip? i clip)))

;; Live title-bar drag: ghost only, never a primitive (spec 9.1). Vertical
;; travel is ignored — cross-track moves are invalid for the same per-track
;; pattern-pool reason as cross-track paste (region spec 8), so the widget's
;; :lane is dropped here.
(def arrangement-track-move-ghost (i event)
  (let ((clip (arrangement-find-track-clip i (get event :anchor-id))))
    (if (= clip nil)
      (set! arrangement-ghost nil)
      (if (arrangement-clip-drags-region? i clip)
        ;; Region move: the ghost is the whole rectangle shifted by the drag
        ;; delta, clamped so it can never run before beat 0 (the primitive
        ;; rejects that rather than truncating the leading clips). It carries
        ;; the SOURCE rectangle too, so every covered lane can preview its own
        ;; clips sliding with it.
        (let ((delta (max (- 0 (nth SEQ.song-region 2))
                       (- (get event :start) (get clip :start-beat)))))
          (do
            (set! arrangement-ghost
              (dict :kind :region-move :track i :delta delta
                :track-a (nth SEQ.song-region 0)
                :track-b (nth SEQ.song-region 1)
                :start (nth SEQ.song-region 2)
                :end (nth SEQ.song-region 3)))
            (set! arrangement-region-ghost
              (dict :track-a (nth SEQ.song-region 0)
                :track-b (nth SEQ.song-region 1)
                :start (+ (nth SEQ.song-region 2) delta)
                :end (+ (nth SEQ.song-region 3) delta)
                :scene-lane (nth SEQ.song-region 4)))))
        (set! arrangement-ghost
          (dict :kind :track-move :track i
            :clip-id (get clip :clip-id)
            :start (max 0 (get event :start))))))))

;; Release: one primitive from the ghost's final values, and never a stale
;; ghost on any path (including the guard failures).
(def arrangement-track-move-finish (i)
  (let ((kind (arrangement-ghost-kind))
        (track (get arrangement-ghost :track)))
    (if (and (= kind :region-move) (= track i))
      (let ((delta (get arrangement-ghost :delta)))
        (do
          (set! arrangement-region-ghost nil)
          (if (= delta 0)
            (set! arrangement-ghost nil)
            (arrangement-edit-finish (dict :type :region-move :delta delta)))))
      (if (and (= kind :track-move) (= track i))
        (let ((clip (arrangement-find-track-clip i (get arrangement-ghost :clip-id)))
              (start (get arrangement-ghost :start)))
          (do
            (set! arrangement-ghost nil)
            (if (or (= clip nil) (= start (get clip :start-beat)))
              nil
              (arrangement-clip-edit i clip
                (dict :type :clip-move :start start)))))
        (do
          (set! arrangement-region-ghost nil)
          (set! arrangement-ghost nil))))))

(def arrangement-track-delete (i ids)
  (do
    (set! arrangement-track-selection '())
    ;; Deleting the clip deletes what the selection pointed at: the region
    ;; goes with it, or the highlight would stay lit over empty lane (region
    ;; spec 4.1 — a clip selection IS its region).
    (arrangement-region-clear)
    (seq-song-deselect-clip)
    (map
      (lambda (clip-id)
        (let ((clip (arrangement-find-track-clip i clip-id)))
          (if (= clip nil)
            nil
            (arrangement-clip-edit i clip (dict :type :clip-delete)))))
      ids)))

(def arrangement-track-action (i event)
  (if (arrangement-view-action? event)
    (arrangement-view-action event)
    (match event.type
      :select
      ;; Only ids that name a stored clip survive: a provisional recording
      ;; item has none, so clicking one selects nothing (realtime feedback
      ;; spec 3.4).
      (let ((ids (arrangement-real-clip-ids i (get event :ids))))
        (do
          (seqv-select-track-for-edit i)
          (set! arrangement-selection '())
          (set! arrangement-selection-rect nil)
          ;; A clip and a region are mutually exclusive (region spec 4.1); the
          ;; Rust side drops the region too, this clears the in-flight ghost.
          (set! arrangement-region-ghost nil)
          (set! arrangement-selected-track i)
          (set! arrangement-track-selection ids)
          ;; Selecting a clip is the explicit sound-binding gesture (takes
          ;; spec 16.2/16.6): it re-binds this track's device panel, monitor
          ;; sound and take punch-in template. The binding lives in Rust so it
          ;; survives view switches and transport.
          (if (= (len ids) 0)
            (do (seq-song-deselect-clip) (seq-song-clear-region))
            (arrangement-select-clip i (nth ids 0)))
          (set-arrangement-cursor (get event :time) i)))
      ;; Degenerate zero-movement release, or a click on empty lane space:
      ;; drop the region and park the edit cursor here, Ableton-style
      ;; (region spec 4.4).
      :clear-selection
      (do
        (seqv-select-track-for-edit i)
        (set! arrangement-track-selection '())
        (arrangement-region-clear)
        (seq-song-deselect-clip)
        (set-arrangement-cursor (get event :time) i))
      :set-cursor
      (do
        (seqv-select-track-for-edit i)
        (set-arrangement-cursor (get event :time) i))
      ;; Cross-track region sweep (region spec 4.2/4.4): live frames update
      ;; the ghost only; the release commits the Rust-owned region.
      :marquee-select
      (set! arrangement-region-ghost (arrangement-region-from-event i event))
      :finish-marquee-select
      (arrangement-region-commit (arrangement-region-from-event i event))
      ;; Live edge drag: ghost preview only (spec 9.1). Either edge; the
      ;; ghost carries which one so the preview and the commit agree.
      :resize-item-absolute
      (set! arrangement-ghost
        (dict :kind :track-resize :track i
          :clip-id (get event :id)
          :edge (if (= (get event :edge) :start) :start :end)
          :time (get event :time)))
      :finish-resize-items
      (if (and (= (arrangement-ghost-kind) :track-resize)
            (= (get arrangement-ghost :track) i))
        (arrangement-track-resize-finish i)
        (set! arrangement-ghost nil))
      :delete-items
      (arrangement-track-delete i (get event :ids))
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
      (arrangement-track-move-ghost i event)
      :finish-move-items
      (arrangement-track-move-finish i))))

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
    :grid-density arrangement-grid-density
    :background-color arrangement-timeline-background-color
    :title-bar-height arrangement-clip-title-bar-height
    :item-label-font-size arrangement-clip-label-font-size
    :item-label-color arrangement-clip-label-color
    :item-corner-radius arrangement-clip-corner-radius
    :item-color (list 0.52 0.56 0.62)
    :loop-color (list 0.92 0.72 0.25)
    :playhead-time (bind-seq "song-position-beats")
    :cursor-time (arrangement-lane-cursor-time -1)
    :drop-types (list "transport-scene")
    :on-drop (lambda (event) (arrangement-drop-scene event))
    :items (arrangement-scene-items)
    :selection arrangement-selection
    :selection-rect (arrangement-scene-region-rect)
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
    ;; Region drags quantize to the zoom-adaptive grid, min down / max up
    ;; (region spec 4.3), so "grab exactly 4 bars" is a sloppy drag.
    :marquee-snap :grid
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
    :background-color arrangement-timeline-background-color
    :title-bar-height arrangement-clip-title-bar-height
    :item-label-font-size arrangement-clip-label-font-size
    :item-label-color arrangement-clip-label-color
    :item-corner-radius arrangement-clip-corner-radius
    :sidebar-width 0
    :header-height 0
    ;; Track lanes draw NO ruler (:header-height 0 gates every bit of ruler
    ;; chrome), but they still need the bar/beat time base: it is what picks
    ;; the zoom-adaptive grid ladder that both the lane's grid lines and the
    ;; :grid snapping (region marquee, clip resize) quantize to. Without it a
    ;; lane falls back to the SECONDS ladder — steps like 5 or 10 beats — so
    ;; its grid lines miss bar lines the ruler above is labelling, and a
    ;; region drag snaps to those same wrong positions.
    :time-ruler (dict :mode :bars-beats :beats-per-bar arrangement-beats-per-bar)
    :grid-density arrangement-grid-density
    :focusable true
    :playhead-time (bind-seq "song-position-beats")
    :cursor-time (arrangement-lane-cursor-time i)
    :drop-types (list "transport-scene")
    :on-drop (lambda (event) (arrangement-drop-scene event))
    :items (arrangement-track-items i)
    :selection (arrangement-lane-selection i)
    ;; Region highlight (region spec 4.4): the ghost while a drag is live,
    ;; else the committed Rust-owned region. Empty lanes light up too — the
    ;; region is a rectangle over time, not over clips. `:region` style lights
    ;; the lane and each covered clip's BODY instead of washing a translucent
    ;; marquee over the top of everything (Ableton).
    :selection-rect (arrangement-region-for-track i)
    :selection-rect-style :region
    :view-start arrangement-view-start
    :view-duration arrangement-view-duration
    :zoom-min-duration arrangement-min-view-duration
    :zoom-max-duration arrangement-max-view-duration
    :content-length (arrangement-content-length)
    :lane-scroll 0
    :snap arrangement-snap
    :min-duration 1
    :resize-snap :grid
    :marquee-snap :grid
    :snap-mode :floor
    ;; A title-bar drag snaps the same way an edge drag does (region spec 6.3):
    ;; the zoom-adaptive grid ladder plus neighbouring clip edges.
    :move-snap-mode :alignment-helper
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
    (h-stack :width :fill :align :start
      (box
        :key (str "arrangement-track-header-" i)
        :height :fill :width arrangement-header-width
        :selected (seqv-track-selected-binding i)
        :background-color :buffer-bg
        :selected-background-color :mixer-strip-selected-bg
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
      (h-stack :width :fill :align :start
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
