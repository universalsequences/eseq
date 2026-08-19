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

;; Pad note backing a member track, or -1 when the member has no pad.
(def pad-note-of-track (gidx track)
  (reduce |acc pad|
    (if (>= acc 0) acc (if (= (get pad :track) track) (get pad :pad-note) acc))
    -1
    (get (group-at gidx) :pads)))

;; Storage index of the group's backing bus in the SEQ.bus-* lists, or -1.
(def bus-index (gidx)
  (let ((bid (get (group-at gidx) :bus-id)))
    (reduce |acc i|
      (if (>= acc 0) acc (if (= (nth SEQ.bus-ids i) bid) i acc))
      -1
      (range 0 (len SEQ.bus-ids)))))

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
