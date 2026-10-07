;; Band coupling matrix demo: four real tracks conform to each other's harmony.
;;
;; Evaluate directly or load from project scratch:
;;   (load "content/scripts/sequencers/band-coupling-matrix-demo.lisp")
;;
;; Put DISTINCT melodic patterns on tracks 1-4 (indexes 0-3) and start the
;; transport. Each track publishes a HARMONIC POLICY - the pitch-class set of
;; its recent fired notes (transpose space, 0 = C4) - as a typed pitch field
;; (:band-0 .. :band-3) through a `band-voice` process. Each track also
;; carries one `band-ear` whose four amount inlets decide how often its notes
;; are conformed to each policy. The 4x4 matrix IS those sixteen inlets:
;;
;;   row    = source   (whose harmony is heard)
;;   column = listener (who conforms to it)
;;
;; Conformance is a SCALE QUANTIZER, never an interpolation: when a note is
;; conformed it snaps fully to the nearest pitch whose pitch class is in the
;; policy (a move of at most 6 semitones, register preserved). The cell value
;; is the PROBABILITY that a given note obeys (deterministic locked-seed
;; roll), so 0.5 means half your notes are borrowed into the source's key and
;; half play as written - there is no chromatic middle ground, which is what
;; makes closed coupling loops stable: conformed notes only ever land on
;; pitch classes that already belong to somebody's melody.
;;
;; All cells at zero plays your composition untouched. Mutual cells converge
;; the two tracks toward a shared mode; the loops are deterministic by the
;; previous-tick rule (`hear`/`read` always see last tick's resolved state).
;; The diagonal is self-coupling: conform to your own recent pitch classes
;; (an ostinato-glue at lag 0, a delayed self-tonality at higher lag).
;;
;; Per-row voice controls:
;;   weight = how loudly this track suggests (scales every listener's odds)
;;   lag    = how far back (in fired notes) its published policy sits
;;   memory = how many recent fired notes make up the policy's pitch classes
;;
;; Intent knobs: coupling (master obedience multiplier on every ear) and
;; grace (pitch-class tolerance in semitones: notes already within grace of
;; the policy are left alone). Preset buttons write whole matrices: clear /
;; ring (each follows the previous - a canon of keys) / hub (everyone adopts
;; track 1's harmony) / mesh (everyone leans on everyone).
;;
;; A policy is just a pitch-field of pitch classes, so you can also hand-play
;; the brain from any buffer - e.g. force C major on everyone listening to
;; :band-0 by attaching your own publisher that suggests
;;   (pitch-field (list 0 2 4 5 7 9 11) :root 0 :weight 1)
;;
;; Loading attaches the voice+ear chains to tracks 0-3 with all cells at zero
;; (silent coupling). For the ring starting point, explicitly run:
;;   (script-init-fn)
;;
;; Useful live calls:
;;   (band-apply-matrix (band-mesh-matrix))
;;   (band-set-cell 0 2 0.9)   ;; track 3 mostly obeys track 1's harmony
;;   (band-detach)             ;; clear all four chains
;;   (band-attach)
;;   (ps)

;; The panel reads each track's band processes back through the kinds
;; (kind-bindings spec §14.2h: `t.processes`, their `inlets`) and its events
;; from `transport.track-events` (§14.2s).
(import eseq.kinds :refer (tracks transport))

;; ── processes ──────────────────────────────────────────────────────────────

(def-process band-voice
  :doc "Publish this track's recent fired pitch classes as a lagged harmonic-policy field."
  :in ((chan :field :default :band-0)
       (self :track :default 0)
       (weight :float 0 2 :default 1 :lane true)
       (lag :int 0 32 :default 0)
       (memory :int 1 4 :default 3))
  ;; The field holds pitch CLASSES (mod 12, normalized to 0..11).
  ;; `field-nearest-delta` folds pitch-class space itself (shortest signed
  ;; delta, octave wrap included), so classes are all a policy needs.
  :run (let ((classes
              (map (lambda (k)
                     (mod (+ (mod (read (track (in :self)
                                               :transpose
                                               :trigs-ago (+ (in :lag) (+ k 1))))
                                  12)
                             12)
                          12))
                   (range 0 (in :memory)))))
         (suggest (in :chan)
           (pitch-field classes
                        :root (nth classes 0)
                        :weight (in :weight)))))

(def-process band-ear
  :doc "Probabilistically conform the current note to one heard harmonic policy; a full snap to the nearest allowed pitch class, never an interpolation."
  :target (step-param :transpose)
  :seed :locked
  :in ((a0 :float 0 1 :default 0 :lane true)
       (a1 :float 0 1 :default 0 :lane true)
       (a2 :float 0 1 :default 0 :lane true)
       (a3 :float 0 1 :default 0 :lane true)
       (coupling :float 0 2 :default 1 :lane true)
       (grace :int 0 3 :default 0))
  :run (let ((f0 (hear :band-0))
             (f1 (hear :band-1))
             (f2 (hear :band-2))
             (f3 (hear :band-3)))
         (let ((w0 (if f0 (* (in :a0) (field-weight f0)) 0))
               (w1 (if f1 (* (in :a1) (field-weight f1)) 0))
               (w2 (if f2 (* (in :a2) (field-weight f2)) 0))
               (w3 (if f3 (* (in :a3) (field-weight f3)) 0)))
           (let ((total (+ w0 (+ w1 (+ w2 w3)))))
             (if (> total 0)
               ;; One roll decides IF this note obeys at all; a second roll
               ;; picks WHICH policy, weighted by the incoming cells. The
               ;; class delta is applied whole (field-nearest-delta returns
               ;; the shortest signed pitch-class move, at most 6 semitones),
               ;; so the sounding pitch class always lands inside the chosen
               ;; policy and register is preserved.
               (if (< (rand) (min 1 (* (in :coupling) total)))
                 (let ((pick (* (rand) total)))
                   (target-add!
                     (if (< pick w0)
                       (field-nearest-delta f0 (current-note) (in :grace))
                       (if (< pick (+ w0 w1))
                         (field-nearest-delta f1 (current-note) (in :grace))
                         (if (< pick (+ w0 (+ w1 w2)))
                           (field-nearest-delta f2 (current-note) (in :grace))
                           (field-nearest-delta f3 (current-note) (in :grace)))))))
                 nil)
               nil)))))

;; ── instances: one voice + one ear per track ───────────────────────────────

(def band-voice-0-h (band-voice :chan :band-0 :self 0))
(def band-voice-1-h (band-voice :chan :band-1 :self 1))
(def band-voice-2-h (band-voice :chan :band-2 :self 2))
(def band-voice-3-h (band-voice :chan :band-3 :self 3))

(def band-ear-0-h (band-ear))
(def band-ear-1-h (band-ear))
(def band-ear-2-h (band-ear))
(def band-ear-3-h (band-ear))

(def band-attach ()
  (do
    (processes :track 0 band-voice-0-h band-ear-0-h)
    (processes :track 1 band-voice-1-h band-ear-1-h)
    (processes :track 2 band-voice-2-h band-ear-2-h)
    (processes :track 3 band-voice-3-h band-ear-3-h)))

(def band-detach ()
  (do
    (processes :track 0)
    (processes :track 1)
    (processes :track 2)
    (processes :track 3)))

;; ── inlet dispatch (handles are def'd symbols; keywords arrive evaluated) ──

(def band-amount-key (source)
  (if (= source 0) :a0
    (if (= source 1) :a1
      (if (= source 2) :a2 :a3))))

(def band-ear-set (listener key v)
  (if (= listener 0) (band-ear-0-h key v)
    (if (= listener 1) (band-ear-1-h key v)
      (if (= listener 2) (band-ear-2-h key v)
        (band-ear-3-h key v)))))

(def band-voice-set (n key v)
  (if (= n 0) (band-voice-0-h key v)
    (if (= n 1) (band-voice-1-h key v)
      (if (= n 2) (band-voice-2-h key v)
        (band-voice-3-h key v)))))

;; ── UI state mirrors (the panel's own edits: a scene whose chains lack the
;;    band processes shows these; every edit goes through the handles) ──────

(def-kind band-ui
  :key ()
  :state ((cells (list (list 0 0 0 0) (list 0 0 0 0) (list 0 0 0 0) (list 0 0 0 0)))
          (weights (list 1 1 1 1))
          (lags (list 0 0 0 0))
          (memories (list 3 3 3 3))
          (coupling 1)
          (grace 0)))

(def band-set-cell (r c v)
  (set! band-ui.cells (set-nth band-ui.cells r (set-nth (nth band-ui.cells r) c v)))
  (band-ear-set c (band-amount-key r) v))

(def band-apply-matrix (m)
  (set! band-ui.cells m)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c) (band-ear-set c (band-amount-key r) (nth (nth m r) c)))
        (range 0 4)))
    (range 0 4)))

(def band-zero-matrix ()
  (map (lambda (r) (list 0 0 0 0)) (range 0 4)))

(def band-ring-matrix ()
  (map (lambda (r)
         (map (lambda (c) (if (= c (mod (+ r 1) 4)) 0.8 0)) (range 0 4)))
       (range 0 4)))

(def band-hub-matrix ()
  (map (lambda (r)
         (map (lambda (c) (if (and (= r 0) (> c 0)) 0.7 0)) (range 0 4)))
       (range 0 4)))

(def band-mesh-matrix ()
  (map (lambda (r)
         (map (lambda (c) (if (= r c) 0 0.35)) (range 0 4)))
       (range 0 4)))

(def band-set-weight (n v)
  (set! band-ui.weights (set-nth band-ui.weights n v))
  (band-voice-set n :weight v))

(def band-set-lag (n v)
  (set! band-ui.lags (set-nth band-ui.lags n v))
  (band-voice-set n :lag v))

(def band-set-memory (n v)
  (set! band-ui.memories (set-nth band-ui.memories n v))
  (band-voice-set n :memory v))

(def band-set-coupling (v)
  (set! band-ui.coupling v)
  (for-each (lambda (j) (band-ear-set j :coupling v)) (range 0 4)))

(def band-set-grace (v)
  (set! band-ui.grace v)
  (for-each (lambda (j) (band-ear-set j :grace v)) (range 0 4)))

;; ── scene/pattern sync ─────────────────────────────────────────────────────
;; Process chains live on the PATTERN, so a scene switch swaps every inlet
;; value under the panel. The panel shows each track's inlets as its chain
;; holds them (the composed chain the fx panel reads), and the edits above
;; where a scene's chain lacks the band processes.

;; Track t's `class` process, or nil.
(def band-track-process (t class)
  (first (filter (lambda (p) (= p.class-name class)) t.processes)))

;; Track n's `class` process, or nil.
(def band-process (n class)
  (let ((ts (tracks)))
    (if (< n (len ts)) (band-track-process (nth ts n) class) nil)))

;; Tracks 0-3's `class` processes, nil where a track lacks one: the panel
;; takes them once a render.
(def band-processes (class)
  (let ((ts (tracks)))
    (map (lambda (n) (if (< n (len ts)) (band-track-process (nth ts n) class) nil))
         (range 0 4))))

;; Inlet `name` of process p (or nil), else `echo`.
(def band-inlet (p name echo)
  (let ((i (if p (first (filter (lambda (i) (= i.name name)) p.inlets)) nil)))
    (if i i.value echo)))

;; `ears`: (band-processes "band-ear")
(def band-cells-shown (ears)
  (map (lambda (r)
         (map (lambda (c) (band-inlet (nth ears c) (str "a" r) (nth (nth band-ui.cells r) c)))
              (range 0 4)))
       (range 0 4)))

;; ── UI ─────────────────────────────────────────────────────────────────────

(def script-buffer-name "*band-matrix*")
(def script-tab-label "band")
(def script-sequencer-name "")

(def script-init-fn ()
  (do
    (band-attach)
    (band-apply-matrix (band-ring-matrix))
    true))

(def band-row-height 1.0)
(def band-row-gap 0.2)
(def band-label-width 4.0)
(def band-control-width 6.0)

(def band-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width band-control-width :height band-row-height :font-size 9
    :on-change on-change))

(def band-dim-label (text w)
  (label text :width w :height band-row-height :font-size 8 :h-align :center :color :dim :bg :transparent))

(def band-preset-button (key text m)
  (button text :key key :width 4.6 :height 1.1 :font-size 8
    :on-click (lambda (event) (band-apply-matrix m))))

(def band-matrix-height
  (+ (* 4 band-row-height) (* 3 band-row-gap)))

(def band-matrix-block (ears)
  (v-stack :gap 0.3
    (h-stack :gap 0.3 :align :center
      (label "" :width band-label-width :height 0.9 :font-size 1 :bg :transparent)
      (each (range 0 4) |c|
        (band-dim-label (str "to " (+ c 1)) 3.1)))
    (h-stack :gap 0.3 :align :center
      (v-stack :gap band-row-gap
        (each (range 0 4) |r|
          (label (str "from " (+ r 1)) :width band-label-width :height band-row-height :font-size 8 :h-align :right :color :dim :bg :transparent)))
      (matrix
        :key "band-cell-matrix"
        :rows 4
        :cols 4
        :width 13
        :height band-matrix-height
        :min 0
        :max 1
        :color :blue
        :value (band-cells-shown ears)
        :on-cell-change band-set-cell))))

;; `voice`: track n's band-voice process, or nil
(def band-voice-row (n voice)
  (h-stack :gap 0.4 :align :center
    (label (str "trk " (+ n 1)) :width band-label-width :height band-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
    (band-num (str "band-weight-" n) (band-inlet voice "weight" (nth band-ui.weights n))
      0 2 0.05 2 (lambda (v) (band-set-weight n v)))
    (band-num (str "band-lag-" n) (band-inlet voice "lag" (nth band-ui.lags n))
      0 32 1 0 (lambda (v) (band-set-lag n v)))
    (band-num (str "band-memory-" n) (band-inlet voice "memory" (nth band-ui.memories n))
      1 4 1 0 (lambda (v) (band-set-memory n v)))))

;; `voices`: (band-processes "band-voice")
(def band-voice-block (voices)
  (v-stack :gap band-row-gap
    (h-stack :gap 0.4 :align :center
      (label "" :width band-label-width :height band-row-height :font-size 1 :bg :transparent)
      (band-dim-label "weight" band-control-width)
      (band-dim-label "lag" band-control-width)
      (band-dim-label "memory" band-control-width))
    (each (range 0 4) |n| (band-voice-row n (nth voices n)))))

(def band-panel ()
  (let ((ears (band-processes "band-ear")) (voices (band-processes "band-voice")))
    (box
      :padding 0.85
      :gap 0.6
      (h-stack :gap 0.8 :align :top
        (box :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 0.85 :corner-radius 16
          (v-stack :gap 0.5
            (label "band coupling" :width 12 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
            (h-stack :gap 0.5 :align :center
              (label "coupling" :width 5 :height band-row-height :font-size 9 :h-align :right :color :dim :bg :transparent)
              (band-num "band-coupling" (band-inlet (first ears) "coupling" band-ui.coupling) 0 2 0.05 2
                band-set-coupling))
            (h-stack :gap 0.5 :align :center
              (label "grace" :width 5 :height band-row-height :font-size 9 :h-align :right :color :dim :bg :transparent)
              (band-num "band-grace" (band-inlet (first ears) "grace" band-ui.grace) 0 3 1 0
                band-set-grace))
            (h-stack :gap 0.4 :align :center
              (band-preset-button "band-preset-clear" "clear" (band-zero-matrix))
              (band-preset-button "band-preset-ring" "ring" (band-ring-matrix)))
            (h-stack :gap 0.4 :align :center
              (band-preset-button "band-preset-hub" "hub" (band-hub-matrix))
              (band-preset-button "band-preset-mesh" "mesh" (band-mesh-matrix)))
            (h-stack :gap 0.4 :align :center
              (button "attach" :key "band-attach-button" :width 4.6 :height 1.1 :font-size 8
                :on-click (lambda (event) (band-attach)))
              (button "detach" :key "band-detach-button" :width 4.6 :height 1.1 :font-size 8
                :on-click (lambda (event) (band-detach))))))

        (box :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 0.85 :corner-radius 16
          (v-stack :gap 0.5
            (label "who follows whom" :width 17 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
            (band-matrix-block ears)))

        (box :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 0.85 :corner-radius 16
          (v-stack :gap 0.5
            (label "voices" :width 17 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
            (band-voice-block voices)))

        (box :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 0.85 :corner-radius 16
          (v-stack :gap 0.5
            (label "track events" :width 26 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
            (subtree :key "band-track-event-view" (band-event-view))))))))

;; The tracks' output notes, in their own subtree (whose key it takes): a new
;; event re-runs only this view. Its beat is bound, so the playhead only
;; repaints.
(def band-event-view ()
  (event-view
    :events transport.track-events
    :current-beat #'transport.track-events-beat
    :renderer :heatmap
    :x :beat-phase
    :x-min 0
    :x-max 16
    :y :transpose
    :y-min -24
    :y-max 24
    :phase-beats 16
    :window-beats 16
    :brightness :velocity
    :color-by :track
    :color-mode :categorical
    :color-palette (map (lambda (t) t.color) (tracks))
    :color-min 0
    :color-max 15
    :color-count 16
    :x-bins 64
    :y-bins 48
    :background (rgba 0.1 0.1 0.1 0.5)
    :width 26
    :height 10))

(effect-buffer "*band-matrix*" (band-panel))

(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")

(band-attach)
