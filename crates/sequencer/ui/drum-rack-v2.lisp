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

;; ── Lazy pads (docs/drum-rack-v2-spec.md, "Track budget") ───────────────
;; "A pad only claims a track when a sound is dropped on it": an empty cell in
;; the pad grid is a drop target, and the note it claims comes from its grid
;; position. The pad map is dense — grid position IS the pad index — so an
;; empty cell always sits past the end of the map and the note its index names
;; is normally free. Pad notes are nudgeable though, so a taken candidate falls
;; back to the lowest free note rather than handing the host a collision.

(def %pad-note-free? (gidx note)
  (= (len (filter (lambda (p) (= (get p :pad-note) note)) (pads gidx))) 0))

;; Lowest note no pad answers to yet, or -1 for a full rack. Mirrors the host's
;; `ProjectRackConfig::next_free_pad_note`.
(def %next-free-pad-note (gidx)
  (reduce |acc n| (if (>= acc 0) acc (if (%pad-note-free? gidx n) n acc))
    -1
    (range 0 128)))

;; The pad note an empty grid cell at absolute pad index `idx` claims.
(def pad-note-for-cell (gidx idx)
  (let ((wanted (%clamp-pad-note idx)))
    (if (%pad-note-free? gidx wanted) wanted (%next-free-pad-note gidx))))
