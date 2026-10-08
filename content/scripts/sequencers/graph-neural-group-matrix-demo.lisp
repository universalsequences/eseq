;; Graph-mode neural sequencer with NEURAL GROUP matrices — a fork of
;; graph-neural-variable-reset-demo.lisp that adds the two k×k group control
;; surfaces from docs/neural-groups-spec.md:
;;
;;   G (group gain)     — propagation gain between groups (§4.3). Cell
;;                        [A][B] scales every deposit from a group-A node
;;                        into a group-B node. 1 = inert, 0 = unplugged.
;;   H (group coupling) — activity→threshold coupling (§4.5). Positive
;;                        [A][B]: activity in A raises B's effective
;;                        threshold (cross-inhibition); negative excites;
;;                        the diagonal is a per-group density governor.
;;
;; Both render as editable 4×4 matrices to the right of the connection-weight
;; matrix (rows = source group A–D, cols = target group). Each drag sets ONE
;; cell (`set-group-gain!` / `set-group-coupling!`), like a weight-matrix cell
;; sets one edge. Assign nodes to groups with the per-row `grp` dropdown.
;;
;; The graph defaults to eight all-to-all nodes and can be grown to sixteen without
;; losing dormant node or edge overrides. Per-row seed controls choose whether a node
;; listens to its routed track and whether it starts hot at the reset boundary. The
;; :update rule shapes the emitted/propagated event in lisp: note
;; can either accumulate the per-node transpose around feedback loops or reset to the
;; node/global transpose value, and velocity can either decay each hop or reset to
;; full scale.
;;
;; All nodes route to track 0 by default so every firing is audible on one instrument;
;; change a node's route in your own copy if you want it to drive other tracks. The
;; control panel exposes threshold / max-poly selection / global transpose / timing
;; batch controls, per-node route / seed / delay / transpose / transpose-reset /
;; vel-decay / vel-reset / resolution / quantize plus the active NxN
;; connection-weight matrix.
;;
;; The panel reads and edits the graph through the kinds (kind-bindings spec §14.2k,
;; §14.2s): every control edit is one undo entry, a drag's frames joining one. The
;; batch controls (threshold, global transpose, dur x) set every node's param at once
;; through the graph-* natives, one undo entry each; the threshold every node up to the
;; graph's capacity, so a node that becomes active later already carries it.
;;
;; Project scratch entrypoint:
;;   (load "content/scripts/sequencers/graph-neural-group-matrix-demo.lisp")
;;
;; Loading this file only publishes the graph/UI. It does not write graph overrides.
;; For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(import eseq.kinds :refer (graph-of graph-param-named graph-quantize-options
                           graph-max-poly-selection-options set-group-gain! set-group-coupling!))
(import eseq.view-kit :refer (rgb-part index-of nothing))
(import eseq.graph-kit :refer (route-tracks route-options node-route-label set-route-label!
                               weight-rows set-weight! column rack-name res-options
                               set-param-on-nodes!))

;; `def-sequencer` returns the instance handle; every graph-* native below takes
;; it, so this script also works when a drum rack owns it (routes then address
;; rack members and the handle stays unambiguous next to a project-owned copy).
(def ggm-name (def-sequencer "neural-group-matrix-demo"
  :shape (line :default 8 :min 1 :max 16)
  :energy-decay 0.992
  :reset-every (bars 4)
  :seed-on-reset 0
  :max-poly 4
  ;; Which fires survive when more than :max-poly land in one boundary. Options:
  ;; :deterministic :propagation :random :markov :loudest :lowest-transpose :highest-transpose
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
      (global-transpose :int -48 48 :default 0)
      (transpose :int -48 48 :default 0)
      (transpose-reset :int 0 1 :default 0)
      (dur-factor :float 0 8 :default 1)
      (vel-decay :float 0 2 :default 0.9)
      (vel-reset :int 0 1 :default 0)
      (dampening :float 0 1 :default 0.14)
      (recovery :float 0 1 :default 0.94))
    :state ((energy :leak (per-step :energy-decay)))
    ;; Fire when energy clears threshold. The else-branch returns nil (no fire).
    :update (if (>= (energy) (param :threshold))
      (do
        (dampen-incoming (param :dampening))
        (emit :note (+ (param :global-transpose)
            (if (>= (param :transpose-reset) 1)
              (param :transpose)
              (+ (in-note) (param :transpose))))
          :dur (* (delay) (param :dur-factor))
          :vel  (if (>= (param :vel-reset) 1)
            1
            (* (in-vel) (param :vel-decay)))))
      (recover-incoming (param :recovery))))
  
  (edges
    :from nrn
    :to nrn
    :topology (all-to-all)
    :gather (- (edge :weight) (edge :dampening))
    :params ((weight :float -1 1 :default 0.0)
      (dampening :float 0 1 :default 0)))))

(def ggm-min-node-count 1)
(def ggm-max-node-count 16)
(def script-buffer-name "*group-matrix*")
;; Owned by a rack: routes address its members and the tab wears its name.
(def ggm-owner-rack (graph-owner ggm-name))
(def script-tab-label (if ggm-owner-rack (rack-name ggm-owner-rack) "grp mtx"))
(def script-sequencer-name "neural-group-matrix-demo")

;; Neural-group assignment (docs/neural-groups-spec.md §3.1): a node's group
;; is the option's index (group A = 0).
(def ggm-group-options (list "A" "B" "C" "D"))
(def ggm-group-count 4)
(def ggm-route-off-color (list 0.20 0.21 0.23))

;; The neuron whose weight-matrix column is pressed (-1: none); its row lights.
(def-kind ggm-view
  :key ()
  :state ((selected-neuron -1)))

;; ── init helpers (explicit-only; loading the file does NOT call these) ──
;; They write through the graph-* natives, which answer at once: the graph is
;; not a kind instance until the host's next sync.

(def ggm-node-count ()
  (max ggm-min-node-count
    (min ggm-max-node-count
      (round (graph-config-value ggm-name :node-count)))))

(def ggm-init-ring-defaults ()
  (let ((count (ggm-node-count)))
    (for-each
      (lambda (r)
        (for-each
          (lambda (c)
            (graph-edge ggm-name :from r :to c :weight (if (= c (mod (+ r 1) count)) 1 0)))
          (range 0 count)))
      (range 0 count))
    (for-each
      (lambda (seed)
        (when (< (first seed) count)
          (graph-node ggm-name (first seed) :seed-from (nth seed 1))))
      (list (list 0 0) (list 1 1) (list 2 2) (list 3 4)))))

(def script-init-fn ()
  (ggm-init-ring-defaults))

;; ── UI ──

(def ggm-row-height 1.0)
(def ggm-row-gap 0.2)
(def ggm-row-panel-padding 1.0)
(def ggm-matrix-column-gap 0.35)
(def ggm-route-bar-width 0.28)
(def ggm-node-width 1.4)
(def ggm-control-width 6.0)
(def ggm-seed-control-width 4.8)
(def ggm-group-width 3.0)

(def ggm-matrix-data-height (count)
  (+ (* count ggm-row-height)
     (* (max 0 (- count 1)) ggm-row-gap)))

(def ggm-matrix-header-spacer-height ()
  (+ -0.5 (max 0 (- (+ ggm-row-panel-padding ggm-row-height ggm-row-gap) ggm-matrix-column-gap))))

;; ── group matrices ──
;; Both k×k group grids share one footprint; the row/column labels around them
;; are sized from the cell pitch so "A" lines up with its row and column.
(def ggm-group-matrix-width 16)
(def ggm-group-matrix-height 9.2)
(def ggm-group-grid-gap 0.2)
(def ggm-group-label-width 1.6)
(def ggm-group-label-height 1.0)
(def ggm-group-cell-width () (/ ggm-group-matrix-width ggm-group-count))
(def ggm-group-cell-height () (/ ggm-group-matrix-height ggm-group-count))
;; Title row + column-label row; the read-only act/θΔ columns pad by this much
;; so their cells sit level with the labeled grids' cells.
(def ggm-group-grid-header-height ()
  (+ ggm-group-label-height ggm-group-grid-gap ggm-group-label-height))

(def ggm-group-label (text w h)
  (label text :width w :height h :font-size 8 :h-align :center :color :dim :bg :transparent))

;; Wrap a k×k group matrix with a title, "to" column labels across the top and
;; "from" row labels down the left. Cell [A][B] = from group A, to group B.
(def ggm-group-grid (title body)
  (v-stack :gap ggm-group-grid-gap
    (ggm-group-label title (+ ggm-group-label-width ggm-group-matrix-width) ggm-group-label-height)
    (h-stack :gap 0
      (ggm-group-label "" ggm-group-label-width ggm-group-label-height)
      (each ggm-group-options |g|
        (ggm-group-label (str "to " g) (ggm-group-cell-width) ggm-group-label-height)))
    (h-stack :gap 0
      (v-stack :gap 0
        (each ggm-group-options |g|
          (ggm-group-label g ggm-group-label-width (ggm-group-cell-height))))
      body)))

(def ggm-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :border-color :dim
    :background-color :mixer-strip-bg
    :value value :min lo :max hi :step stp :decimals dec
    :width ggm-control-width :height ggm-row-height :font-size 9
    :on-change on-change))

(def ggm-pick-sized (key value options width on-change)
  (dropdown
    :key key
    :value value :options options
    :badge-color :transparent
    :bg-color :mixer-strip-bg
    :border-color :mixer-strip-selected-bg
    :width width :height ggm-row-height :font-size 6
    :on-change on-change))

(def ggm-pick (key value options on-change)
  (ggm-pick-sized key value options ggm-control-width on-change))

(def ggm-toggle-sized (key width value on-change)
  (box
    :width width :height ggm-row-height
    :padding 0 :h-align :center :v-align :center
    (toggle
      :key key
      :value value
      :color :blue
      :off-color :mixer-strip-bg
      :knob-color "#e8ecf4"
      :off-knob-color "#d8dde8"
      :on-change on-change)))

;; Node n's param `name`, bound.
(def ggm-param (n name lo hi stp dec)
  (let ((p (graph-param-named n name)))
    (ggm-num (str "graph-group-matrix-" name "-" n.index) #'p.value lo hi stp dec
      (lambda (v) (set! p.value v)))))

;; Node n's 0 / 1 param `name` as a toggle, bound.
(def ggm-switch (n name)
  (let ((p (graph-param-named n name)))
    (ggm-toggle-sized (str "graph-group-matrix-" name "-" n.index) ggm-control-width #'p.value
      (lambda (on) (set! p.value (if on 1 0))))))

;; A param every node carries alike: shows node 0's, sets the first count
;; nodes'.
(def ggm-global-param (g count key name lo hi stp dec)
  (let ((p (graph-param-named (first g.nodes) name)))
    (ggm-num key #'p.value lo hi stp dec
      (lambda (v) (set-param-on-nodes! g count name v)))))

(defwidget ggm-route-color-strip
  :width 0.28 :height 1.0
  :paint-margin 0.08
  :state (active track-r track-g track-b)
  :shader
  (sdf/fill (sdf/rounded-rect width height 0.08)
    (material
      :color (if (= active 1)
        (rgba track-r track-g track-b 1.0)
        (rgba track-r track-g track-b 0.62)))))

;; The node's route color: its track's, dimmed grey while off.
(def ggm-route-bar (n)
  (let ((t n.route)
        (channel (lambda (i) (if t (rgb-part t.color i) (nth ggm-route-off-color i)))))
    (box
      :key (str "graph-group-matrix-route-color-" n.index)
      :width ggm-route-bar-width
      :height ggm-row-height
      :background "ggm-route-color-strip"
      :active (if t 1 0)
      :track-r (channel 0)
      :track-g (channel 1)
      :track-b (channel 2))))

;; Node n's controls, in a subtree of their own: a weight-cell press,
;; which lights a row, re-runs only the rows' highlight boxes (below), which
;; reuse these.
(def ggm-row-controls (n routes)
  (subtree :key (str "graph-group-matrix-row-controls-" n.index)
    (h-stack :gap 0.4 :align :center
      (ggm-route-bar n)
      (label (str n.index) :width ggm-node-width :height ggm-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
      (ggm-pick (str "graph-group-matrix-route-" n.index)
        (node-route-label n) routes
        (lambda (label) (set-route-label! n label)))
      (ggm-pick-sized (str "graph-group-matrix-group-" n.index)
        (nth ggm-group-options n.group) ggm-group-options ggm-group-width
        (lambda (v) (set! n.group (index-of ggm-group-options v))))
      (ggm-toggle-sized (str "graph-group-matrix-seed-route-" n.index) ggm-seed-control-width
        #'n.seed-route
        (lambda (on) (set! n.seed-route on)))
      (ggm-toggle-sized (str "graph-group-matrix-reset-seed-" n.index) ggm-seed-control-width
        (> n.seed-on-reset 0)
        (lambda (on) (set! n.seed-on-reset (if on 1 0))))
      (ggm-num (str "graph-group-matrix-delay-" n.index) #'n.delay 0 16 1 0
        (lambda (v) (set! n.delay v)))
      (ggm-param n "transpose" -48 48 1 0)
      (ggm-switch n "transpose-reset")
      (ggm-param n "vel-decay" 0 2 0.01 2)
      (ggm-switch n "vel-reset")
      (ggm-param n "dampening" 0 1 0.01 2)
      (ggm-param n "recovery" 0 1 0.01 2)
      (ggm-pick (str "graph-group-matrix-resolution-" n.index) n.resolution res-options
        (lambda (v) (set! n.resolution v)))
      (ggm-pick (str "graph-group-matrix-quantize-" n.index) n.quantize graph-quantize-options
        (lambda (v) (set! n.quantize v))))))

;; Node n's row, lit while its weight column is pressed.
(def ggm-row (n routes)
  (subtree :key (str "graph-group-matrix-row-" n.index)
    (box
      :key (str "graph-group-matrix-row-" n.index)
      :height ggm-row-height
      :padding 0
      :selected (= ggm-view.selected-neuron n.index)
      :background-color :transparent
      :selected-background-color :mixer-strip-selected-bg
      :corner-radius 4
      (ggm-row-controls n routes))))

(def ggm-header-label (text width)
  (label text :width width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent))

(def ggm-header ()
  (h-stack :gap 0.4 :align :center
    (label "" :width ggm-route-bar-width :height 1.0 :font-size 1 :bg :transparent)
    (ggm-header-label "node" ggm-node-width)
    (ggm-header-label "route" ggm-control-width)
    (ggm-header-label "grp" ggm-group-width)
    (ggm-header-label "seed rt" ggm-seed-control-width)
    (ggm-header-label "rst seed" ggm-seed-control-width)
    (ggm-header-label "delay" ggm-control-width)
    (ggm-header-label "transp" ggm-control-width)
    (ggm-header-label "trn rst" ggm-control-width)
    (ggm-header-label "vel x" ggm-control-width)
    (ggm-header-label "vel rst" ggm-control-width)
    (ggm-header-label "dampen" ggm-control-width)
    (ggm-header-label "recover" ggm-control-width)
    (ggm-header-label "res" ggm-control-width)
    (ggm-header-label "quant" ggm-control-width)))

(def ggm-config-label (text)
  (label text :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent))

(def ggm-config-knob (key label-text value lo hi on-change)
  (knob-number
    :key key
    :debug-name key
    :label label-text
    :value value
    :min lo :max hi :decimals 2
    :width 7.0 :height 3.0 :knob-size 2.2
    :font-size 9.0 :label-font-size 9.0
    :label-color :dim
    :on-change on-change))

;; A playback column (each node's trigger or energy), in a subtree of its
;; own so playback re-runs only it.
(def ggm-column-matrix (key count width hi values)
  (v-stack :gap ggm-matrix-column-gap
    (label "" :width 0.1 :height (ggm-matrix-header-spacer-height) :font-size 1 :bg :transparent)
    (subtree :key key
      (matrix
        :key key
        :rows count
        :cols 1
        :width width
        :height (ggm-matrix-data-height count)
        :min 0
        :max hi
        :value (column (values))))))

(def ggm-graph-panel (g)
  (let ((active-count (len g.nodes)))
    (box
      :padding 0.85
      :gap 0.6
      (v-stack :gap 0.5
        ;; ── sequencer-level config (on top) ──
        (box
          :width 102
          :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 1 :corner-radius 16
          (h-stack
            (v-stack
              (h-stack :gap 0.6 :align :center
                (label "variable graph" :width 8 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
                (ggm-config-label "nodes")
                (ggm-num "graph-group-matrix-node-count" #'g.node-count 1 16 1 0
                  (lambda (v) (set! g.node-count v))))
              (h-stack :gap 0.6 :align :center
                (ggm-config-label "reset bars")
                (ggm-num "graph-group-matrix-reset-bars" #'g.reset-bars 0 64 1 0
                  (lambda (v) (set! g.reset-bars v))))
              (h-stack :gap 0.6 :align :center
                (ggm-config-label "max poly")
                (ggm-num "graph-group-matrix-max-poly" #'g.max-poly 0 16 1 0
                  (lambda (v) (set! g.max-poly v))))
              (h-stack :gap 0.6 :align :center
                (ggm-config-label "poly mode")
                (ggm-pick-sized "graph-group-matrix-max-poly-selection"
                  g.max-poly-selection graph-max-poly-selection-options 9.5
                  (lambda (v) (set! g.max-poly-selection v))))
              (h-stack :gap 0.6 :align :center
                (ggm-config-label "threshold")
                (ggm-global-param g g.max-nodes "graph-group-matrix-threshold" "threshold" 0 4 0.01 2))
              (h-stack :gap 0.6 :align :center
                (ggm-config-label "global trn")
                (ggm-global-param g active-count "graph-group-matrix-global-transpose" "global-transpose" -48 48 1 0)
                (ggm-config-label "dur x")
                (ggm-global-param g active-count "graph-group-matrix-dur-factor" "dur-factor" 0 8 0.25 2)))
            ;; Time-constant of the whole H coupling layer: the per-beat decay of the
            ;; group activity traces. ~0.5 = beat-scale sidechain-style coupling;
            ;; 0.85+ = bar-scale swells (the back-off cycle time for excite +
            ;; self-limit diagonal patches).
            (v-stack :gap 0.3
              (ggm-config-knob "graph-group-matrix-trace-decay" "trace decay"
                #'g.group-trace-decay 0 1
                (lambda (v) (set! g.group-trace-decay v)))
              ;; Global multiplier on the whole H matrix. H is touchy: a few
              ;; tenths per cell already gates groups hard, so scale the layer
              ;; here (0 = off, 1 = cells as drawn) instead of retouching cells.
              (ggm-config-knob "graph-group-matrix-coupling-scale" "H scale"
                #'g.group-coupling-scale 0 2
                (lambda (v) (set! g.group-coupling-scale v)))
              ;; How far excitation (blue H cells) may lower a node's threshold,
              ;; as a fraction of its authored value. 0 lets an excited group fire
              ;; on zero energy (self-oscillates); 1 disables excitation entirely.
              (ggm-config-knob "graph-group-matrix-excite-floor" "exc floor"
                #'g.group-excite-floor 0 1
                (lambda (v) (set! g.group-excite-floor v))))
            (subtree :key "graph-group-matrix-dampening-matrix"
              (matrix
                :key "graph-group-matrix-dampening-matrix"
                :rows active-count
                :cols active-count
                :width 16
                :height 7
                :control :grid
                :background-color :bg
                :fill :primary
                :min 0
                :max 1
                :value g.dampening))

            ;; ── group matrices (rows = FROM group, cols = TO group) ──
            ;; ── G: group propagation gain (rows = from group A–D, cols = to group) ──
            (ggm-group-grid "G gain"
              (subtree :key "graph-group-matrix-group-gain-matrix"
                (matrix
                  :key "graph-group-matrix-group-gain-matrix"
                  :rows ggm-group-count
                  :cols ggm-group-count
                  :width ggm-group-matrix-width
                  :height ggm-group-matrix-height
                  :min 0
                  :max 2
                  :default 1
                  :background :mixer-strip-bg
                  :color (rgba 0.16 0.66 0.44 1)
                  :empty-fill-color (rgba 0.04 0.04 0.05 1)
                  :stroke-color (rgba 0.36 0.62 0.57 1)
                  :stroke-width 1.5
                  :stroke-active-only true
                  :value (chunks g.group-gain ggm-group-count)
                  :on-cell-change (lambda (r c v) (set-group-gain! g r c v)))))

            ;; ── H: activity→threshold coupling (positive = suppress, negative = excite) ──
            ;; Bipolar, so :control :pie — wedge sweep = |H| from zero, clockwise
            ;; orange = suppression, counter-clockwise blue = excitation; an empty
            ;; ring is an untouched (zero) coupling.
            (ggm-group-grid "H couple"
              (subtree :key "graph-group-matrix-group-coupling-matrix"
                (matrix
                  :key "graph-group-matrix-group-coupling-matrix"
                  :rows ggm-group-count
                  :cols ggm-group-count
                  :width ggm-group-matrix-width
                  :height ggm-group-matrix-height
                  :min -2
                  :max 2
                  :default 0
                  :control :pie
                  :background :mixer-strip-bg
                  :color (rgba 0.9 0.5 0.16 1)
                  :negative-color (rgba 0.3 0.55 0.95 1)
                  ;; Zero cells draw only this outline ring (at 0.6 alpha) — it must
                  ;; clearly beat the matrix background or the 4x4 grid reads as
                  ;; just-the-nonzero-cells.
                  :empty-fill-color (rgba 0.42 0.44 0.5 1)
                  :stroke-color (rgba 0.62 0.5 0.36 1)
                  :stroke-width 1.5
                  :stroke-active-only true
                  :value (chunks g.group-coupling ggm-group-count)
                  :on-cell-change (lambda (r c v) (set-group-coupling! g r c v)))))

            ;; ── live group state (read-only, rows = groups A–D like the H matrix) ──
            ;; act: each group's leaky activity trace. θΔ: the signed threshold offset
            ;; H imposes on that group this boundary — orange wedge = suppressed,
            ;; blue = excited, empty ring = untouched.
            (v-stack :gap ggm-group-grid-gap
              (label "act" :width 2 :height (ggm-group-grid-header-height) :font-size 8 :h-align :center :color :dim :bg :transparent)
              (subtree :key "graph-group-matrix-group-activity-matrix"
                (matrix
                  :key "graph-group-matrix-group-activity-matrix"
                  :rows ggm-group-count
                  :cols 1
                  :width 2
                  :height ggm-group-matrix-height
                  :min 0
                  :max 2
                  :color (rgba 0.16 0.66 0.44 1)
                  :value (column g.group-activity))))

            (v-stack :gap ggm-group-grid-gap
              (label "θΔ" :width 2.5 :height (ggm-group-grid-header-height) :font-size 8 :h-align :center :color :dim :bg :transparent)
              (subtree :key "graph-group-matrix-group-suppression-matrix"
                (matrix
                  :key "graph-group-matrix-group-suppression-matrix"
                  :rows ggm-group-count
                  :cols 1
                  :width 2.5
                  :height ggm-group-matrix-height
                  :min -2
                  :max 2
                  :control :pie
                  :background :mixer-strip-bg
                  :color (rgba 0.9 0.5 0.16 1)
                  :negative-color (rgba 0.3 0.55 0.95 1)
                  :empty-fill-color (rgba 0.42 0.44 0.5 1)
                  :value (column g.group-suppression))))))

        (h-stack
          (box
            :padding ggm-row-panel-padding
            :border-color :mixer-strip-border
            :background-color :mixer-strip-bg :corner-radius 16
            (v-stack :gap 0.5
              (v-stack :gap ggm-row-gap
                (ggm-header)
                (let ((routes (route-options g)))
                  (each g.nodes |n| (ggm-row n routes))))))
          (ggm-column-matrix "graph-group-matrix-trigger-matrix" active-count 1 1 (lambda () g.triggers))
          (ggm-column-matrix "graph-group-matrix-energy-matrix" active-count 2 4 (lambda () g.energy))
          (v-stack :gap ggm-matrix-column-gap
            (label "" :width 0.1 :height (ggm-matrix-header-spacer-height) :font-size 1 :bg :transparent)
            (subtree :key "graph-group-matrix-weight-matrix"
              (matrix
                :key "graph-group-matrix-weight-matrix"
                :rows active-count
                :cols active-count
                :width (max 26 (* active-count 3.25))
                :height (ggm-matrix-data-height active-count)
                :min 0
                :background :mixer-strip-bg
                :color (rgba 0.14 0.3 0.9 1)
                :empty-fill-color (rgba 0.04 0.04 0.05 1)
                :stroke-color (rgba 0.36 0.62 0.57 1)
                :stroke-width 1.5
                :stroke-active-only true
                :max 1
                :value (weight-rows g)
                :on-cell-press (lambda (r c) (set! ggm-view.selected-neuron c))
                :on-cell-release (lambda (r c) (set! ggm-view.selected-neuron -1))
                :on-cell-change (lambda (r c v) (set-weight! g r c v))))))
        (box
          :debug-name "graph-group-matrix-piano-panel"
          :padding 1
          :background-color :mixer-strip-bg
          :border-color :mixer-strip-border
          :corner-radius 12
          (subtree :key "graph-group-matrix-piano"
            (piano-keyboard
              :key "graph-group-matrix-piano"
              :notes-by-track (map (lambda (t) t.active-notes) (route-tracks g))
              :track-colors (map (lambda (t) t.color) (route-tracks g))
              :tracks (range 0 active-count)
              :overlap-mode :loudest
              :press-depth 0.6
              :start-note 12
              :key-count 80
              :width 84
              :height 3.5)))))))

;; The panel, empty until the host publishes the graph (at its next sync).
(def ggm-panel ()
  (let ((g (graph-of ggm-name)))
    (if g (ggm-graph-panel g) (nothing))))

(effect-buffer "*group-matrix*" (ggm-panel))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
