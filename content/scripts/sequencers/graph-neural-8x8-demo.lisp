;; Graph-mode 8x8 neural sequencer — a playground for the lisp node-graph DSL.
;;
;; Eight all-to-all nodes. Seed it by putting a trigger on track 0 (node 0 subscribes
;; to track 0). The :update rule shapes the emitted/propagated event in lisp: note
;; accumulates the per-node transpose around feedback loops, and velocity is scaled by
;; a per-node vel-decay each hop — the velocity analogue of the transpose cascade.
;;
;; All nodes route to track 0 by default so every firing is audible on one instrument;
;; change a node's route in your own copy if you want it to drive other tracks. The
;; control panel exposes per-node route / delay / transpose / vel-decay / resolution /
;; quantize plus the 8x8 connection-weight matrix.
;;
;; The panel reads and edits the graph through the kinds (kind-bindings spec §14.2k,
;; §14.2s): its `graph`, each `graph-node` and their `graph-param`s, bound with `#'` so
;; an edit or playback repaints only its widget. Every control edit is one undo entry
;; (a drag's frames join one); the weight matrix sets one edge's weight per cell. The
;; rows are one `each` over the graph's nodes, so a 16- or 64-node sequencer costs no
;; extra lines.
;;
;; Project scratch entrypoint:
;;   (load "content/scripts/sequencers/graph-neural-8x8-demo.lisp")
;;
;; Loading this file only publishes the graph/UI. It does not write graph overrides.
;; For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(import eseq.kinds :refer (tracks transport graph-of graph-param-named graph-quantize-options))
(import eseq.view-kit :refer (nothing))
(import eseq.graph-kit :refer (route-options node-route-label set-route-label! weight-rows
                               set-weight! column rack-name res-options))

;; `def-sequencer` returns the instance handle; every graph-* native below takes
;; it, so this script also works when a drum rack owns it (routes then address
;; rack members and the handle stays unambiguous next to a project-owned copy).
(def g8-name (def-sequencer "neural-8x8-demo"
  :shape (line 8)
  :energy-decay 0.992
  :reset-every (bars 4)
  :seed-on-reset 0
  :max-poly 4
  ;; Which fires survive when more than :max-poly land in one boundary. Options:
  ;; :deterministic :propagation :random :loudest :lowest-transpose :highest-transpose
  ;; :seed-first (seed-originated fires win their slots before neural-only ones).
  :max-poly-selection :propagation
  :duration (steps 1)
  
  (def-node nrn
    :resolution :16
    :delay 1
    :quantize :16
    :route 0
    :seed-from ()
    :reduce :sum
    ;; :reduce folds the ENERGY of coinciding inputs; :event folds the PAYLOAD
    ;; (note/velocity). :loudest keeps the highest-velocity arrival, so a full-velocity
    ;; seed punches through instead of being clobbered by a decayed neural hit (the old
    ;; :newest = last-writer-wins behavior). Options: :newest :loudest :seed-priority
    ;; :strongest.
    :event :newest
    :params ((threshold :float 0 4 :default 0.55)
      (transpose :int -48 48 :default 0)
      (vel-decay :float 0 2 :default 0.9)
      (dampening :float 0 1 :default 0.14)
      (recovery :float 0 1 :default 0.94))
    :state ((energy :leak (per-step :energy-decay)))
    ;; Fire when energy clears threshold. The else-branch returns nil (no fire).
    :update (if (>= (energy) (param :threshold))
      (do
        (dampen-incoming (param :dampening))
        (emit :note (+ (in-note) (param :transpose))
          :dur (* 4 (delay))
          :vel  (* (in-vel) (param :vel-decay))))
      (recover-incoming (param :recovery))))
  
  (edges
    :from nrn
    :to nrn
    :topology (all-to-all)
    :gather (- (edge :weight) (edge :dampening))
    :params ((weight :float -1 1 :default 0.0)
      (dampening :float 0 1 :default 0)))))


(def g8-node-count 8)
(def script-buffer-name "*8x8*")
;; Owned by a rack: routes address its members and the tab wears its name.
(def g8-owner-rack (graph-owner g8-name))
(def script-tab-label (if g8-owner-rack (rack-name g8-owner-rack) "8x8"))
(def script-sequencer-name "neural-8x8-demo")

;; The panel's own state: how far a sounding key presses down.
(def-kind g8-view
  :key ()
  :state ((press-depth 0.6)))

;; ── init helpers (explicit-only; loading the file does NOT call these) ──
;; They write through the graph-* natives, which answer at once: the graph is
;; not a kind instance until the host's next sync.

(def g8-ring-weights ()
  (list
    (list 0 1 0 0 0 0 0 0)
    (list 0 0 1 0 0 0 0 0)
    (list 0 0 0 1 0 0 0 0)
    (list 0 0 0 0 1 0 0 0)
    (list 0 0 0 0 0 1 0 0)
    (list 0 0 0 0 0 0 1 0)
    (list 0 0 0 0 0 0 0 1)
    (list 1 0 0 0 0 0 0 0)))

(def g8-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g8-name :from r :to c :weight (nth (nth w r) c)))
        (range g8-node-count)))
    (range g8-node-count)))

(def g8-init-ring-defaults ()
  (g8-apply-weights (g8-ring-weights))
  (graph-node g8-name 0 :seed-from 0)
  (graph-node g8-name 1 :seed-from 1)
  (graph-node g8-name 2 :seed-from 2)
  (graph-node g8-name 3 :seed-from 4))

(def script-init-fn ()
  (g8-init-ring-defaults))

;; ── UI ──

(def g8-row-height 1.0)
(def g8-node-width 1.4)
(def g8-control-width 6)

(def g8-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :border-color :dim
    :background-color :mixer-strip-bg
    :value value :min lo :max hi :step stp :decimals dec
    :width g8-control-width :height g8-row-height :font-size 9
    :on-change on-change))

(def g8-pick (key value options on-change)
  (dropdown
    :key key
    :value value :options options
    :badge-color :transparent
    :bg-color :mixer-strip-bg
    :border-color :mixer-strip-selected-bg
    :width g8-control-width :height g8-row-height :font-size 7
    :on-change on-change))

;; Node n's param `name` (a graph-param of its prototype), bound.
(def g8-param (n name lo hi stp dec)
  (let ((p (graph-param-named n name)))
    (g8-num (str "graph-8x8-" name "-" n.index) #'p.value lo hi stp dec
      (lambda (v) (set! p.value v)))))

(def g8-row (n routes)
  (subtree :key (str "graph-8x8-row-" n.index)
    (h-stack :gap 0.4 :align :center
      (label (str n.index) :width g8-node-width :height g8-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
      (g8-pick (str "graph-8x8-route-" n.index)
        (node-route-label n) routes
        (lambda (label) (set-route-label! n label)))
      (g8-num (str "graph-8x8-delay-" n.index) #'n.delay 0 16 1 0
        (lambda (v) (set! n.delay v)))
      (g8-param n "transpose" -48 48 1 0)
      (g8-param n "vel-decay" 0 2 0.01 2)
      (g8-param n "dampening" 0 1 0.01 2)
      (g8-param n "recovery" 0 1 0.01 2)
      (g8-pick (str "graph-8x8-resolution-" n.index) n.resolution res-options
        (lambda (v) (set! n.resolution v)))
      (g8-pick (str "graph-8x8-quantize-" n.index) n.quantize graph-quantize-options
        (lambda (v) (set! n.quantize v))))))

(def g8-header-label (text width)
  (label text :width width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent))

(def g8-header ()
  (h-stack :gap 0.4 :align :center
    (g8-header-label "node" g8-node-width)
    (g8-header-label "route" g8-control-width)
    (g8-header-label "delay" g8-control-width)
    (g8-header-label "transp" g8-control-width)
    (g8-header-label "vel x" g8-control-width)
    (g8-header-label "dampen" g8-control-width)
    (g8-header-label "recover" g8-control-width)
    (g8-header-label "res" g8-control-width)
    (g8-header-label "quant" g8-control-width)))

(def g8-config-label (text)
  (label text :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent))

;; The playback views read live fields, each in a subtree of its own, so
;; playback re-runs only them.
(def g8-column-matrix (key width hi values)
  (v-stack :gap 0.35
    (box :width 0 :height 1.2)
    (subtree :key key
      (matrix
        :key key
        :rows 8
        :cols 1
        :width width
        :height 9.5
        :min 0
        :max hi
        :value (column (values))))))

;; The track output heatmap's palette: each track's color.
(def g8-track-colors () (map (lambda (t) t.color) (tracks)))

;; `notes` stands in for the tracks' active notes (a capture's fixed ones).
(def g8-graph-panel (g notes)
  (box
    :padding 0.85
    :gap 0.6
    :width 95
    :height 45
    (v-stack :gap 0.5 :width :fill
      ;; ── sequencer-level config (on top) ──
      (box :padding 0.5 :background-color :mixer-strip-bg :border-color :mixer-strip-border :corner-radius 16
        (h-stack :gap 1.0
          (v-stack
            (label "8x8 graph" :width 8 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
            (h-stack :gap 0.6 :align :center
              (v-stack :gap 0.05 :align :center
                (g8-config-label "reset bars")
                (g8-num "graph-8x8-reset-bars" #'g.reset-bars 0 64 1 0
                  (lambda (v) (set! g.reset-bars v))))
              (v-stack :gap 0.1 :align :center
                (g8-config-label "max poly")
                (g8-num "graph-8x8-max-poly" #'g.max-poly 0 16 1 0
                  (lambda (v) (set! g.max-poly v))))
              (v-stack :gap 0.1 :align :center
                (g8-config-label "key press")
                (g8-num "graph-8x8-piano-press-depth" #'g8-view.press-depth 0 2 0.05 2
                  (lambda (v) (set! g8-view.press-depth v))))))
          (subtree :key "graph-8x8-dampening-matrix"
            (matrix
              :key "graph-8x8-dampening-matrix"
              :rows 8
              :cols 8
              :width 12
              :height 6
              :control :grid
              :background-color :bg
              :fill :primary
              :min 0
              :max 1
              :value g.dampening))
          (subtree :key "graph-8x8-event-view"
            (event-view
              :key "graph-8x8-event-view"
              :events g.events
              :current-beat #'g.beat
              :renderer :isometric
              :x :transpose
              :x-min -24
              :background :bg
              :x-max 24
              :y :node
              :y-min 0
              :y-max 7
              :z :beat-phase
              :z-min 0
              :z-max 16
              :phase-beats 16
              :window-beats 16
              :brightness :velocity
              :cube-padding 0
              :auto-rotate true
              :width 12
              :height 6))
          (subtree :key "graph-8x8-track-event-view"
            (event-view
              :key "graph-8x8-track-event-view"
              :events transport.track-events
              :current-beat #'transport.track-events-beat
              :renderer :heatmap
              :x :beat-phase
              :x-min 0
              :x-max 16
              :y :transpose
              :y-min -60
              :y-max 60
              :phase-beats 16
              :window-beats 16
              :brightness :velocity
              :color-by :track
              :color-mode :categorical
              :color-palette (g8-track-colors)
              :color-min 0
              :color-max 15
              :color-count 16
              :x-bins 64
              :y-bins 120
              :background :bg
              :width 42
              :height 6))))
      (h-stack
        (box :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 1.0 :corner-radius 16
          (v-stack :gap 0.5
            (v-stack :gap 0.2
              (g8-header)
              (let ((routes (route-options g)))
                (each g.nodes |n| (g8-row n routes))))))
        (g8-column-matrix "graph-8x8-trigger-matrix" 1 1 (lambda () g.triggers))
        (g8-column-matrix "graph-8x8-energy-matrix" 2 4 (lambda () g.energy))
        (v-stack :gap 0.35
          (box :width 0 :height 1.2)
          (subtree :key "graph-8x8-weight-matrix"
            (matrix
              :key "graph-8x8-weight-matrix"
              :rows 8
              :cols 8
              :width 22
              :height 9.5
              :min 0
              :background :mixer-strip-bg
              :color (rgba 0.14 0.3 0.9 1)
              :empty-fill-color (rgba 0.04 0.04 0.05 1)
              :stroke-color (rgba 0.36 0.62 0.57 1)
              :stroke-width 1.5
              :stroke-active-only true
              :max 1
              :value (weight-rows g)
              :on-cell-change (lambda (r c v) (set-weight! g r c v))))))
      (box
        :debug-name "graph-8x8-piano-panel"
        :padding 1
        :background-color :mixer-strip-bg
        :border-color :mixer-strip-border
        :corner-radius 12
        (subtree :key "graph-8x8-piano"
          (piano-keyboard
            :key "graph-8x8-piano"
            :notes-by-track (if notes notes (map (lambda (t) t.active-notes) (tracks)))
            :track-colors (g8-track-colors)
            :tracks (range 0 g8-node-count)
            :overlap-mode :loudest
            :press-depth #'g8-view.press-depth
            :start-note 12
            :key-count 80
            :width 84
            :height 3.5))))))

;; The panel, empty until the host publishes the graph (at its next sync).
;; :notes replaces the tracks' active notes on the keyboard.
(def g8-panel (&key notes)
  (let ((g (graph-of g8-name)))
    (if g (g8-graph-panel g notes) (nothing))))

(effect-buffer "*8x8*" (g8-panel))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
