;; ui/drum-rack-v2.lisp — Track-group and drum-rack lookups over the group
;; kinds (`(groups)`, `g.tracks`, `g.racks`, `g.clips`, `g.groove`, …).
;;
;; A drum rack is a track group with a pad map (docs/drum-rack-v2-spec.md).
;; Shared group topology here determines nesting and the grid's and mixer's
;; render order; rack-only helpers add the pad geometry and the groove a rack
;; plays. Rendering lives with the widgets it uses — the grid header/member
;; rows and the pad grid in ui/sequencer.lisp, the group strip in
;; ui/mixer.lisp, the groove in ui/rack-groove-buffer.lisp.
;;
;; Pure lookups: no state of its own, no buffers, no host calls at import,
;; so the effects manifest (ui/effects/buffers.lisp) can import it on its
;; own (`selected-bus-rack` reads eseq.seq-core-state's selected bus, spelled
;; qualified, when called).

(module eseq.drum-rack-v2)

(import eseq.kinds :refer (tracks groups project))
(import eseq.view-kit :refer (index-of listed?))

(export rack-of-bus
        selected-bus-rack
        rack-of-track
        render-items
        shown-members
        visible-track-order
        mixer-visible-track-order
        track-relative
        scene-plays-clip?
        pad-role-options
        note-label
        clamp-pad-note
        clamp-pad-page
        min-grid-pad-note
        max-grid-pad-note
        page-of-note
        cell-note
        pad-map-row-count
        pad-map-row-base
        pad-map-row-on-page?
        playing-groove
        groove-of-track
        groove-off-label
        groove-extract-label
        groove-reserved-labels
        pool-groove-labels
        pool-groove-label)

;; The drum rack backed by the bus at position `bus-idx` (what a bus-driven
;; surface, the *fx* buffer, asks: "is this selection a kit?"), else nil.
;; Selecting a rack selects its bus (ui/sequencer.lisp, select-group).
(def rack-of-bus (bus-idx)
  (first (filter (lambda (g) (and g.rack g.bus (= g.bus.index bus-idx))) (groups))))

;; The drum rack behind the selected bus, else nil (no bus selected: -1).
(def selected-bus-rack ()
  (rack-of-bus eseq.seq-core-state/selected-bus))

;; The drum rack track t is a member of, else nil.
(def rack-of-track (t)
  (let ((g t.group))
    (when (and g g.rack) g)))

;; ── Render order ────────────────────────────────────────────────────────
;; Loose tracks stay in track order. Every top-level group collapses its member
;; run into one item anchored at its lowest member, so regular groups and drum
;; racks use the same nested block model. Unanchored groups (an empty, lazy
;; drum rack) follow the tracks. A group drawn inside another's block (a rack
;; in a plain group, `g.parent`) is its parent's to draw.

;; The lowest position among tracks, or -1 for none.
(def lowest-index (ts)
  (reduce |acc t| (if (or (< acc 0) (< t.index acc)) t.index acc) -1 ts))

;; Where a group sits in track order: its lowest member, or that of a rack
;; drawn inside it; -1 before it claims a track.
(def group-anchor (g)
  (reduce |acc child|
    (let ((a (lowest-index child.tracks)))
      (if (< acc 0) a (if (< a 0) acc (min a acc))))
    (lowest-index g.tracks)
    g.racks))

;; The top-level rows in order: `(dict :kind "track" :track t)` for a loose
;; track, `(dict :kind "group" :group g)` for a top-level group. The grid
;; leaves collapsed loose tracks out; the mixer (`collapsed-tracks` true)
;; draws them as narrow badges.
(def render-items (collapsed-tracks)
  (let ((top (filter (lambda (g) (not g.parent)) (groups)))
        (anchored (map (lambda (g) (dict :group g :anchor (group-anchor g))) top))
        (group-item (lambda (ga) (dict :kind "group" :group (get ga :group)))))
    (append
      (reduce |acc t|
        (let ((hit (first (filter (lambda (ga) (= (get ga :anchor) t.index)) anchored))))
          (if hit
            (append acc (list (group-item hit)))
            (if (or t.group (and t.collapsed (not collapsed-tracks)))
              acc
              (append acc (list (dict :kind "track" :track t))))))
        (list)
        (tracks))
      (map group-item (filter (lambda (ga) (< (get ga :anchor) 0)) anchored)))))

;; Group g's member tracks the grid shows: collapsed ones hide exactly as
;; loose ones do.
(def shown-members (g)
  (filter (lambda (m) (not m.collapsed)) g.tracks))

;; Group g's track rows in the order its block draws them: its members, then
;; each rack drawn inside it. `respect-collapse`: none while g is collapsed;
;; `hide-collapsed`: collapsed members left out. With neither, the structural
;; order, hidden rows included (where a hidden selection sits).
(def group-track-order (g respect-collapse hide-collapsed)
  (if (and respect-collapse g.collapsed)
    (list)
    (reduce |acc child|
      (append acc (group-track-order child respect-collapse hide-collapsed))
      (if hide-collapsed (shown-members g) g.tracks)
      g.racks)))

(def flatten-track-order (items respect-collapse hide-collapsed)
  (reduce |acc item|
    (append acc
      (if (= (get item :kind) "group")
        (group-track-order (get item :group) respect-collapse hide-collapsed)
        (list (get item :track))))
    (list)
    items))

;; The grid's selectable track rows in their rendered order (the shift-click
;; range, the UP / DOWN keys). Group headers and buses are deliberately
;; absent: they retain their click-only selection path.
(def visible-track-order ()
  (flatten-track-order (render-items false) true true))

;; The mixer's: individually collapsed tracks still draw (as narrow badges);
;; only a collapsed group removes tracks from its visible order.
(def mixer-visible-track-order ()
  (flatten-track-order (render-items true) true false))

;; The visible track `delta` rows from track t, wrapping, or nil when no row
;; is visible. When t is hidden, walk from its position in the structural row
;; order until a visible row is found: collapsed groups, collapsed loose
;; tracks and nested racks need no ordering of their own.
(def track-relative (t delta)
  (let ((visible (visible-track-order))
        (shown (len visible)))
    (if (empty? visible)
      nil
      (let ((at (index-of visible t)))
        (if (>= at 0)
          (nth visible (mod (+ at delta shown) shown))
          (let ((structural (flatten-track-order (render-items true) false false))
                (size (len structural))
                (from (index-of structural t)))
            (if (< from 0)
              nil
              (reduce |found distance|
                (or found
                    (let ((candidate (nth structural (mod (+ from (* delta distance) size) size))))
                      (when (listed? candidate visible) candidate)))
                nil
                (range 1 (+ size 1))))))))))

;; ── Rack clips (docs/rack-clips-and-break-kits-spec.md §2-§4, §6) ────────
;; Whether rack g plays something in scene s: a clip of its bank names the
;; scene. A LEGACY rack (no bank, `g.legacy`, §4.3) keeps its members in the
;; project scenes, so it plays in every one (the kit export drops the scenes
;; it finds empty).
(def scene-plays-clip? (g s)
  (or g.legacy
      (reduce |found rc| (or found (listed? s rc.scenes)) false g.clips)))

;; ── Pad roles (docs/rack-groove-spec.md, "Pad roles") ──────────────────
;; The role menu's entries: eseq.kinds' `pad-role-options` (the `PadRole::ALL`
;; keys, in menu order) with their labels; a test keeps the two in sync.
(def pad-role-options ()
  (list
    (dict :key "kick" :label "Kick")
    (dict :key "snare" :label "Snare")
    (dict :key "rim" :label "Rim")
    (dict :key "clap" :label "Clap")
    (dict :key "closed-hat" :label "Closed Hat")
    (dict :key "pedal-hat" :label "Pedal Hat")
    (dict :key "open-hat" :label "Open Hat")
    (dict :key "tom-low" :label "Low Tom")
    (dict :key "tom-mid" :label "Mid Tom")
    (dict :key "tom-high" :label "High Tom")
    (dict :key "crash" :label "Crash")
    (dict :key "ride" :label "Ride")
    (dict :key "shaker" :label "Shaker")
    (dict :key "perc" :label "Perc")))

;; ── Pad grid geometry (docs/drum-rack-v2-spec.md, "UI") ─────────────────
;; The 4x4 pad grid is NOT a window onto the pad list: a cell IS a fixed pad
;; note, and a pad renders at the cell its `p.note` names (empty everywhere
;; else). A page shows sixteen consecutive notes with the LOWEST bottom-left,
;; ascending left-to-right then bottom-to-top, the way a drum rack reads
;; everywhere else. Pages are octave-aligned — page k starts at note 12k — so
;; the bottom-left cell of every page is a C; the price is a four-note overlap
;; between adjacent pages, which is exactly what a plain 16-note stride cannot
;; buy. Pages are clamped to the host's pad-note domain, C1 (page -3) up to
;; D#8 (page 3), so no cell ever names a note no pad could be placed on.

(def note-names '("C" "C#" "D" "D#" "E" "F" "F#" "G" "G#" "A" "A#" "B"))

;; Pages -3..3, mirroring DRUM_RACK_FIRST_PAD_NOTE/DRUM_RACK_LAST_PAD_NOTE: the
;; bottom page starts at C1 (-36), the drum rack's home octave, and the top one
;; spans C8..D#8 (36..51). C4 — transpose 0 — therefore sits in the MIDDLE of
;; the pad space, where a drum rack's notes actually live, instead of at its
;; floor.
(def min-pad-page () -3)
(def max-pad-page () 3)

(def clamp-pad-page (page)
  (max (min-pad-page) (min (max-pad-page) page)))

;; Lowest note of a page — always a C.
(def pad-page-base (page)
  (* 12 (clamp-pad-page page)))

;; The notes the grid can name: the bottom page's C (C1) up to the top page's
;; top-right cell (D#8). This is the host's pad-note domain (`p.note`'s range,
;; DRUM_RACK_FIRST_PAD_NOTE..DRUM_RACK_LAST_PAD_NOTE), so nothing can be
;; placed where no page could render it.
(def min-grid-pad-note ()
  (* 12 (min-pad-page)))

(def max-grid-pad-note ()
  (+ (* 12 (max-pad-page)) 15))

(def clamp-pad-note (note)
  (max (min-grid-pad-note) (min (max-grid-pad-note) note)))

;; Note name as the host writes pad labels (`p.label`). A pad note is a
;; TRANSPOSE, the same one the step sequencer and piano roll speak, so 0 is C4
;; and notes below middle C are negative — hence the euclidean remainder
;; rather than a bare `mod`, which would index the name table backwards.
(def note-label (note)
  (let ((n (clamp-pad-note note)))
    (str (nth note-names (mod (+ (mod n 12) 12) 12)) (+ 4 (floor (/ n 12))))))

;; The page a note is drawn on. Overlap means a note can also appear in the
;; top row of the page below; this names the canonical one.
(def page-of-note (note)
  (clamp-pad-page (floor (/ (clamp-pad-note note) 12))))

;; Cell -> note, a pure function of (page, cell). Cell 0 is the TOP-left cell
;; of the rendered grid, so row 0 carries the page's highest four notes.
(def cell-note (page cell)
  (+ (pad-page-base page)
    (+ (* 4 (- 3 (floor (/ cell 4)))) (mod cell 4))))

;; ── Octave overview geometry (eseq-4b5.15) ──────────────────────────────
;; The mini-map beside the pad grid lays the WHOLE grid-addressable note range
;; out four notes to a row — the same four-wide reading order the pads use —
;; with the lowest notes at the bottom. It is a second view of the page state
;; above, never a second state: which rows light up is derived from the page
;; the grid is already showing.

;; Rows of four covering the whole pad-note domain (C1..D#8): 22 rows, so every
;; cell of the map names a note some page can actually render, C1 is the bottom
;; row and C4 lands on the middle one.
(def pad-map-row-count ()
  (floor (/ (+ (- (max-grid-pad-note) (min-grid-pad-note)) 1) 4)))

;; Lowest note of a map row. Row 0 renders at the TOP and carries the highest
;; four notes, matching the grid's bottom-up reading.
(def pad-map-row-base (row)
  (+ (min-grid-pad-note) (* 4 (- (- (pad-map-row-count) 1) row))))

;; Whether a map row's four notes fall inside a page's sixteen-note window —
;; what draws the highlighted block.
(def pad-map-row-on-page? (row page)
  (let ((base (pad-map-row-base row))
      (page-base (pad-page-base page)))
    (and (>= base page-base) (< base (+ page-base 16)))))

;; ── Rack grooves (docs/rack-groove-spec.md, "UI") ───────────────────────
;; A groove is an extracted feel, applied wherever a trig aimed at a pad
;; becomes a sample time. Grooves live in the project groove pool
;; (`project.groove-pool`); a rack's groove (`g.groove`, a clip's own
;; `rc.groove`) points at one. The view is the *groove* buffer
;; (ui/rack-groove-buffer.lisp).

;; The groove rack g plays now: the playing clip's own when it has one, else
;; the rack's.
(def playing-groove (g)
  (let ((rc g.rack-clip))
    (or (and rc rc.groove) g.groove)))

;; The groove member track t plays through: its rack's, or nil when t is
;; loose or its rack plays straight (no groove picked, or its switch off).
;; The scheduler replaces the track's swing with it.
(def groove-of-track (t)
  (let ((g (rack-of-track t)))
    (when g
      (let ((gr (playing-groove g)))
        (when (and gr gr.pool-groove gr.enabled) gr)))))

;; The groove picker's fixed rows: no groove, the section headers and the
;; footer's action (ui/rack-groove-buffer.lisp).
(def groove-off-label "No groove")
(def groove-extract-label "Extract from this rack’s clip…")
(def groove-reserved-labels
  (list groove-off-label "This project" "Factory" "Library" groove-extract-label))

;; The picker labels of the pool's grooves, in pool order. Labels are unique,
;; so a picked label names one groove: a pool groove's own name unless it is
;; empty, taken by an earlier one or spelled like a fixed row, else its name
;; and id. The library's rows follow the pool's, so these never depend on them.
(def pool-groove-labels ()
  (reduce |labels pg|
    (append labels
      (list
        (if (or (empty? pg.name) (listed? pg.name labels)
                (listed? pg.name groove-reserved-labels))
          (str pg.name " #" pg.groove-id)
          pg.name)))
    (list)
    project.groove-pool))

;; Pool groove pg's picker label ("" when it is not in the pool).
(def pool-groove-label (pg)
  (let ((at (index-of project.groove-pool pg)))
    (if (< at 0) "" (nth (pool-groove-labels) at))))
