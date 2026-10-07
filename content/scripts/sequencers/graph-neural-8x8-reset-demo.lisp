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
;; The panel reads and edits the graph through the kinds (kind-bindings spec §14.2k,
;; §14.2s): every control edit is one undo entry, a drag's frames joining one. The
;; batch controls (global transpose, dur x, delay x, res/q x) edit every node at once
;; through the graph-* natives, unrecorded.
;;
;; Project scratch entrypoint:
;;   (load "content/scripts/sequencers/graph-neural-8x8-reset-demo.lisp")
;;
;; Loading this file only publishes the graph/UI. It does not write graph overrides.
;; For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(import eseq.kinds :refer (graph-of graph-param-named graph-quantize-options))
(import eseq.view-kit :refer (nothing))
(import eseq.graph-kit :refer (route-options node-route-label set-route-label! weight-rows
                               set-weight! column rack-name res-options factor-options
                               set-param-on-nodes! scale-delays! shift-timebases!))

;; `def-sequencer` returns the instance handle; every graph-* native below takes
;; it, so this script also works when a drum rack owns it (routes then address
;; rack members and the handle stays unambiguous next to a project-owned copy).
(def g8r-name (def-sequencer "neural-8x8-reset-demo"
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
      (dampening :float 0 1 :default 0)))))

(def g8r-node-count 8)
(def script-buffer-name "*8x8-reset*")
;; Owned by a rack: routes address its members and the tab wears its name.
(def g8r-owner-rack (graph-owner g8r-name))
(def script-tab-label (if g8r-owner-rack (rack-name g8r-owner-rack) "8x8 rst"))
(def script-sequencer-name "neural-8x8-reset-demo")

;; ── init helpers (explicit-only; loading the file does NOT call these) ──
;; They write through the graph-* natives, which answer at once: the graph is
;; not a kind instance until the host's next sync.

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

(def g8r-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g8r-name :from r :to c :weight (nth (nth w r) c)))
        (range g8r-node-count)))
    (range g8r-node-count)))

(def g8r-init-ring-defaults ()
  (g8r-apply-weights (g8r-ring-weights))
  (graph-node g8r-name 0 :seed-from 0)
  (graph-node g8r-name 1 :seed-from 1)
  (graph-node g8r-name 2 :seed-from 2)
  (graph-node g8r-name 3 :seed-from 4))

(def script-init-fn ()
  (g8r-init-ring-defaults))

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

(def g8r-pick (key value options on-change)
  (dropdown
    :key key
    :value value :options options
    :badge-color :transparent
    :bg-color :mixer-strip-bg
    :border-color :mixer-strip-selected-bg
    :width g8r-control-width :height g8r-row-height :font-size 6
    :on-change on-change))

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

;; Node n's param `name`, bound.
(def g8r-param (n name lo hi stp dec)
  (let ((p (graph-param-named n name)))
    (g8r-num (str "graph-8x8-reset-" name "-" n.index) #'p.value lo hi stp dec
      (lambda (v) (set! p.value v)))))

;; Node n's 0 / 1 param `name` as a toggle, bound.
(def g8r-switch (n name)
  (let ((p (graph-param-named n name)))
    (g8r-toggle (str "graph-8x8-reset-" name "-" n.index) #'p.value
      (lambda (on) (set! p.value (if on 1 0))))))

;; A param every node carries alike: shows node 0's, sets every node's.
(def g8r-global-param (g key name lo hi stp dec)
  (let ((p (graph-param-named (first g.nodes) name)))
    (g8r-num key #'p.value lo hi stp dec
      (lambda (v) (set-param-on-nodes! g g8r-node-count name v)))))

(def g8r-row (n routes)
  (subtree :key (str "graph-8x8-reset-row-" n.index)
    (h-stack :gap 0.4 :align :center
      (label (str n.index) :width g8r-node-width :height g8r-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
      (g8r-pick (str "graph-8x8-reset-route-" n.index)
        (node-route-label n) routes
        (lambda (label) (set-route-label! n label)))
      (g8r-num (str "graph-8x8-reset-delay-" n.index) #'n.delay 0 16 1 0
        (lambda (v) (set! n.delay v)))
      (g8r-param n "transpose" -48 48 1 0)
      (g8r-switch n "transpose-reset")
      (g8r-param n "vel-decay" 0 2 0.01 2)
      (g8r-switch n "vel-reset")
      (g8r-param n "dampening" 0 1 0.01 2)
      (g8r-param n "recovery" 0 1 0.01 2)
      (g8r-pick (str "graph-8x8-reset-resolution-" n.index) n.resolution res-options
        (lambda (v) (set! n.resolution v)))
      (g8r-pick (str "graph-8x8-reset-quantize-" n.index) n.quantize graph-quantize-options
        (lambda (v) (set! n.quantize v))))))

(def g8r-header-label (text width)
  (label text :width width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent))

(def g8r-header ()
  (h-stack :gap 0.4 :align :center
    (g8r-header-label "node" g8r-node-width)
    (g8r-header-label "route" g8r-control-width)
    (g8r-header-label "delay" g8r-control-width)
    (g8r-header-label "transp" g8r-control-width)
    (g8r-header-label "trn rst" g8r-control-width)
    (g8r-header-label "vel x" g8r-control-width)
    (g8r-header-label "vel rst" g8r-control-width)
    (g8r-header-label "dampen" g8r-control-width)
    (g8r-header-label "recover" g8r-control-width)
    (g8r-header-label "res" g8r-control-width)
    (g8r-header-label "quant" g8r-control-width)))

(def g8r-config-label (text)
  (label text :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent))

;; A playback column (each node's trigger or energy), in a subtree of its
;; own so playback re-runs only it.
(def g8r-column-matrix (spacer key width hi values)
  (v-stack :gap 0.35
    spacer
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

(def g8r-graph-panel (g)
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
              (g8r-config-label "reset bars")
              (g8r-num "graph-8x8-reset-reset-bars" #'g.reset-bars 0 64 1 0
                (lambda (v) (set! g.reset-bars v))))
            (h-stack :gap 0.6 :align :center
              (g8r-config-label "max poly")
              (g8r-num "graph-8x8-reset-max-poly" #'g.max-poly 0 16 1 0
                (lambda (v) (set! g.max-poly v))))
            (h-stack :gap 0.6 :align :center
              (g8r-config-label "global trn")
              (g8r-global-param g "graph-8x8-reset-global-transpose" "global-transpose" -48 48 1 0)
              (g8r-config-label "dur x")
              (g8r-global-param g "graph-8x8-reset-dur-factor" "dur-factor" 0 8 0.25 2))
            ;; One-shot batch edits: the factor applies and the picker shows 1 again.
            (h-stack :gap 0.6 :align :center
              (g8r-config-label "delay x")
              (g8r-pick "graph-8x8-reset-delay-factor" "1" factor-options
                (lambda (label) (scale-delays! g label)))
              (g8r-config-label "res/q x")
              (g8r-pick "graph-8x8-reset-timebase-factor" "1" factor-options
                (lambda (label) (shift-timebases! g label)))))
          (subtree :key "graph-8x8-reset-dampening-matrix"
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
              :value g.dampening))
          (subtree :key "graph-8x8-reset-event-view"
            (event-view
              :key "graph-8x8-reset-event-view"
              :events g.events
              :current-beat #'g.beat
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
              :height 8))
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
            :max-color (rgba 1.0 0.72 0.28 1))))
      (h-stack
        (box
          :padding 1
          :border-color :mixer-strip-border
          :background-color :mixer-strip-bg :corner-radius 16
          (v-stack :gap 0.5
            (v-stack :gap 0.2
              (g8r-header)
              (let ((routes (route-options g)))
                (each g.nodes |n| (g8r-row n routes))))))
        (g8r-column-matrix (box :border-width 2 :border-color :white :width 0 :height 1.3)
          "graph-8x8-reset-trigger-matrix" 1 1 (lambda () g.triggers))
        (g8r-column-matrix (box :width 0 :height 1.3)
          "graph-8x8-reset-energy-matrix" 2 4 (lambda () g.energy))
        (v-stack :gap 0.35
          (box :width 0 :height 1.3)
          (subtree :key "graph-8x8-reset-weight-matrix"
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
              :value (weight-rows g)
              :on-cell-change (lambda (r c v) (set-weight! g r c v)))))))))

;; The panel, empty until the host publishes the graph (at its next sync).
(def g8r-panel ()
  (let ((g (graph-of g8r-name)))
    (if g (g8r-graph-panel g) (nothing))))

(effect-buffer "*8x8-reset*" (g8r-panel))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
