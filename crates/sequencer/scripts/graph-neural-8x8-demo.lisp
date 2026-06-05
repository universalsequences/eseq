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
;; SCALING: every per-node control binds DIRECTLY to the resolved graph value via
;; `bind-graph` (number knobs) / `bind-graph` + an options list (enum dropdowns).
;; There is no per-node shadow `defstate` and no per-node sync function — the rows are
;; generated with a single `each` over (range NODE-COUNT), so a 16- or 64-node
;; sequencer costs zero extra lines. Edits write back through `reactive-set` (to dirty
;; just the one bound widget) plus the `graph-*` setter (to persist the override). The
;; weight matrix uses `:on-cell-change`, so dragging one cell writes ONE edge override
;; instead of re-applying all 64.
;;
;; Project scratch entrypoint:
;;   (load "crates/sequencer/scripts/graph-neural-8x8-demo.lisp")
;;
;; Loading this file only publishes the graph/UI and syncs controls from the current
;; pattern. It does not write graph overrides. For a fresh demo patch, explicitly run:
;;   (g8-init-ring-defaults)

(def-sequencer "neural-8x8-demo"
  :shape (line 8)
  :energy-decay 0.992
  :reset-every (bars 4)
  :seed-on-reset 0
  :max-poly 4
  ;; Which fires survive when more than :max-poly land in one boundary. Options:
  ;; :deterministic :propagation :random :loudest :lowest-transpose :highest-transpose
  ;; :seed-first (seed-originated fires win their slots before neural-only ones).
  :max-poly-selection :propagation

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
                      :vel  (* (in-vel) (param :vel-decay))))
              (recover-incoming (param :recovery))))

  (edges
    :from nrn
    :to nrn
    :topology (all-to-all)
    :gather (- (edge :weight) (edge :dampening))
    :params ((weight :float -1 1 :default 0.0)
             (dampening :float 0 1 :default 0))))

(def g8-name "neural-8x8-demo")
(def g8-node-count 8)

;; ── dropdown option lists (order is the index space bind-graph maps into) ──

(def g8-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def g8-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def g8-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
        "Off"))

(def g8-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

;; Route dropdown label -> the internal route the engine stores (:off or a track index).
(def g8-route->internal (label)
  (if (= label "Off") :off (g8-index-of g8-route-options label)))

;; ── connection weights: one list-valued widget, so a single state cell is fine ──
;; (Per-node knobs avoid defstate via bind-graph; the matrix is one widget for all 64
;;  cells, so it keeps a single g8-weights cell that is rebuilt from the graph on render
;;  and patched one cell at a time on edit.)

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

(defstate g8-weights (list))

(def g8-read-weights ()
  (map
    (lambda (r) (map (lambda (c) (graph-edge-value g8-name r c :weight)) (range 0 g8-node-count)))
    (range 0 g8-node-count)))

(def g8-set-cell (w r c v)
  (set-nth w r (set-nth (nth w r) c v)))

(def g8-zero-row ()
  (map (lambda (n) 0) (range 0 g8-node-count)))

(def g8-zero-matrix ()
  (map (lambda (n) (g8-zero-row)) (range 0 g8-node-count)))

(def g8-zero-column-matrix ()
  (map (lambda (n) (list 0)) (range 0 g8-node-count)))

(def g8-viz (visualizations)
  (let ((hits (filter (lambda (viz) (= (get viz :name) g8-name)) visualizations)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def g8-viz-matrix (viz field fallback)
  (if viz
    (let ((value (get viz field)))
      (if value value fallback))
    fallback))

;; ── init helpers (explicit-only; loading the file does NOT call these) ──

(def g8-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g8-name :from r :to c :weight (nth (nth w r) c)))
        (range g8-node-count)))
    (range g8-node-count)))

(def g8-init-ring-defaults ()
  (do
    (set! g8-weights (g8-ring-weights))
    (g8-apply-weights g8-weights)
    (graph-node g8-name 0 :seed-from 0)))

;; ── edit helpers: dirty the bound widget (reactive-set) + persist the override ──
;; `graph-key` gives the canonical GRAPH field name a `bind-graph` handle reads, so a
;; reactive-set on the same key re-renders exactly that one widget.

(def g8-edit-num (n field v)
  (do
    (reactive-set "GRAPH" (graph-key g8-name n field) v)
    (graph-node g8-name n field v)))

(def g8-edit-param (n field v)
  (do
    (reactive-set "GRAPH" (graph-key g8-name n field) v)
    (graph-param g8-name n field v)))

(def g8-edit-enum (n field options label internal)
  (do
    (reactive-set "GRAPH" (graph-key g8-name n field) (g8-index-of options label))
    (graph-node g8-name n field internal)))

;; Sequencer-level config (reset-every / max-poly) is per-pattern like the node/edge
;; overrides; bind via `bind-graph-config`, key via `graph-config-key`.
(def g8-edit-config (field v)
  (do
    (reactive-set "GRAPH" (graph-config-key g8-name field) v)
    (graph-config g8-name field v)))

;; ── UI ──

(def g8-row-height 1.3)
(def g8-node-width 1.4)
(def g8-control-width 7.0)

(def g8-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width g8-control-width :height g8-row-height :font-size 9
    :on-change on-change))

(def g8-pick (key value-index options on-change)
  (dropdown
    :key key
    :value-index value-index :options options
    :width g8-control-width :height g8-row-height :font-size 9
    :on-change on-change))

(def g8-row (n)
  (h-stack :gap 0.4 :align :center
    (label (str n) :width g8-node-width :height g8-row-height :font-size 9 :h-align :center :color :dim)
    (g8-pick (str "graph-8x8-route-" n)
      (bind-graph g8-name n :route g8-route-options) g8-route-options
      (lambda (v) (g8-edit-enum n :route g8-route-options v (g8-route->internal v))))
    (g8-num (str "graph-8x8-delay-" n)
      (bind-graph g8-name n :delay) 0 16 1 0
      (lambda (v) (g8-edit-num n :delay v)))
    (g8-num (str "graph-8x8-transpose-" n)
      (bind-graph g8-name n :transpose) -48 48 1 0
      (lambda (v) (g8-edit-param n :transpose v)))
    (g8-num (str "graph-8x8-vel-decay-" n)
      (bind-graph g8-name n :vel-decay) 0 2 0.01 2
      (lambda (v) (g8-edit-param n :vel-decay v)))
    (g8-num (str "graph-8x8-dampening-" n)
      (bind-graph g8-name n :dampening) 0 1 0.01 2
      (lambda (v) (g8-edit-param n :dampening v)))
    (g8-num (str "graph-8x8-recovery-" n)
      (bind-graph g8-name n :recovery) 0 1 0.01 2
      (lambda (v) (g8-edit-param n :recovery v)))
    (g8-pick (str "graph-8x8-resolution-" n)
      (bind-graph g8-name n :resolution g8-res-options) g8-res-options
      (lambda (v) (g8-edit-enum n :resolution g8-res-options v v)))
    (g8-pick (str "graph-8x8-quantize-" n)
      (bind-graph g8-name n :quantize g8-quant-options) g8-quant-options
      (lambda (v) (g8-edit-enum n :quantize g8-quant-options v v)))))

(def g8-header ()
  (h-stack :gap 0.4 :align :center
    (label "node"   :width g8-node-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "route"  :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "delay"  :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "transp" :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "vel x"  :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "dampen" :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "recover" :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "res"    :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "quant"  :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)))

(def g8-panel (current-pattern graph-visualizations)
  (do
    ;; Re-derive the matrix snapshot from the resolved current-pattern graph. The
    ;; per-node knobs need no sync — `bind-graph` re-seeds their slots as the rows
    ;; render below.
    current-pattern
    (set! g8-weights (g8-read-weights))
    (let ((viz (g8-viz graph-visualizations)))
      (box
        :padding 0.85
        :gap 0.6
        :width 37
        :height 45
        (v-stack :gap 0.5
          ;; ── sequencer-level config (on top) ──
          (h-stack :gap 0.6 :align :center
            (label "8x8 graph" :width 8 :height 1.2 :font-size 11 :color :foreground)
            (label "reset bars" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (g8-num "graph-8x8-reset-bars"
              (bind-graph-config g8-name :reset-bars) 0 64 1 0
              (lambda (v) (g8-edit-config :reset-bars v)))
            (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (g8-num "graph-8x8-max-poly"
              (bind-graph-config g8-name :max-poly) 0 16 1 0
              (lambda (v) (g8-edit-config :max-poly v))))
          (h-stack
            (v-stack :gap 0.5
              (h-stack :gap 0.5 :align :center
                (label "per-node knobs" :width 14 :height 1.2 :font-size 9 :color :dim))
              (v-stack :gap 0.2
                (g8-header)
                (each (range 0 g8-node-count) |n| (g8-row n))))
            
            (v-stack :gap 0.35
              (label "trig" :width 2 :height 2.5 :font-size 8 :color :dim)
              (matrix
                :key "graph-8x8-trigger-matrix"
                :rows 8
                :cols 1
                :width 1
                :height 12
                :min 0
                :max 1
                :value (g8-viz-matrix viz :trigger-matrix (g8-zero-column-matrix))))            
            
            (v-stack :gap 0.35
              (label "energy" :width 3 :height 2.5 :font-size 8 :color :dim)
              (matrix
                :key "graph-8x8-energy-matrix"
                :rows 8
                :cols 1
                :width 2
                :height 12
                :min 0
                :max 4
                :value (g8-viz-matrix viz :energy-matrix (g8-zero-column-matrix))))            
            (v-stack :gap 0.35
              (label "weights (from row -> to col)" :width 18 :height 2.5 :font-size 8 :color :dim)
              (matrix
                :key "graph-8x8-weight-matrix"
                :rows 8
                :cols 8
                :width 26
                :height 12
                :min 0
                :max 1
                :value g8-weights
                :on-cell-change (lambda (r c v)
                  (do
                    (set! g8-weights (g8-set-cell g8-weights r c v))
                    (graph-edge g8-name :from r :to c :weight v))))))
          (v-stack :gap 0.5
            
            
            (label "live dampening (from row -> to col)" :width 18 :height 1.2 :font-size 8 :color :dim)
            (matrix
              :key "graph-8x8-dampening-matrix"
              :rows 8
              :cols 8
              :width 26
              :height 12
              
            :control :grid
            :background :black
            :fill :primary
              :min 0
              :max 1
              :value (g8-viz-matrix viz :dampening-matrix (g8-zero-matrix)))))))))

(effect-buffer "*8x8*" (g8-panel SEQ.current-pattern SEQ.graph-visualizations))
(seq-register-step-sequencer-tab "8x8" "*8x8*")
