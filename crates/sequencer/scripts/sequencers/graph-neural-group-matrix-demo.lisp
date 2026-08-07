;; Graph-mode neural sequencer with NEURAL GROUP matrices — a fork of
;; graph-neural-variable-reset-demo.lisp that adds the two k×k group control
;; surfaces from docs/neural-groups-spec.md:
;;
;;   G (:group-gain-<r>-<c>)     — propagation gain between groups (§4.3). Cell
;;                                 [A][B] scales every deposit from a group-A node
;;                                 into a group-B node. 1 = inert, 0 = unplugged.
;;   H (:group-coupling-<r>-<c>) — activity→threshold coupling (§4.5). Positive
;;                                 [A][B]: activity in A raises B's effective
;;                                 threshold (cross-inhibition); negative excites;
;;                                 the diagonal is a per-group density governor.
;;
;; Both render as editable 4×4 matrices to the right of the connection-weight
;; matrix (rows = source group A–D, cols = target group). Each drag writes ONE
;; config cell override via `graph-config`, like a weight-matrix cell writes one
;; edge. Assign nodes to groups with the per-row `grp` dropdown.
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
;; SCALING: every per-node control binds DIRECTLY to the resolved graph value via
;; `bind-graph` (number knobs) / `bind-graph` + an options list (enum dropdowns).
;; There is no per-node shadow `defstate` and no per-node sync function — the rows are
;; generated with a single `each` over (range NODE-COUNT), so a 16- or 64-node
;; sequencer costs zero extra lines. Edits write back through `reactive-set` (to dirty
;; just the one bound widget) plus the `graph-*` setter (to persist the override). The
;; weight matrix uses `:on-cell-change`, so dragging one cell writes ONE edge override
;; instead of re-applying the full active matrix.
;;
;; Project scratch entrypoint:
;;   (load "crates/sequencer/scripts/sequencers/graph-neural-group-matrix-demo.lisp")
;;
;; Loading this file only publishes the graph/UI and syncs controls from the current
;; pattern. It does not write graph overrides. For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(def-sequencer "neural-group-matrix-demo"
  :shape (line :default 8 :min 1 :max 16)
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
      (dampening :float 0 1 :default 0))))

(def ggm-name "neural-group-matrix-demo")
(def ggm-min-node-count 1)
(def ggm-max-node-count 16)
(def script-buffer-name "*group-matrix*")
(def script-tab-label "grp mtx")
(def script-sequencer-name ggm-name)

;; ── dropdown option lists (order is the index space bind-graph maps into) ──

(def ggm-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def ggm-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def ggm-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
        "Off"))
(def ggm-max-poly-selection-options
  (list "deterministic" "propagation" "random" "loudest" "lowest-transpose" "highest-transpose" "seed-first"))
;; Neural-group assignment (docs/neural-groups-spec.md §3.1). The stored value IS the
;; dropdown index (group A = 0), so the numeric bind-graph handle seeds it directly.
(def ggm-group-options (list "A" "B" "C" "D"))
(def ggm-route-off-index (- (len ggm-route-options) 1))
(def ggm-route-off-color (list 0.20 0.21 0.23))

(def ggm-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

;; Route dropdown label -> the internal route the engine stores (:off or a track index).
(def ggm-route->internal (label)
  (if (= label "Off") :off (ggm-index-of ggm-route-options label)))

(def ggm-route-color-field (n channel)
  (str "ggm-route-color-" n "-" channel))

(def ggm-route-option-index (n)
  (round (reactive-value (bind-graph ggm-name n :route ggm-route-options))))

(def ggm-route-color-valid? (track-colors route-index)
  (and (>= route-index 0) (< route-index (len track-colors)) (< route-index ggm-route-off-index)))

(def ggm-color-channel (color channel fallback)
  (if (< channel (len color)) (nth color channel) fallback))

(def ggm-route-color-channel (track-colors route-index channel)
  (if (ggm-route-color-valid? track-colors route-index)
    (ggm-color-channel (nth track-colors route-index) channel (nth ggm-route-off-color channel))
    (nth ggm-route-off-color channel)))

(def ggm-sync-route-color (n track-colors route-index)
  (do
    (reactive-set "GRAPH" (ggm-route-color-field n "active")
      (if (ggm-route-color-valid? track-colors route-index) 1 0))
    (reactive-set "GRAPH" (ggm-route-color-field n "r")
      (ggm-route-color-channel track-colors route-index 0))
    (reactive-set "GRAPH" (ggm-route-color-field n "g")
      (ggm-route-color-channel track-colors route-index 1))
    (reactive-set "GRAPH" (ggm-route-color-field n "b")
      (ggm-route-color-channel track-colors route-index 2))))

;; ── connection weights: one list-valued widget, so a single state cell is fine ──
;; (Per-node knobs avoid defstate via bind-graph; the matrix is one widget for all active
;;  cells, so it keeps a single ggm-weights cell that is rebuilt from the graph on render
;;  and patched one cell at a time on edit.)

(def ggm-node-count ()
  (max ggm-min-node-count
    (min ggm-max-node-count
      (round (reactive-value (bind-graph-config ggm-name :node-count))))))

(def ggm-ring-weights ()
  (let ((count (ggm-node-count)))
    (map
      (lambda (r)
        (map
          (lambda (c) (if (= c (mod (+ r 1) count)) 1 0))
          (range 0 count)))
      (range 0 count))))

(defstate ggm-weights (list))
(defstate ggm-group-gain (list))
(defstate ggm-group-coupling (list))
(defstate ggm-selected-neuron -1)
(defstate ggm-threshold 0.55)
(defstate ggm-global-transpose 0)
(defstate ggm-dur-factor 1)
(defstate ggm-piano-press-depth 0.6)
(defstate ggm-delay-factor-index 2)
(defstate ggm-timebase-factor-index 2)

(def ggm-read-weights ()
  (map
    (lambda (r) (map (lambda (c) (graph-edge-value ggm-name r c :weight)) (range 0 (ggm-node-count))))
    (range 0 (ggm-node-count))))

(def ggm-set-cell (w r c v)
  (set-nth w r (set-nth (nth w r) c v)))

(def ggm-zero-row ()
  (map (lambda (n) 0) (range 0 (ggm-node-count))))

(def ggm-zero-matrix ()
  (map (lambda (n) (ggm-zero-row)) (range 0 (ggm-node-count))))

(def ggm-zero-column-matrix ()
  (map (lambda (n) (list 0)) (range 0 (ggm-node-count))))

;; ── neural-group matrices (docs/neural-groups-spec.md §3.2) ──
;; Config cells are addressed as :group-gain-<row>-<col> / :group-coupling-<row>-<col>
;; (row = source group, col = target group). Like ggm-weights, each matrix widget is
;; one list-valued state cell rebuilt from the resolved config on render and patched
;; one cell at a time on edit.

(def ggm-group-count 4)

(def ggm-group-cell-field (prefix r c)
  (str prefix "-" r "-" c))

(def ggm-read-group-matrix (prefix)
  (map
    (lambda (r)
      (map
        (lambda (c) (graph-config-value ggm-name (ggm-group-cell-field prefix r c)))
        (range 0 ggm-group-count)))
    (range 0 ggm-group-count)))

(def ggm-edit-group-cell (prefix r c v)
  (do
    (reactive-set "GRAPH" (graph-config-key ggm-name (ggm-group-cell-field prefix r c)) v)
    (graph-config ggm-name (ggm-group-cell-field prefix r c) v)))

(def ggm-viz (visualizations)
  (let ((hits (filter (lambda (viz) (= (get viz :name) ggm-name)) visualizations)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def ggm-matrix-shape? (value rows cols)
  (if value
    (if (= (len value) rows)
      (if (> rows 0)
        (= (len (nth value 0)) cols)
        true)
      false)
    false))

(def ggm-viz-matrix (viz field fallback rows cols)
  (if viz
    (let ((value (get viz field)))
      (if (ggm-matrix-shape? value rows cols) value fallback))
    fallback))

;; ── init helpers (explicit-only; loading the file does NOT call these) ──

(def ggm-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge ggm-name :from r :to c :weight (nth (nth w r) c)))
        (range 0 (ggm-node-count))))
    (range 0 (ggm-node-count))))

(def ggm-seed-if-active (n track)
  (if (< n (ggm-node-count))
    (graph-node ggm-name n :seed-from track)
    nil))

(def ggm-init-ring-defaults ()
  (do
    (set! ggm-weights (ggm-ring-weights))
    (ggm-apply-weights ggm-weights)
    (ggm-seed-if-active 0 0)
    (ggm-seed-if-active 1 1)
    (ggm-seed-if-active 2 2)
    (ggm-seed-if-active 3 4)
    ))

(def script-init-fn ()
  (ggm-init-ring-defaults))

;; ── edit helpers: dirty the bound widget (reactive-set) + persist the override ──
;; `graph-key` gives the canonical GRAPH field name a `bind-graph` handle reads, so a
;; reactive-set on the same key re-renders exactly that one widget.

(def ggm-edit-num (n field v)
  (do
    (reactive-set "GRAPH" (graph-key ggm-name n field) v)
    (graph-node ggm-name n field v)))

(def ggm-edit-param (n field v)
  (do
    (reactive-set "GRAPH" (graph-key ggm-name n field) v)
    (graph-param ggm-name n field v)))

(def ggm-edit-global-param (field v)
  (for-each
    (lambda (n)
      (do
        (reactive-set "GRAPH" (graph-key ggm-name n field) v)
        (graph-param ggm-name n field v)))
    (range 0 (ggm-node-count))))

(def ggm-edit-capacity-param (field v)
  (for-each
    (lambda (n)
      (do
        (reactive-set "GRAPH" (graph-key ggm-name n field) v)
        (graph-param ggm-name n field v)))
    (range 0 ggm-max-node-count)))

(def ggm-edit-enum (n field options label internal)
  (do
    (reactive-set "GRAPH" (graph-key ggm-name n field) (ggm-index-of options label))
    (graph-node ggm-name n field internal)))

(def ggm-edit-route (n label track-colors)
  (let ((route-index (ggm-index-of ggm-route-options label)))
    (do
      (ggm-edit-enum n :route ggm-route-options label (ggm-route->internal label))
      (ggm-sync-route-color n track-colors route-index))))

(def ggm-edit-seed-route (n enabled)
  (do
    (reactive-set "GRAPH" (graph-key ggm-name n :seed-route) (if enabled 1 0))
    (graph-node ggm-name n :seed-from (if enabled :route :off))))

(def ggm-edit-reset-seed (n enabled)
  (do
    (reactive-set "GRAPH" (graph-key ggm-name n :seed-on-reset) (if enabled 1 0))
    (graph-node ggm-name n :seed-on-reset (if enabled 1 0))))

(def ggm-factor-options (list "1/4" "1/2" "1" "2" "4"))

(def ggm-factor-value (label)
  (if (= label "1/4") 0.25
    (if (= label "1/2") 0.5
      (if (= label "2") 2
        (if (= label "4") 4 1)))))

(def ggm-factor-shift (label)
  (if (= label "1/4") -2
    (if (= label "1/2") -1
      (if (= label "2") 1
        (if (= label "4") 2 0)))))

(def ggm-clamp-index (idx len)
  (max 0 (min (- len 1) idx)))

(def ggm-scale-res-label (label shift)
  (nth ggm-res-options
    (ggm-clamp-index (+ (ggm-index-of ggm-res-options label) shift) (len ggm-res-options))))

(def ggm-scale-quant-label (label shift)
  (let ((idx (ggm-index-of ggm-quant-options label)))
    (if (= label "off")
      "off"
      (if (= label "Prh")
        "Prh"
        (if (< idx 8)
          (nth ggm-quant-options (max 1 (min 7 (+ idx shift))))
          (nth ggm-quant-options (+ 8 (ggm-clamp-index (+ (- idx 8) shift) 6))))))))

(def ggm-apply-delay-factor (label)
  (let ((factor (ggm-factor-value label)))
    (do
      (for-each
        (lambda (n)
          (let ((current (graph-node-value ggm-name n :delay)))
            (ggm-edit-num n :delay
              (if (<= current 0)
                0
                (max 1 (round (* current factor)))))))
        (range 0 (ggm-node-count)))
      (set! ggm-delay-factor-index (ggm-index-of ggm-factor-options "1")))))

(def ggm-apply-timebase-factor (label)
  (let ((shift (ggm-factor-shift label)))
    (do
      (for-each
        (lambda (n)
          (let ((res (ggm-scale-res-label (graph-node-value ggm-name n :resolution) shift))
                (quant (ggm-scale-quant-label (graph-node-value ggm-name n :quantize) shift)))
            (do
              (ggm-edit-enum n :resolution ggm-res-options res res)
              (ggm-edit-enum n :quantize ggm-quant-options quant quant))))
        (range 0 (ggm-node-count)))
      (set! ggm-timebase-factor-index (ggm-index-of ggm-factor-options "1")))))

;; Sequencer-level config is per-pattern like the node/edge overrides; bind via
;; `bind-graph-config`, key via `graph-config-key`.
(def ggm-edit-config (field v)
  (do
    (reactive-set "GRAPH" (graph-config-key ggm-name field) v)
    (graph-config ggm-name field v)))

(def ggm-edit-config-enum (field options label)
  (do
    (reactive-set "GRAPH" (graph-config-key ggm-name field) (ggm-index-of options label))
    (graph-config ggm-name field label)))

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

(def ggm-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :border-color :dim
    :background-color :mixer-strip-bg
    :value value :min lo :max hi :step stp :decimals dec
    :width ggm-control-width :height ggm-row-height :font-size 9
    :on-change on-change))

(def ggm-pick-sized (key value-index options width on-change)
  (dropdown
    :key key
    :value-index value-index :options options
    :badge-color :transparent
    :bg-color :mixer-strip-bg
    :border-color :mixer-strip-selected-bg
    :width width :height ggm-row-height :font-size 6
    :on-change on-change))

(def ggm-pick (key value-index options on-change)
  (ggm-pick-sized key value-index options ggm-control-width on-change))

(def ggm-reset-value (n field)
  (>= (reactive-value (bind-graph ggm-name n field)) 1))

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

(def ggm-toggle (key value on-change)
  (ggm-toggle-sized key ggm-control-width value on-change))

(def ggm-seed-toggle (key value on-change)
  (ggm-toggle-sized key ggm-seed-control-width value on-change))

(def ggm-seed-route-value (n)
  (>= (reactive-value (bind-graph ggm-name n :seed-route)) 1))

(def ggm-reset-seed-value (n)
  (>= (reactive-value (bind-graph ggm-name n :seed-on-reset)) 1))

(defwidget ggm-route-color-strip
  :width 0.28 :height 1.0
  :paint-margin 0.08
  :state (active track-r track-g track-b)
  :bindable (active track-r track-g track-b)
  :shader
  (sdf/fill (sdf/rounded-rect width height 0.08)
    (material
      :color (if (= active 1)
        (rgba track-r track-g track-b 1.0)
        (rgba track-r track-g track-b 0.62)))))

(def ggm-route-bar (n track-colors)
  (do
    (ggm-sync-route-color n track-colors (ggm-route-option-index n))
    (box
      :key (str "graph-group-matrix-route-color-" n)
      :width ggm-route-bar-width
      :height ggm-row-height
      :background "ggm-route-color-strip"
      :active (bind "GRAPH" (ggm-route-color-field n "active"))
      :track-r (bind "GRAPH" (ggm-route-color-field n "r"))
      :track-g (bind "GRAPH" (ggm-route-color-field n "g"))
      :track-b (bind "GRAPH" (ggm-route-color-field n "b")))))

(def ggm-row (n track-colors)
  (box
    :key (str "graph-group-matrix-row-" n)
    :height ggm-row-height
    :padding 0
    :selected (= ggm-selected-neuron n)
    :background-color :transparent
    :selected-background-color :mixer-strip-selected-bg
    :corner-radius 4
    (h-stack :gap 0.4 :align :center
      (ggm-route-bar n track-colors)
      (label (str n) :width ggm-node-width :height ggm-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
      (ggm-pick (str "graph-group-matrix-route-" n)
        (bind-graph ggm-name n :route ggm-route-options) ggm-route-options
        (lambda (v) (ggm-edit-route n v track-colors)))
      (ggm-pick-sized (str "graph-group-matrix-group-" n)
        (bind-graph ggm-name n :group) ggm-group-options ggm-group-width
        (lambda (v) (ggm-edit-num n :group (ggm-index-of ggm-group-options v))))
      (ggm-seed-toggle (str "graph-group-matrix-seed-route-" n)
        (ggm-seed-route-value n)
        (lambda (v) (ggm-edit-seed-route n v)))
      (ggm-seed-toggle (str "graph-group-matrix-reset-seed-" n)
        (ggm-reset-seed-value n)
        (lambda (v) (ggm-edit-reset-seed n v)))
      (ggm-num (str "graph-group-matrix-delay-" n)
        (bind-graph ggm-name n :delay) 0 16 1 0
        (lambda (v) (ggm-edit-num n :delay v)))
      (ggm-num (str "graph-group-matrix-transpose-" n)
        (bind-graph ggm-name n :transpose) -48 48 1 0
        (lambda (v) (ggm-edit-param n :transpose v)))
      (ggm-toggle (str "graph-group-matrix-transpose-reset-" n)
        (ggm-reset-value n :transpose-reset)
        (lambda (v) (ggm-edit-param n :transpose-reset (if v 1 0))))
      (ggm-num (str "graph-group-matrix-vel-decay-" n)
        (bind-graph ggm-name n :vel-decay) 0 2 0.01 2
        (lambda (v) (ggm-edit-param n :vel-decay v)))
      (ggm-toggle (str "graph-group-matrix-vel-reset-" n)
        (ggm-reset-value n :vel-reset)
        (lambda (v) (ggm-edit-param n :vel-reset (if v 1 0))))
      (ggm-num (str "graph-group-matrix-dampening-" n)
        (bind-graph ggm-name n :dampening) 0 1 0.01 2
        (lambda (v) (ggm-edit-param n :dampening v)))
      (ggm-num (str "graph-group-matrix-recovery-" n)
        (bind-graph ggm-name n :recovery) 0 1 0.01 2
        (lambda (v) (ggm-edit-param n :recovery v)))
      (ggm-pick (str "graph-group-matrix-resolution-" n)
        (bind-graph ggm-name n :resolution ggm-res-options) ggm-res-options
        (lambda (v) (ggm-edit-enum n :resolution ggm-res-options v v)))
      (ggm-pick (str "graph-group-matrix-quantize-" n)
        (bind-graph ggm-name n :quantize ggm-quant-options) ggm-quant-options
        (lambda (v) (ggm-edit-enum n :quantize ggm-quant-options v v))))))

(def ggm-header ()
  (h-stack :gap 0.4 :align :center
    (label "" :width ggm-route-bar-width :height 1.0 :font-size 1 :bg :transparent)
    (label "node"   :width ggm-node-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "route"  :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "grp"    :width ggm-group-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "seed rt" :width ggm-seed-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "rst seed" :width ggm-seed-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "delay"  :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "transp" :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "trn rst" :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "vel x"  :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "vel rst" :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "dampen" :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "recover" :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "res"    :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "quant"  :width ggm-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)))

(def ggm-panel (current-pattern graph-visualizations track-colors track-active-notes)
  (do
    ;; Re-derive the matrix snapshot from the resolved current-pattern graph. The
    ;; per-node knobs need no sync — `bind-graph` re-seeds their slots as the rows
    ;; render below.
    current-pattern
    (set! ggm-weights (ggm-read-weights))
    (set! ggm-group-gain (ggm-read-group-matrix "group-gain"))
    (set! ggm-group-coupling (ggm-read-group-matrix "group-coupling"))
    (set! ggm-threshold (graph-param-value ggm-name 0 :threshold))
    (set! ggm-global-transpose (graph-param-value ggm-name 0 :global-transpose))
    (set! ggm-dur-factor (graph-param-value ggm-name 0 :dur-factor))
    (let ((active-count (ggm-node-count))
        (viz (ggm-viz graph-visualizations)))
      (box 
        :padding 0.85
        :gap 0.6
        (v-stack :gap 0.5
          ;; ── sequencer-level config (on top) ──
          (box 
            :width 90.5
            :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 1 :corner-radius 16
            
            (h-stack
              (v-stack
                (h-stack :gap 0.6 :align :center
                  (label "variable graph" :width 8 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
                  (label "nodes" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (ggm-num "graph-group-matrix-node-count"
                    (bind-graph-config ggm-name :node-count) 1 16 1 0
                    (lambda (v) (ggm-edit-config :node-count v))))
                (h-stack :gap 0.6 :align :center
                  (label "reset bars" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (ggm-num "graph-group-matrix-reset-bars"
                    (bind-graph-config ggm-name :reset-bars) 0 64 1 0
                    (lambda (v) (ggm-edit-config :reset-bars v))))
                (h-stack :gap 0.6 :align :center
                  (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (ggm-num "graph-group-matrix-max-poly"
                    (bind-graph-config ggm-name :max-poly) 0 16 1 0
                    (lambda (v) (ggm-edit-config :max-poly v))))
                (h-stack :gap 0.6 :align :center
                  (label "poly mode" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (ggm-pick-sized "graph-group-matrix-max-poly-selection"
                    (bind-graph-config ggm-name :max-poly-selection ggm-max-poly-selection-options)
                    ggm-max-poly-selection-options 9.5
                    (lambda (v) (ggm-edit-config-enum :max-poly-selection ggm-max-poly-selection-options v))))
                (h-stack :gap 0.6 :align :center
                  (label "threshold" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (ggm-num "graph-group-matrix-threshold"
                    ggm-threshold 0 4 0.01 2
                    (lambda (v)
                      (do
                        (set! ggm-threshold v)
                        (ggm-edit-capacity-param :threshold v)))))
                (h-stack :gap 0.6 :align :center
                  (label "global trn" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (ggm-num "graph-group-matrix-global-transpose"
                    ggm-global-transpose -48 48 1 0
                    (lambda (v)
                      (do
                        (set! ggm-global-transpose v)
                        (ggm-edit-global-param :global-transpose v))))
                  (label "dur x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (ggm-num "graph-group-matrix-dur-factor"
                    ggm-dur-factor 0 8 0.25 2
                    (lambda (v)
                      (do
                        (set! ggm-dur-factor v)
                        (ggm-edit-global-param :dur-factor v)))))
                
                )
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
                :value (ggm-viz-matrix viz :dampening-matrix (ggm-zero-matrix) active-count active-count)
                )
              
              (event-view
                :key "graph-group-matrix-event-view"
                :events (if viz (get viz :event-history) (list))
                :current-beat (if viz (get viz :current-beat) 0)
                :renderer :isometric
                :x :transpose
                :x-min -24
                :x-max 24
                :y :node
                :y-min 0
                :y-max (- active-count 1)
                :z :beat-phase
                :z-min 0
                :z-max 16
                :phase-beats 16
                :auto-rotate true
                :window-beats 16
                :brightness :velocity
                :background :bg
                :width 16
                :height 7)              
              	(spectrogram
                :key "graph-group-matrix-master-spectrogram"
                :source :master
                :mode :waterfall
                :freq-scale :log
                :fft-size 2048
                :time-slices 180
                :min-db -64
                :max-db 0
                :smoothing 0.68
                :width 20
                :height 7.0
                :background-color :bg
                :min-color (rgba 0.05 0.05 0.11 1)
                :mid-color (rgba 0.16 0.66 0.88 1)
                :max-color (rgba 1.0 0.72 0.28 1)
                		  )
              
              ))
          
          (h-stack
            (box 
              :padding ggm-row-panel-padding
              :border-color :mixer-strip-border
              :background-color :mixer-strip-bg :corner-radius 16
              (v-stack :gap 0.5
                (v-stack :gap ggm-row-gap
                  (ggm-header)
                  (each (range 0 active-count) |n| (ggm-row n track-colors)))))
            
            (v-stack :gap ggm-matrix-column-gap 
              (label "" :width 0.1 :height (ggm-matrix-header-spacer-height) :font-size 1 :bg :transparent)
              (matrix
                :key "graph-group-matrix-trigger-matrix"
                :rows active-count
                :cols 1
                :width 1
                :height (ggm-matrix-data-height active-count)
                :min 0
                :max 1
                :value (ggm-viz-matrix viz :trigger-matrix (ggm-zero-column-matrix) active-count 1)))            
            
            (v-stack :gap ggm-matrix-column-gap
              (label "" :width 0.1 :height (ggm-matrix-header-spacer-height) :font-size 1 :bg :transparent)
              (matrix
                :key "graph-group-matrix-energy-matrix"
                :rows active-count
                :cols 1
                :width 2
                :height (ggm-matrix-data-height active-count)
                :min 0
                :max 4
                :value (ggm-viz-matrix viz :energy-matrix (ggm-zero-column-matrix) active-count 1)))            
           
            (v-stack :gap ggm-matrix-column-gap
              (label "" :width 0.1 :height (ggm-matrix-header-spacer-height) :font-size 1 :bg :transparent)
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
                :value ggm-weights
                :on-cell-press (lambda (r c)
                  (set! ggm-selected-neuron c))
                :on-cell-release (lambda (r c)
                  (set! ggm-selected-neuron -1))
                :on-cell-change (lambda (r c v)
                  (do
                    (set! ggm-weights (ggm-set-cell ggm-weights r c v))
                    (graph-edge ggm-name :from r :to c :weight v)))))

            ;; ── G: group propagation gain (rows = from group A–D, cols = to group) ──
            (v-stack :gap ggm-matrix-column-gap
              (label "G gain" :width 8 :height (ggm-matrix-header-spacer-height) :font-size 8 :h-align :center :color :dim :bg :transparent)
              (matrix
                :key "graph-group-matrix-group-gain-matrix"
                :rows ggm-group-count
                :cols ggm-group-count
                :width 8
                :height (ggm-matrix-data-height ggm-group-count)
                :min 0
                :max 2
                :background :mixer-strip-bg
                :color (rgba 0.16 0.66 0.44 1)
                :empty-fill-color (rgba 0.04 0.04 0.05 1)
                :stroke-color (rgba 0.36 0.62 0.57 1)
                :stroke-width 1.5
                :stroke-active-only true
                :value ggm-group-gain
                :on-cell-change (lambda (r c v)
                  (do
                    (set! ggm-group-gain (ggm-set-cell ggm-group-gain r c v))
                    (ggm-edit-group-cell "group-gain" r c v)))))

            ;; ── H: activity→threshold coupling (positive = suppress, negative = excite) ──
            (v-stack :gap ggm-matrix-column-gap
              (label "H couple" :width 8 :height (ggm-matrix-header-spacer-height) :font-size 8 :h-align :center :color :dim :bg :transparent)
              (matrix
                :key "graph-group-matrix-group-coupling-matrix"
                :rows ggm-group-count
                :cols ggm-group-count
                :width 8
                :height (ggm-matrix-data-height ggm-group-count)
                :min -2
                :max 2
                :background :mixer-strip-bg
                :color (rgba 0.9 0.5 0.16 1)
                :empty-fill-color (rgba 0.04 0.04 0.05 1)
                :stroke-color (rgba 0.62 0.5 0.36 1)
                :stroke-width 1.5
                :stroke-active-only true
                :value ggm-group-coupling
                :on-cell-change (lambda (r c v)
                  (do
                    (set! ggm-group-coupling (ggm-set-cell ggm-group-coupling r c v))
                    (ggm-edit-group-cell "group-coupling" r c v))))))
          (box
            :debug-name "graph-group-matrix-piano-panel"
            :padding 1
            :background-color :mixer-strip-bg
            :border-color :mixer-strip-border
            :corner-radius 12
            (piano-keyboard
              :key "graph-group-matrix-piano"
              :notes-by-track track-active-notes
              :track-colors track-colors
              :tracks (range 0 active-count)
              :overlap-mode :loudest
              :press-depth ggm-piano-press-depth
              :start-note 12
              :key-count 80
              :width 84
              :height 3.5))
          )))))

(effect-buffer "*group-matrix*" (ggm-panel SEQ.current-pattern SEQ.graph-visualizations SEQ.track-colors SEQ.track-active-notes))
(seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
