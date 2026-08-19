;; ui/drum-rack-v2.lisp — Drum rack v2 lookups over SEQ.groups.
;;
;; A drum rack is a track group with a pad map (docs/drum-rack-v2-spec.md), so
;; every rack question is a question about SEQ.groups: which group (if any) a
;; track belongs to, which members are visible, and where the rack sits in the
;; grid's render order. Rendering lives with the widgets it uses — the grid
;; header/member rows in ui/sequencer.lisp, the group strip in ui/mixer.lisp —
;; and both read their structure from here.

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

;; Lowest member index — where the rack sits in the flat track order. A rack
;; with no members yet (lazy pads) reports -1 and is rendered after the tracks.
(def anchor (gidx)
  (let ((ms (members gidx)))
    (if (= (len ms) 0) -1 (get (group-at gidx) :anchor))))

;; Index (in SEQ.groups) of the RACK containing track i, else -1. Plain groups
;; are deliberately invisible here: the grid leaves their tracks loose.
(def rack-of-track (i)
  (reduce |acc gidx|
    (if (>= acc 0)
      acc
      (if (and (rack? gidx) (%contains? (members gidx) i)) gidx acc))
    -1
    (range 0 (len SEQ.groups))))

(def rack-member? (i)
  (>= (rack-of-track i) 0))

;; Members the grid draws: collapsed member tracks hide exactly as loose ones do.
(def visible-members (gidx)
  (filter
    (lambda (m) (not (eseq.track-collapse/collapsed? m)))
    (members gidx)))

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
;; Loose tracks stay in track order; a rack collapses its member run into one
;; "rack" item anchored at its lowest member, so members render nested under
;; the rack header instead of as siblings. Empty racks (no pad has claimed a
;; track yet) have no anchor and follow the tracks so the kit is still visible.

(def %empty-rack-items ()
  (reduce |acc gidx|
    (if (and (rack? gidx) (= (len (members gidx)) 0))
      (append acc (list (dict :kind "rack" :gidx gidx)))
      acc)
    (list)
    (range 0 (len SEQ.groups))))

(def grid-render-items ()
  (append
    (reduce |acc i|
      (let ((gidx (rack-of-track i)))
        (if (>= gidx 0)
          (if (= i (anchor gidx))
            (append acc (list (dict :kind "rack" :gidx gidx)))
            acc)
          (if (eseq.track-collapse/collapsed? i)
            acc
            (append acc (list (dict :kind "track" :track i))))))
      (list)
      (range 0 SEQ.num-tracks))
    (%empty-rack-items)))

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
  (max 0 (min 127 note)))

;; Move a pad by a semitone. The host rejects a collision with another pad and
;; leaves the map alone, so the badge simply does not move.
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

;; Kit = group config + one Sound per pad, saved as a browser object. Patterns
;; stay behind (docs/drum-rack-v2-spec.md, "Polish").
(def save-kit (gidx)
  (host-command "save-rack-as-kit"
    (dict :group-id (group-id gidx) :name (group-name gidx) :overwrite true)))

;; ── Pad grid geometry (docs/drum-rack-v2-spec.md, "UI") ─────────────────
;; The 4x4 pad grid is NOT a window onto the pad vec: a cell IS a fixed MIDI
;; note, and a pad renders at the cell its `pad_note` names (empty everywhere
;; else). A page shows sixteen consecutive notes with the LOWEST bottom-left,
;; ascending left-to-right then bottom-to-top, the way a drum rack reads
;; everywhere else. Pages are octave-aligned — page k starts at note 12k — so
;; the bottom-left cell of every page is a C; the price is a four-note overlap
;; between adjacent pages, which is exactly what a plain 16-note stride cannot
;; buy. The top page is clamped so no cell ever names a note above 127.

(def %note-names '("C" "C#" "D" "D#" "E" "F" "F#" "G" "G#" "A" "A#" "B"))

;; Note name as the host writes pad labels (`drum_rack_pad_label`): note 0 is C4.
(def note-label (note)
  (let ((n (%clamp-pad-note note)))
    (str (nth %note-names (mod n 12)) (+ 4 (floor (/ n 12))))))

;; Pages 0..9: page 9 spans notes 108..123, and 12*10+15 would run past 127.
(def pad-page-count () 10)

(def clamp-pad-page (page)
  (max 0 (min (- (pad-page-count) 1) page)))

;; Lowest note of a page — always a C.
(def pad-page-base (page)
  (* 12 (clamp-pad-page page)))

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

;; The page an empty rack opens on: the drum home (note 36, the GM kick).
(def %empty-rack-home-note () 36)

;; Where a rack's grid opens: the page holding its lowest pad, so a kit that
;; lives at C7 does not open onto empty octaves.
(def default-pad-page (gidx)
  (let ((ps (pads gidx)))
    (if (= (len ps) 0)
      (page-of-note (%empty-rack-home-note))
      (page-of-note (reduce |acc pad| (min acc (get pad :pad-note)) 127 ps)))))
