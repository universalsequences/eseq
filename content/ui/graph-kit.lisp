;; ui/graph-kit.lisp — Helpers the graph demo panels share
;; (scripts/sequencers/graph-*): a node's route as a track instance, the
;; edge weights as matrix rows, playback as matrix columns, cycle text
;; (kind-bindings spec §14.2k, §14.2s).
;;
;; Side-effect free, like eseq.view-kit: no state, no buffers, no host calls
;; at import.

(module eseq.graph-kit)

(import eseq.kinds :refer (tracks groups graph-param-named graph-edge-to graph-quantize-options))
(import eseq.view-kit :refer (index-of))

(export route-tracks route-options node-route-label set-route-label!
        weight-rows set-weight! column cycle-text cycle-labels rack-name
        res-options factor-options set-param-on-nodes! scale-delays! shift-timebases!)

;; ── routes ────────────────────────────────────────────────────────────────
;; A node routes to a track of its graph's owner: a rack-owned graph's
;; members ("3 hat": the track's number and name), else the project's tracks
;; ("Track 3"). "Off" (nil) is always last.

(def route-tracks (g)
  (let ((rack g.owner)) (if rack rack.tracks (tracks))))

(def route-label (g t)
  (if g.owner (str (+ t.index 1) " " t.name) (str "Track " (+ t.index 1))))

(def route-options (g)
  (append (map (lambda (t) (route-label g t)) (route-tracks g)) (list "Off")))

(def node-route-label (n)
  (let ((t n.route)) (if t (route-label n.graph t) "Off")))

;; The track a route label names; nil for Off.
(def route-track (g label)
  (first (filter (lambda (t) (= (route-label g t) label)) (route-tracks g))))

;; Route node n to the track its graph's route `label` names (Off: none).
(def set-route-label! (n label)
  (set! n.route (route-track n.graph label)))

;; ── weights ───────────────────────────────────────────────────────────────

(def edge-weight (e)
  (let ((p (graph-param-named e "weight"))) (if p p.value 0)))

;; Graph g's edge weights as matrix rows: from node by row, to node by
;; column, over its active nodes; 0 where there is no edge.
(def weight-rows (g)
  (let ((count (len g.nodes)))
    (map
      (lambda (n)
        (reduce
          (lambda (row e)
            (let ((to e.to.index))
              (if (< to count) (set-nth row to (edge-weight e)) row)))
          (map (lambda (c) 0) (range 0 count))
          n.edges))
      g.nodes)))

;; Set the weight of graph g's edge from node `from` to node `to` (indices).
(def set-weight! (g from to v)
  (let ((e (graph-edge-to (nth g.nodes from) to)))
    (when e
      (let ((p (graph-param-named e "weight")))
        (set! p.value v)))))

;; ── playback ──────────────────────────────────────────────────────────────

;; One value per row, as a one-column matrix.
(def column (xs) (map (lambda (x) (list x)) xs))

;; ── cycles ────────────────────────────────────────────────────────────────

;; A cycle's labels as space-separated text ("16 16 4").
(def cycle-text (labels)
  (reduce (lambda (text label) (if (= text "") label (str text " " label))) "" labels))

;; A timebase word the graph-* natives also take, as its label (their
;; parse_timebase_arg spellings); anything else unchanged.
(def timebase-word (token)
  (match token
    "whole" "1"
    "half" "2"
    "quarter" "4"
    "eighth" "8"
    "sixteenth" "16"
    "thirtysecond" "32"
    "thirty-second" "32"
    "sixtyfourth" "64"
    "sixty-fourth" "64"
    "halftriplet" "2t"
    "half-triplet" "2t"
    "quartertriplet" "4t"
    "quarter-triplet" "4t"
    "eighthtriplet" "8t"
    "eighth-triplet" "8t"
    "sixteenthtriplet" "16t"
    "sixteenth-triplet" "16t"
    "thirtysecondtriplet" "32t"
    "thirty-second-triplet" "32t"
    "sixtyfourthtriplet" "64t"
    "sixty-fourth-triplet" "64t"
    "polyrhythm" "prh"
    _ token))

;; The labels of `text` that are among `options`, any case ("16t" is
;; "16T"), the natives' words included ("sixteenth" is "16"); the rest, a
;; half-typed token included, are dropped.
(def cycle-labels (text options)
  (let ((lowered (map string-downcase options)))
    (reduce
      (lambda (labels token)
        (let ((i (index-of lowered (timebase-word (string-downcase token)))))
          (if (< i 0) labels (append labels (list (nth options i))))))
      (list)
      (string-split text " "))))

;; ── batch edits ───────────────────────────────────────────────────────────
;; An edit of every node (a param on each, every delay scaled) writes through
;; the graph-* natives, unrecorded, as the legacy panels did: a kind setter
;; per node would record an undo entry per node. The kinds show it at the
;; host's next sync.

;; The step resolutions the panels offer (graph-timebase-options' straight
;; ones).
(def res-options (list "1" "2" "4" "8" "16" "32" "64"))

;; A batch edit's factor: a delay multiplier, or octaves for the timebases.
(def factor-options (list "1/4" "1/2" "1" "2" "4"))

(def factor-value (label)
  (match label
    "1/4" 0.25
    "1/2" 0.5
    "2" 2
    "4" 4
    _ 1))

(def factor-shift (label)
  (match label
    "1/4" -2
    "1/2" -1
    "2" 1
    "4" 2
    _ 0))

;; Set param `name` of nodes 0 to count - 1 of graph g; count
;; may pass the active nodes, up to g.max-nodes.
(def set-param-on-nodes! (g count name v)
  (for-each (lambda (i) (graph-param g.gid i name v)) (range 0 count)))

;; Scale every node's delay by the factor `label` names (a delay above 0
;; stays at least 1).
(def scale-delays! (g label)
  (let ((factor (factor-value label)))
    (for-each
      (lambda (n)
        (let ((delay n.delay))
          (graph-node g.gid n.index :delay
            (if (<= delay 0) 0 (max 1 (round (* delay factor)))))))
      g.nodes)))

(def clamp (v lo hi) (max lo (min hi v)))

;; A resolution or quantize label moved `shift` octaves within its family
;; (straight 1-64, triplets 2T-64T); off and Prh stay.
(def shift-timebase (label shift)
  (let ((i (index-of graph-quantize-options label)))
    (if (or (= label "off") (= label "Prh") (< i 0))
      label
      (nth graph-quantize-options
        (if (< i 8) (clamp (+ i shift) 1 7) (clamp (+ i shift) 8 13))))))

;; Move every node's resolution and quantize the octaves the factor `label`
;; names.
(def shift-timebases! (g label)
  (let ((shift (factor-shift label)))
    (for-each
      (lambda (n)
        (graph-node g.gid n.index
          :resolution (shift-timebase n.resolution shift)
          :quantize (shift-timebase n.quantize shift)))
      g.nodes)))

;; ── owners ────────────────────────────────────────────────────────────────

;; The name of the rack whose group id is gid, or nil.
(def rack-name (gid)
  (let ((rack (first (filter (lambda (g) (= g.gid gid)) (groups)))))
    (when rack rack.name)))
