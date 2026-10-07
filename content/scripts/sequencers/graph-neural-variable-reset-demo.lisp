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
;; The panel reads and edits the graph through the kinds (kind-bindings spec §14.2k,
;; §14.2s): every control edit is one undo entry, a drag's frames joining one. The
;; batch controls (threshold, global transpose, dur x) set every node's param at once
;; through the graph-* natives, one undo entry each; the threshold every node up to the
;; graph's capacity, so a node that becomes active later already carries it.
;;
;; LEGACY plain script. The supported form of this sequencer is the alez/neural
;; package's `neural` instance kind (content/packages/alez.neural): each instance
;; is host-created, keeps its own id, :state view cells and overrides, and renders
;; in its own `*neural · <label>*` buffer and tab. This copy stays as the reference
;; kind-less script (one global `gvr-name` handle, one `*variable-reset*` buffer),
;; the path plain rack scripts and their tests still exercise.
;;
;; Project scratch entrypoint:
;;   (load "content/scripts/sequencers/graph-neural-variable-reset-demo.lisp")
;;
;; Loading this file only publishes the graph/UI. It does not write graph overrides.
;; For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(import eseq.kinds :refer (graph-of graph-param-named graph-quantize-options
                           graph-max-poly-selection-options))
(import eseq.view-kit :refer (rgb-part index-of nothing))
(import eseq.graph-kit :refer (route-tracks route-options node-route-label set-route-label!
                               weight-rows set-weight! column rack-name res-options
                               set-param-on-nodes!))

;; `def-sequencer` returns the instance handle. Every graph-* native below takes
;; it, so this script also works when a drum rack owns it (attached via the
;; rack menu or `attach-rack-sequencer`): the handle is unambiguous even when
;; the project and a rack both run a copy, and routes then address rack members.
(def gvr-name (def-sequencer "neural-variable-reset-demo"
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

(def gvr-min-node-count 1)
(def gvr-max-node-count 16)
(def script-buffer-name "*variable-reset*")
;; Owned by a rack: the tab wears the rack's name and routes are its members.
(def gvr-owner-rack (graph-owner gvr-name))
(def script-tab-label (if gvr-owner-rack (rack-name gvr-owner-rack) "var rst"))
(def script-sequencer-name "neural-variable-reset-demo")

;; Neural-group assignment (docs/neural-groups-spec.md §3.1): a node's group
;; is the option's index (group A = 0).
(def gvr-group-options (list "A" "B" "C" "D"))
(def gvr-route-off-color (list 0.20 0.21 0.23))

;; The neuron whose weight-matrix column is pressed (-1: none); its row lights.
(def-kind gvr-view
  :key ()
  :state ((selected-neuron -1)))

;; ── init helpers (explicit-only; loading the file does NOT call these) ──
;; They write through the graph-* natives, which answer at once: the graph is
;; not a kind instance until the host's next sync.

(def gvr-node-count ()
  (max gvr-min-node-count
    (min gvr-max-node-count
      (round (graph-config-value gvr-name :node-count)))))

(def gvr-init-ring-defaults ()
  (let ((count (gvr-node-count)))
    (for-each
      (lambda (r)
        (for-each
          (lambda (c)
            (graph-edge gvr-name :from r :to c :weight (if (= c (mod (+ r 1) count)) 1 0)))
          (range 0 count)))
      (range 0 count))
    (for-each
      (lambda (seed)
        (when (< (first seed) count)
          (graph-node gvr-name (first seed) :seed-from (nth seed 1))))
      (list (list 0 0) (list 1 1) (list 2 2) (list 3 4)))))

(def script-init-fn ()
  (gvr-init-ring-defaults))

;; ── UI ──

(def gvr-row-height 1.0)
(def gvr-row-gap 0.2)
(def gvr-row-panel-padding 1.0)
(def gvr-matrix-column-gap 0.35)
(def gvr-route-bar-width 0.28)
(def gvr-node-width 1.4)
(def gvr-control-width 6.0)
(def gvr-seed-control-width 4.8)
(def gvr-group-width 3.0)

(def gvr-matrix-data-height (count)
  (+ (* count gvr-row-height)
     (* (max 0 (- count 1)) gvr-row-gap)))

(def gvr-matrix-header-spacer-height ()
  (+ -0.5 (max 0 (- (+ gvr-row-panel-padding gvr-row-height gvr-row-gap) gvr-matrix-column-gap))))

(def gvr-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :border-color :dim
    :background-color :mixer-strip-bg
    :value value :min lo :max hi :step stp :decimals dec
    :width gvr-control-width :height gvr-row-height :font-size 9
    :on-change on-change))

(def gvr-pick-sized (key value options width on-change)
  (dropdown
    :key key
    :value value :options options
    :badge-color :transparent
    :bg-color :mixer-strip-bg
    :border-color :mixer-strip-selected-bg
    :width width :height gvr-row-height :font-size 6
    :on-change on-change))

(def gvr-pick (key value options on-change)
  (gvr-pick-sized key value options gvr-control-width on-change))

(def gvr-toggle-sized (key width value on-change)
  (box
    :width width :height gvr-row-height
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
(def gvr-param (n name lo hi stp dec)
  (let ((p (graph-param-named n name)))
    (gvr-num (str "graph-variable-reset-" name "-" n.index) #'p.value lo hi stp dec
      (lambda (v) (set! p.value v)))))

;; Node n's 0 / 1 param `name` as a toggle, bound.
(def gvr-switch (n name)
  (let ((p (graph-param-named n name)))
    (gvr-toggle-sized (str "graph-variable-reset-" name "-" n.index) gvr-control-width #'p.value
      (lambda (on) (set! p.value (if on 1 0))))))

;; A param every node carries alike: shows node 0's, sets the first count
;; nodes'.
(def gvr-global-param (g count key name lo hi stp dec)
  (let ((p (graph-param-named (first g.nodes) name)))
    (gvr-num key #'p.value lo hi stp dec
      (lambda (v) (set-param-on-nodes! g count name v)))))

(defwidget gvr-route-color-strip
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
(def gvr-route-bar (n)
  (let ((t n.route)
        (channel (lambda (i) (if t (rgb-part t.color i) (nth gvr-route-off-color i)))))
    (box
      :key (str "graph-variable-reset-route-color-" n.index)
      :width gvr-route-bar-width
      :height gvr-row-height
      :background "gvr-route-color-strip"
      :active (if t 1 0)
      :track-r (channel 0)
      :track-g (channel 1)
      :track-b (channel 2))))

;; Node n's controls, in a subtree of their own: a weight-cell press,
;; which lights a row, re-runs only the rows' highlight boxes (below), which
;; reuse these.
(def gvr-row-controls (n routes)
  (subtree :key (str "graph-variable-reset-row-controls-" n.index)
    (h-stack :gap 0.4 :align :center
      (gvr-route-bar n)
      (label (str n.index) :width gvr-node-width :height gvr-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
      (gvr-pick (str "graph-variable-reset-route-" n.index)
        (node-route-label n) routes
        (lambda (label) (set-route-label! n label)))
      (gvr-pick-sized (str "graph-variable-reset-group-" n.index)
        (nth gvr-group-options n.group) gvr-group-options gvr-group-width
        (lambda (v) (set! n.group (index-of gvr-group-options v))))
      (gvr-toggle-sized (str "graph-variable-reset-seed-route-" n.index) gvr-seed-control-width
        #'n.seed-route
        (lambda (on) (set! n.seed-route on)))
      (gvr-toggle-sized (str "graph-variable-reset-reset-seed-" n.index) gvr-seed-control-width
        (> n.seed-on-reset 0)
        (lambda (on) (set! n.seed-on-reset (if on 1 0))))
      (gvr-num (str "graph-variable-reset-delay-" n.index) #'n.delay 0 16 1 0
        (lambda (v) (set! n.delay v)))
      (gvr-param n "transpose" -48 48 1 0)
      (gvr-switch n "transpose-reset")
      (gvr-param n "vel-decay" 0 2 0.01 2)
      (gvr-switch n "vel-reset")
      (gvr-param n "dampening" 0 1 0.01 2)
      (gvr-param n "recovery" 0 1 0.01 2)
      (gvr-pick (str "graph-variable-reset-resolution-" n.index) n.resolution res-options
        (lambda (v) (set! n.resolution v)))
      (gvr-pick (str "graph-variable-reset-quantize-" n.index) n.quantize graph-quantize-options
        (lambda (v) (set! n.quantize v))))))

;; Node n's row, lit while its weight column is pressed.
(def gvr-row (n routes)
  (subtree :key (str "graph-variable-reset-row-" n.index)
    (box
      :key (str "graph-variable-reset-row-" n.index)
      :height gvr-row-height
      :padding 0
      :selected (= gvr-view.selected-neuron n.index)
      :background-color :transparent
      :selected-background-color :mixer-strip-selected-bg
      :corner-radius 4
      (gvr-row-controls n routes))))

(def gvr-header-label (text width)
  (label text :width width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent))

(def gvr-header ()
  (h-stack :gap 0.4 :align :center
    (label "" :width gvr-route-bar-width :height 1.0 :font-size 1 :bg :transparent)
    (gvr-header-label "node" gvr-node-width)
    (gvr-header-label "route" gvr-control-width)
    (gvr-header-label "grp" gvr-group-width)
    (gvr-header-label "seed rt" gvr-seed-control-width)
    (gvr-header-label "rst seed" gvr-seed-control-width)
    (gvr-header-label "delay" gvr-control-width)
    (gvr-header-label "transp" gvr-control-width)
    (gvr-header-label "trn rst" gvr-control-width)
    (gvr-header-label "vel x" gvr-control-width)
    (gvr-header-label "vel rst" gvr-control-width)
    (gvr-header-label "dampen" gvr-control-width)
    (gvr-header-label "recover" gvr-control-width)
    (gvr-header-label "res" gvr-control-width)
    (gvr-header-label "quant" gvr-control-width)))

(def gvr-config-label (text)
  (label text :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent))

;; A playback column (each node's trigger or energy), in a subtree of its
;; own so playback re-runs only it.
(def gvr-column-matrix (key count width hi values)
  (v-stack :gap gvr-matrix-column-gap
    (label "" :width 0.1 :height (gvr-matrix-header-spacer-height) :font-size 1 :bg :transparent)
    (subtree :key key
      (matrix
        :key key
        :rows count
        :cols 1
        :width width
        :height (gvr-matrix-data-height count)
        :min 0
        :max hi
        :value (column (values))))))

;; Playback is read only inside the visualizers' subtrees: a firing history
;; or a track's notes never rebuild the graph controls.
(def gvr-graph-panel (g)
  (let ((active-count (len g.nodes)))
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
                (gvr-num "graph-variable-reset-node-count" #'g.node-count 1 16 1 0
                  (lambda (v) (set! g.node-count v))))
              (h-stack :gap 0.6 :align :center
                (gvr-config-label "reset bars")
                (gvr-num "graph-variable-reset-reset-bars" #'g.reset-bars 0 64 1 0
                  (lambda (v) (set! g.reset-bars v))))
              (h-stack :gap 0.6 :align :center
                (gvr-config-label "max poly")
                (gvr-num "graph-variable-reset-max-poly" #'g.max-poly 0 16 1 0
                  (lambda (v) (set! g.max-poly v))))
              (h-stack :gap 0.6 :align :center
                (gvr-config-label "poly mode")
                (gvr-pick-sized "graph-variable-reset-max-poly-selection"
                  g.max-poly-selection graph-max-poly-selection-options 9.5
                  (lambda (v) (set! g.max-poly-selection v))))
              (h-stack :gap 0.6 :align :center
                (gvr-config-label "threshold")
                (gvr-global-param g g.max-nodes "graph-variable-reset-threshold" "threshold" 0 4 0.01 2))
              (h-stack :gap 0.6 :align :center
                (gvr-config-label "global trn")
                (gvr-global-param g active-count "graph-variable-reset-global-transpose" "global-transpose" -48 48 1 0)
                (gvr-config-label "dur x")
                (gvr-global-param g active-count "graph-variable-reset-dur-factor" "dur-factor" 0 8 0.25 2)))
            (subtree :key "graph-variable-reset-dampening-matrix"
              (matrix
                :key "graph-variable-reset-dampening-matrix"
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
            (subtree :key "graph-variable-reset-event-view"
              (event-view
                :key "graph-variable-reset-event-view"
                :events g.events
                :current-beat #'g.beat
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
                :height 7))
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
              :height 7.0
              :background-color :bg
              :min-color (rgba 0.05 0.05 0.11 1)
              :mid-color (rgba 0.16 0.66 0.88 1)
              :max-color (rgba 1.0 0.72 0.28 1))))
        (h-stack
          (box
            :padding gvr-row-panel-padding
            :border-color :mixer-strip-border
            :background-color :mixer-strip-bg :corner-radius 16
            (v-stack :gap 0.5
              (v-stack :gap gvr-row-gap
                (gvr-header)
                (let ((routes (route-options g)))
                  (each g.nodes |n| (gvr-row n routes))))))
          (gvr-column-matrix "graph-variable-reset-trigger-matrix" active-count 1 1 (lambda () g.triggers))
          (gvr-column-matrix "graph-variable-reset-energy-matrix" active-count 2 4 (lambda () g.energy))
          (v-stack :gap gvr-matrix-column-gap
            (label "" :width 0.1 :height (gvr-matrix-header-spacer-height) :font-size 1 :bg :transparent)
            (subtree :key "graph-variable-reset-weight-matrix"
              (matrix
                :key "graph-variable-reset-weight-matrix"
                :rows active-count
                :cols active-count
                :width (max 26 (* active-count 3.25))
                :height (gvr-matrix-data-height active-count)
                :min 0
                :background :mixer-strip-bg
                :color (rgba 0.14 0.3 0.9 1)
                :empty-fill-color (rgba 0.04 0.04 0.05 1)
                :stroke-color (rgba 0.36 0.62 0.57 1)
                :stroke-width 1.5
                :stroke-active-only true
                :max 1
                :value (weight-rows g)
                :on-cell-press (lambda (r c) (set! gvr-view.selected-neuron c))
                :on-cell-release (lambda (r c) (set! gvr-view.selected-neuron -1))
                :on-cell-change (lambda (r c v) (set-weight! g r c v))))))
        (box
          :debug-name "graph-variable-reset-piano-panel"
          :padding 1
          :background-color :mixer-strip-bg
          :border-color :mixer-strip-border
          :corner-radius 12
          (subtree :key "graph-variable-reset-piano"
            (piano-keyboard
              :key "graph-variable-reset-piano"
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
(def gvr-panel ()
  (let ((g (graph-of gvr-name)))
    (if g (gvr-graph-panel g) (nothing))))

(effect-buffer "*variable-reset*" (gvr-panel))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
