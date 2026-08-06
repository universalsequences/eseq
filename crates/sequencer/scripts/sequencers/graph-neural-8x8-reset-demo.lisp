;; Graph-mode 8x8 neural sequencer with reset/global timing controls — a playground for the lisp node-graph DSL.
;;
;; Eight all-to-all nodes. Seed it by putting a trigger on track 0 (node 0 subscribes
;; to track 0). The :update rule shapes the emitted/propagated event in lisp: note
;; can either accumulate the per-node transpose around feedback loops or reset to the
;; node/global transpose value, and velocity can either decay each hop or reset to
;; full scale.
;;
;; All nodes route to track 0 by default so every firing is audible on one instrument;
;; change a node's route in your own copy if you want it to drive other tracks. The
;; control panel exposes global transpose/timing batch controls, per-node route /
;; delay / transpose / transpose-reset / vel-decay / vel-reset / resolution /
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
;;   (load "crates/sequencer/scripts/sequencers/graph-neural-8x8-reset-demo.lisp")
;;
;; Loading this file only publishes the graph/UI and syncs controls from the current
;; pattern. It does not write graph overrides. For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(def-sequencer "neural-8x8-reset-demo"
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

(def g8r-name "neural-8x8-reset-demo")
(def g8r-node-count 8)
(def script-buffer-name "*8x8-reset*")
(def script-tab-label "8x8 rst")
(def script-sequencer-name g8r-name)

;; ── dropdown option lists (order is the index space bind-graph maps into) ──

(def g8r-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def g8r-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def g8r-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
        "Off"))

(def g8r-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

;; Route dropdown label -> the internal route the engine stores (:off or a track index).
(def g8r-route->internal (label)
  (if (= label "Off") :off (g8r-index-of g8r-route-options label)))

;; ── connection weights: one list-valued widget, so a single state cell is fine ──
;; (Per-node knobs avoid defstate via bind-graph; the matrix is one widget for all 64
;;  cells, so it keeps a single g8r-weights cell that is rebuilt from the graph on render
;;  and patched one cell at a time on edit.)

(def g8r-ring-weights ()
  (list
    (list 0 1 0 0 0 0 0 0)
    (list 0 0 1 0 0 0 0 0)
    (list 0 0 0 1 0 0 0 0)
    (list 0 0 0 0 1 0 0 0)
    (list 0 0 0 0 0 1 0 0)
    (list 0 0 0 0 0 0 1 0)
    (list 0 0 0 0 0 0 0 1)
    (list 1 0 0 0 0 0 0 0)))

(defstate g8r-weights (list))
(defstate g8r-global-transpose 0)
(defstate g8r-dur-factor 1)
(defstate g8r-delay-factor-index 2)
(defstate g8r-timebase-factor-index 2)

(def g8r-read-weights ()
  (map
    (lambda (r) (map (lambda (c) (graph-edge-value g8r-name r c :weight)) (range 0 g8r-node-count)))
    (range 0 g8r-node-count)))

(def g8r-set-cell (w r c v)
  (set-nth w r (set-nth (nth w r) c v)))

(def g8r-zero-row ()
  (map (lambda (n) 0) (range 0 g8r-node-count)))

(def g8r-zero-matrix ()
  (map (lambda (n) (g8r-zero-row)) (range 0 g8r-node-count)))

(def g8r-zero-column-matrix ()
  (map (lambda (n) (list 0)) (range 0 g8r-node-count)))

(def g8r-viz (visualizations)
  (let ((hits (filter (lambda (viz) (= (get viz :name) g8r-name)) visualizations)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def g8r-viz-matrix (viz field fallback)
  (if viz
    (let ((value (get viz field)))
      (if value value fallback))
    fallback))

;; ── init helpers (explicit-only; loading the file does NOT call these) ──

(def g8r-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g8r-name :from r :to c :weight (nth (nth w r) c)))
        (range g8r-node-count)))
    (range g8r-node-count)))

(def g8r-init-ring-defaults ()
  (do
    (set! g8r-weights (g8r-ring-weights))
    (g8r-apply-weights g8r-weights)
    (graph-node g8r-name 0 :seed-from 0)
    (graph-node g8r-name 1 :seed-from 1)
    (graph-node g8r-name 2 :seed-from 2)
    (graph-node g8r-name 3 :seed-from 4)
    ))

(def script-init-fn ()
  (g8r-init-ring-defaults))

;; ── edit helpers: dirty the bound widget (reactive-set) + persist the override ──
;; `graph-key` gives the canonical GRAPH field name a `bind-graph` handle reads, so a
;; reactive-set on the same key re-renders exactly that one widget.

(def g8r-edit-num (n field v)
  (do
    (reactive-set "GRAPH" (graph-key g8r-name n field) v)
    (graph-node g8r-name n field v)))

(def g8r-edit-param (n field v)
  (do
    (reactive-set "GRAPH" (graph-key g8r-name n field) v)
    (graph-param g8r-name n field v)))

(def g8r-edit-global-param (field v)
  (for-each
    (lambda (n)
      (do
        (reactive-set "GRAPH" (graph-key g8r-name n field) v)
        (graph-param g8r-name n field v)))
    (range 0 g8r-node-count)))

(def g8r-edit-enum (n field options label internal)
  (do
    (reactive-set "GRAPH" (graph-key g8r-name n field) (g8r-index-of options label))
    (graph-node g8r-name n field internal)))

(def g8r-factor-options (list "1/4" "1/2" "1" "2" "4"))

(def g8r-factor-value (label)
  (if (= label "1/4") 0.25
    (if (= label "1/2") 0.5
      (if (= label "2") 2
        (if (= label "4") 4 1)))))

(def g8r-factor-shift (label)
  (if (= label "1/4") -2
    (if (= label "1/2") -1
      (if (= label "2") 1
        (if (= label "4") 2 0)))))

(def g8r-clamp-index (idx len)
  (max 0 (min (- len 1) idx)))

(def g8r-scale-res-label (label shift)
  (nth g8r-res-options
    (g8r-clamp-index (+ (g8r-index-of g8r-res-options label) shift) (len g8r-res-options))))

(def g8r-scale-quant-label (label shift)
  (let ((idx (g8r-index-of g8r-quant-options label)))
    (if (= label "off")
      "off"
      (if (= label "Prh")
        "Prh"
        (if (< idx 8)
          (nth g8r-quant-options (max 1 (min 7 (+ idx shift))))
          (nth g8r-quant-options (+ 8 (g8r-clamp-index (+ (- idx 8) shift) 6))))))))

(def g8r-apply-delay-factor (label)
  (let ((factor (g8r-factor-value label)))
    (do
      (for-each
        (lambda (n)
          (let ((current (graph-node-value g8r-name n :delay)))
            (g8r-edit-num n :delay
              (if (<= current 0)
                0
                (max 1 (round (* current factor)))))))
        (range 0 g8r-node-count))
      (set! g8r-delay-factor-index (g8r-index-of g8r-factor-options "1")))))

(def g8r-apply-timebase-factor (label)
  (let ((shift (g8r-factor-shift label)))
    (do
      (for-each
        (lambda (n)
          (let ((res (g8r-scale-res-label (graph-node-value g8r-name n :resolution) shift))
                (quant (g8r-scale-quant-label (graph-node-value g8r-name n :quantize) shift)))
            (do
              (g8r-edit-enum n :resolution g8r-res-options res res)
              (g8r-edit-enum n :quantize g8r-quant-options quant quant))))
        (range 0 g8r-node-count))
      (set! g8r-timebase-factor-index (g8r-index-of g8r-factor-options "1")))))

;; Sequencer-level config (reset-every / max-poly) is per-pattern like the node/edge
;; overrides; bind via `bind-graph-config`, key via `graph-config-key`.
(def g8r-edit-config (field v)
  (do
    (reactive-set "GRAPH" (graph-config-key g8r-name field) v)
    (graph-config g8r-name field v)))

;; ── UI ──

(def g8r-row-height 1.0)
(def g8r-node-width 1.4)
(def g8r-control-width 6.0)

(def g8r-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :border-color :dim
    :background-color :mixer-strip-bg
    :value value :min lo :max hi :step stp :decimals dec
    :width g8r-control-width :height g8r-row-height :font-size 9
    :on-change on-change))

(def g8r-pick (key value-index options on-change)
  (dropdown
    :key key
    :value-index value-index :options options
    :badge-color :transparent
    :bg-color :mixer-strip-bg
    :border-color :mixer-strip-selected-bg
    :width g8r-control-width :height g8r-row-height :font-size 6
    :on-change on-change))

(def g8r-reset-value (n field)
  (>= (reactive-value (bind-graph g8r-name n field)) 1))

(def g8r-toggle (key value on-change)
  (box
    :width g8r-control-width :height g8r-row-height
    :padding 0 :h-align :center :v-align :center
    (toggle
      :key key
      :value value
      :color :blue
      :off-color :mixer-strip-bg
      :knob-color "#e8ecf4"
      :off-knob-color "#d8dde8"
      :on-change on-change)))

(def g8r-row (n)
  (h-stack :gap 0.4 :align :center
    (label (str n) :width g8r-node-width :height g8r-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
    (g8r-pick (str "graph-8x8-reset-route-" n)
      (bind-graph g8r-name n :route g8r-route-options) g8r-route-options
      (lambda (v) (g8r-edit-enum n :route g8r-route-options v (g8r-route->internal v))))
    (g8r-num (str "graph-8x8-reset-delay-" n)
      (bind-graph g8r-name n :delay) 0 16 1 0
      (lambda (v) (g8r-edit-num n :delay v)))
    (g8r-num (str "graph-8x8-reset-transpose-" n)
      (bind-graph g8r-name n :transpose) -48 48 1 0
      (lambda (v) (g8r-edit-param n :transpose v)))
    (g8r-toggle (str "graph-8x8-reset-transpose-reset-" n)
      (g8r-reset-value n :transpose-reset)
      (lambda (v) (g8r-edit-param n :transpose-reset (if v 1 0))))
    (g8r-num (str "graph-8x8-reset-vel-decay-" n)
      (bind-graph g8r-name n :vel-decay) 0 2 0.01 2
      (lambda (v) (g8r-edit-param n :vel-decay v)))
    (g8r-toggle (str "graph-8x8-reset-vel-reset-" n)
      (g8r-reset-value n :vel-reset)
      (lambda (v) (g8r-edit-param n :vel-reset (if v 1 0))))
    (g8r-num (str "graph-8x8-reset-dampening-" n)
      (bind-graph g8r-name n :dampening) 0 1 0.01 2
      (lambda (v) (g8r-edit-param n :dampening v)))
    (g8r-num (str "graph-8x8-reset-recovery-" n)
      (bind-graph g8r-name n :recovery) 0 1 0.01 2
      (lambda (v) (g8r-edit-param n :recovery v)))
    (g8r-pick (str "graph-8x8-reset-resolution-" n)
      (bind-graph g8r-name n :resolution g8r-res-options) g8r-res-options
      (lambda (v) (g8r-edit-enum n :resolution g8r-res-options v v)))
    (g8r-pick (str "graph-8x8-reset-quantize-" n)
      (bind-graph g8r-name n :quantize g8r-quant-options) g8r-quant-options
      (lambda (v) (g8r-edit-enum n :quantize g8r-quant-options v v)))))

(def g8r-header ()
  (h-stack :gap 0.4 :align :center
    (label "node"   :width g8r-node-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "route"  :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "delay"  :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "transp" :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "trn rst" :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "vel x"  :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "vel rst" :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "dampen" :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "recover" :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "res"    :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "quant"  :width g8r-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)))

(def g8r-panel (current-pattern graph-visualizations)
  (do
    ;; Re-derive the matrix snapshot from the resolved current-pattern graph. The
    ;; per-node knobs need no sync — `bind-graph` re-seeds their slots as the rows
    ;; render below.
    current-pattern
    (set! g8r-weights (g8r-read-weights))
    (set! g8r-global-transpose (graph-param-value g8r-name 0 :global-transpose))
    (set! g8r-dur-factor (graph-param-value g8r-name 0 :dur-factor))
    (let ((viz (g8r-viz graph-visualizations)))
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
                  (label "8x8 graph" :width 8 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
                  (label "reset bars" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (g8r-num "graph-8x8-reset-reset-bars"
                    (bind-graph-config g8r-name :reset-bars) 0 64 1 0
                    (lambda (v) (g8r-edit-config :reset-bars v))))
                (h-stack :gap 0.6 :align :center
                  (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (g8r-num "graph-8x8-reset-max-poly"
                    (bind-graph-config g8r-name :max-poly) 0 16 1 0
                    (lambda (v) (g8r-edit-config :max-poly v))))
                (h-stack :gap 0.6 :align :center
                  (label "global trn" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (g8r-num "graph-8x8-reset-global-transpose"
                    g8r-global-transpose -48 48 1 0
                    (lambda (v)
                      (do
                        (set! g8r-global-transpose v)
                        (g8r-edit-global-param :global-transpose v))))
                  (label "dur x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (g8r-num "graph-8x8-reset-dur-factor"
                    g8r-dur-factor 0 8 0.25 2
                    (lambda (v)
                      (do
                        (set! g8r-dur-factor v)
                        (g8r-edit-global-param :dur-factor v)))))
                (h-stack :gap 0.6 :align :center
                  (label "delay x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (g8r-pick "graph-8x8-reset-delay-factor"
                    g8r-delay-factor-index g8r-factor-options
                    (lambda (v) (g8r-apply-delay-factor v)))
                  (label "res/q x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                  (g8r-pick "graph-8x8-reset-timebase-factor"
                    g8r-timebase-factor-index g8r-factor-options
                    (lambda (v) (g8r-apply-timebase-factor v))))
                )
              (matrix
                :key "graph-8x8-reset-dampening-matrix"
                :rows 8
                :cols 8
                :width 11
                :height 5
                
                :control :grid
                :background-color :bg
                :fill :primary
                :min 0
                :max 1
                :value (g8r-viz-matrix viz :dampening-matrix (g8r-zero-matrix))
                )
              
              (event-view
                :key "graph-8x8-reset-event-view"
                :events (if viz (get viz :event-history) (list))
                :current-beat (if viz (get viz :current-beat) 0)
                :renderer :isometric
                :x :transpose
                :x-min -24
                :x-max 24
                :y :node
                :y-min 0
                :y-max 7
                :z :beat-phase
                :z-min 0
                :z-max 16
                :phase-beats 16
                :auto-rotate true
                :window-beats 16
                :brightness :velocity
                :background :bg
                :width 20
                :height 8)              
	(spectrogram
                  :key "graph-8c-master-spectrogram"
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
                  :background-color :bg
                  :min-color (rgba 0.05 0.05 0.11 1)
                  :mid-color (rgba 0.16 0.66 0.88 1)
                  :max-color (rgba 1.0 0.72 0.28 1)
		  )

              ))
          
          (h-stack
            (box 
              :padding 1
              :border-color :mixer-strip-border
              :background-color :mixer-strip-bg :corner-radius 16
              (v-stack :gap 0.5
                (v-stack :gap 0.2
                  (g8r-header)
                  (each (range 0 g8r-node-count) |n| (g8r-row n)))))
            
            (v-stack :gap 0.35 
              (box 
                :border-width 2
                :border-color :white
                :width 0 :height 1.3)
              (matrix
                :key "graph-8x8-reset-trigger-matrix"
                :rows 8
                :cols 1
                :width 1
                :height 9.5
                :min 0
                :max 1
                :value (g8r-viz-matrix viz :trigger-matrix (g8r-zero-column-matrix))))            
            
            (v-stack :gap 0.35
              (box :width 0 :height 1.3 :font-size 8 :color :dim)
              (matrix
                :key "graph-8x8-reset-energy-matrix"
                :rows 8
                :cols 1
                :width 2
                :height 9.5
                :min 0
                :max 4
                :value (g8r-viz-matrix viz :energy-matrix (g8r-zero-column-matrix))))            
            (v-stack :gap 0.35
              (box :width 0 :height 1.3 :font-size 8 :color :dim)
              (matrix
                :key "graph-8x8-reset-weight-matrix"
                :rows 8
                :cols 8
                :width 26
                :height 9.5
                :min 0
                :background :mixer-strip-bg
                :color (rgba 0.14 0.3 0.9 1)
                :empty-fill-color (rgba 0.04 0.04 0.05 1)
                :stroke-color (rgba 0.36 0.62 0.57 1)
                :stroke-width 1.5
                :stroke-active-only true
                :max 1
                :value g8r-weights
                :on-cell-change (lambda (r c v)
                  (do
                    (set! g8r-weights (g8r-set-cell g8r-weights r c v))
                    (graph-edge g8r-name :from r :to c :weight v))))))
          
          
          )))))

(effect-buffer "*8x8-reset*" (g8r-panel SEQ.current-pattern SEQ.graph-visualizations))
(seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
