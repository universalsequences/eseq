;; ui/arrangement.lisp -- arrangement timeline view over song mode
;; (docs/arrangement-timeline-ui-spec.md). Renders to *arrangement* buffer;
;; loaded by ui/main.lisp. One timeline widget instance per lane: the scene
;; lane (with the primary time ruler) plus one headerless, sidebar-less
;; instance per visible track. A trailing grid fills unused scroll space and
;; a pinned footer mirrors the ruler. All share one time axis (spec 5).
;;
;; The song is the host kinds' (kind-bindings spec §14.2d, §14.2o): `song`,
;; its `spans`, each track's `clips` and `cells`, the `region` and the
;; provisional capture surface (`song.pending-*`). Lanes are addressed by
;; track position (`i`), the address of the timeline commands and the
;; sequencer's track helpers; `(track i)` is the lane's track.

(module eseq.arrangement)

;; Migration aliases (module spec §10 step 2,
;; tools/module-compat-aliases.tsv) for the names unconverted callers still
;; spell flat: transport.lisp's `set-arrangement-cursor` and the entry points
;; the Rust arrangement tests drive by name. Deleted as each consumer
;; converts.

;; Declared dependencies (spec §4). `import` is a *runtime* form — it runs
;; after this file is compiled — so it guarantees only that the target is
;; evaluated before our body executes. That is exactly what the lane bodies
;; need: they call `seq-visible-track-indices` and `sound-palette-panel`
;; while the buffer tree is being built. These two edges are the ordering
;; ui/main.lisp used to encode by listing those files above us.
(import eseq.track-collapse)
(import eseq.sound-palette)
(import eseq.sample-import)
(import eseq.retrospective)
(import eseq.resample)
(import eseq.factory-promote)
(import eseq.export-song)
(import eseq.file-dialogs)
(import eseq.view-kit :refer (open-menu! menu-of nothing listed? index-of rgb-part))
(import eseq.kinds :refer (track tracks scenes song transport selection select-region!
                           clear-region!))

(export arr-view
        arr-select
        arr-lanes
        arr-drag
        arr-placement
        arr-scene-menu
        arr-pattern-menu
        track-row-pitch
        scroll-extent
        set-view-start
        set-cursor
        lane-cursor-time
        scene-name
        scene-items
        content-length
        content-length-min
        clip-color
        track-clips
        pattern-dots
        clip-cycle
        track-items
        lane-selection
        region-other-track
        scene-action
        lane-region-rect
        lane-ghost
        track-drag-kind
        track-action
        drop-scene
        select-all-clips
        clip-span-all-tracks
        begin-placement
        cancel-placement)

;; ── View state (`:key ()` singletons) ──────────────────────────────────────
;; Every fast-changing surface a lane draws is a field bound with #': scroll
;; and zoom, the edit cursor, the click selection, the sound binding, the
;; region highlight and the drag ghosts repaint the bound timeline widgets
;; without rerunning the arrangement effect (UI_PERFORMANCE_TUNING.md). The
;; track lanes all bind the same fields; each passes its `:lane-key` (its
;; track position) and the timeline draws a per-lane channel only on the lane
;; (or lane range) that owns it.

;; The shared time axis and the edit cursor. The cursor is track-specific
;; (Ableton-style): clicking a time in a track lane parks it on that track;
;; -1 is the scene lane. Later edits ("paste clip at cursor") get both a time
;; and a target track.
(def-kind arr-view
  :key ()
  :state ((start 0)
          (duration 64)
          (content-length 64)
          (cursor-time 0)
          (cursor-track -1)))

;; The click selection. `scenes` are the scene lane's selected events (their
;; start beats) and `rect` its marquee echo while a drag is live. Track-clip
;; selection (spec 9.2 extension): `clips` are the clicked lane's clip ids;
;; `lane` and `clip` are what the lanes draw (-1: none). Selecting in any
;; lane clears the other kind so Backspace is never ambiguous.
(def-kind arr-select
  :key ()
  :state ((scenes '())
          (rect :any :default nil)
          (clips '())
          (lane -1)
          (clip -1)))

;; The host's song selection as the lanes draw it (the lane sync below): the
;; bound clip (takes spec 16.6) and the committed region (region spec 4.4),
;; with the lanes they light.
(def-kind arr-lanes
  :key ()
  :state ((bound-lane -1)
          (bound-clip -1)
          (region-on false)
          (region-lane-a -1)
          (region-lane-b -1)
          (region-a 0)
          (region-b 0)))

;; Live drags. `scene` is the SCENE lane's ghost (spec 9.1): live gesture
;; actions update it only; the terminal :finish-* action lowers to exactly
;; one song primitive via seq-arrangement-action and clears it. A primitive
;; rejection reports in song.edit-error and, because items derive from the
;; committed song, the view snaps back on its own. `track` is the TRACK-lane
;; drag's commit data and `region` the region the pointer is sweeping
;; (region spec 4.4); no render reads either. The rest is the lanes' ghost
;; channel, which wins over the committed region while set: `ghost` is its
;; kind (the widget decoder's: 0 none, 1 move, 2 resize-start, 3 resize-end,
;; 4 marquee rect, 5 region-move), `clip` the clip it moves or resizes.
(def-kind arr-drag
  :key ()
  :state ((scene :any :default nil)
          (track :any :default nil)
          (region :any :default nil)
          (ghost 0)
          (clip -1)
          (time -1)
          (region-a 0)
          (region-b 0)
          (lane-a -1)
          (lane-b -1)))

;; Pattern placement is a sticky mode: it stays active across clicks until
;; Place (or Command-P) toggles it off, Escape cancels it, or the track list
;; changes. `active` holds the tracks and each one's source, captured on
;; activation; `choice` is the cell the pattern dropdown picked.
(def-kind arr-placement
  :key ()
  :state ((active :any :default nil)
          (choice cell :default nil)))

;; The scene lane's context menu: the beat it opened at and the span under
;; the pointer, with the beat that span started at then (spans are
;; positional: an edit while the menu is open can put another scene change
;; in the same instance).
(def-kind arr-scene-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (time 0)
          (span scene-span :default nil)
          (span-start 0)))

;; A track lane's context menu: its track, the pattern placement would put,
;; the beat it opened at and the clip under the pointer.
(def-kind arr-pattern-menu
  :key ()
  :state ((open false)
          (at :point :default nil)
          (track track :default nil)
          (source :any :default nil)
          (time 0)
          (clip clip :default nil)))

(def min-view-duration 4)
(def max-view-duration 1024)
(def view-padding 8)
(def beats-per-bar 4)
(def snap beats-per-bar)
(def header-height 2.6)
;; One cell (~20 px at the default scale) between ruler/loop chrome and the
;; scene lane. The transport-start triangle lives in this gutter.
(def cursor-gutter-height 0.5)
(def scene-lane-height 4.6)
(def track-lane-height 3.85)
;; Timeline borders are drawn inside the widget, in physical pixels, without
;; changing row pitch or the shared ruler/grid alignment. Lanes that stack
;; flush (the track rows) must draw only ONE of the two edges, otherwise two
;; adjacent 1 px borders read as a 2 px seam — see `track-lane`.
(def lane-border-top-color :bg)
(def lane-border-bottom-color :bg)
(def lane-border-width 3)
;; Vertical distance in CELLS between one track row's top and the next.
;; Track rows stack in a :gap 0 v-stack (see the buffer composition below), so
;; the pitch is exactly the lane height — no gap and no per-row chrome to add.
;; A cross-track region drag converts the widget's `row-delta` (cells) into a
;; count of tracks by dividing by this, so it MUST track the lane height and
;; the v-stack gap; metal_seq_arrangement_region_row_pitch_matches_layout
;; measures the rendered rows and fails if they ever drift apart.
(def track-row-pitch track-lane-height)
;; Clip title-bar height in cells (region spec 3.1): the move/resize strip
;; above each clip's body. Fixed rather than proportional so clips read the
;; same at any lane height; tune by eye against the Ableton reference.
(def clip-title-bar-height 1.1)
(def clip-label-font-size 10)
(def clip-label-color :clip-label-fg)

(def scene-color () THEME.scene_clip_bg)
(def timeline-background-color :buffer-bg)
(def cursor-color :arrangement-cursor)
;; Clip corner radius in CELLS (GarageBand-style rounded clips), so it scales
;; with the UI zoom like the lane heights above. 0 gives the square clips
;; every other timeline host draws.
(def clip-corner-radius 0.142)
;; Requests one finer candidate from the timeline's zoom-adaptive grid. The
;; widget promotes crowded candidates to a readable aligned interval, and that
;; one resolved interval drives lines, labels, cursor placement, marquee, and
;; resize snapping. Every arrangement lane passes the same value or the ruler
;; and lanes would quantize differently; the piano roll uses the stock density.
(def grid-density 2)
;; Fixed width for the composed seqv-track-header column so every lane's time
;; axis starts at the same x; the scene lane leads with a spacer of the same
;; width (spec 4.2: the per-track sidebar role is played by the header).
(def header-width 30.0)

;; Whether lane i is within lanes a..b (inclusive, either order).
(def lane-in? (i a b)
  (and (>= i (min a b)) (<= i (max a b))))

;; ── Per-lane channels ──────────────────────────────────────────────────────

;; Lane i's click selection is clip clip-id (-1 for none).
(def select-lane-clip! (i clip-id)
  (set! arr-select.lane i)
  (set! arr-select.clip clip-id))

;; No clip is clicked on any lane.
(def deselect-clips! ()
  (set! arr-select.clips '())
  (select-lane-clip! -1 -1))

(def clear-ghost! ()
  (set! arr-drag.ghost 0))

;; Lane i's ghost for clip clip-id: a move (1) or an edge resize (2, 3) to
;; `time`.
(def ghost-clip! (i kind clip-id time)
  (set! arr-drag.lane-a i)
  (set! arr-drag.lane-b i)
  (set! arr-drag.clip clip-id)
  (set! arr-drag.time time)
  (set! arr-drag.ghost kind))

(def clear-region-ghost! ()
  (set! arr-drag.region nil)
  (clear-ghost!))

;; The region the pointer sweeps (nil: none), drawn as a ghost rect over the
;; lanes it spans: 4 for a marquee sweep, 5 for a region move, whose `delta`
;; shifts the region and the clips it covers.
(def ghost-region! (region kind delta)
  (if (= region nil)
    (clear-region-ghost!)
    (let ((start (get region :start))
          (end (get region :end)))
      (set! arr-drag.region (merge region :start (+ start delta) :end (+ end delta)))
      (set! arr-drag.lane-a (get region :track-a))
      (set! arr-drag.lane-b (get region :track-b))
      (set! arr-drag.region-a start)
      (set! arr-drag.region-b end)
      (set! arr-drag.time delta)
      (set! arr-drag.ghost kind))))

;; ── Shared time axis (spec 5.1) ────────────────────────────────────────────

;; The furthest beat there is anything to look at. While a capture runs the
;; committed song end can still be BEHIND the recording (it is zero for a
;; whole-song capture), and clamping the view to it pinned the arrangement at
;; bar 1 with no way to scroll after the playhead. The record head is the
;; honest extent for as long as the recording is the content; it reads 0 when
;; no capture is running, so nothing else changes.
(def scroll-extent ()
  (max song.end
    song.pending-head
    ;; Open-ended playback runs PAST the end (jam room, unified-transport
    ;; spec 4.2); the playhead is the honest extent out there. Reads 0
    ;; while stopped, so the parked view is unchanged.
    (if (= song.mode "stopped") 0 song.position)))

(def max-view-start (duration)
  (max 0 (- (+ (scroll-extent) view-padding) duration)))

(def set-view-start (start duration)
  (set! arr-view.start
    (max 0 (min (max-view-start duration) start))))

;; Zoom by the event's factor around its anchor time, which keeps its place
;; on screen.
(def set-zoom (event)
  (let ((start arr-view.start)
        (duration arr-view.duration)
        (next-duration (max min-view-duration
                         (min max-view-duration
                           (/ duration (or (get event :factor) 1)))))
        (anchor (or (get event :anchor-time) start))
        (anchor-ratio (if (<= duration 0)
                        0.5
                        (max 0 (min 1 (/ (- anchor start) duration))))))
    (set! arr-view.duration next-duration)
    (set-view-start (- anchor (* anchor-ratio next-duration)) next-duration)))

(def set-cursor (time track)
  (unless (= time nil)
    (let ((beat (max 0 time)))
      (set! arr-view.cursor-time beat)
      (set! arr-view.cursor-track track)
      ;; Mirror into Rust (region spec 5.3): Cmd-V is handled Rust-side and
      ;; pastes at the cursor, so the paste target cannot live only here.
      (seq-song-set-arr-cursor beat track))))

;; The lane that owns the cursor shows it; every other lane passes nil.
(def lane-cursor-time (track)
  (when (= arr-view.cursor-track track) arr-view.cursor-time))

;; Shared view-action routing (spec 5.2): every lane funnels scroll/zoom
;; into the one shared time axis, regardless of which lane the pointer is
;; over. `:lane-scroll`/`:delta-lanes` are ignored per spec 4.2 — vertical
;; navigation belongs to the track scroll container.
(def view-action (event)
  (match event.type
    :scroll-view
    ;; An absolute start (0 included) wins over a delta.
    (let ((start (get event :view-start)))
      (set-view-start
        (if (= start nil) (+ arr-view.start (or (get event :delta-time) 0)) start)
        arr-view.duration))
    :zoom-view
    (set-zoom event)))

(def view-action? (event)
  (or (= event.type :scroll-view)
      (= event.type :zoom-view)))

;; ── Items from the song (spec 6/8) ─────────────────────────────────────────

(def scene-name (s)
  (if (= s nil) "" s.name))

(def pattern-label (pid) (str "Pattern " pid))

;; A clip's or a captured launch's cell as its label (nil: a take's).
(def cell-label (cell)
  (pattern-label (if (= cell nil) "" cell.pid)))

;; The scene lane's ghost kind.
(def ghost-kind ()
  (let ((ghost arr-drag.scene))
    (when ghost (get ghost :kind))))

;; Scene-lane gestures address a scene EVENT, whose identity is its start
;; beat (lane spec 12: every span IS a scene event, so there is no ambiguity
;; left to resolve).
(def ghost-event? (kind beat)
  (and (= (ghost-kind) kind)
    (= (get arr-drag.scene :beat) beat)))

;; Ghost overlay for one scene span (spec 9.1 live preview): a move ghost
;; shifts its event's span; a resize ghost moves the boundary shared by the
;; resized span's end and the next event's start.
(def scene-span-start (spans index start)
  (if (ghost-event? :move start)
    (get arr-drag.scene :start)
    (if (and (> index 0)
          (let ((previous (nth spans (- index 1))))
            (ghost-event? :resize previous.start)))
      (get arr-drag.scene :end)
      start)))

(def scene-span-end (start end)
  (if (ghost-event? :move start)
    (+ (get arr-drag.scene :start) (- end start))
    (if (ghost-event? :resize start)
      (get arr-drag.scene :end)
      end)))

(def row-selected? (id)
  (listed? id arr-select.scenes))

;; Scene-lane items (lane spec 12): ONE span per scene EVENT, running to the
;; next event (song.spans derives the end). A clip edge on any track can no
;; longer split this lane, so every span is labeled.
(def scene-row-items ()
  (let ((spans song.spans)
        ;; The scene identity is the performer's while scene-latched: dim
        ;; the lane like any overridden track (rev 4).
        (color (if song.scene-latched (override-dim (scene-color)) (scene-color))))
    (map |index|
      (let ((span (nth spans index)))
        (dict
          :id span.start
          :lane 0
          :start (scene-span-start spans index span.start)
          :end (scene-span-end span.start span.end)
          :label (scene-name span.scene)
          :kind :scene
          :selected (row-selected? span.start)
          :color color))
      (range 0 (len spans)))))

(def scene-items ()
  (append
    (scene-row-items)
    (if (= (ghost-kind) :create)
      (list (dict
              :id :ghost-create
              :lane 0
              :start (get arr-drag.scene :start)
              :end (get arr-drag.scene :end)
              :kind :scene
              :label (scene-name transport.scene)
              :color (list 0.72 0.76 0.82)))
      '())
    ;; Launches captured so far, filling in the scene lane as you perform
    ;; (realtime feedback spec 3.1). Defined below with the rest of the
    ;; provisional surface.
    (pending-scene-items)))

;; Song end, with the content-length drag ghost applied so the end marker
;; previews in every lane while dragging (spec 9.3).
;; While a capture runs past the old song end the marker follows the record
;; head: that IS where the song will end once the take commits (the splice
;; extends `end_beat` to the Stop beat), so leaving it behind would draw the
;; recording as if it fell outside the song.
(def content-length ()
  (if (= (ghost-kind) :end)
    (get arr-drag.scene :length)
    (scroll-extent)))

;; f folded over every stored clip of every track, from init.
(def fold-clips (f init)
  (reduce |acc t| (reduce f acc t.clips) init (tracks)))

;; The furthest beat any stored clip runs to (0 when the lanes are empty).
(def last-clip-end ()
  (fold-clips |acc c| (max acc c.end) 0))

;; App::arr_set_end rejects an end AT or before the last scene change, and an
;; end before the last clip's end (spec 9.3). The clamp has to stay strictly
;; inside both boundaries: the widget clamps inclusively, so a min that equals
;; the last scene start hands the primitive the one value it refuses and the
;; drag comes back as an error banner instead of a clamp.
(def content-length-min ()
  (let ((spans song.spans))
    (max 1 (last-clip-end)
      (if (empty? spans)
        0
        (let ((last (nth spans (- (len spans) 1))))
          (+ last.start 1))))))

;; An :rgb value as the (r g b) list timeline items take.
(def rgb-list (c)
  (map |k| (rgb-part c k) (range 0 3)))

;; An (r g b) list scaled by k, each part at most 1.
(def scale-rgb (color k)
  (map |part| (min 1 (* k part)) color))

(def track-color (i)
  (let ((t (track i)))
    (if (= t nil) (list 0.34 0.48 0.98) (rgb-list t.color))))

;; Every audible span is a clip now (lane spec 6.2), so there is exactly one
;; clip tint: the track color lifted slightly. The lift is multiplicative, not
;; a lerp toward white: mixing in white desaturated the track color into
;; pastel, so arrangement clips no longer read as the same color the session
;; grid and piano roll use for the track.
(def clip-color (i)
  (scale-rgb (track-color i) 1.15))

;; The STORED clips of one track lane (lane spec 12): already merged, with
;; real ids — the view derives nothing, and every clip has a source. A stretch
;; of lane with no clip is silence and draws as empty.
(def track-clips (i)
  (let ((t (track i)))
    (if (= t nil) '() t.clips)))

(def find-track-clip (i clip-id)
  (first (filter |c| (= c.cid clip-id) (track-clips i))))

;; The ids from a :select that name a REAL stored clip. Provisional recording
;; items (realtime feedback spec 3.4) carry no id, so selecting one selects
;; nothing — and every other gesture already resolves through
;; find-track-clip, which cannot match them either.
(def real-clip-ids (i ids)
  (filter |id| (not (= (find-track-clip i id) nil)) ids))

;; Same guard for the scene lane, whose item ids are scene-event start beats.
(def real-scene-ids (ids)
  (let ((starts (map |span| span.start song.spans)))
    (filter |id| (listed? id starts) ids)))

;; A take clip (else a pattern clip).
(def take? (c) (>= c.take 0))

;; ── MIDI content (spec 7.1) ────────────────────────────────────────────────
;; A clip's notes arrive flattened (`c.note-dots`: the host flattens each
;; clip once, not on every lane re-render). The provisional capture content
;; below still carries raw (time transpose velocity duration) events, so it
;; flattens here, the same way: normalized (offset, value) dots — a
;; snapshot-time preview, deliberately impressionistic at arrangement zoom.

(def dot-cap 256)

;; Vertical placement: spread the pattern's own transpose range across the
;; item rect (single-pitch patterns sit mid-rect).
(def dot-value (note lo hi)
  (if (= hi lo)
    0.5
    (+ 0.15 (* 0.7 (/ (- note lo) (- hi lo))))))

;; Note length in steps (4th event element, region spec 3.2). Older/short
;; event rows degrade to a point dot rather than erroring.
(def event-duration (event)
  (if (< (len event) 4)
    0
    (max 0 (or (nth event 3) 0))))

;; Cap dots per item at dot-cap, densest-first: events collapse
;; into 1/cap-wide time buckets (one dot per bucket), so dense clusters thin
;; out first while isolated events always survive. Events arrive step-ordered.
;; Only events inside the step window [from, from + span) are shown,
;; normalized to the window — a re-anchored take renders the slice it
;; actually plays.
(def windowed-dots (all-events from span)
  (let ((events (filter |event| (and (>= (nth event 0) from)
                                     (< (nth event 0) (+ from span)))
                  all-events))
        (notes (map |event| (nth event 1) events)))
    (unless (empty? events)
      (let ((lo (reduce |acc note| (min acc note) (first notes) notes))
            (hi (reduce |acc note| (max acc note) (first notes) notes)))
        ;; Accumulate with cons + one final reverse: `append` copies the
        ;; whole accumulated list per event, which is quadratic at the
        ;; 256-dot cap.
        (reverse
          (get
            (reduce |acc event|
              (let ((offset (max 0 (min 0.999 (/ (- (nth event 0) from) span))))
                    (bucket (floor (* offset dot-cap))))
                (if (= bucket (get acc :last))
                  acc
                  (dict :last bucket
                    :dots (cons (dict :offset offset
                                  :value (dot-value (nth event 1) lo hi)
                                  ;; Real note length normalized to the
                                  ;; drawn window, clamped so a note never
                                  ;; paints past the item's end.
                                  :width (max 0
                                           (min (- 1 offset)
                                             (/ (event-duration event) span))))
                            (get acc :dots)))))
              (dict :last -1 :dots '())
              events)
            :dots))))))

;; One whole source (a pattern cycle) of `num-steps` steps as dots.
(def pattern-dots (events num-steps)
  (windowed-dots events 0 (max 1 num-steps)))

;; True drawn end of a take clip (takes spec 11.3): a take is finite and
;; never loops, so the item ends at min(clip end, start + remaining take
;; length at this clip's offset). A clip extending past the take's end is the
;; silent tail — it renders as empty lane, matching what plays.
(def take-clip-end (c)
  (if (or (<= c.length 0) (<= c.num-steps 0))
    c.end
    (let ((step-beats (/ c.length c.num-steps))
          (remaining-steps (max 0 (- c.num-steps c.offset))))
      (min c.end (+ c.start (* remaining-steps step-beats))))))

;; One repetition's length relative to the clip span (widget :cycle key).
;; This is deliberately allowed above 1: a clip shorter than its pattern
;; shows only the source window it actually plays instead of squeezing the
;; whole pattern into the clip.
(def clip-cycle (c)
  (let ((span (- c.end c.start)))
    (if (and (> c.length 0) (> span 0))
      (/ c.length span)
      1)))

;; A pattern clip's dots are its whole source cycle, tiled and phased by the
;; widget; a take clip's are already the exact step window it plays,
;; normalized to the drawn item, so they start at phase zero and never
;; repeat (takes spec 11.3).
(def clip-content (c)
  (unless (empty? c.note-dots)
    (if (take? c)
      (dict :dots c.note-dots :cycle 1 :phase 0 :wrap false)
      (dict :dots c.note-dots
        :cycle (clip-cycle c)
        :phase (if (<= c.num-steps 0) 0 (/ c.offset c.num-steps))
        ;; The widget's live start-edge ghost wraps a pattern's phase and
        ;; clamps a take's at zero (takes spec 8).
        :wrap true))))

(def track-clip-label (c)
  (if (take? c)
    ;; Take ids are zero-based internally; their default user-facing names
    ;; and every other take badge are one-based.
    (str "Take " (+ c.take 1))
    (cell-label c.cell)))

;; ── Per-lane override dim (unified-transport rev 4, Ableton-style) ────────
;; A latched lane's committed clips darken: the arrangement is NOT what you
;; hear on that lane until Back to Arrangement. Per-track, not global —
;; un-latched lanes keep playing (and showing) the arrangement at full color.
(def lane-latched? (i)
  (let ((t (track i)))
    (and (not (= t nil)) t.latched)))

(def override-dim (color)
  (scale-rgb color 0.35))

(def lane-clip-color (i)
  (let ((color (clip-color i)))
    (if (lane-latched? i) (override-dim color) color)))

;; Track-lane items (lane spec 12): the stored clips, and nothing else. A gap
;; between clips produces NO item — the lane really is silent there.
(def track-clip-items (i)
  (let ((color (lane-clip-color i)))
    (map |c|
      (dict
        :id c.cid
        :lane 0
        :start c.start
        ;; Take items clamp to the take's true remaining length (takes spec
        ;; 11.3) — the clip may extend past the take's end, but that tail is
        ;; silent and draws as empty lane.
        :end (if (take? c) (take-clip-end c) c.end)
        :kind :midi
        :label (track-clip-label c)
        :content (clip-content c)
        :color color)
      (track-clips i))))

;; ── Provisional capture content (realtime feedback spec 3) ────────────────
;; song.pending-* is the recording IN FLIGHT: the pending take lanes and the
;; launches captured so far, present only while arrangement capture is
;; running and emptied on stop, cancel and failure alike. Its items are drawn
;; and never edited — provisional content has no clip id yet, so it appears
;; nowhere in track-clips and no gesture can resolve one. That is the same
;; items-for-drawing vs clips-for-editing split the ghost preview uses
;; (spec 3.4).

;; Provisional items wear the SAME tint and labels as the committed clips
;; they are about to become: a capture preview whose whole job is to show
;; where the music is landing should not be a differently-coloured stand-in
;; for it. What marks them as in-flight is that they are growing under the
;; playhead, not a paint job.

;; Provisional lanes carry raw events in clip.events' shape, so they go
;; through the windowed-dots pipeline.
;;
;; The window is the item's OWN span, not the recorded content's length: the
;; item grows to the record head while the notes stay put, so normalizing over
;; the content would stretch the same dots across an ever-wider rect (they
;; would snap back on every new note and creep apart again in between). A take
;; never loops, so this is one cycle at phase 0 and the span past the last
;; note is honestly empty.
(def pending-span-steps (lane)
  (let ((num-steps (max 1 lane.num-steps)))
    (if (<= lane.length 0)
      num-steps
      (max 1 (/ (- lane.end lane.start) (/ lane.length num-steps))))))

(def pending-content (lane)
  (let ((dots (windowed-dots lane.events 0 (pending-span-steps lane))))
    (unless (empty? dots)
      (dict :dots dots :cycle 1 :phase 0))))

;; Whether pending row p (a lane or a launch) is on track lane i.
(def on-lane? (p i)
  (and (not (= p.track nil)) (= p.track.index i)))

;; No :id — the one thing that would make a provisional item addressable.
;; The take has no TakeId until the stop-commit registers it, so the label
;; cannot name a number yet.
(def pending-track-items (i)
  (let ((color (clip-color i)))
    (map |lane|
      (dict
        :lane 0
        :start lane.start
        :end lane.end
        :kind :midi
        :label "Take"
        :content (pending-content lane)
        :color color)
      (filter |lane| (on-lane? lane i) song.pending-lanes))))

;; ── Provisional launch clips ──────────────────────────────────────────────
;; What each captured launch put on a TRACK lane: a clip-launched pattern, or
;; the scene's cell pattern on every lane a captured scene change claimed.
;; These are the clips the stop-commit will write, so they preview the same
;; way — a looping pattern tiled over its span.

(def pending-launch-content (launch span)
  (let ((dots (pattern-dots launch.events launch.num-steps)))
    (unless (empty? dots)
      (dict :dots dots
        :cycle (if (and (> launch.length 0) (> span 0))
                 (/ launch.length span)
                 1)
        :phase 0))))

;; A captured launch's provisional span runs to the next one in `rows`, or to
;; the record head while it is the last.
(def pending-end (rows index)
  (if (< (+ index 1) (len rows))
    (let ((next (nth rows (+ index 1)))) next.start)
    song.pending-head))

;; The captured launches `rows` as items, `(item row end)` each; a launch
;; the record head has not passed yet has nothing to draw.
(def pending-span-items (rows item)
  (filter |x| (not (= x nil))
    (map |index|
      (let ((row (nth rows index))
            (end (pending-end rows index)))
        (when (> end row.start) (item row end)))
      (range 0 (len rows)))))

(def pending-launch-items (i)
  (let ((color (clip-color i)))
    (pending-span-items (filter |launch| (on-lane? launch i) song.pending-launches)
      |launch end| (dict
                     :lane 0
                     :start launch.start
                     :end end
                     :kind :midi
                     :label (cell-label launch.cell)
                     :content (pending-launch-content launch (- end launch.start))
                     :color color))))

(def pending-scene-items ()
  (pending-span-items song.pending-scenes
    |launch end| (dict
                   :lane 0
                   :start launch.start
                   :end end
                   :kind :scene
                   :label (scene-name launch.scene)
                   :color (scene-color))))

;; Committed clips first, provisional content on top (spec 3.4). Recorded
;; takes last of all: the stop-commit paints them OVER whatever the launches
;; put on the lane, so the preview stacks the same way.
(def track-items (i)
  (append (track-clip-items i)
    (pending-launch-items i)
    (pending-track-items i)))

;; Selection surface for one lane, as the lane draws it (for handlers and
;; tests): only the owning track shows its click selection; the bound clip
;; (takes spec 16.6) is the host's persistent timeline state, so it carries
;; the highlight after a view switch.
(def lane-selection (i)
  (let ((selected (if (= arr-select.lane i) arr-select.clip -1))
        (bound (if (= arr-lanes.bound-lane i) arr-lanes.bound-clip -1)))
    (append
      (if (>= selected 0) (list selected) '())
      (if (and (>= bound 0) (not (= bound selected))) (list bound) '()))))

;; ── Region selection (region spec 4) ───────────────────────────────────────

;; Drag capture is per widget instance, so a cross-track marquee never reaches
;; the lanes it sweeps over: the originating lane reports the pointer's
;; VERTICAL TRAVEL (`:row-delta`, cells, signed and unclamped) and the host
;; reconstructs the track span from it (spec 4.2).

;; The far end of a region drag that started in track `i`: convert the
;; vertical travel to a count of track rows, step that many places through the
;; visible order, and map back to a model index. Collapsed tracks are simply
;; absent from that order, so a region drag always spans visible tracks, and
;; one that runs off the top or bottom selects out to the edge.
(def region-other-track (i row-delta)
  (let ((visible (eseq.track-collapse/visible-track-indices))
        (ordinal (index-of visible i)))
    (if (< ordinal 0)
      i
      (let ((rows (round (/ (or row-delta 0) track-row-pitch))))
        (nth visible (max 0 (min (- (len visible) 1) (+ ordinal rows))))))))

(def region-from-event (i event)
  (dict
    :track-a i
    :track-b (region-other-track i (get event :row-delta))
    :start (get event :time-a)
    :end (get event :time-b)))

;; The region from `start` to `end` over every visible track (nil when none
;; is visible).
(def visible-region (start end scene-lane)
  (let ((visible (eseq.track-collapse/visible-track-indices)))
    (unless (empty? visible)
      (dict
        :track-a (first visible)
        :track-b (nth visible (- (len visible) 1))
        :start start
        :end end
        :scene-lane scene-lane))))

;; Scene-lane marquees select the time span across EVERY visible track
;; (spec 4.2): the scene lane spans the whole arrangement, so its vertical
;; travel carries no track information.
(def region-all-tracks (event)
  (visible-region (get event :time-a) (get event :time-b) true))

;; The committed region's first and last lanes (track positions).
(def region-lane-a (r) (let ((t (first r.tracks))) t.index))
(def region-lane-b (r) (let ((t (nth r.tracks (- (len r.tracks) 1)))) t.index))

;; The scene-lane bit (lane spec 8): a marquee swept in the scene lane
;; copies/pastes/deletes the scene EVENTS inside it as well as the clips,
;; which a track-lane marquee covering the same rectangle never does.
(def region-commit (region)
  (clear-region-ghost!)
  (let ((a (when region (track (get region :track-a))))
        (b (when region (track (get region :track-b)))))
    (if (or (= a nil) (= b nil))
      (clear-region!)
      (select-region! a b (get region :start) (get region :end)
        :scene-lane (= (get region :scene-lane) true)))))

;; Clicking a clip's title bar is BOTH gestures: it binds the track's sound to
;; the clip AND selects the clip's span as a one-track region, so the body
;; lights up exactly like a swept region does and copy/delete have a target
;; (Ableton; song.bound-clip does both). A free marquee, which names no single
;; clip, still releases the binding — and so does a click on a BACKDROP ghost
;; (negative id, lane spec 12): a gap is not a clip, so there is nothing to
;; bind or select.
(def select-clip (c)
  (set! song.bound-clip c)
  (when (= c nil) (clear-region!)))

;; Cmd+A (Ableton): select EVERY clip on EVERY track as one region — the
;; visible-track span over [first clip start, last clip end). A region is the
;; thing copy/duplicate/delete already act on, so this needs no new verb. It
;; is a track-lane sweep, not a scene-lane one: scene events are not clips.
;; Returns nil when the song holds no clips so the caller can say so.
(def clip-span-all-tracks ()
  (let ((hi (fold-clips |acc c| (max acc c.end) -1)))
    (unless (< hi 0)
      ;; Seeded with `hi`, so the min is a real clip start, never the seed.
      (dict :start (max 0 (fold-clips |acc c| (min acc c.start) hi)) :end hi))))

(def select-all-clips ()
  (let ((span (clip-span-all-tracks))
        (region (when span (visible-region (get span :start) (get span :end) false))))
    (if (= region nil)
      false
      (do
        (set! arr-select.scenes '())
        (set! arr-select.rect nil)
        (deselect-clips!)
        (region-commit region)
        true))))

;; Any other selection gesture drops the region: the two are mutually
;; exclusive (spec 4.1). Clip selection additionally clears it host-side, so
;; this is really about the in-flight ghost and the scene-row path.
(def drop-region! ()
  (clear-region-ghost!)
  (clear-region!))

;; What a gesture that names no single clip does: drop the region and release
;; the sound binding (takes spec 16.6 cause 2).
(def release-selection! ()
  (drop-region!)
  (set! song.bound-clip nil))

;; The scene lane draws its own marquee echo while a drag is live, then keeps
;; the committed rect lit for a SCENE-LANE region (region spec 4.4) — the
;; visible sign that the region carries the scene events, not just the clips.
(def scene-region-rect ()
  (let ((rect arr-select.rect)
        (r song.region))
    (if (not (= rect nil))
      rect
      (when (and (not (= r nil)) r.scene-lane)
        (dict :time-a r.start :time-b r.end :lane-a 0 :lane-b 0)))))

;; ── Action handlers ────────────────────────────────────────────────────────

;; Lower one finished gesture to song primitives through the Rust translator
;; (spec 9.1): exactly one primitive per gesture, validation/undo/rejection
;; reporting owned by the song host commands.
(def edit-finish (payload)
  (set! arr-drag.scene nil)
  (seq-arrangement-action payload))

;; The scene to create or drop: the playing one.
(def current-scene-index ()
  (let ((s transport.scene))
    (if (= s nil) 0 s.index)))

;; A scene-lane click selects scene events. A scene span covers every track
;; and names no single clip, so it releases the sound binding — and, for the
;; same reason, drops the region (region spec 4.1).
(def scene-select (event)
  (set! arr-select.rect nil)
  (deselect-clips!)
  ;; Provisional captured launches carry no id (realtime feedback spec
  ;; 3.4), so only real scene events can enter the selection.
  (set! arr-select.scenes (real-scene-ids (get event :ids)))
  (release-selection!)
  (set-cursor (get event :time) -1))

(def scene-clear-selection (event)
  (set! arr-select.rect nil)
  (set! arr-select.scenes '())
  (release-selection!)
  (set-cursor (get event :time) -1))

;; Commit the scene lane's move ghost (the widget's finish actions carry ids,
;; not times); any other ghost just goes.
(def scene-move-finish ()
  (if (= (ghost-kind) :move)
    (edit-finish
      (dict :type :finish-move-items
        :from-beat (get arr-drag.scene :beat)
        :start (get arr-drag.scene :start)))
    (set! arr-drag.scene nil)))

(def scene-delete (event)
  ;; The ids ARE the scene events' beats (lane spec 12). Removing a scene
  ;; change can never touch a clip, but the selection/region were measured
  ;; against the old lane — drop them with it.
  (set! arr-select.scenes '())
  (drop-region!)
  ;; A provisional captured launch has no id, so a delete aimed at one
  ;; resolves to nothing and never reaches a primitive.
  (let ((ids (real-scene-ids (get event :ids))))
    (unless (empty? ids)
      (edit-finish (dict :type :delete-items :ids ids)))))

;; Scene lane (spec 9.2: the only editable lane). View actions route to the
;; shared axis; live edit actions update the ghost preview only; terminal
;; actions commit through edit-finish.
(def scene-action (event)
  (if (view-action? event)
    (view-action event)
    (match event.type
      :select
      (scene-select event)
      :clear-selection
      (scene-clear-selection event)
      :set-cursor
      (set-cursor (get event :time) -1)
      ;; A scene-lane marquee selects the time span across ALL visible tracks
      ;; (region spec 4.2): the lane has no per-track geometry to sweep. The
      ;; scene lane keeps its own dashed marquee echo while the drag is live.
      :marquee-select
      (do
        (set! arr-select.rect event)
        (ghost-region! (region-all-tracks event) 4 0))
      :finish-marquee-select
      (do
        (set! arr-select.rect nil)
        (region-commit (region-all-tracks event)))
      ;; Live drags: ghost only, never a primitive (spec 9.1). A scene-lane
      ;; item's id IS its scene event's start beat (lane spec 12).
      :move-items-absolute
      (set! arr-drag.scene
        (dict :kind :move
          :beat (get event :anchor-id)
          :start (get event :start)))
      ;; The scene lane is contiguous, so dragging an event's START edge IS
      ;; moving that event: the boundary it owns is the same one the previous
      ;; span's end handle drags. Lower it to the move ghost rather than the
      ;; resize one, or the drag would write the event's start into its end.
      :resize-item-absolute
      (set! arr-drag.scene
        (if (= (get event :edge) :start)
          (dict :kind :move
            :beat (get event :id)
            :start (get event :time))
          (dict :kind :resize
            :beat (get event :id)
            :end (get event :time))))
      :create-item
      (set! arr-drag.scene
        (dict :kind :create
          :start (get event :start)
          :end (get event :end)))
      :resize-content-length
      (set! arr-drag.scene
        (dict :kind :end :length (get event :length)))
      ;; Terminal actions: one primitive each, from the ghost's final values.
      :finish-move-items
      (scene-move-finish)
      :finish-resize-items
      (if (= (ghost-kind) :resize)
        (edit-finish
          (dict :type :finish-resize-items
            :from-beat (get arr-drag.scene :beat)
            :end (get arr-drag.scene :end)))
        ;; Start-edge drag: the ghost is a move (see above), so it commits as
        ;; one.
        (scene-move-finish))
      :finish-create-item
      (edit-finish
        (dict :type :finish-create-item
          :start (get event :start)
          :end (get event :end)
          :scene (current-scene-index)))
      :finish-resize-content-length
      (edit-finish
        (dict :type :finish-resize-content-length
          :length (get event :length)))
      :delete-items
      (scene-delete event)
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
(def clip-edit (i c payload)
  (edit-finish (merge payload :track i :clip-id c.cid)))

(def track-resize-end-finish (i c time)
  (let ((new-end (max 0 (min song.end time))))
    (unless (= new-end c.end)
      (if (<= new-end c.start)
        ;; Dragged past its own start: the clip is gone.
        (clip-edit i c (dict :type :clip-delete))
        (clip-edit i c (dict :type :clip-resize :start c.start :end new-end))))))

;; Start-edge resize is NOT a move: the span's left edge and the clip's phase
;; anchor move together, so the surviving music stays where it was (lane spec
;; 8 / takes spec 8 — `arrangement-clip-resize` re-stamps `offset-steps` by
;; the split rule, in both directions). Same primitive as the end edge, only
;; the other coordinate changes.
(def track-resize-start-finish (i c time)
  (let ((new-start (max 0 time)))
    (unless (= new-start c.start)
      (if (>= new-start c.end)
        ;; Dragged past its own end: the clip is gone.
        (clip-edit i c (dict :type :clip-delete))
        (clip-edit i c (dict :type :clip-resize :start new-start :end c.end))))))

(def track-drag-kind ()
  (let ((drag arr-drag.track))
    (when drag (get drag :kind))))

;; Live edge drag on lane i: ghost preview only (spec 9.1). Either edge; the
;; ghost carries which one so the preview and the commit agree.
(def track-resize-ghost (i event)
  (let ((edge (if (= (get event :edge) :start) :start :end))
        (id (get event :id)))
    (set! arr-drag.track
      (dict :kind :track-resize :track i
        :clip-id id
        :edge edge
        :time (get event :time)))
    ;; A provisional item has no id: its ghost names no clip.
    (ghost-clip! i (if (= edge :start) 2 3) (if (= id nil) -1 id) (get event :time))))

;; Release: lane i's resize commits when the live drag is its own, and the
;; ghost goes on every path.
(def track-resize-finish (i)
  (let ((drag arr-drag.track)
        (mine (and (= (track-drag-kind) :track-resize) (= (get drag :track) i)))
        (c (when mine (find-track-clip i (get drag :clip-id)))))
    (set! arr-drag.track nil)
    (clear-ghost!)
    (unless (= c nil)
      (if (= (get drag :edge) :start)
        (track-resize-start-finish i c (get drag :time))
        (track-resize-end-finish i c (get drag :time))))))

;; ── Clip / region move (region spec 6) ─────────────────────────────────────

;; Does the committed region cover this clip?
(def clip-in-region? (i c)
  (let ((r song.region))
    (and (not (= r nil)) (not (= c nil))
      (lane-in? i (region-lane-a r) (region-lane-b r))
      (> c.end r.start)
      (< c.start r.end))))

;; ...and does it reach BEYOND it — another track, or more time?
;;
;; This is what separates the two title-bar gestures. Selecting a clip makes
;; its own span a one-clip region (spec 4.1), and the widget selects before it
;; drags, so "the region covers this clip" is true of every single-clip drag:
;; testing only that would turn every move into a region move, previewed as a
;; bare rectangle instead of the clip itself. A rectangle that is exactly the
;; dragged clip IS the clip, so it moves as one.
(def region-beyond-clip? (i c)
  (let ((r song.region))
    (and (not (= r nil)) (not (= c nil))
      (or (< (region-lane-a r) i)
        (> (region-lane-b r) i)
        (< r.start c.start)
        (> r.end c.end)))))

(def clip-drags-region? (i c)
  (and (clip-in-region? i c)
    (region-beyond-clip? i c)))

;; A region move: the widget shifts every covered clip and the region rect by
;; the published delta, clamped so the rectangle can never run before beat 0
;; (the primitive rejects that rather than truncating the leading clips).
(def region-move-ghost (i c event)
  (let ((r song.region)
        (delta (max (- 0 r.start) (- (get event :start) c.start))))
    (set! arr-drag.track (dict :kind :region-move :track i :delta delta))
    (ghost-region!
      (dict :track-a (region-lane-a r) :track-b (region-lane-b r)
        :start r.start :end r.end :scene-lane r.scene-lane)
      5 delta)))

;; Live title-bar drag: ghost only, never a primitive (spec 9.1). Vertical
;; travel is ignored — cross-track moves are invalid for the same per-track
;; pattern-pool reason as cross-track paste (region spec 8), so the widget's
;; :lane is dropped here.
(def track-move-ghost (i event)
  (let ((c (find-track-clip i (get event :anchor-id)))
        (start (max 0 (get event :start))))
    (if (= c nil)
      (set! arr-drag.track nil)
      (if (clip-drags-region? i c)
        (region-move-ghost i c event)
        (do
          (set! arr-drag.track (dict :kind :track-move :track i :clip-id c.cid :start start))
          (ghost-clip! i 1 c.cid start))))))

;; Test/introspection surface: the region rect lane i currently renders, as
;; the widget reconstructs it from the bound fields (the in-flight ghost wins
;; over the committed region).
(def lane-region-rect (i)
  (let ((kind arr-drag.ghost))
    (if (and (>= kind 4) (lane-in? i arr-drag.lane-a arr-drag.lane-b))
      (let ((delta (if (>= kind 5) arr-drag.time 0)))
        (dict
          :time-a (+ arr-drag.region-a delta)
          :time-b (+ arr-drag.region-b delta)))
      (when (and arr-lanes.region-on
              (lane-in? i arr-lanes.region-lane-a arr-lanes.region-lane-b))
        (dict :time-a arr-lanes.region-a :time-b arr-lanes.region-b)))))

;; Test/introspection surface: the ghost lane i draws, as the widget reads
;; the bound fields (kind 0 on a lane outside the ghost's lanes).
(def lane-ghost (i)
  (if (lane-in? i arr-drag.lane-a arr-drag.lane-b)
    (dict :kind arr-drag.ghost :clip arr-drag.clip :time arr-drag.time)
    (dict :kind 0 :clip -1 :time -1)))

;; Release: one primitive from the ghost's final values, and never a stale
;; ghost on any path (including the guard failures).
(def track-move-finish (i)
  (let ((drag arr-drag.track)
        (kind (when (and drag (= (get drag :track) i)) (get drag :kind))))
    (set! arr-drag.track nil)
    (clear-region-ghost!)
    (match kind
      :region-move
      (unless (= (get drag :delta) 0)
        (edit-finish (dict :type :region-move :delta (get drag :delta))))
      :track-move
      (let ((c (find-track-clip i (get drag :clip-id))))
        (unless (or (= c nil) (= (get drag :start) c.start))
          (clip-edit i c (dict :type :clip-move :start (get drag :start))))))))

(def track-delete (i ids)
  (deselect-clips!)
  ;; Deleting the clip deletes what the selection pointed at: the region
  ;; goes with it, or the highlight would stay lit over empty lane (region
  ;; spec 4.1 — a clip selection IS its region).
  (release-selection!)
  (each ids |clip-id|
    (let ((c (find-track-clip i clip-id)))
      (unless (= c nil)
        (clip-edit i c (dict :type :clip-delete))))))

;; The single-click select body, shared with :double-click-item (clip-edit
;; target spec 4.2: a double-click is "bind + ensure the editor is open", so
;; it must run the exact same bind/region/track-select path first). Returns
;; the clicked clip.
(def track-select (i event)
  ;; Only ids that name a stored clip survive: a provisional recording
  ;; item has none, so clicking one selects nothing (realtime feedback
  ;; spec 3.4).
  (let ((ids (real-clip-ids i (get event :ids)))
        (c (if (empty? ids) nil (find-track-clip i (first ids)))))
    (eseq.sequencer/select-track-for-edit (track i))
    (set! arr-select.scenes '())
    (set! arr-select.rect nil)
    ;; A clip and a region are mutually exclusive (region spec 4.1); the
    ;; host drops the region too, this clears the in-flight ghost.
    (clear-region-ghost!)
    (set! arr-select.clips ids)
    (select-lane-clip! i (if (= c nil) -1 c.cid))
    ;; Selecting a clip is the explicit sound-binding gesture (takes spec
    ;; 16.2/16.6): it re-binds this track's device panel, monitor sound and
    ;; take punch-in template. The binding lives in the host so it survives
    ;; view switches and transport.
    (select-clip c)
    ;; A clip click also parks the transport start at the clip's beginning,
    ;; independent of where in its title bar was hit.
    (set-cursor (if (= c nil) (get event :time) c.start) i)
    c))

(def track-clear-selection (i event)
  (eseq.sequencer/select-track-for-edit (track i))
  (deselect-clips!)
  (release-selection!)
  (set-cursor (get event :time) i))

;; Background double-click: follow the same empty-space selection path as the
;; first click, then atomically mint a real silent take + clip. The host
;; command selects the new clip when it commits, so the track-shaped open
;; call resolves onto that take on the next UI tick.
(def create-empty-take (i start end)
  (track-select i (dict :ids '() :time start))
  (seq-arrangement-empty-take-create i start end)
  (eseq.seq-panels/seq-open-arrangement-piano-roll-bottom-for-track i))

;; ── Pattern placement ──────────────────────────────────────────────────────

(def cancel-placement ()
  (let ((was-active (not (= arr-placement.active nil))))
    (set! arr-placement.active nil)
    was-active))

;; The pattern placement would put on track t: the chosen cell when it is
;; t's, else the track's playing pattern (nil without patterns).
(def placement-source (t)
  ;; These reads invalidate the selector when the scene or a pattern's
  ;; activity changes.
  transport.scene
  (let ((cells t.cells))
    (unless (empty? cells)
      (map |c| c.active cells)
      (let ((choice arr-placement.choice)
            (source (seq-arrangement-pattern t.index
                      (if (listed? choice cells) choice.pid nil))))
        (when source (merge source :track t))))))

(def placement-label (pid) (str pid))

(def begin-placement ()
  (if (= arr-placement.active nil)
    (let ((ts (tracks))
          (sources (map |t| (placement-source t) ts)))
      (unless (empty? (filter |source| (not (= source nil)) sources))
        (set! arr-placement.active (dict :tracks ts :sources sources))))
    (cancel-placement)))

;; The placement source for lane i, while placing and the lane still holds
;; the track it was captured for.
(def placement-target-source (i)
  (let ((active arr-placement.active)
        (sources (if (= active nil) '() (get active :sources)))
        (source (when (and (>= i 0) (< i (len sources))) (nth sources i))))
    (when (and source (= (get source :track) (track i)))
      source)))

;; A placement captured for another track list ends.
(def sync-placement-target ()
  (let ((active arr-placement.active))
    (when (and active (not (= (tracks) (get active :tracks))))
      (cancel-placement))))

(def place-pattern (source time)
  (unless (= source nil)
    (let ((t (get source :track)))
      (set! arr-pattern-menu.open false)
      (when (listed? t (tracks))
        (host-command "arrangement-pattern-place"
          (dict :track t.index :pattern-id (get source :pattern-id) :track-id t.tid
                :start-beat time))))))

(def track-action (i event)
  (if (view-action? event)
    (view-action event)
    (match event.type
      :place-item
      (place-pattern (placement-target-source i) (get event :time))
      :cancel-placement
      (cancel-placement)
      :select
      (track-select i event)
      ;; Title-bar double-click (clip-edit-target spec 4): bind exactly like a
      ;; single click, then open the piano roll on the bound source. The
      ;; piano-roll targeting comes from the binding itself, so the open call
      ;; stays track-shaped.
      :double-click-item
      (when (track-select i event)
        (eseq.seq-panels/seq-open-arrangement-piano-roll-bottom-for-track i))
      :finish-create-item
      (create-empty-take i (get event :start) (get event :end))
      ;; Degenerate zero-movement release, or a click on empty lane space:
      ;; in FX mode a clip BODY remains a region/cursor surface. In the
      ;; arrangement piano-roll mode the widget includes the body clip id,
      ;; and the same press retargets the editor instead. A true background
      ;; click carries no real id, so it clears focus and the piano roll shows
      ;; "No clip selected".
      :clear-selection
      (if (and (eseq.piano-roll/piano-roll-arrangement-mode?)
            (= eseq.seq-step-tabs/lower-panel-buffer "*piano-roll*")
            (not (empty? (real-clip-ids i (or (get event :ids) '())))))
        (track-select i event)
        (track-clear-selection i event))
      :set-cursor
      (do
        (eseq.sequencer/select-track-for-edit (track i))
        (set-cursor (get event :time) i))
      ;; Cross-track region sweep (region spec 4.2/4.4): live frames update
      ;; the ghost only; the release commits the host's region.
      :marquee-select
      (ghost-region! (region-from-event i event) 4 0)
      :finish-marquee-select
      (region-commit (region-from-event i event))
      :resize-item-absolute
      (track-resize-ghost i event)
      :finish-resize-items
      (track-resize-finish i)
      :delete-items
      (track-delete i (get event :ids))
      ;; Clipboard (region spec 5.3): focused lanes and the arrangement mode
      ;; both converge on these region primitives, which read the host's
      ;; region — a clip click already made that a one-clip region.
      :copy-items
      (seq-song-region-copy)
      :paste-items
      (seq-song-region-paste (get event :time))
      ;; Title-bar drag (region spec 6): one rigid clip move, or a move of the
      ;; whole region when the dragged clip lies inside it.
      :move-items-absolute
      (track-move-ghost i event)
      :finish-move-items
      (track-move-finish i))))

;; ── Scene drag-and-drop (Ableton-style, replaces the draw tool) ────────────
;; The transport scene pills are drag sources (:drag-type "transport-scene");
;; dropping one on any lane sets scene state, preserving clips, at the drop
;; beat, snapped to the bar grid. The drop event's :sx is the normalized
;; (-1..1) position within the lane, which maps straight onto the shared view
;; span because lanes have no sidebar.
(def pointer-time (event)
  ;; 0 is the lane's middle: only a missing :sx falls back to its left edge.
  (let ((sx (get event :sx))
        (ratio (max 0 (min 1 (/ (+ (if (= sx nil) -1 sx) 1) 2)))))
    (+ arr-view.start (* ratio arr-view.duration))))

(def drop-time (event)
  (max 0 (* snap (floor (/ (pointer-time event) snap)))))

(def drop-scene (event)
  (let ((scene (get (get event :payload) :scene)))
    (unless (= scene nil)
      (let ((start (drop-time event)))
        (edit-finish
          (dict :type :finish-create-item
            :start start
            ;; Dropping at/past the song end extends it by four bars from
            ;; the drop point (the translator only uses :end in that case).
            :end (+ start (* beats-per-bar 4))
            :scene scene))))))

(def scene-choice-label (s)
  (str (+ s.index 1) " · " s.name))

(def set-scene-at (beat s)
  (set! arr-scene-menu.open false)
  (host-command "arrangement-scene-insert" (dict :beat beat :scene s.index)))

(def starting-scene-control ()
  (let ((span (first song.spans))
        (choices (scenes)))
    (h-stack :padding 0.6 :gap 0 :align :center
      (dropdown :key "arr-starting-scene" :width 15 :height 1.15 :font-size 9
        :bg-color :mixer-strip-bg :border-color :mixer-strip-selected-bg
        :badge-color :transparent
        :value (if (and (not (= span nil)) (= span.start 0) (not (= span.scene nil)))
                 (str "Start: " (scene-choice-label span.scene))
                 "Set starting scene")
        :options (map |s| (scene-choice-label s) choices)
        :on-change |label|
          (let ((s (first (filter |s| (= (scene-choice-label s) label) choices))))
            (unless (= s nil) (set-scene-at 0 s)))))))

(def open-scene-menu (event)
  (cancel-placement)
  (set! arr-pattern-menu.open false)
  (let ((at (pointer-time event))
        (span (first (filter |span| (and (<= span.start at) (> span.end at)) song.spans))))
    (set! arr-scene-menu.time (drop-time event))
    (set! arr-scene-menu.span span)
    (set! arr-scene-menu.span-start (if (= span nil) 0 span.start))
    (open-menu! arr-scene-menu event)))

;; The span the menu opened on, while the lane still has it where it was
;; (nil once it is gone or another scene change took its place).
(def scene-menu-span ()
  (let ((span arr-scene-menu.span))
    (when (and (listed? span song.spans) (= span.start arr-scene-menu.span-start))
      span)))

(def scene-menu-choices (beat current prefix)
  (each (scenes) |s|
    (menu-item (scene-choice-label s) :key (str prefix s.index)
      :checked (= s current)
      :on-select |event| (set-scene-at beat s))))

(def close-scene-menu-with (command payload)
  (set! arr-scene-menu.open false)
  (host-command command payload))

(def scene-context-menu ()
  (let ((span (scene-menu-span))
        (beat arr-scene-menu.span-start))
    (apply menu-of arr-scene-menu
      (menu-item "Set Scene Here" :key "arr-set-scene"
        (scene-menu-choices arr-scene-menu.time nil "arr-set-scene-"))
      (if (= span nil)
        '()
        (list
          (menu-item "Change Scene" :key "arr-change-scene"
            (scene-menu-choices beat span.scene "arr-change-scene-"))
          (menu-separator)
          (menu-item "Place Scene Patterns" :key "arr-place-scene-patterns"
            :on-select |event|
              (close-scene-menu-with "arrangement-scene-patterns-place" (dict :beat beat)))
          (menu-item "Remove Scene" :key "arr-remove-scene"
            :on-select |event|
              (close-scene-menu-with "arrangement-scene-remove" (dict :beat beat))))))))

(def placement-toolbar ()
  (let ((t selection.track)
        (source (if (= t nil) nil (placement-source t)))
        (cells (if (= t nil) '() t.cells)))
    (h-stack :key "arr-placement-toolbar" :gap 0.4 :padding 0.6 :align :center
      (button "Place" :key "arr-place-pattern" :width 4.2 :height 1.15 :font-size 9
        :corner-radius 8
        :background-color :mixer-strip-bg :color :fg
        :border-color :mixer-strip-selected-bg
        :highlight-color :transparent :shadow-color :transparent
        :active-background-color (rgba 0.88 0.69 0.28 1)
        :active-color (rgba 0.16 0.14 0.10 1)
        :disabled (= source nil)
        :active (if (= arr-placement.active nil) 0 1)
        :on-click |event| (begin-placement))
      (dropdown :key "arr-placement-pattern" :width 3.5 :height 1.15 :font-size 9
        :bg-color :mixer-strip-bg
        :border-color :mixer-strip-selected-bg
        :badge-color :transparent
        :value (if (= source nil) "—" (placement-label (get source :pattern-id)))
        :options (map |c| (placement-label c.pid) cells)
        :on-change |label|
          (let ((cell (first (filter |c| (= (placement-label c.pid) label) cells))))
            (unless (= cell nil)
              (cancel-placement)
              (set! arr-placement.choice cell)))))))

;; The hint under the toolbar while placing.
(def placement-hint ()
  (if (= arr-placement.active nil)
    (nothing)
    (h-stack :gap 0
      (box :width 0.6)
      (label "Click to place · Esc cancels" :font-size 8 :color :dim))))

(def placement-item (i)
  (let ((source (placement-target-source i)))
    (unless (= source nil)
      (dict :id -1 :lane 0 :start 0 :end (get source :length-beats)
        :label (pattern-label (placement-label (get source :pattern-id)))
        :color (clip-color i)
        :content (dict :dots (windowed-dots (get source :events) 0 (get source :num-steps))
                   :cycle 1 :phase 0 :wrap true)))))

(def open-placement-menu (i event)
  (set! arr-scene-menu.open false)
  (cancel-placement)
  (let ((t (track i))
        (at (pointer-time event)))
    (set! arr-pattern-menu.track t)
    (set! arr-pattern-menu.source (if (= t nil) nil (placement-source t)))
    (set! arr-pattern-menu.time (drop-time event))
    (set! arr-pattern-menu.clip
      (first (filter |c| (and (<= c.start at) (> c.end at)) (track-clips i))))
    (open-menu! arr-pattern-menu event)))

;; Retarget clip c (a pattern clip of the menu's track) to the track's cell,
;; while both are still listed.
(def change-clip-pattern (c cell)
  (set! arr-pattern-menu.open false)
  (let ((t arr-pattern-menu.track))
    (when (and (listed? t (tracks)) (listed? c t.clips) (not (= c.cell cell)))
      (set! c.cell cell))))

(def pattern-context-menu ()
  (let ((t arr-pattern-menu.track)
        (c arr-pattern-menu.clip)
        (source arr-pattern-menu.source)
        (time arr-pattern-menu.time))
    (apply menu-of arr-pattern-menu
      (if (not (= c nil))
        (list
          (menu-item "Change Pattern" :key "arr-change-pattern"
            (each (if (= t nil) '() t.cells) |cell|
              (menu-item (pattern-label cell.pid) :key (str "arr-change-pattern-" cell.pid)
                :checked (= c.cell cell)
                :on-select |event| (change-clip-pattern c cell)))))
        (list
          (menu-item "Create empty take here" :key "arr-create-empty-take"
            :on-select |event|
              (do
                (set! arr-pattern-menu.open false)
                (when (listed? t (tracks))
                  (create-empty-take t.index time (+ time beats-per-bar)))))
          (menu-item
            (if (= source nil)
              "Insert current pattern here"
              (str "Insert " (pattern-label (placement-label (get source :pattern-id))) " here"))
            :key "arr-insert-pattern" :disabled (= source nil)
            :on-select |event| (place-pattern source time)))))))

(def drop-track-pattern (i event)
  (let ((payload (get event :payload))
        (t (track i)))
    (when (and (not (= t nil)) (= (get payload :track) i))
      (let ((source (seq-arrangement-pattern i (get payload :pattern-id))))
        (unless (= source nil)
          (place-pattern (merge source :track t) (drop-time event)))))))

;; ── Lane instances (spec 4.1/4.2) ──────────────────────────────────────────

;; The scene lane owns the primary bar/beat ruler and transport-start marker;
;; the footer mirrors its time scale without duplicating scene editing.
;; Every lane is a flex child (:width 0 :flex 1) of its row: it absorbs
;; exactly the width remaining after the fixed header column, so no row can
;; overflow the pane and drag the buffer viewport into horizontal scrolling.
(def scene-lane ()
  (timeline
    :key "scene-lane"
    :border-top-color lane-border-top-color
    :border-bottom-color lane-border-bottom-color
    :border-width lane-border-width
    :width 0 :flex 1
    :height scene-lane-height
    :focusable true
    :sidebar-width 0
    :header-height header-height
    :header-bottom-gutter cursor-gutter-height
    :time-ruler (dict :mode :bars-beats :beats-per-bar beats-per-bar)
    :grid-density grid-density
    :background-color timeline-background-color
    :title-bar-height clip-title-bar-height
    :item-label-font-size clip-label-font-size
    :item-label-color :scene-clip-fg
    :item-corner-radius clip-corner-radius
    :item-color :scene-clip-bg
    :loop-color :arrangement-loop
    :playhead-time #'song.position
    ;; The ruler always owns the transport-start triangle, while the
    ;; track-specific cursor line remains in the lane the user clicked.
    :cursor-time #'arr-view.cursor-time
    :cursor-marker-visible true
    :cursor-marker-scale 1.6
    :cursor-marker-width-scale 1.5
    :cursor-marker-height-scale 0.7
    :cursor-line-visible false
    :cursor-color cursor-color
    :drop-types (list "transport-scene")
    :on-drop |event| (drop-scene event)
    :on-right-click |event| (open-scene-menu event)
    :items (scene-items)
    :selection arr-select.scenes
    :selection-rect (scene-region-rect)
    :view-start #'arr-view.start
    :view-duration #'arr-view.duration
    :zoom-min-duration min-view-duration
    :zoom-max-duration max-view-duration
    :content-length #'arr-view.content-length
    :content-length-min (content-length-min)
    :content-length-max 8192
    :lane-scroll 0
    :snap snap
    :min-duration 1
    :create-duration (* beats-per-bar 4)
    :move-snap-mode :alignment-helper
    :resize-snap :grid
    ;; Region drags quantize to the zoom-adaptive grid, min down / max up
    ;; (region spec 4.3), so "grab exactly 4 bars" is a sloppy drag.
    :marquee-snap :grid
    :snap-mode :floor
    :resize-snap-mode :alignment-helper
    :scroll-mode :smooth
    :on-action |event| (scene-action event)))

;; The footer and the unused track area share the same time transform as
;; real lanes, but cannot create scene clips or acquire track selection.
(def continuation-lane (key height ruler-height)
  (timeline :key key :width 0 :flex 1 :height height
    :loop-visible false
    :border-top-color (if (> ruler-height 0) lane-border-top-color :transparent)
    :border-bottom-color (if (> ruler-height 0) lane-border-bottom-color :transparent)
    :border-width lane-border-width
    :sidebar-width 0 :header-height ruler-height
    :time-ruler (dict :mode :bars-beats :beats-per-bar beats-per-bar)
    :grid-density grid-density
    :background-color timeline-background-color
    :items '()
    :playhead-time #'song.position
    :view-start #'arr-view.start
    :view-duration #'arr-view.duration
    :content-length #'arr-view.content-length
    :zoom-min-duration min-view-duration
    :zoom-max-duration max-view-duration
    :scroll-passthrough :vertical
    :scroll-mode :smooth
    :on-action |event| (when (view-action? event) (view-action event))))

;; Headerless, sidebar-less, single-lane track instance (spec 4.2). Lane
;; scrolling is inert; the outer buffer viewport owns vertical navigation.
;; `top?`: the lane is the first visible one.
(def track-lane (i t top?)
  (timeline
    :key (str "track-lane-" i)
    ;; Lanes stack flush in a :gap 0 v-stack, so a lane that drew both edges
    ;; would butt its 1 px top against the previous lane's 1 px bottom and
    ;; read as a 2 px seam. Each lane owns only its bottom separator; the
    ;; topmost visible lane adds the top edge back so the block still closes.
    :border-top-color (if top? lane-border-top-color :transparent)
    :border-bottom-color lane-border-bottom-color
    :border-width lane-border-width
    :width 0 :flex 1
    :height track-lane-height
    ;; Vertical scrolling belongs to the enclosing track scroll container;
    ;; horizontal deltas still pan the shared time axis.
    :scroll-passthrough :vertical
    :background-color timeline-background-color
    :title-bar-height clip-title-bar-height
    ;; Title-bar double-click opens an existing clip; background double-click
    ;; uses the timeline's standard :finish-create-item action to create an
    ;; empty take clip. The scene lane deliberately does NOT opt into item
    ;; double-clicks.
    :double-click-items true
    :item-label-font-size clip-label-font-size
    :item-label-color clip-label-color
    :item-corner-radius clip-corner-radius
    :sidebar-width 0
    :header-height 0
    ;; Track lanes draw NO ruler (:header-height 0 gates every bit of ruler
    ;; chrome), but they still need the bar/beat time base: it is what picks
    ;; the zoom-adaptive grid ladder that both the lane's grid lines and the
    ;; :grid snapping (region marquee, clip resize) quantize to. Without it a
    ;; lane falls back to the SECONDS ladder — steps like 5 or 10 beats — so
    ;; its grid lines miss bar lines the ruler above is labelling, and a
    ;; region drag snaps to those same wrong positions.
    :time-ruler (dict :mode :bars-beats :beats-per-bar beats-per-bar)
    :grid-density grid-density
    :focusable true
    :playhead-time #'song.position
    ;; Every fast-changing surface below is bound: the cursor, the live drag
    ;; ghost, the click selection, the sound binding and the region highlight
    ;; update by repainting the lanes, never by rerunning the arrangement
    ;; effect (UI_PERFORMANCE_TUNING.md ownership boundaries). The lanes
    ;; share the fields; :lane-key and the *-lane props pick the lane that
    ;; draws each one.
    :lane-key i
    :cursor-time #'arr-view.cursor-time
    :cursor-lane #'arr-view.cursor-track
    :cursor-marker-visible false
    :cursor-line-visible true
    :cursor-color cursor-color
    :placement-active (if (= (placement-target-source i) nil) 0 1)
    :placement-item (placement-item i)
    :on-right-click |event| (open-placement-menu i event)
    :drop-types (list "transport-scene" (str "track-pattern-" t.tid))
    :on-drop |event|
      (if (= (get (get event :payload) :pattern-id) nil)
        (drop-scene event)
        (drop-track-pattern i event))
    :items (track-items i)
    :selected-id #'arr-select.clip
    :selected-lane #'arr-select.lane
    :bound-id #'arr-lanes.bound-clip
    :bound-lane #'arr-lanes.bound-lane
    :ghost-kind #'arr-drag.ghost
    :ghost-id #'arr-drag.clip
    :ghost-time #'arr-drag.time
    :ghost-region-a #'arr-drag.region-a
    :ghost-region-b #'arr-drag.region-b
    :ghost-lane-a #'arr-drag.lane-a
    :ghost-lane-b #'arr-drag.lane-b
    ;; Region highlight (region spec 4.4): the ghost while a drag is live,
    ;; else the committed region. Empty lanes light up too — the region is a
    ;; rectangle over time, not over clips.
    :region-a #'arr-lanes.region-a
    :region-b #'arr-lanes.region-b
    :region-on #'arr-lanes.region-on
    :region-lane-a #'arr-lanes.region-lane-a
    :region-lane-b #'arr-lanes.region-lane-b
    :selection-rect-style :region
    :view-start #'arr-view.start
    :view-duration #'arr-view.duration
    :zoom-min-duration min-view-duration
    :zoom-max-duration max-view-duration
    :content-length #'arr-view.content-length
    :lane-scroll 0
    :snap snap
    :min-duration 1
    ;; A background double-click creates an editable take clip. Give that
    ;; discrete gesture a musically useful span instead of falling back to
    ;; the timeline's one-beat resize minimum.
    :create-duration beats-per-bar
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

(def track-header (i)
  (let ((t (track i)))
    (box
      :key (str "track-header-" i)
      :height :fill :width header-width
      :selected #'t.in-selection
      :background-color :buffer-bg
      :selected-background-color :mixer-strip-selected-bg
      (eseq.sequencer/track-header t true))))

;; Rows wrap their h-stack in a :width :fill box (the sequencer.lisp track-row
;; idiom): the box stretches to the pane, which gives the inner h-stack a
;; bounded width for flex distribution — without it the row collapses to its
;; fixed content and the flexed lane measures ~zero wide. The row is the
;; header's subtree (the sequencer's header body is a subtree of its own) and
;; the lane a subtree nested in it, so a lane re-render (a clip edit, a
;; recording's growing take) leaves the header alone. A subtree's key names
;; its root widget, so the lane's root is an unkeyed flex row around the
;; `track-lane-<i>` timeline.
(def track-row (i t top?)
  (box :width :fill :border-color :bg :border-width 3
    (h-stack :width :fill :align :start :gap 0
      (track-header i)
      (subtree :key (str "arr-lane-" t.tid)
        (h-stack :width 0 :flex 1 :align :start :gap 0
          (track-lane i t top?))))))

;; The visible tracks' rows, keyed by track so a reorder moves them.
(def track-rows ()
  (let ((visible (eseq.track-collapse/visible-track-indices))
        (top (first visible)))
    (each visible |i|
      (let ((t (track i)))
        (if (= t nil)
          (nothing)
          (subtree :key (str "arr-track-" t.tid)
            (track-row i t (= i top))))))))

;; The step tile hides the global status line, so song-primitive rejections
;; (including "song editing is unavailable during arrangement capture")
;; surface here; the strip disappears on the next successful edit.
(def error-banner ()
  (let ((error song.edit-error))
    (if (= error "")
      (box :width 0 :height 0 :bg :transparent)
      (box :width :fill :height 1.5 :padding 0.3
        :background-color (rgba 0.32 0.13 0.12 1.0)
        (label (str "Edit rejected: " error)
          :key "edit-error-label"
          :font-size 10.5 :color (rgba 1.0 0.78 0.72 1.0) :bg :transparent)))))

;; ── Lane sync: the host's song selection -> the lanes' bound fields ────────
;; Reruns cost microseconds (a handful of field writes); the lanes that bind
;; the fields just repaint. Without it every lane subtree would read
;; song.bound-clip / song.region directly and a clip click would rebuild
;; every lane's item list. It lives as an invisible subtree INSIDE the buffer
;; (not a bare top-level effect): subtree reruns ride the widget-flush
;; machinery and cannot disturb the active buffer's layout. The song end
;; (`content-length`, which moves with the playhead past it) syncs from a
;; subtree of its own, so playback reruns only that one.
(def sync-lanes ()
  (sync-placement-target)
  (let ((c song.bound-clip)
        (r song.region)
        (region-on (and (not (= r nil)) (not (empty? r.tracks)))))
    (set! arr-lanes.bound-lane (if (= c nil) -1 c.track.index))
    (set! arr-lanes.bound-clip (if (= c nil) -1 c.cid))
    (when region-on
      (set! arr-lanes.region-lane-a (region-lane-a r))
      (set! arr-lanes.region-lane-b (region-lane-b r))
      (set! arr-lanes.region-a r.start)
      (set! arr-lanes.region-b r.end))
    (set! arr-lanes.region-on region-on)))

(def new-track-drop-zone ()
  ;; Keep a useful minimum drop target; the adjacent grid absorbs the rest
  ;; of the viewport through the enclosing flex row.
  (box :key "arr-new-track-drop-zone"
    :width :fill :height (* 3 track-lane-height)
    :background-color :buffer-bg
    :drop-hover-background-color :mixer-control-bg
    :border-width 2
    :border-color :mixer-strip-border
    :drop-hover-border-color :mixer-strip-selected-border
    :corner-radius 16
    :padding 0.5
    :align :center
    :drop-types (list "sample" "instrument" "instrument-preset" "sound")
    :drop-meta (dict :kind "new-sample-track")
    :on-drop |event| (eseq.sequencer/drop-new-track event)
    (label "Drop sounds here to add a track"
      :font-size 9.5
      :color :gray
      :bg :transparent)))

;; Rows stack with :gap 0 so the timeline instances are vertically flush —
;; the pointer is always over a lane, keeping scroll/zoom gestures captured
;; by the timelines instead of leaking to the buffer viewport.
;;
;; The compact placement toolbar shares the pinned scene header. The scene
;; lane (the arrangement's one ruler) sits OUTSIDE the track scroll container
;; so it stays pinned while track rows scroll vertically inside it.
(effect-buffer "*arrangement*"
  (v-stack :padding 0.0 :gap 0.0
    ;; Every root-level read lives inside its own subtree: a whole-list read
    ;; (like the scene lane's content-length-min reduce over every track's
    ;; clips) at the root would turn each song push into a full-buffer rerun
    ;; and defeat the per-track invalidation of the lane subtrees.
    (subtree :key "arr-lane-sync"
      (do (sync-lanes) (nothing)))
    (subtree :key "arr-content-length"
      (do (set! arr-view.content-length (content-length)) (nothing)))
    ;; The "No song yet" banner is gone (empty-arrangement spec 8): the
    ;; arrangement always exists, so there is no mode to explain.
    (subtree :key "arr-scene-context-menu"
      (scene-context-menu))
    (subtree :key "arr-placement-context-menu"
      (pattern-context-menu))
    (subtree :key "arr-error-banner"
      (error-banner))
    ;; Sound palette overlay (takes spec 17.6): opened from a clip via the
    ;; toolbar/badge gestures; renders as a centered modal over the whole
    ;; frame (modal spec §4) with zero footprint here while closed.
    (subtree :key "arr-sound-palette"
      (eseq.sound-palette/panel))
    ;; Sample import modal: same mount as *sequencer* (the two buffers never
    ;; share a tile); Rust activates whichever tile shows it before opening.
    (subtree :key "arr-export-song"
      (eseq.export-song/panel))
    (subtree :key "arr-file-dialogs"
      (eseq.file-dialogs/panel))
    (subtree :key "arr-sample-import"
      (eseq.sample-import/panel))
    (subtree :key "arr-retrospective"
      (eseq.retrospective/panel))
    (subtree :key "arr-resample"
      (eseq.resample/panel))
    (subtree :key "arr-factory-promote"
      (eseq.factory-promote/panel))
    ;; The scene row: the placement toolbar (with its hint), the starting
    ;; scene and the scene lane each re-render on their own (a selection, a
    ;; scene change, a scene-lane edit).
    (box :width :fill
      (h-stack :width :fill :align :start :gap 0
        (box :key "scene-header-spacer"
          :width header-width :height scene-lane-height
          (v-stack :width :fill :align :start :gap 0
            (subtree :key "arr-scene-row-placement"
              (placement-toolbar))
            (subtree :key "arr-scene-row-start"
              (starting-scene-control))
            (subtree :key "arr-scene-row-hint"
              (placement-hint))))
        (subtree :key "arr-scene-row-lane"
          (h-stack :width 0 :flex 1 :align :start :gap 0
            (scene-lane)))))
    (scroll :key "track-scroll" :width :fill :flex 1
      (v-stack :width :fill :height :fill :gap 0.0
        (track-rows)
        ;; New-track drop zone (the *sequencer* buffer's idiom): the mixer is
        ;; hidden by default in this view, so without this there is no place
        ;; to drop a sample / instrument / Sound to add a track.
        (h-stack :key "arr-grid-continuation-row"
          :width :fill :flex 1 :align :stretch :gap 0
          (box :width header-width (new-track-drop-zone))
          (continuation-lane "arr-grid-continuation" 0 0))))
    (h-stack :key "arr-bottom-ruler-row" :gap 0 :width :fill :align :start
      (box :width header-width :height 1)
      ;; Footer ruler: total height 1 cell, all of it header/ruler, so the
      ;; lane's content rect below the ruler is zero cells tall. The timeline
      ;; widget tolerates that degenerate content rect and simply draws the
      ;; ruler; there are no items or grid rows to lay out underneath it.
      (continuation-lane "arr-bottom-ruler" 1 1))))

;; Arrangement-local keyboard commands belong to the arrangement mode, not
;; the host event loop. Backspace/Delete are the exception: their priority
;; between marquee regions, focused clips, and selected steps stays in Rust.
(def arrangement-place-key ()
  (begin-placement)
  true)

(def arrangement-region-copy-key ()
  (if (= song.region nil)
    false
    (do (seq-song-region-copy) true)))

(def arrangement-region-paste-key ()
  (seq-song-region-paste)
  true)

(def arrangement-region-duplicate-key ()
  (if (= song.region nil)
    false
    (do (seq-song-region-duplicate) true)))

(define-mode "arrangement-mode" :read-only true :live-keys true
  :inherit "eseq.sequencer-keys/sequencer-keys")
;; Super is the macOS primary modifier; Ctrl is the primary modifier elsewhere.
;; Register both platform spellings so the authored UI remains portable.
(mode-bind-key "arrangement-mode" "s-p" "arrangement-place-key")
(mode-bind-key "arrangement-mode" "C-p" "arrangement-place-key")
(mode-bind-key "arrangement-mode" "s-c" "arrangement-region-copy-key")
(mode-bind-key "arrangement-mode" "C-S-c" "arrangement-region-copy-key")
(mode-bind-key "arrangement-mode" "s-v" "arrangement-region-paste-key")
(mode-bind-key "arrangement-mode" "C-S-v" "arrangement-region-paste-key")
(mode-bind-key "arrangement-mode" "s-d" "arrangement-region-duplicate-key")
(mode-bind-key "arrangement-mode" "C-d" "arrangement-region-duplicate-key")
(set-buffer-mode-for "*arrangement*" "arrangement-mode")
