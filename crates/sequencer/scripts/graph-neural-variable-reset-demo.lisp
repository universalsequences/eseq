;; Graph-mode variable-count neural sequencer with reset/global timing controls — a playground for the lisp node-graph DSL.
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
;;   (load "crates/sequencer/scripts/graph-neural-variable-reset-demo.lisp")
;;
;; Loading this file only publishes the graph/UI and syncs controls from the current
;; pattern. It does not write graph overrides. For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(def-sequencer "neural-variable-reset-demo"
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

(def gvr-name "neural-variable-reset-demo")
(def gvr-min-node-count 1)
(def gvr-max-node-count 16)
(def script-buffer-name "*variable-reset*")
(def script-tab-label "var rst")
(def script-sequencer-name gvr-name)

;; ── dropdown option lists (order is the index space bind-graph maps into) ──

(def gvr-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def gvr-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def gvr-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
        "Off"))
(def gvr-max-poly-selection-options
  (list "deterministic" "propagation" "random" "loudest" "lowest-transpose" "highest-transpose" "seed-first"))
(def gvr-route-off-index (- (len gvr-route-options) 1))
(def gvr-route-off-color (list 0.20 0.21 0.23))

(def gvr-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

;; Route dropdown label -> the internal route the engine stores (:off or a track index).
(def gvr-route->internal (label)
  (if (= label "Off") :off (gvr-index-of gvr-route-options label)))

(def gvr-route-color-field (n channel)
  (str "gvr-route-color-" n "-" channel))

(def gvr-route-option-index (n)
  (round (reactive-value (bind-graph gvr-name n :route gvr-route-options))))

(def gvr-route-color-valid? (track-colors route-index)
  (and (>= route-index 0) (< route-index (len track-colors)) (< route-index gvr-route-off-index)))

(def gvr-color-channel (color channel fallback)
  (if (< channel (len color)) (nth color channel) fallback))

(def gvr-route-color-channel (track-colors route-index channel)
  (if (gvr-route-color-valid? track-colors route-index)
    (gvr-color-channel (nth track-colors route-index) channel (nth gvr-route-off-color channel))
    (nth gvr-route-off-color channel)))

(def gvr-sync-route-color (n track-colors route-index)
  (do
    (reactive-set "GRAPH" (gvr-route-color-field n "active")
      (if (gvr-route-color-valid? track-colors route-index) 1 0))
    (reactive-set "GRAPH" (gvr-route-color-field n "r")
      (gvr-route-color-channel track-colors route-index 0))
    (reactive-set "GRAPH" (gvr-route-color-field n "g")
      (gvr-route-color-channel track-colors route-index 1))
    (reactive-set "GRAPH" (gvr-route-color-field n "b")
      (gvr-route-color-channel track-colors route-index 2))))

;; ── connection weights: one list-valued widget, so a single state cell is fine ──
;; (Per-node knobs avoid defstate via bind-graph; the matrix is one widget for all active
;;  cells, so it keeps a single gvr-weights cell that is rebuilt from the graph on render
;;  and patched one cell at a time on edit.)

(def gvr-node-count ()
  (max gvr-min-node-count
    (min gvr-max-node-count
      (round (reactive-value (bind-graph-config gvr-name :node-count))))))

(def gvr-ring-weights ()
  (let ((count (gvr-node-count)))
    (map
      (lambda (r)
        (map
          (lambda (c) (if (= c (mod (+ r 1) count)) 1 0))
          (range 0 count)))
      (range 0 count))))

(defstate gvr-weights (list))
(defstate gvr-threshold 0.55)
(defstate gvr-global-transpose 0)
(defstate gvr-dur-factor 1)
(defstate gvr-delay-factor-index 2)
(defstate gvr-timebase-factor-index 2)

(def gvr-read-weights ()
  (map
    (lambda (r) (map (lambda (c) (graph-edge-value gvr-name r c :weight)) (range 0 (gvr-node-count))))
    (range 0 (gvr-node-count))))

(def gvr-set-cell (w r c v)
  (set-nth w r (set-nth (nth w r) c v)))

(def gvr-zero-row ()
  (map (lambda (n) 0) (range 0 (gvr-node-count))))

(def gvr-zero-matrix ()
  (map (lambda (n) (gvr-zero-row)) (range 0 (gvr-node-count))))

(def gvr-zero-column-matrix ()
  (map (lambda (n) (list 0)) (range 0 (gvr-node-count))))

(def gvr-viz (visualizations)
  (let ((hits (filter (lambda (viz) (= (get viz :name) gvr-name)) visualizations)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def gvr-matrix-shape? (value rows cols)
  (if value
    (if (= (len value) rows)
      (if (> rows 0)
        (= (len (nth value 0)) cols)
        true)
      false)
    false))

(def gvr-viz-matrix (viz field fallback rows cols)
  (if viz
    (let ((value (get viz field)))
      (if (gvr-matrix-shape? value rows cols) value fallback))
    fallback))

;; ── init helpers (explicit-only; loading the file does NOT call these) ──

(def gvr-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge gvr-name :from r :to c :weight (nth (nth w r) c)))
        (range 0 (gvr-node-count))))
    (range 0 (gvr-node-count))))

(def gvr-seed-if-active (n track)
  (if (< n (gvr-node-count))
    (graph-node gvr-name n :seed-from track)
    nil))

(def gvr-init-ring-defaults ()
  (do
    (set! gvr-weights (gvr-ring-weights))
    (gvr-apply-weights gvr-weights)
    (gvr-seed-if-active 0 0)
    (gvr-seed-if-active 1 1)
    (gvr-seed-if-active 2 2)
    (gvr-seed-if-active 3 4)
    ))

(def script-init-fn ()
  (gvr-init-ring-defaults))

;; ── edit helpers: dirty the bound widget (reactive-set) + persist the override ──
;; `graph-key` gives the canonical GRAPH field name a `bind-graph` handle reads, so a
;; reactive-set on the same key re-renders exactly that one widget.

(def gvr-edit-num (n field v)
  (do
    (reactive-set "GRAPH" (graph-key gvr-name n field) v)
    (graph-node gvr-name n field v)))

(def gvr-edit-param (n field v)
  (do
    (reactive-set "GRAPH" (graph-key gvr-name n field) v)
    (graph-param gvr-name n field v)))

(def gvr-edit-global-param (field v)
  (for-each
    (lambda (n)
      (do
        (reactive-set "GRAPH" (graph-key gvr-name n field) v)
        (graph-param gvr-name n field v)))
    (range 0 (gvr-node-count))))

(def gvr-edit-capacity-param (field v)
  (for-each
    (lambda (n)
      (do
        (reactive-set "GRAPH" (graph-key gvr-name n field) v)
        (graph-param gvr-name n field v)))
    (range 0 gvr-max-node-count)))

(def gvr-edit-enum (n field options label internal)
  (do
    (reactive-set "GRAPH" (graph-key gvr-name n field) (gvr-index-of options label))
    (graph-node gvr-name n field internal)))

(def gvr-edit-route (n label track-colors)
  (let ((route-index (gvr-index-of gvr-route-options label)))
    (do
      (gvr-edit-enum n :route gvr-route-options label (gvr-route->internal label))
      (gvr-sync-route-color n track-colors route-index))))

(def gvr-edit-seed-route (n enabled)
  (do
    (reactive-set "GRAPH" (graph-key gvr-name n :seed-route) (if enabled 1 0))
    (graph-node gvr-name n :seed-from (if enabled :route :off))))

(def gvr-edit-reset-seed (n enabled)
  (do
    (reactive-set "GRAPH" (graph-key gvr-name n :seed-on-reset) (if enabled 1 0))
    (graph-node gvr-name n :seed-on-reset (if enabled 1 0))))

(def gvr-factor-options (list "1/4" "1/2" "1" "2" "4"))

(def gvr-factor-value (label)
  (if (= label "1/4") 0.25
    (if (= label "1/2") 0.5
      (if (= label "2") 2
        (if (= label "4") 4 1)))))

(def gvr-factor-shift (label)
  (if (= label "1/4") -2
    (if (= label "1/2") -1
      (if (= label "2") 1
        (if (= label "4") 2 0)))))

(def gvr-clamp-index (idx len)
  (max 0 (min (- len 1) idx)))

(def gvr-scale-res-label (label shift)
  (nth gvr-res-options
    (gvr-clamp-index (+ (gvr-index-of gvr-res-options label) shift) (len gvr-res-options))))

(def gvr-scale-quant-label (label shift)
  (let ((idx (gvr-index-of gvr-quant-options label)))
    (if (= label "off")
      "off"
      (if (= label "Prh")
        "Prh"
        (if (< idx 8)
          (nth gvr-quant-options (max 1 (min 7 (+ idx shift))))
          (nth gvr-quant-options (+ 8 (gvr-clamp-index (+ (- idx 8) shift) 6))))))))

(def gvr-apply-delay-factor (label)
  (let ((factor (gvr-factor-value label)))
    (do
      (for-each
        (lambda (n)
          (let ((current (graph-node-value gvr-name n :delay)))
            (gvr-edit-num n :delay
              (if (<= current 0)
                0
                (max 1 (round (* current factor)))))))
        (range 0 (gvr-node-count)))
      (set! gvr-delay-factor-index (gvr-index-of gvr-factor-options "1")))))

(def gvr-apply-timebase-factor (label)
  (let ((shift (gvr-factor-shift label)))
    (do
      (for-each
        (lambda (n)
          (let ((res (gvr-scale-res-label (graph-node-value gvr-name n :resolution) shift))
                (quant (gvr-scale-quant-label (graph-node-value gvr-name n :quantize) shift)))
            (do
              (gvr-edit-enum n :resolution gvr-res-options res res)
              (gvr-edit-enum n :quantize gvr-quant-options quant quant))))
        (range 0 (gvr-node-count)))
      (set! gvr-timebase-factor-index (gvr-index-of gvr-factor-options "1")))))

;; Sequencer-level config is per-pattern like the node/edge overrides; bind via
;; `bind-graph-config`, key via `graph-config-key`.
(def gvr-edit-config (field v)
  (do
    (reactive-set "GRAPH" (graph-config-key gvr-name field) v)
    (graph-config gvr-name field v)))

(def gvr-edit-config-enum (field options label)
  (do
    (reactive-set "GRAPH" (graph-config-key gvr-name field) (gvr-index-of options label))
    (graph-config gvr-name field label)))

;; ── UI ──

(def gvr-row-height 1.0)
(def gvr-row-gap 0.2)
(def gvr-row-panel-padding 1.0)
(def gvr-matrix-column-gap 0.35)
(def gvr-route-bar-width 0.28)
(def gvr-node-width 1.4)
(def gvr-control-width 6.0)
(def gvr-seed-control-width 4.8)

(def gvr-matrix-data-height (count)
  (+ (* count gvr-row-height)
     (* (max 0 (- count 1)) gvr-row-gap)))

(def gvr-matrix-header-spacer-height ()
  (max 0 (- (+ gvr-row-panel-padding gvr-row-height gvr-row-gap) gvr-matrix-column-gap)))

(def gvr-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width gvr-control-width :height gvr-row-height :font-size 9
    :on-change on-change))

(def gvr-pick-sized (key value-index options width on-change)
  (dropdown
    :key key
    :value-index value-index :options options
    :width width :height gvr-row-height :font-size 6
    :on-change on-change))

(def gvr-pick (key value-index options on-change)
  (gvr-pick-sized key value-index options gvr-control-width on-change))

(def gvr-reset-value (n field)
  (>= (reactive-value (bind-graph gvr-name n field)) 1))

(def gvr-toggle-sized (key width value on-change)
  (box
    :width width :height gvr-row-height
    :padding 0 :h-align :center :v-align :center
    (toggle
      :key key
      :value value
      :color "#4f7dff"
      :off-color "#5f687a"
      :knob-color "#e8ecf4"
      :off-knob-color "#d8dde8"
      :on-change on-change)))

(def gvr-toggle (key value on-change)
  (gvr-toggle-sized key gvr-control-width value on-change))

(def gvr-seed-toggle (key value on-change)
  (gvr-toggle-sized key gvr-seed-control-width value on-change))

(def gvr-seed-route-value (n)
  (>= (reactive-value (bind-graph gvr-name n :seed-route)) 1))

(def gvr-reset-seed-value (n)
  (>= (reactive-value (bind-graph gvr-name n :seed-on-reset)) 1))

(defwidget gvr-route-color-strip
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

(def gvr-route-bar (n track-colors)
  (do
    (gvr-sync-route-color n track-colors (gvr-route-option-index n))
    (box
      :key (str "graph-variable-reset-route-color-" n)
      :width gvr-route-bar-width
      :height gvr-row-height
      :background "gvr-route-color-strip"
      :active (bind "GRAPH" (gvr-route-color-field n "active"))
      :track-r (bind "GRAPH" (gvr-route-color-field n "r"))
      :track-g (bind "GRAPH" (gvr-route-color-field n "g"))
      :track-b (bind "GRAPH" (gvr-route-color-field n "b")))))

(def gvr-row (n track-colors)
  (h-stack :gap 0.4 :align :center
    (gvr-route-bar n track-colors)
    (label (str n) :width gvr-node-width :height gvr-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
    (gvr-pick (str "graph-variable-reset-route-" n)
      (bind-graph gvr-name n :route gvr-route-options) gvr-route-options
      (lambda (v) (gvr-edit-route n v track-colors)))
    (gvr-seed-toggle (str "graph-variable-reset-seed-route-" n)
      (gvr-seed-route-value n)
      (lambda (v) (gvr-edit-seed-route n v)))
    (gvr-seed-toggle (str "graph-variable-reset-reset-seed-" n)
      (gvr-reset-seed-value n)
      (lambda (v) (gvr-edit-reset-seed n v)))
    (gvr-num (str "graph-variable-reset-delay-" n)
      (bind-graph gvr-name n :delay) 0 16 1 0
      (lambda (v) (gvr-edit-num n :delay v)))
    (gvr-num (str "graph-variable-reset-transpose-" n)
      (bind-graph gvr-name n :transpose) -48 48 1 0
      (lambda (v) (gvr-edit-param n :transpose v)))
    (gvr-toggle (str "graph-variable-reset-transpose-reset-" n)
      (gvr-reset-value n :transpose-reset)
      (lambda (v) (gvr-edit-param n :transpose-reset (if v 1 0))))
    (gvr-num (str "graph-variable-reset-vel-decay-" n)
      (bind-graph gvr-name n :vel-decay) 0 2 0.01 2
      (lambda (v) (gvr-edit-param n :vel-decay v)))
    (gvr-toggle (str "graph-variable-reset-vel-reset-" n)
      (gvr-reset-value n :vel-reset)
      (lambda (v) (gvr-edit-param n :vel-reset (if v 1 0))))
    (gvr-num (str "graph-variable-reset-dampening-" n)
      (bind-graph gvr-name n :dampening) 0 1 0.01 2
      (lambda (v) (gvr-edit-param n :dampening v)))
    (gvr-num (str "graph-variable-reset-recovery-" n)
      (bind-graph gvr-name n :recovery) 0 1 0.01 2
      (lambda (v) (gvr-edit-param n :recovery v)))
    (gvr-pick (str "graph-variable-reset-resolution-" n)
      (bind-graph gvr-name n :resolution gvr-res-options) gvr-res-options
      (lambda (v) (gvr-edit-enum n :resolution gvr-res-options v v)))
    (gvr-pick (str "graph-variable-reset-quantize-" n)
      (bind-graph gvr-name n :quantize gvr-quant-options) gvr-quant-options
      (lambda (v) (gvr-edit-enum n :quantize gvr-quant-options v v)))))

(def gvr-header ()
  (h-stack :gap 0.4 :align :center
    (label "" :width gvr-route-bar-width :height 1.0 :font-size 1 :bg :transparent)
    (label "node"   :width gvr-node-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "route"  :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "seed rt" :width gvr-seed-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "rst seed" :width gvr-seed-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "delay"  :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "transp" :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "trn rst" :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "vel x"  :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "vel rst" :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "dampen" :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "recover" :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "res"    :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "quant"  :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)))

(def gvr-panel (current-pattern graph-visualizations track-colors)
  (do
    ;; Re-derive the matrix snapshot from the resolved current-pattern graph. The
    ;; per-node knobs need no sync — `bind-graph` re-seeds their slots as the rows
    ;; render below.
    current-pattern
    (set! gvr-weights (gvr-read-weights))
    (set! gvr-threshold (graph-param-value gvr-name 0 :threshold))
    (set! gvr-global-transpose (graph-param-value gvr-name 0 :global-transpose))
    (set! gvr-dur-factor (graph-param-value gvr-name 0 :dur-factor))
    (let ((active-count (gvr-node-count))
          (viz (gvr-viz graph-visualizations)))
      (box 
        :padding 0.85
        :gap 0.6
        (v-stack :gap 0.5
          ;; ── sequencer-level config (on top) ──
          (box 
            :width 81.5
            :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 1 :corner-radius 16
            
            (h-stack
              (v-stack
                (h-stack :gap 0.6 :align :center
                  (label "variable graph" :width 8 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
                  (label "nodes" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-num "graph-variable-reset-node-count"
                    (bind-graph-config gvr-name :node-count) 1 16 1 0
                    (lambda (v) (gvr-edit-config :node-count v))))
                (h-stack :gap 0.6 :align :center
                  (label "reset bars" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-num "graph-variable-reset-reset-bars"
                    (bind-graph-config gvr-name :reset-bars) 0 64 1 0
                    (lambda (v) (gvr-edit-config :reset-bars v))))
                (h-stack :gap 0.6 :align :center
                  (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-num "graph-variable-reset-max-poly"
                    (bind-graph-config gvr-name :max-poly) 0 16 1 0
                    (lambda (v) (gvr-edit-config :max-poly v))))
                (h-stack :gap 0.6 :align :center
                  (label "poly mode" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-pick-sized "graph-variable-reset-max-poly-selection"
                    (bind-graph-config gvr-name :max-poly-selection gvr-max-poly-selection-options)
                    gvr-max-poly-selection-options 9.5
                    (lambda (v) (gvr-edit-config-enum :max-poly-selection gvr-max-poly-selection-options v))))
                (h-stack :gap 0.6 :align :center
                  (label "threshold" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-num "graph-variable-reset-threshold"
                    gvr-threshold 0 4 0.01 2
                    (lambda (v)
                      (do
                        (set! gvr-threshold v)
                        (gvr-edit-capacity-param :threshold v)))))
                (h-stack :gap 0.6 :align :center
                  (label "global trn" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-num "graph-variable-reset-global-transpose"
                    gvr-global-transpose -48 48 1 0
                    (lambda (v)
                      (do
                        (set! gvr-global-transpose v)
                        (gvr-edit-global-param :global-transpose v))))
                  (label "dur x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-num "graph-variable-reset-dur-factor"
                    gvr-dur-factor 0 8 0.25 2
                    (lambda (v)
                      (do
                        (set! gvr-dur-factor v)
                        (gvr-edit-global-param :dur-factor v)))))
                (h-stack :gap 0.6 :align :center
                  (label "delay x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-pick "graph-variable-reset-delay-factor"
                    gvr-delay-factor-index gvr-factor-options
                    (lambda (v) (gvr-apply-delay-factor v)))
                  (label "res/q x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (gvr-pick "graph-variable-reset-timebase-factor"
                    gvr-timebase-factor-index gvr-factor-options
                    (lambda (v) (gvr-apply-timebase-factor v))))
                )
              (matrix
                :key "graph-variable-reset-dampening-matrix"
                :rows active-count
                :cols active-count
                :width (max 11 (* active-count 0.75))
                :height (max 5 (* active-count 0.35))
                
                :control :grid
                :background (rgba 0.1 0.1 0.1 0.6)
                :fill :primary
                :min 0
                :max 1
                :value (gvr-viz-matrix viz :dampening-matrix (gvr-zero-matrix) active-count active-count)
                )
              
              (event-view
                :key "graph-variable-reset-event-view"
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
                :background (rgba 0.1 0.1 0.1 0.7)
                :width 20
                :height 8)              
	(spectrogram
                  :key "graph-variable-reset-master-spectrogram"
                  :source :master
                  :mode :waterfall
                  :freq-scale :log
                  :fft-size 2048
                  :time-slices 180
                  :min-db -64
                  :max-db 0
                  :smoothing 0.68
                  :width 20
                  :height 8.0
                  :background-color (rgba 0.13 0.13 0.13 1.00)
                  :min-color (rgba 0.05 0.05 0.11 1)
                  :mid-color (rgba 0.16 0.66 0.88 1)
                  :max-color (rgba 1.0 0.72 0.28 1)
		  )

              ))
          
          (h-stack
            (box 
              :padding gvr-row-panel-padding
              :border-color :mixer-strip-border
              :background-color :mixer-strip-bg :corner-radius 16
                (v-stack :gap 0.5
                  (v-stack :gap gvr-row-gap
                    (gvr-header)
                  (each (range 0 active-count) |n| (gvr-row n track-colors)))))
            
            (v-stack :gap gvr-matrix-column-gap 
              (label "" :width 0.1 :height (gvr-matrix-header-spacer-height) :font-size 1 :bg :transparent)
              (matrix
                :key "graph-variable-reset-trigger-matrix"
                :rows active-count
                :cols 1
                :width 1
                :height (gvr-matrix-data-height active-count)
                :min 0
                :max 1
                :value (gvr-viz-matrix viz :trigger-matrix (gvr-zero-column-matrix) active-count 1)))            
            
            (v-stack :gap gvr-matrix-column-gap
              (label "" :width 0.1 :height (gvr-matrix-header-spacer-height) :font-size 1 :bg :transparent)
              (matrix
                :key "graph-variable-reset-energy-matrix"
                :rows active-count
                :cols 1
                :width 2
                :height (gvr-matrix-data-height active-count)
                :min 0
                :max 4
                :value (gvr-viz-matrix viz :energy-matrix (gvr-zero-column-matrix) active-count 1)))            
            (v-stack :gap gvr-matrix-column-gap
              (label "" :width 0.1 :height (gvr-matrix-header-spacer-height) :font-size 1 :bg :transparent)
              (matrix
                :key "graph-variable-reset-weight-matrix"
                :rows active-count
                :cols active-count
                :width (max 26 (* active-count 3.25))
                :height (gvr-matrix-data-height active-count)
                :min 0
                :color :blue
                :max 1
                :value gvr-weights
                :on-cell-change (lambda (r c v)
                  (do
                    (set! gvr-weights (gvr-set-cell gvr-weights r c v))
                    (graph-edge gvr-name :from r :to c :weight v))))))
          
          
          )))))

(effect-buffer "*variable-reset*" (gvr-panel SEQ.current-pattern SEQ.graph-visualizations SEQ.track-colors))
(seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
