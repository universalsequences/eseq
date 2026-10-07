;; Graph-mode 16-neuron sequencer - a playground for the lisp node-graph DSL.
;;
;; Sixteen all-to-all nodes. Seed it by putting a trigger on track 0 (node 0
;; subscribes to track 0). The :update rule shapes the emitted/propagated event in
;; lisp: note accumulates the per-node transpose around feedback loops, and velocity
;; is either scaled by a per-node vel-decay each hop or reset to 1.0.
;;
;; All nodes route to track 0 by default so every firing is audible on one
;; instrument. Change a node's route in your own copy if you want it to drive other
;; tracks. The control panel exposes per-node route / delay / transpose /
;; transpose-reset / vel-decay / vel-reset / state-reset / resolution / quantize plus
;; the 16x16 connection-weight matrix.
;;
;; The panel reads and edits the graph through the kinds (kind-bindings spec §14.2k,
;; §14.2s): every control edit is one undo entry, a drag's frames joining one. The
;; timing controls (dur x, swing) set every node's param at once through the graph-*
;; natives, unrecorded.
;;
;; Project scratch entrypoint:
;;   (load "content/scripts/sequencers/graph-neural-16-demo.lisp")
;;
;; Loading this file only publishes the graph/UI. It does not write graph overrides.
;; For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(import eseq.kinds :refer (graph-of graph-param-named graph-quantize-options))
(import eseq.view-kit :refer (nothing))
(import eseq.graph-kit :refer (route-options node-route-label set-route-label! weight-rows
                               set-weight! column rack-name res-options
                               set-param-on-nodes!))

;; `def-sequencer` returns the instance handle; every graph-* native below takes
;; it, so this script also works when a drum rack owns it (routes then address
;; rack members and the handle stays unambiguous next to a project-owned copy).
(def g16-name (def-sequencer "neural-16-demo"
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
      (dampening :float 0 1 :default 0)))))

(def g16-node-count 16)
(def script-buffer-name "*16x16*")
;; Owned by a rack: routes address its members and the tab wears its name.
(def g16-owner-rack (graph-owner g16-name))
(def script-tab-label (if g16-owner-rack (rack-name g16-owner-rack) "16x16"))
(def script-sequencer-name "neural-16-demo")

;; ── init helpers (explicit-only; loading the file does NOT call these) ──
;; They write through the graph-* natives, which answer at once: the graph is
;; not a kind instance until the host's next sync.

(def g16-ring-weights ()
  (map
    (lambda (r)
      (map
        (lambda (c) (if (= c (if (= r (- g16-node-count 1)) 0 (+ r 1))) 1 0))
        (range 0 g16-node-count)))
    (range 0 g16-node-count)))

(def g16-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g16-name :from r :to c :weight (nth (nth w r) c)))
        (range g16-node-count)))
    (range g16-node-count)))

(def g16-init-ring-defaults ()
  (g16-apply-weights (g16-ring-weights))
  (graph-node g16-name 0 :seed-from 0))

(def script-init-fn ()
  (g16-init-ring-defaults))

;; ── UI ──

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

(def g16-pick (key value options on-change)
  (dropdown
    :key key
    :value value :options options
    :width g16-dropdown-width :height g16-row-height :font-size 9
    :on-change on-change))

;; Node n's param `name`, bound.
(def g16-param (n name lo hi stp dec)
  (let ((p (graph-param-named n name)))
    (g16-num (str "graph-16-" name "-" n.index) #'p.value lo hi stp dec
      (lambda (v) (set! p.value v)))))

;; A param every node carries alike: shows node 0's, sets every node's.
(def g16-global-param (g key name lo hi stp dec)
  (let ((p (graph-param-named (first g.nodes) name)))
    (g16-num key #'p.value lo hi stp dec
      (lambda (v) (set-param-on-nodes! g g16-node-count name v)))))

(def g16-row (n routes)
  (subtree :key (str "graph-16-row-" n.index)
    (h-stack :gap 0.4 :align :center
      (label (str n.index) :width g16-node-width :height g16-row-height :font-size 9 :h-align :center :color :dim)
      (g16-pick (str "graph-16-route-" n.index)
        (node-route-label n) routes
        (lambda (label) (set-route-label! n label)))
      (g16-num (str "graph-16-delay-" n.index) #'n.delay 0 16 1 0
        (lambda (v) (set! n.delay v)))
      (g16-param n "transpose" -48 48 1 0)
      (g16-param n "transpose-reset" 0 1 1 0)
      (g16-param n "vel-decay" 0 2 0.01 2)
      (g16-param n "vel-reset" 0 1 1 0)
      (g16-param n "state-reset" 0 1 1 0)
      (g16-param n "dampening" 0 1 0.01 2)
      (g16-param n "recovery" 0 1 0.01 2)
      (g16-pick (str "graph-16-resolution-" n.index) n.resolution res-options
        (lambda (v) (set! n.resolution v)))
      (g16-pick (str "graph-16-quantize-" n.index) n.quantize graph-quantize-options
        (lambda (v) (set! n.quantize v))))))

(def g16-header-label (text width)
  (label text :width width :height 1.0 :font-size 8 :h-align :center :color :dim))

(def g16-header ()
  (h-stack :gap 0.4 :align :center
    (g16-header-label "node" g16-node-width)
    (g16-header-label "route" g16-dropdown-width)
    (g16-header-label "delay" g16-control-width)
    (g16-header-label "transp" g16-control-width)
    (g16-header-label "trn rst" g16-control-width)
    (g16-header-label "vel x" g16-control-width)
    (g16-header-label "vel rst" g16-control-width)
    (g16-header-label "state rst" g16-control-width)
    (g16-header-label "dampen" g16-control-width)
    (g16-header-label "recover" g16-control-width)
    (g16-header-label "res" g16-dropdown-width)
    (g16-header-label "quant" g16-dropdown-width)))

(def g16-config-label (text width)
  (label text :width width :height 1.2 :font-size 9 :h-align :right :color :dim))

;; A playback column (each node's trigger or energy), in a subtree of its
;; own so playback re-runs only it.
(def g16-column-matrix (key width hi values)
  (v-stack :gap 0.35
    (box :height 2.5)
    (subtree :key key
      (matrix
        :key key
        :rows 16
        :cols 1
        :width width
        :height 24
        :min 0
        :max hi
        :value (column (values))))))

(def g16-graph-panel (g)
  (box
    :padding 0.85
    :gap 0.6
    :width 37
    :height 47
    (v-stack :gap 0.5
      (h-stack
        (v-stack
          (h-stack :gap 0.6 :align :center
            (label "16x16 graph" :width 8 :height 1.2 :font-size 11 :color :foreground)
            (g16-config-label "reset bars" 6)
            (g16-num "graph-16-reset-bars" #'g.reset-bars 0 64 1 0
              (lambda (v) (set! g.reset-bars v)))
            (g16-config-label "max poly" 6)
            (g16-num "graph-16-max-poly" #'g.max-poly 0 16 1 0
              (lambda (v) (set! g.max-poly v))))
          (h-stack :gap 0.6 :align :center
            (label "timing" :width 8 :height 1.2 :font-size 9 :color :dim)
            (g16-config-label "dur x" 6)
            (g16-global-param g "graph-16-dur-factor" "dur-factor" 0 8 0.25 2)
            (g16-config-label "swing" 6)
            (g16-global-param g "graph-16-swing" "swing" 50 75 1 0)))
        (subtree :key "graph-16-event-view"
          (event-view
            :key "graph-16-event-view"
            :events g.events
            :current-beat #'g.beat
            :renderer :isometric
            :x :transpose
            :x-min -24
            :x-max 24
            :y :node
            :y-min 0
            :y-max 15
            :z :beat-phase
            :z-min 0
            :z-max 16
            :phase-beats 16
            :window-beats 16
            :brightness :velocity
            :cube-padding 0
            :width 12
            :height 6)))
      (h-stack
        (v-stack :gap 0.5
          (h-stack :gap 0.5 :align :center
            (label "per-node knobs" :width 14 :height 1.2 :font-size 9 :color :dim))
          (v-stack :gap 0.2
            (g16-header)
            (let ((routes (route-options g)))
              (each g.nodes |n| (g16-row n routes)))))
        (g16-column-matrix "graph-16-trigger-matrix" 1 1 (lambda () g.triggers))
        (g16-column-matrix "graph-16-energy-matrix" 2 4 (lambda () g.energy))
        (v-stack :gap 0.35
          (box :height 2.5)
          (subtree :key "graph-16-weight-matrix"
            (matrix
              :key "graph-16-weight-matrix"
              :rows 16
              :cols 16
              :width 52
              :height 24
              :min 0
              :max 1
              :value (weight-rows g)
              :on-cell-change (lambda (r c v) (set-weight! g r c v))))))
      (v-stack :gap 0.5
        (label "live dampening (from row -> to col)" :width 18 :height 1.2 :font-size 8 :color :dim)
        (subtree :key "graph-16-dampening-matrix"
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
            :value g.dampening))))))

;; The panel, empty until the host publishes the graph (at its next sync).
(def g16-panel ()
  (let ((g (graph-of g16-name)))
    (if g (g16-graph-panel g) (nothing))))

(effect-buffer "*16x16*" (g16-panel))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
