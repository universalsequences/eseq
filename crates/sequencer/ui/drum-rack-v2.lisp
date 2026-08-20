;; ui/drum-rack-v2.lisp — Track-group and drum-rack lookups over SEQ.groups.
;;
;; A drum rack is a track group with a pad map (docs/drum-rack-v2-spec.md).
;; Shared group topology here determines membership, nesting and grid render
;; order; rack-only helpers add the pad map and arming behavior. Rendering lives
;; with the widgets it uses — the grid header/member rows in ui/sequencer.lisp
;; and the group strip in ui/mixer.lisp.

(module eseq.drum-rack-v2)

(import eseq.track-collapse)

(def %contains? (xs v)
  (> (len (filter (lambda (x) (= x v)) xs)) 0))

(def group-at (gidx)
  (nth SEQ.groups gidx))

;; A rack group is a group carrying a pad map; a plain mixer group is not.
(def rack? (gidx)
  (and (>= gidx 0)
    (< gidx (len SEQ.groups))
    (get (group-at gidx) :rack)))

(def members (gidx)
  (get (group-at gidx) :members))

(def collapsed? (gidx)
  (get (group-at gidx) :collapsed))

(def group-name (gidx)
  (get (group-at gidx) :name))

(def group-id (gidx)
  (get (group-at gidx) :id))

(def color (gidx)
  (let ((c (get (group-at gidx) :color)))
    (if (>= (len c) 3) c (list 0.5 0.5 0.5))))

;; Lowest direct member index — where the group sits in flat track order. A
;; group with no direct members reports -1; group-anchor also considers child
;; racks, while a truly empty lazy rack is rendered after the tracks.
(def anchor (gidx)
  (let ((ms (members gidx)))
    (if (= (len ms) 0) -1 (get (group-at gidx) :anchor))))

;; Index (in SEQ.groups) of the group containing track i, else -1.
(def group-of-track (i)
  (reduce |acc gidx|
    (if (>= acc 0)
      acc
      (if (%contains? (members gidx) i) gidx acc))
    -1
    (range 0 (len SEQ.groups))))

;; Index (in SEQ.groups) of the rack containing track i, else -1.
(def rack-of-track (i)
  (let ((gidx (group-of-track i)))
    (if (and (>= gidx 0) (rack? gidx)) gidx -1)))

(def rack-member? (i)
  (>= (rack-of-track i) 0))

;; Members the grid draws: collapsed member tracks hide exactly as loose ones do.
(def visible-members (gidx)
  (filter
    (lambda (m) (not (eseq.track-collapse/collapsed? m)))
    (members gidx)))

;; Group nesting currently permits a regular group to contain drum racks. The
;; host publishes both directions: :rack-members on the parent and :parent on
;; the child. Nested racks are rendered by their parent, never at top level.
(def group-index-by-id (gid)
  (reduce |acc gidx|
    (if (>= acc 0)
      acc
      (if (= (group-id gidx) gid) gidx acc))
    -1
    (range 0 (len SEQ.groups))))

(def nested? (gidx)
  (let ((parent (get (group-at gidx) :parent)))
    (if parent (>= parent 0) false)))

(def child-racks (gidx)
  (filter (lambda (child) (>= child 0))
    (map (lambda (gid) (group-index-by-id gid))
      (or (get (group-at gidx) :rack-members) (list)))))

;; Lowest track owned by this group or by a rack nested in it.
(def group-anchor (gidx)
  (reduce |acc child|
    (let ((child-anchor (anchor child)))
      (if (< acc 0)
        child-anchor
        (if (< child-anchor 0) acc (min acc child-anchor))))
    (anchor gidx)
    (child-racks gidx)))

;; Storage index of the group's backing bus in the SEQ.bus-* lists, or -1.
(def bus-index (gidx)
  (let ((bid (get (group-at gidx) :bus-id)))
    (reduce |acc i|
      (if (>= acc 0) acc (if (= (nth SEQ.bus-ids i) bid) i acc))
      -1
      (range 0 (len SEQ.bus-ids)))))

;; Index (in SEQ.groups) of the rack backed by bus `bus-idx`, else -1. Selecting
;; a rack selects its bus (ui/sequencer.lisp, %select-rack), so this is how a
;; bus-driven surface — the *fx* buffer — asks "is this selection a kit?".
(def rack-of-bus (bus-idx)
  (if (< bus-idx 0)
    -1
    (reduce |acc gidx|
      (if (>= acc 0)
        acc
        (if (and (rack? gidx) (= (bus-index gidx) bus-idx)) gidx acc))
      -1
      (range 0 (len SEQ.groups)))))

;; ── Rack arming ─────────────────────────────────────────────────────────
;; Which rack the live keyboard plays as pads (SEQ.armed-rack-id, -1 = none).
;; The host owns this: `seq-toggle-rack-arm` flips it, disarms the rack's own
;; member tracks, and the live-keyboard path routes note->pad->member track.

(def armed? (gidx)
  (and (>= gidx 0) (= SEQ.armed-rack-id (group-id gidx))))

(def toggle-armed (gidx)
  (seq-toggle-rack-arm (group-id gidx)))

(def toggle-collapsed (gidx)
  (seq-toggle-group-collapsed (group-id gidx)))

;; ── Grid render order ───────────────────────────────────────────────────
;; Loose tracks stay in track order. Every top-level group collapses its member
;; run into one item anchored at its lowest member, so regular groups and drum
;; racks use the same nested block model. Unanchored groups follow the tracks;
;; this keeps empty, lazy drum racks visible.

(def group-anchored-at (track)
  (reduce |acc gidx|
    (if (>= acc 0)
      acc
      (if (and (not (nested? gidx)) (= (group-anchor gidx) track)) gidx acc))
    -1
    (range 0 (len SEQ.groups))))

(def %unanchored-group-items ()
  (reduce |acc gidx|
    (if (and (not (nested? gidx)) (< (group-anchor gidx) 0))
      (append acc (list (dict :kind "group" :gidx gidx)))
      acc)
    (list)
    (range 0 (len SEQ.groups))))

(def %grid-render-items (include-collapsed-tracks)
  (append
    (reduce |acc i|
      (let ((gidx (group-anchored-at i)))
        (if (>= gidx 0)
          (append acc (list (dict :kind "group" :gidx gidx)))
          (if (>= (group-of-track i) 0)
            acc
            (if (and (not include-collapsed-tracks)
                     (eseq.track-collapse/collapsed? i))
              acc
              (append acc (list (dict :kind "track" :track i)))))))
      (list)
      (range 0 SEQ.num-tracks))
    (%unanchored-group-items)))

(def grid-render-items ()
  (%grid-render-items false))

;; Flatten a group in exactly the order `%group-block` draws it: direct members
;; first, then each nested rack. The structural form includes hidden rows and is
;; used only to locate a selection that has become invisible.
(def %group-track-order (gidx respect-group-collapse hide-collapsed-tracks)
  (if (and respect-group-collapse (collapsed? gidx))
    (list)
    (reduce |acc child|
      (append acc
        (%group-track-order child respect-group-collapse hide-collapsed-tracks))
      (if hide-collapsed-tracks (visible-members gidx) (members gidx))
      (child-racks gidx))))

(def %flatten-track-order (items respect-group-collapse hide-collapsed-tracks)
  (reduce |acc item|
    (append acc
      (if (= (get item :kind) "track")
        (list (get item :track))
        (%group-track-order
          (get item :gidx) respect-group-collapse hide-collapsed-tracks)))
    (list)
    items))

;; Selectable sequencer track rows in their rendered order. Group headers and
;; buses are deliberately absent: they retain their click-only selection path.
(def visible-track-order ()
  (%flatten-track-order (grid-render-items) true true))

;; The mixer shares the same group topology but still draws individually
;; collapsed tracks as narrow badges. Only a collapsed group removes tracks
;; from its visible order.
(def mixer-visible-track-order ()
  (%flatten-track-order (%grid-render-items true) true false))

(def %index-of (xs value)
  (reduce |found i|
    (if (>= found 0) found (if (= (nth xs i) value) i found))
    -1
    (range 0 (len xs))))

;; Return the adjacent visible track in `delta`'s direction. When `track` is
;; hidden, walk from its position in the same structural row order until a
;; visible row is found. This handles collapsed groups, collapsed loose tracks,
;; and nested racks without reconstructing any ordering in the host.
(def track-relative (track delta)
  (let ((visible (visible-track-order)))
    (if (= (len visible) 0)
      nil
      (let ((visible-pos (%index-of visible track)))
        (if (>= visible-pos 0)
          (nth visible (mod (+ visible-pos delta (len visible)) (len visible)))
          (let ((structural
                  (%flatten-track-order (%grid-render-items true) false false)))
            (let ((structural-pos (%index-of structural track)))
              (if (< structural-pos 0)
                nil
                (reduce |found distance|
                  (if (= found nil)
                    (let ((candidate
                            (nth structural
                              (mod (+ structural-pos (* delta distance) (len structural))
                                   (len structural)))))
                      (if (%contains? visible candidate) candidate found))
                    found)
                  nil
                  (range 1 (+ (len structural) 1)))))))))))

;; ── Pad map (slice 6 polish) ────────────────────────────────────────────
;; The pad map is the rack's whole curation layer: an ordered list of pads,
;; each naming a MIDI note, the member track behind it and its choke group.
;; Every pad-facing control below reads it and writes back through a host
;; command keyed by (group id, pad note) — never by track index, which moves
;; under track delete/reindex.

(def pads (gidx)
  (get (group-at gidx) :pads))

;; The pad backing a member track. Every member gets a pad when it joins the
;; rack, so nil is only the defensive case (a project the load repair could not
;; map because the rack was full).
(def pad-of-track (gidx track)
  (reduce |acc pad|
    (if (= acc nil) (if (= (get pad :track) track) pad acc) acc)
    nil
    (pads gidx)))

;; Note-name badge for a member row ("" when the member has no pad).
(def pad-label-of-track (gidx track)
  (let ((pad (pad-of-track gidx track)))
    (if (= pad nil) "" (get pad :label))))

;; Choke group of a member's pad: -1 when unassigned or padless.
(def choke-of-track (gidx track)
  (let ((pad (pad-of-track gidx track)))
    (if (= pad nil) -1 (get pad :choke))))

(def %clamp-pad-note (note)
  (max (min-grid-pad-note) (min (max-grid-pad-note) note)))

;; Move a pad by a semitone. The host rejects a collision with another pad and
;; leaves the map alone, so the badge simply does not move. Nudges clamp to the
;; note-positional grid's range, not raw MIDI: notes above the top page's last
;; cell exist but no page can show them, and a nudge must never strand a pad
;; where the grid cannot render it.
(def nudge-pad-note (gidx pad delta)
  (let ((note (%clamp-pad-note (+ (get pad :pad-note) delta))))
    (if (= note (get pad :pad-note))
      nil
      (host-command "set-rack-pad-note"
        (dict :group-id (group-id gidx)
              :pad-note (get pad :pad-note)
              :note note)))))

(def choke-options ()
  (list "Off" "1" "2" "3" "4" "5" "6" "7" "8" "9" "10" "11" "12" "13" "14" "15" "16"))

;; Dropdown index for a pad's choke value: 0 = Off, otherwise the group number.
(def choke-value-index (choke)
  (if (< choke 0) 0 choke))

(def %choke-value-from-label (label)
  (let ((opts (choke-options)))
    (reduce |acc i| (if (= (nth opts i) label) i acc) 0 (range 0 (len opts)))))

(def set-pad-choke (gidx pad label)
  (host-command "set-rack-pad-choke-group"
    (dict :group-id (group-id gidx)
          :pad-note (get pad :pad-note)
          :value (%choke-value-from-label label))))

;; A pad-grid hit takes the same live path a pad key takes: the pad's member
;; track at base pitch, so choke groups and the member's fx chain apply.
(def trigger-pad (gidx pad)
  (host-command "trigger-rack-pad"
    (dict :group-id (group-id gidx) :pad-note (get pad :pad-note))))

;; ── Pad grid geometry (docs/drum-rack-v2-spec.md, "UI") ─────────────────
;; The 4x4 pad grid is NOT a window onto the pad vec: a cell IS a fixed pad
;; note, and a pad renders at the cell its `pad_note` names (empty everywhere
;; else). A page shows sixteen consecutive notes with the LOWEST bottom-left,
;; ascending left-to-right then bottom-to-top, the way a drum rack reads
;; everywhere else. Pages are octave-aligned — page k starts at note 12k — so
;; the bottom-left cell of every page is a C; the price is a four-note overlap
;; between adjacent pages, which is exactly what a plain 16-note stride cannot
;; buy. Pages are clamped to the host's pad-note domain, C1 (page -3) up to
;; D#8 (page 3), so no cell ever names a note no pad could be placed on.

(def %note-names '("C" "C#" "D" "D#" "E" "F" "F#" "G" "G#" "A" "A#" "B"))

;; Note name as the host writes pad labels (`drum_rack_pad_label`). A pad note
;; is a TRANSPOSE, the same one the step sequencer and piano roll speak, so 0
;; is C4 and notes below middle C are negative — hence the euclidean remainder
;; rather than a bare `mod`, which would index the name table backwards.
(def note-label (note)
  (let ((n (%clamp-pad-note note)))
    (str (nth %note-names (mod (+ (mod n 12) 12) 12)) (+ 4 (floor (/ n 12))))))

;; Pages -3..3, mirroring DRUM_RACK_FIRST_PAD_NOTE/DRUM_RACK_LAST_PAD_NOTE: the
;; bottom page starts at C1 (-36), the drum rack's home octave, and the top one
;; spans C8..D#8 (36..51). C4 — transpose 0 — therefore sits in the MIDDLE of
;; the pad space, where a drum rack's notes actually live, instead of at its
;; floor.
(def min-pad-page () -3)
(def max-pad-page () 3)

(def pad-page-count ()
  (+ (- (max-pad-page) (min-pad-page)) 1))

(def clamp-pad-page (page)
  (max (min-pad-page) (min (max-pad-page) page)))

;; Lowest note of a page — always a C.
(def pad-page-base (page)
  (* 12 (clamp-pad-page page)))

;; The notes the grid can name: the bottom page's C (C1) up to the top page's
;; top-right cell (D#8). Pad-placing UI paths clamp here, which is also the
;; host's pad-note domain (DRUM_RACK_FIRST_PAD_NOTE..DRUM_RACK_LAST_PAD_NOTE),
;; so nothing can be placed where no page could render it.
(def min-grid-pad-note ()
  (* 12 (min-pad-page)))

(def max-grid-pad-note ()
  (+ (* 12 (max-pad-page)) 15))

;; The page a note is drawn on. Overlap means a note can also appear in the
;; top row of the page below; this names the canonical one.
(def page-of-note (note)
  (clamp-pad-page (floor (/ (%clamp-pad-note note) 12))))

;; Cell -> note, a pure function of (page, cell). Cell 0 is the TOP-left cell
;; of the rendered grid, so row 0 carries the page's highest four notes.
(def cell-note (page cell)
  (+ (pad-page-base page)
    (+ (* 4 (- 3 (floor (/ cell 4)))) (mod cell 4))))

(def pad-page-label (page)
  (str (note-label (pad-page-base page)) "–" (note-label (+ (pad-page-base page) 15))))

;; The pad answering to a note, or nil — how a grid cell finds what to draw.
(def pad-at-note (gidx note)
  (reduce |acc pad| (if (= (get pad :pad-note) note) pad acc)
    nil
    (pads gidx)))

;; The page an empty rack opens on: the drum home, C1 — the bottom of the pad
;; space and where a rack's first pad lands (DRUM_RACK_FIRST_PAD_NOTE).
(def %empty-rack-home-note () (min-grid-pad-note))

;; Where a rack's grid opens: the page holding its lowest pad, so a kit that
;; lives at C7 does not open onto empty octaves.
(def default-pad-page (gidx)
  (let ((ps (pads gidx)))
    (if (= (len ps) 0)
      (page-of-note (%empty-rack-home-note))
      (page-of-note (reduce |acc pad| (min acc (get pad :pad-note))
        (max-grid-pad-note) ps)))))

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
