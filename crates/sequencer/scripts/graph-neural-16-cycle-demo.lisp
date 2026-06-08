;; Graph-mode 16-neuron sequencer - round-robin resolution/quantize cycles edition.
;;
;; Same all-to-all neural net as graph-neural-16-demo, but the per-node resolution and
;; quantize fields are *round-robin cycles* instead of single values, edited as a
;; space-separated mini-notation string in a text-input (à la Autechre's per-step
;; resolution columns / Tidal's `slowcat`). Each node advances one slot through its
;; cycle every time it FIRES (not every evaluation), and the cycle position resets with
;; the node's state on graph reset.
;;
;;   resolution "16 16 16 16 16 4"  -> mostly 1/16, breaks out to 1/4 every 6th fire
;;   quantize   "off"               -> a single-slot cycle (ordinary static quantize)
;;
;; Tokens use the same vocabulary as the dropdowns: 1 2 4 8 16 32 64 2T..64T Prh (and
;; `off` for quantize). Unparseable tokens are dropped, so a half-typed field is safe.
;; The strings are stored in local state and only re-read from the graph on a pattern
;; switch, so typing (including spaces) isn't clobbered mid-keystroke.
;;
;; Project scratch entrypoint:
;;   (load "crates/sequencer/scripts/graph-neural-16-cycle-demo.lisp")
;;
;; Loading this file only publishes the graph/UI and syncs controls from the current
;; pattern. It does not write graph overrides. For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(def-sequencer "neural-16-cycle-demo"
  :shape (line 16)
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
    ;; (note/velocity). :loudest keeps the highest-velocity arrival, so a
    ;; full-velocity seed punches through instead of being clobbered by a decayed
    ;; neural hit. Options: :newest :loudest :seed-priority :strongest.
    :event :loudest
    :params ((threshold :float 0 4 :default 0.8)
      (transpose :int -48 48 :default 0)
      (transpose-reset :int 0 1 :default 0)
      (dur-factor :float 0 8 :default 1)
      (swing :float 50 75 :default 62)
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
        (emit :note (if (>= (param :transpose-reset) 1)
            (param :transpose)
            (+ (in-note) (param :transpose)))
          :dur (* (delay) (param :dur-factor))
          :swing (swing (param :swing) :16)
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

(def g16c-name "neural-16-cycle-demo")
(def g16c-node-count 16)
(def script-buffer-name "*16x16-cycle*")
(def script-tab-label "16x16 cyc")

;; Dropdown option lists. Order is the index space bind-graph maps into.

(def g16c-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def g16c-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def g16c-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
        "Off"))

(def g16c-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

;; Route dropdown label -> the internal route the engine stores (:off or a track index).
(def g16c-route->internal (label)
  (if (= label "Off") :off (g16c-index-of g16c-route-options label)))

;; Connection weights: one list-valued widget, so a single state cell is fine.
;; Per-node knobs avoid defstate via bind-graph. The matrix keeps one g16c-weights
;; cell rebuilt from the graph on render and patched one cell at a time on edit.

(def g16c-ring-weights ()
  (map
    (lambda (r)
      (map
        (lambda (c) (if (= c (if (= r (- g16c-node-count 1)) 0 (+ r 1))) 1 0))
        (range 0 g16c-node-count)))
    (range 0 g16c-node-count)))

(defstate g16c-weights (list))
(defstate g16c-dur-factor 1)
(defstate g16c-swing 62)

;; Per-node resolution/quantize cycles, held as raw mini-notation strings. Unlike the
;; weight matrix (re-read every render), these are only refreshed from the graph when the
;; pattern changes (tracked by g16c-cycles-pattern) so in-progress typing - spaces and
;; all - isn't clobbered by the canonical re-serialization mid-keystroke.
(defstate g16c-res-cycles (list))
(defstate g16c-quant-cycles (list))
(defstate g16c-cycles-pattern -1)

(def g16c-read-weights ()
  (map
    (lambda (r) (map (lambda (c) (graph-edge-value g16c-name r c :weight)) (range 0 g16c-node-count)))
    (range 0 g16c-node-count)))

;; Read every node's resolution-cycle / quantize-cycle back as a canonical
;; space-separated string ("16 16 16 16 16 4").
(def g16c-read-cycles (field)
  (map (lambda (n) (graph-node-value g16c-name n field)) (range 0 g16c-node-count)))

(def g16c-nth-string (xs n)
  (if (< n (len xs)) (nth xs n) ""))

;; Resync the local cycle strings from the graph only when the pattern changed.
(def g16c-sync-cycles (current-pattern)
  (if (= g16c-cycles-pattern current-pattern)
    nil
    (do
      (set! g16c-res-cycles (g16c-read-cycles :resolution-cycle))
      (set! g16c-quant-cycles (g16c-read-cycles :quantize-cycle))
      (set! g16c-cycles-pattern current-pattern))))

;; Edit one node's resolution or quantize cycle: store the raw text locally (so the
;; text-input shows exactly what's typed) and write it through to the graph, which
;; parses the mini-notation leniently into a per-pattern override.
(def g16c-edit-cycle (n field s)
  (do
    (if (= field :resolution)
      (set! g16c-res-cycles (set-nth g16c-res-cycles n s))
      (set! g16c-quant-cycles (set-nth g16c-quant-cycles n s)))
    (graph-node g16c-name n field s)))

(def g16c-set-cell (w r c v)
  (set-nth w r (set-nth (nth w r) c v)))

(def g16c-zero-row ()
  (map (lambda (n) 0) (range 0 g16c-node-count)))

(def g16c-zero-matrix ()
  (map (lambda (n) (g16c-zero-row)) (range 0 g16c-node-count)))

(def g16c-zero-column-matrix ()
  (map (lambda (n) (list 0)) (range 0 g16c-node-count)))

(def g16c-viz (visualizations)
  (let ((hits (filter (lambda (viz) (= (get viz :name) g16c-name)) visualizations)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def g16c-viz-matrix (viz field fallback)
  (if viz
    (let ((value (get viz field)))
      (if value value fallback))
    fallback))

;; Init helpers are explicit-only; loading the file does not call these.

(def g16c-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g16c-name :from r :to c :weight (nth (nth w r) c)))
        (range g16c-node-count)))
    (range g16c-node-count)))

(def g16c-init-ring-defaults ()
  (do
    (set! g16c-weights (g16c-ring-weights))
    (g16c-apply-weights g16c-weights)
    (graph-node g16c-name 0 :seed-from 0)
    ;; Showcase the feature: node 0 runs mostly 1/16 with a 1/4 break-out every 6th fire,
    ;; node 1 lurches on a 3-slot cycle that phases against it.
    (graph-node g16c-name 0 :resolution "16 16 16 16 16 4")
    (graph-node g16c-name 1 :resolution "16 8 16")
    ;; Force a resync on the next render so the new cycles show in the text fields.
    (set! g16c-cycles-pattern -1)))

(def script-init-fn ()
  (g16c-init-ring-defaults))

;; Edit helpers dirty the bound widget (reactive-set) and persist the override.

(def g16c-edit-num (n field v)
  (do
    (reactive-set "GRAPH" (graph-key g16c-name n field) v)
    (graph-node g16c-name n field v)))

(def g16c-edit-param (n field v)
  (do
    (reactive-set "GRAPH" (graph-key g16c-name n field) v)
    (graph-param g16c-name n field v)))

(def g16c-edit-global-param (field v)
  (for-each
    (lambda (n)
      (do
        (reactive-set "GRAPH" (graph-key g16c-name n field) v)
        (graph-param g16c-name n field v)))
    (range 0 g16c-node-count)))

(def g16c-edit-enum (n field options label internal)
  (do
    (reactive-set "GRAPH" (graph-key g16c-name n field) (g16c-index-of options label))
    (graph-node g16c-name n field internal)))

;; Sequencer-level config is per-pattern like node/edge overrides.
(def g16c-edit-config (field v)
  (do
    (reactive-set "GRAPH" (graph-config-key g16c-name field) v)
    (graph-config g16c-name field v)))

;; UI

(def g16c-row-height 1.3)
(def g16c-node-width 1.4)
(def g16c-control-width 4.8)
(def g16c-dropdown-width 6.8)
(def g16c-cycle-width 9.5)

(def g16c-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width g16c-control-width :height g16c-row-height :font-size 9
    :on-change on-change))

(def g16c-pick (key value-index options on-change)
  (dropdown
    :key key
    :value-index value-index :options options
    :width g16c-dropdown-width :height g16c-row-height :font-size 9
    :on-change on-change))

;; Text field for a resolution/quantize cycle. `value` is the raw mini-notation string;
;; `on-change` receives the new string verbatim.
(def g16c-cycle-input (key value placeholder on-change)
  (text-input
    :key key
    :value value :placeholder placeholder
    :width g16c-cycle-width :height g16c-row-height :font-size 9
    :on-change on-change))

(def g16c-row (n)
  (h-stack :gap 0.4 :align :center
    (label (str n) :width g16c-node-width :height g16c-row-height :font-size 9 :h-align :center :color :dim)
    (g16c-pick (str "graph-16-route-" n)
      (bind-graph g16c-name n :route g16c-route-options) g16c-route-options
      (lambda (v) (g16c-edit-enum n :route g16c-route-options v (g16c-route->internal v))))
    (g16c-num (str "graph-16-delay-" n)
      (bind-graph g16c-name n :delay) 0 16 1 0
      (lambda (v) (g16c-edit-num n :delay v)))
    (g16c-num (str "graph-16-transpose-" n)
      (bind-graph g16c-name n :transpose) -48 48 1 0
      (lambda (v) (g16c-edit-param n :transpose v)))
    (g16c-num (str "graph-16-transpose-reset-" n)
      (bind-graph g16c-name n :transpose-reset) 0 1 1 0
      (lambda (v) (g16c-edit-param n :transpose-reset v)))
    (g16c-num (str "graph-16-vel-decay-" n)
      (bind-graph g16c-name n :vel-decay) 0 2 0.01 2
      (lambda (v) (g16c-edit-param n :vel-decay v)))
    (g16c-num (str "graph-16-vel-reset-" n)
      (bind-graph g16c-name n :vel-reset) 0 1 1 0
      (lambda (v) (g16c-edit-param n :vel-reset v)))
    (g16c-num (str "graph-16-state-reset-" n)
      (bind-graph g16c-name n :state-reset) 0 1 1 0
      (lambda (v) (g16c-edit-param n :state-reset v)))
    (g16c-num (str "graph-16-dampening-" n)
      (bind-graph g16c-name n :dampening) 0 1 0.01 2
      (lambda (v) (g16c-edit-param n :dampening v)))
    (g16c-num (str "graph-16-recovery-" n)
      (bind-graph g16c-name n :recovery) 0 1 0.01 2
      (lambda (v) (g16c-edit-param n :recovery v)))
    (g16c-cycle-input (str "graph-16c-resolution-" n)
      (g16c-nth-string g16c-res-cycles n) "16 16 16 16 16 4"
      (lambda (s) (g16c-edit-cycle n :resolution s)))
    (g16c-cycle-input (str "graph-16c-quantize-" n)
      (g16c-nth-string g16c-quant-cycles n) "off"
      (lambda (s) (g16c-edit-cycle n :quantize s)))))

(def g16c-header ()
  (h-stack :gap 0.4 :align :center
    (label "node" :width g16c-node-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "route" :width g16c-dropdown-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "delay" :width g16c-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "transp" :width g16c-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "trn rst" :width g16c-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "vel x" :width g16c-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "vel rst" :width g16c-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "state rst" :width g16c-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "dampen" :width g16c-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "recover" :width g16c-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "res cycle" :width g16c-cycle-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "quant cycle" :width g16c-cycle-width :height 1.0 :font-size 8 :h-align :center :color :dim)))

(def g16c-panel (current-pattern graph-visualizations)
  (do
    ;; Re-derive the matrix snapshot from the resolved current-pattern graph. The
    ;; per-node knobs need no sync; bind-graph re-seeds their slots as rows render.
    current-pattern
    ;; Cycle text fields refresh only on a pattern switch (preserves in-progress typing).
    (g16c-sync-cycles current-pattern)
    (set! g16c-weights (g16c-read-weights))
    (set! g16c-dur-factor (graph-param-value g16c-name 0 :dur-factor))
    (set! g16c-swing (graph-param-value g16c-name 0 :swing))
    (let ((viz (g16c-viz graph-visualizations)))
      (box
        :padding 0.85
        :gap 0.6
        :width 42
        :height 47
        (v-stack :gap 0.5
          (h-stack :gap 0.6 :align :center
            (label "16x16 graph" :width 8 :height 1.2 :font-size 11 :color :foreground)
            (label "reset bars" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (g16c-num "graph-16-reset-bars"
              (bind-graph-config g16c-name :reset-bars) 0 64 1 0
              (lambda (v) (g16c-edit-config :reset-bars v)))
            (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (g16c-num "graph-16-max-poly"
              (bind-graph-config g16c-name :max-poly) 0 16 1 0
              (lambda (v) (g16c-edit-config :max-poly v))))
          (h-stack :gap 0.6 :align :center
            (label "timing" :width 8 :height 1.2 :font-size 9 :color :dim)
            (label "dur x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (g16c-num "graph-16-dur-factor"
              g16c-dur-factor 0 8 0.25 2
              (lambda (v)
                (do
                  (set! g16c-dur-factor v)
                  (g16c-edit-global-param :dur-factor v))))
            (label "swing" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (g16c-num "graph-16-swing"
              g16c-swing 50 75 1 0
              (lambda (v)
                (do
                  (set! g16c-swing v)
                  (g16c-edit-global-param :swing v)))))
          (h-stack
            (v-stack :gap 0.5
              (h-stack :gap 0.5 :align :center
                (label "per-node knobs" :width 14 :height 1.2 :font-size 9 :color :dim))
              (v-stack :gap 0.2
                (g16c-header)
                (each (range 0 g16c-node-count) |n| (g16c-row n))))
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
                :value (g16c-viz-matrix viz :trigger-matrix (g16c-zero-column-matrix))))
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
                :value (g16c-viz-matrix viz :energy-matrix (g16c-zero-column-matrix))))
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
                :value g16c-weights
                :on-cell-change (lambda (r c v)
                  (do
                    (set! g16c-weights (g16c-set-cell g16c-weights r c v))
                    (graph-edge g16c-name :from r :to c :weight v))))))
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
              :value (g16c-viz-matrix viz :dampening-matrix (g16c-zero-matrix)))))))))

(effect-buffer "*16x16-cycle*" (g16c-panel SEQ.current-pattern SEQ.graph-visualizations))
(seq-register-step-sequencer-tab script-tab-label script-buffer-name)
