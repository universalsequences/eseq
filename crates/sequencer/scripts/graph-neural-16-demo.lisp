;; Graph-mode 16-neuron sequencer - a playground for the lisp node-graph DSL.
;;
;; Sixteen all-to-all nodes. Seed it by putting a trigger on track 0 (node 0
;; subscribes to track 0). The :update rule shapes the emitted/propagated event in
;; lisp: note accumulates the per-node transpose around feedback loops, and velocity
;; is either scaled by a per-node vel-decay each hop or reset to 1.0.
;;
;; All nodes route to track 0 by default so every firing is audible on one
;; instrument. Change a node's route in your own copy if you want it to drive other
;; tracks. The control panel exposes per-node route / delay / transpose / vel-decay /
;; vel-reset / state-reset / resolution / quantize plus the 16x16 connection-weight
;; matrix.
;;
;; Project scratch entrypoint:
;;   (load "crates/sequencer/scripts/graph-neural-16-demo.lisp")
;;
;; Loading this file only publishes the graph/UI and syncs controls from the current
;; pattern. It does not write graph overrides. For a fresh demo patch, explicitly run:
;;   (g16-init-ring-defaults)

(def-sequencer "neural-16-demo"
  :shape (line 16)
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
    ;; (note/velocity). :loudest keeps the highest-velocity arrival, so a
    ;; full-velocity seed punches through instead of being clobbered by a decayed
    ;; neural hit. Options: :newest :loudest :seed-priority :strongest.
    :event :newest
    :params ((threshold :float 0 4 :default 0.55)
             (transpose :int -48 48 :default 0)
             (vel-decay :float 0 2 :default 0.9)
             (vel-reset :int 0 1 :default 0)
             (state-reset :int 0 1 :default 0)
             (dampening :float 0 1 :default 0.14)
             (recovery :float 0 1 :default 0.94))
    :state ((energy :leak (per-step :energy-decay)))
    ;; Fire when energy clears threshold. The else-branch returns nil (no fire).
    :update (if (>= (energy) (param :threshold))
              (do
                (dampen-incoming (param :dampening))
                (if (>= (param :state-reset) 1) (reset-graph-state) nil)
                (emit :note (+ (in-note) (param :transpose))
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

(def g16-name "neural-16-demo")
(def g16-node-count 16)

;; Dropdown option lists. Order is the index space bind-graph maps into.

(def g16-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def g16-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def g16-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
        "Off"))

(def g16-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

;; Route dropdown label -> the internal route the engine stores (:off or a track index).
(def g16-route->internal (label)
  (if (= label "Off") :off (g16-index-of g16-route-options label)))

;; Connection weights: one list-valued widget, so a single state cell is fine.
;; Per-node knobs avoid defstate via bind-graph. The matrix keeps one g16-weights
;; cell rebuilt from the graph on render and patched one cell at a time on edit.

(def g16-ring-weights ()
  (map
    (lambda (r)
      (map
        (lambda (c) (if (= c (if (= r (- g16-node-count 1)) 0 (+ r 1))) 1 0))
        (range 0 g16-node-count)))
    (range 0 g16-node-count)))

(defstate g16-weights (list))

(def g16-read-weights ()
  (map
    (lambda (r) (map (lambda (c) (graph-edge-value g16-name r c :weight)) (range 0 g16-node-count)))
    (range 0 g16-node-count)))

(def g16-set-cell (w r c v)
  (set-nth w r (set-nth (nth w r) c v)))

(def g16-zero-row ()
  (map (lambda (n) 0) (range 0 g16-node-count)))

(def g16-zero-matrix ()
  (map (lambda (n) (g16-zero-row)) (range 0 g16-node-count)))

(def g16-zero-column-matrix ()
  (map (lambda (n) (list 0)) (range 0 g16-node-count)))

(def g16-viz (visualizations)
  (let ((hits (filter (lambda (viz) (= (get viz :name) g16-name)) visualizations)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def g16-viz-matrix (viz field fallback)
  (if viz
    (let ((value (get viz field)))
      (if value value fallback))
    fallback))

;; Init helpers are explicit-only; loading the file does not call these.

(def g16-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g16-name :from r :to c :weight (nth (nth w r) c)))
        (range g16-node-count)))
    (range g16-node-count)))

(def g16-init-ring-defaults ()
  (do
    (set! g16-weights (g16-ring-weights))
    (g16-apply-weights g16-weights)
    (graph-node g16-name 0 :seed-from 0)))

;; Edit helpers dirty the bound widget (reactive-set) and persist the override.

(def g16-edit-num (n field v)
  (do
    (reactive-set "GRAPH" (graph-key g16-name n field) v)
    (graph-node g16-name n field v)))

(def g16-edit-param (n field v)
  (do
    (reactive-set "GRAPH" (graph-key g16-name n field) v)
    (graph-param g16-name n field v)))

(def g16-edit-enum (n field options label internal)
  (do
    (reactive-set "GRAPH" (graph-key g16-name n field) (g16-index-of options label))
    (graph-node g16-name n field internal)))

;; Sequencer-level config is per-pattern like node/edge overrides.
(def g16-edit-config (field v)
  (do
    (reactive-set "GRAPH" (graph-config-key g16-name field) v)
    (graph-config g16-name field v)))

;; UI

(def g16-row-height 1.3)
(def g16-node-width 1.4)
(def g16-control-width 4.8)
(def g16-dropdown-width 6.8)

(def g16-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width g16-control-width :height g16-row-height :font-size 9
    :on-change on-change))

(def g16-pick (key value-index options on-change)
  (dropdown
    :key key
    :value-index value-index :options options
    :width g16-dropdown-width :height g16-row-height :font-size 9
    :on-change on-change))

(def g16-row (n)
  (h-stack :gap 0.4 :align :center
    (label (str n) :width g16-node-width :height g16-row-height :font-size 9 :h-align :center :color :dim)
    (g16-pick (str "graph-16-route-" n)
      (bind-graph g16-name n :route g16-route-options) g16-route-options
      (lambda (v) (g16-edit-enum n :route g16-route-options v (g16-route->internal v))))
    (g16-num (str "graph-16-delay-" n)
      (bind-graph g16-name n :delay) 0 16 1 0
      (lambda (v) (g16-edit-num n :delay v)))
    (g16-num (str "graph-16-transpose-" n)
      (bind-graph g16-name n :transpose) -48 48 1 0
      (lambda (v) (g16-edit-param n :transpose v)))
    (g16-num (str "graph-16-vel-decay-" n)
      (bind-graph g16-name n :vel-decay) 0 2 0.01 2
      (lambda (v) (g16-edit-param n :vel-decay v)))
    (g16-num (str "graph-16-vel-reset-" n)
      (bind-graph g16-name n :vel-reset) 0 1 1 0
      (lambda (v) (g16-edit-param n :vel-reset v)))
    (g16-num (str "graph-16-state-reset-" n)
      (bind-graph g16-name n :state-reset) 0 1 1 0
      (lambda (v) (g16-edit-param n :state-reset v)))
    (g16-num (str "graph-16-dampening-" n)
      (bind-graph g16-name n :dampening) 0 1 0.01 2
      (lambda (v) (g16-edit-param n :dampening v)))
    (g16-num (str "graph-16-recovery-" n)
      (bind-graph g16-name n :recovery) 0 1 0.01 2
      (lambda (v) (g16-edit-param n :recovery v)))
    (g16-pick (str "graph-16-resolution-" n)
      (bind-graph g16-name n :resolution g16-res-options) g16-res-options
      (lambda (v) (g16-edit-enum n :resolution g16-res-options v v)))
    (g16-pick (str "graph-16-quantize-" n)
      (bind-graph g16-name n :quantize g16-quant-options) g16-quant-options
      (lambda (v) (g16-edit-enum n :quantize g16-quant-options v v)))))

(def g16-header ()
  (h-stack :gap 0.4 :align :center
    (label "node" :width g16-node-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "route" :width g16-dropdown-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "delay" :width g16-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "transp" :width g16-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "vel x" :width g16-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "vel rst" :width g16-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "state rst" :width g16-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "dampen" :width g16-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "recover" :width g16-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "res" :width g16-dropdown-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "quant" :width g16-dropdown-width :height 1.0 :font-size 8 :h-align :center :color :dim)))

(def g16-panel (current-pattern graph-visualizations)
  (do
    ;; Re-derive the matrix snapshot from the resolved current-pattern graph. The
    ;; per-node knobs need no sync; bind-graph re-seeds their slots as rows render.
    current-pattern
    (set! g16-weights (g16-read-weights))
    (let ((viz (g16-viz graph-visualizations)))
      (box
        :padding 0.85
        :gap 0.6
        :width 37
        :height 45
        (v-stack :gap 0.5
          (h-stack :gap 0.6 :align :center
            (label "16x16 graph" :width 8 :height 1.2 :font-size 11 :color :foreground)
            (label "reset bars" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (g16-num "graph-16-reset-bars"
              (bind-graph-config g16-name :reset-bars) 0 64 1 0
              (lambda (v) (g16-edit-config :reset-bars v)))
            (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (g16-num "graph-16-max-poly"
              (bind-graph-config g16-name :max-poly) 0 16 1 0
              (lambda (v) (g16-edit-config :max-poly v))))
          (h-stack
            (v-stack :gap 0.5
              (h-stack :gap 0.5 :align :center
                (label "per-node knobs" :width 14 :height 1.2 :font-size 9 :color :dim))
              (v-stack :gap 0.2
                (g16-header)
                (each (range 0 g16-node-count) |n| (g16-row n))))
            (v-stack :gap 0.35
              (box :height 2.5)
              (matrix
                :key "graph-16-trigger-matrix"
                :rows 16
                :cols 1
                :width 1
                :height 24
                :min 0
                :max 1
                :value (g16-viz-matrix viz :trigger-matrix (g16-zero-column-matrix))))
            (v-stack :gap 0.35
              (box :height 2.5)
              (matrix
                :key "graph-16-energy-matrix"
                :rows 16
                :cols 1
                :width 2
                :height 24
                :min 0
                :max 4
                :value (g16-viz-matrix viz :energy-matrix (g16-zero-column-matrix))))
            (v-stack :gap 0.35
              (box :height 2.5)
              (matrix
                :key "graph-16-weight-matrix"
                :rows 16
                :cols 16
                :width 52
                :height 24
                :min 0
                :max 1
                :value g16-weights
                :on-cell-change (lambda (r c v)
                  (do
                    (set! g16-weights (g16-set-cell g16-weights r c v))
                    (graph-edge g16-name :from r :to c :weight v))))))
          (v-stack :gap 0.5
            (label "live dampening (from row -> to col)" :width 18 :height 1.2 :font-size 8 :color :dim)
            (matrix
              :key "graph-16-dampening-matrix"
              :rows 16
              :cols 16
              :width 26
              :height 12
              :control :grid
              :background :black
              :fill :primary
              :min 0
              :max 1
              :value (g16-viz-matrix viz :dampening-matrix (g16-zero-matrix)))))))))

(effect-buffer "*16x16*" (g16-panel SEQ.current-pattern SEQ.graph-visualizations))
(seq-register-step-sequencer-tab "16x16" "*16x16*")
