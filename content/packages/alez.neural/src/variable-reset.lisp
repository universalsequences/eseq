;; alez.neural — factory neural / graph sequencers.
;; variable-reset: the `neural` instance kind — a graph-mode neural sequencer
;; with a variable node count, per-node seed / delay / transpose controls and
;; the live NxN weight matrix.
;;
;; Attaching the package (Packages tab) or (import alez.neural.variable-reset)
;; registers the kind and creates nothing. Each instance ("New neural" on the
;; module row, or "New neural in rack" on a rack) is the host's: it publishes
;; the :sequencer body under its own id with its own overrides, gets its own
;; `*neural · <label>*` buffer and step tab, and renders (gvr-panel self) there
;; (docs/instance-kinds-spec.md).
;;
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
;; change a node's route to drive other tracks. The control panel exposes threshold /
;; max-poly selection / global transpose batch controls, per-node route / seed / delay
;; / transpose / transpose-reset / vel-decay / vel-reset / resolution / quantize plus
;; the active NxN connection-weight matrix.
;;
;; STATE: every helper takes the instance (`self`) as its first argument, and
;; the three homes of state stay apart:
;; - document (routes, weights, node params, process patches) is the
;;   instance's graph, read and edited through the graph kinds (kind-bindings
;;   spec §14.2k, §14.2m, §14.2s): `(graph-of self)`, its `graph-node`s, their
;;   `graph-param`s and `process`es. Controls bind their fields (`#'n.delay`,
;;   `#'p.value`) and edit with `set!` or the process setters, one undo entry
;;   each (a drag's frames join one);
;; - view (expanded node, selected neuron, map arming, piano depth) is the
;;   kind's `:state`, one cell per instance;
;; - nothing is global, so two instances never share a selection or a cache.
;;
;; A fresh instance gets the ring patch from :on-create (gvr-init-ring-defaults);
;; call (alez.neural.variable-reset/gvr-init-ring-defaults inst) to write it again.

(module alez.neural.variable-reset)

(import eseq.kinds :refer (tracks generators graph-of graph-param-named gate-generator! process-library
                           set-process-enabled! set-inlet! add-process! remove-process! bind-port!
                           clear-port! graph-quantize-options graph-max-poly-selection-options))
(import eseq.view-kit :refer (rgb-part index-of nothing named))
(import eseq.graph-kit :refer (route-tracks route-track route-label node-route-label set-route-label!
                               weight-rows set-weight! column res-options set-param-on-nodes!))

(export gvr-panel gvr-init-ring-defaults gvr-expand-node gvr-map-arm gvr-jakis gvr-route-menu
        gvr-route-label gvr-set-route-label!)

(def gvr-min-node-count 1)
(def gvr-max-node-count 16)

;; Neural-group assignment (docs/neural-groups-spec.md §3.1): a node's group
;; is the option's index (group A = 0).
(def gvr-group-options (list "A" "B" "C" "D"))
(def gvr-route-off-color (list 0.20 0.21 0.23))

;; "attached to <rack>" chip in the config block's top-right corner, dressed
;; like the sample browser's tag chips: the rack that owns the graph (its
;; routes are the rack's members), or nothing.
(def gvr-owner-rack-badge (g)
  (let ((rack g.owner))
    (when rack
      (h-stack
        (button (str "attached to " (substring rack.name 0 14))
          :key "graph-variable-reset-owner-rack"
          :variant :ghost
          :background-color :mixer-control-bg
          :color :dimmer
          :border-color :none
          :height 1.0 :padding 0.8532 :font-size 12.0 :corner-radius 13)))))

;; ── routes (docs/jaki-trig-modes-spec.md §5-§6) ──
;; A node routes to a track of its graph's owner (eseq.graph-kit: a rack's
;; members, else the project's tracks). Past the tracks, the route menu lists
;; the jaki instances with this graph's owner (the project, or its rack),
;; twice:
;;   → jaki 1   the node gates it instead of playing a note: its fires open the
;;              pattern for their duration, their note adds and their velocity
;;              scales; the jaki's own mode decides whether each fire restarts
;;              it or picks up where it stopped.
;;   ↺ jaki 1   each fire only restarts its pattern, whatever its mode (a
;;              looping jaki jumps to the top of its phrase).
;; The menu is its own list: track-typed process inlets keep offering tracks
;; only (gvr-track-inlet-options).
(def gvr-jaki-kind "alez/jaki:jaki")

;; The tracks graph g's nodes route to, as route menu labels.
(def gvr-route-tracks-labels (g)
  (map (lambda (t) (route-label g t)) (route-tracks g)))

;; The jakis graph g's nodes can gate, as (dict :id :label): the generators
;; of g's owner whose instance is a jaki.
(def gvr-jakis (g)
  (reduce
    (lambda (jakis gen)
      (let ((inst (instance-ref gen.gid)))
        (if (and (= gen.owner g.owner) inst (= inst.kind gvr-jaki-kind))
          (append jakis (list (dict :id gen.gid :label inst.label)))
          jakis)))
    (list)
    (generators)))

;; A jaki's menu label: its label, plus " #id" when another of the jakis
;; shares it. The dropdown hands back only the chosen text, so two
;; "→ drums" entries would both resolve to the first one.
(def gvr-jaki-menu-label (jakis jaki)
  (let ((label (get jaki :label)))
    (if (> (len (filter (lambda (other) (= (get other :label) label)) jakis)) 1)
      (str label " #" (get jaki :id))
      label)))

(def gvr-gate-label (jakis jaki restart)
  (str (if restart "↺ " "→ ") (gvr-jaki-menu-label jakis jaki)))

;; The route menu over `routes` (gvr-route-tracks-labels) and `jakis`
;; (gvr-jakis).
(def gvr-route-menu (routes jakis)
  (append routes
          (map (lambda (jaki) (gvr-gate-label jakis jaki false)) jakis)
          (map (lambda (jaki) (gvr-gate-label jakis jaki true)) jakis)
          (list "Off")))

;; Node n's route as its menu label: its track's, the jaki its fires gate,
;; or Off (a jaki that is gone, or is not its graph owner's, included).
(def gvr-route-label (n jakis)
  (if (>= n.generator 0)
    (let ((jaki (first (filter (lambda (j) (= (get j :id) n.generator)) jakis))))
      (if jaki (gvr-gate-label jakis jaki n.restart) "Off"))
    (node-route-label n)))

;; Route node n to what menu `label` names: a track, a jaki gated or
;; restarted, or nothing (Off).
(def gvr-set-route-label! (n jakis label)
  (let ((gated (first (filter (lambda (j) (= (gvr-gate-label jakis j false) label)) jakis)))
        (restarted (first (filter (lambda (j) (= (gvr-gate-label jakis j true) label)) jakis))))
    (if gated
      (gate-generator! n (get gated :id))
      (if restarted
        (gate-generator! n (get restarted :id) :restart true)
        (set-route-label! n label)))))

;; ── fresh-instance defaults ──
;; The kind's :on-create (spec §11: a ring cannot be an `edges` default, whose
;; params are scalars). Writes the ring n -> n+1 at full weight and lets node 0
;; seed from its routed track, so a new instance plays as soon as that track
;; does. Only the ring cells are written; every other edge keeps the kind's
;; default weight 0. Through the graph-* natives, which answer at once: a
;; fresh instance's graph is no kind instance until the host's next sync.

(def gvr-init-ring-defaults (self)
  (let ((count (max gvr-min-node-count
                 (min gvr-max-node-count (round (graph-config-value self :node-count))))))
    (for-each
      (lambda (r) (graph-edge self :from r :to (mod (+ r 1) count) :weight 1))
      (range 0 count))
    (graph-node self 0 :seed-from :route)))

;; ── UI ──

(def gvr-row-height 1.0)
(def gvr-row-gap 0.2)
(def gvr-row-panel-padding 1.0)
(def gvr-matrix-column-gap 0.35)
(def gvr-route-bar-width 0.28)
(def gvr-node-width 1.4)
(def gvr-control-width 6.0)
(def gvr-seed-control-width 4.8)
(def gvr-group-width 4.0)

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
    :width width :height gvr-row-height :font-size 8
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

;; Node n's 0 / 1 param `name` as a toggle, bound.
(def gvr-switch (n name)
  (let ((p (graph-param-named n name)))
    (gvr-toggle-sized (str "graph-variable-reset-" name "-" n.index) gvr-control-width #'p.value
      (lambda (on) (set! p.value (if on 1 0))))))

;; Node n's param `name`, bound.
(def gvr-param (n name lo hi stp dec)
  (let ((p (graph-param-named n name)))
    (gvr-num (str "graph-variable-reset-" name "-" n.index) #'p.value lo hi stp dec
      (lambda (v) (set! p.value v)))))

;; A param every node carries alike: shows node 0's, sets the first count
;; nodes' (one undo entry).
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

;; The node's route color: its track's, dimmed grey while off (or gating a
;; jaki).
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
;; which lights a row, re-runs only the rows' highlight boxes (gvr-row),
;; which reuse these. `jakis` (gvr-jakis) and the route menu `menu`
;; (gvr-route-menu over them) are built once per panel and passed down.
(def gvr-row-controls (self n jakis menu)
  (subtree :key (str "graph-variable-reset-row-controls-" n.index)
    (h-stack :gap 0.4 :align :center
      (gvr-route-bar n)
      (label (str n.index) :width gvr-node-width :height gvr-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
      (gvr-pick (str "graph-variable-reset-route-" n.index)
        (gvr-route-label n jakis) menu
        (lambda (label) (gvr-set-route-label! n jakis label)))
      (gvr-pick-sized (str "graph-variable-reset-group-" n.index)
        (nth gvr-group-options n.group) gvr-group-options gvr-group-width
        (lambda (v) (set! n.group (index-of gvr-group-options v))))
      (gvr-toggle-sized (str "graph-variable-reset-seed-route-" n.index) gvr-seed-control-width
        #'n.seed-route
        (lambda (on) (set! n.seed-route on)))
      (gvr-toggle-sized (str "graph-variable-reset-reset-seed-" n.index) gvr-seed-control-width
        (> n.seed-on-reset 0)
        (lambda (on) (set! n.seed-on-reset (if on 1 0))))
      (gvr-num-or-target self n "delay" (str "graph-variable-reset-delay-" n.index)
        #'n.delay 0 16 1 0
        (lambda (v) (set! n.delay v)))
      (gvr-param-or-target self n "transpose" "transpose" -48 48 1 0)
      (gvr-switch n "transpose-reset")
      (gvr-param-or-target self n "velocity" "vel-decay" 0 2 0.01 2)
      (gvr-switch n "vel-reset")
      (gvr-param n "dampening" 0 1 0.01 2)
      (gvr-param n "recovery" 0 1 0.01 2)
      (gvr-pick (str "graph-variable-reset-resolution-" n.index) n.resolution res-options
        (lambda (v) (set! n.resolution v)))
      (gvr-pick (str "graph-variable-reset-quantize-" n.index) n.quantize graph-quantize-options
        (lambda (v) (set! n.quantize v)))
      (gvr-expand-button self n)
      (gvr-sounding n))))

;; Node n's row, lit while its weight column is pressed.
(def gvr-row (self n jakis menu)
  (subtree :key (str "graph-variable-reset-row-" n.index)
    (box
      :key (str "graph-variable-reset-row-" n.index)
      :height gvr-row-height
      :padding 0
      :selected (= self.selected-neuron n.index)
      :background-color :transparent
      :selected-background-color :mixer-strip-selected-bg
      :corner-radius 4
      (gvr-row-controls self n jakis menu))))

;; What the node is sounding right now: one chip per open gate, so overlapping
;; notes on a poly route all show, each as opaque as its velocity. Its own
;; subtree (it alone reads the live `n.sounding`), so a note starting or
;; ending re-runs this readout and nothing else. An unkeyed root: the
;; subtree's key would replace the readout's.
(def gvr-sounding-width 12)

(def gvr-sounding (n)
  (subtree :key (str "graph-variable-reset-sounding-run-" n.index)
    (h-stack :gap 0
      (let ((notes n.sounding))
        (number-list
          :key (str "graph-variable-reset-sounding-" n.index)
          :count (len notes)
          :values (map first notes)
          :levels (map (lambda (note) (nth note 1)) notes)
          :signed true
          :chip-width 2.2
          :gap 0.2
          :chip-color :mixer-strip-selected-bg
          :font-size 8
          :width gvr-sounding-width
          :height gvr-row-height)))))

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
    (gvr-header-label "quant" gvr-control-width)
    (gvr-header-label "proc" gvr-expand-width)
    (label "playing" :width gvr-sounding-width :height 1.0 :font-size 8 :h-align :left :color :dim :bg :transparent)))


;; ── Expanded neuron editor ─────────────────────────────────────────────────
;; One node's full editor in place of the row grid: its compact row, its
;; in/out edges as two strips of the weight matrix, and its process PATCH —
;; the processes that run on every fire before the payload is emitted and
;; scattered (docs/graph-node-processes-spec.md): `n.processes`. Every inlet
;; is a knob on a node (no lanes); a connectable port wires into a LATER
;; process's inlet, so `rand -> cmp -> veto` composes here exactly as in the
;; track patch bay.

(def gvr-expand-width 2.4)
(def gvr-proc-card-width 15)
(def gvr-proc-control-width 6.2)

(def gvr-expand-button (self n)
  (let ((patched (not (empty? n.processes))))
    (button (if (= self.expanded-node n.index) "close" "edit")
      :key (str "graph-variable-reset-expand-" n.index)
      :width gvr-expand-width :height gvr-row-height :padding 0.15 :font-size 7
      :background-color (if patched :effect-mode-on-bg :transparent)
      :border-color :effect-mode-on-bg
      :color (if patched :control-on-fg :dim)
      :on-click (lambda (event)
        (gvr-expand-node self (if (= self.expanded-node n.index) -1 n.index))))))

;; Scripting entry: open node n's expanded editor on instance `self` (-1 =
;; back to all nodes). Registers the node with the shared lane patchbay so its
;; bay finds the node (docs/graph-node-processes-spec.md §6).
(def gvr-expand-node (self n)
  (when (>= n 0)
    (eseq.sequencer/lane-patch-register-node self n)
    ;; The *processes* dock draws the selected card with gvr-proc-inspector.
    ;; Through a lambda that calls it by name, not the function value, so
    ;; re-evaluating this file restyles the dock without a re-expand.
    (eseq.processes-buffer/register-node-inspector self.kind
      (lambda (graph node slot-id) (gvr-proc-inspector graph node slot-id))))
  (eseq.sequencer/lane-patch-node-select -1)
  (gvr-map-clear self)
  (set! self.expanded-node n))

;; Track-typed process inlets (lane-harmony :source, xpose-by-track :source, ...)
;; store a real track index. The dropdown lists the graph's route tracks (a
;; rack-owned graph's members), then one "nrn k" entry per active node: a
;; neuron source is stored as -(k+1) (docs/graph-node-processes-spec.md §4;
;; lane-harmony reads it via (neuron k :chord/:key)).
(def gvr-neuron-labels (g)
  (map (lambda (k) (str "nrn " k)) (range 0 (len g.nodes))))

(def gvr-track-inlet-options (g)
  (append (gvr-route-tracks-labels g) (gvr-neuron-labels g)))

;; Inlet value `v` as its option: a neuron, or the track at that index.
(def gvr-track-inlet-label (g v)
  (if (< v 0)
    (str "nrn " (- -1 v))
    (let ((t (nth (tracks) v)))
      (if t (route-label g t) (str "Track " (+ v 1))))))

;; The inlet value the option `label` names (a track's index, a neuron's
;; -(k+1)), or nil.
(def gvr-track-inlet-value (g label)
  (let ((t (route-track g label))
        (k (index-of (gvr-neuron-labels g) label)))
    (if t t.index (if (>= k 0) (- -1 k) nil))))

;; Map arming (spec §6): a mappable port armed here lights the neuron row's
;; delay / transp / vel x pickers; clicking one binds the port to that payload
;; field. The armed process (its proc-id) and port are view state of this
;; instance.
(def gvr-map-active? (self) (>= self.map-slot 0))
(def gvr-map-port-active? (self slot-id port-name)
  (and (= self.map-slot slot-id) (= self.map-port port-name)))
(def gvr-map-clear (self)
  (set! self.map-slot -1)
  (set! self.map-port ""))
;; Scripting entry: arm (or disarm) process `slot-id`'s port `port-name`.
(def gvr-map-arm (self slot-id port-name)
  (if (gvr-map-port-active? self slot-id port-name)
    (gvr-map-clear self)
    (do (set! self.map-slot slot-id) (set! self.map-port port-name))))
;; Bind the armed port (while its process is still node n's) to payload
;; `field`.
(def gvr-map-bind (self n field)
  (when (gvr-map-active? self)
    (let ((p (eseq.sequencer/process-of n self.map-slot))
          (pt (when p (named p.ports self.map-port))))
      (when pt (bind-port! pt field)))
    (gvr-map-clear self)))
(def gvr-map-field-short (field)
  (match field
    "transpose" "tpose"
    "velocity" "vel"
    "duration" "dur"
    _ field))

;; The inlets a cable lands on, as (proc-id inlet) pairs: each port's own
;; wire (unless disconnected) and its fan-out entries' wires.
(def gvr-proc-wired-inlets (n)
  (reduce
    (lambda (wired p)
      (reduce
        (lambda (wired pt)
          (append wired
            (if (and pt.target-process (not pt.disconnected))
              (list (list pt.target-process.proc-id pt.target-inlet))
              (list))
            (map (lambda (fo) (list fo.target-process.proc-id fo.target-inlet))
              (filter (lambda (fo) fo.target-process) pt.fanout))))
        wired
        p.ports))
    (list)
    n.processes))

(def gvr-proc-inlet-wired? (wired p name)
  (not (empty? (filter (lambda (w) (and (= (nth w 0) p.proc-id) (= (nth w 1) name))) wired))))

(def gvr-proc-inlet-row (self g n p i wired)
  (let ((key (str "graph-variable-reset-proc-" n.index "-" p.proc-id "-" i.name))
        (kind i.type))
    (h-stack :gap 0.4 :align :center
      (label i.name :width 4.2 :height gvr-row-height :font-size 8 :h-align :right :color :dim :bg :transparent)
      (if (gvr-proc-inlet-wired? wired p i.name)
        (label "wired" :width gvr-proc-control-width :height gvr-row-height :font-size 8 :h-align :center :color :accent :bg :transparent)
        (match kind
          "gate" (gvr-toggle-sized key gvr-proc-control-width (>= i.value 0.5)
                   (lambda (on) (set-inlet! i (if on 1 0))))
          "enum" (dropdown
                   :key key
                   :value (nth i.options (floor i.value)) :options i.options
                   :badge-color :transparent :bg-color :bg :border-color :mixer-strip-selected-bg
                   :width gvr-proc-control-width :height gvr-row-height :font-size 6
                   :on-change (lambda (v) (set-inlet! i (index-of i.options v))))
          "track" (dropdown
                    :key key
                    :value (gvr-track-inlet-label g (floor i.value)) :options (gvr-track-inlet-options g)
                    :badge-color :transparent :bg-color :bg :border-color :mixer-strip-selected-bg
                    :width gvr-proc-control-width :height gvr-row-height :font-size 6
                    :on-change (lambda (label)
                      (let ((v (gvr-track-inlet-value g label)))
                        (when v (set-inlet! i v)))))
          _ (if (or p.expr (and p.promoted-expr (<= i.min -1000000)))
              (gvr-proc-expr-picker key i)
              (number-picker
                :key key :border-color :dim :background-color :bg
                :value #'i.value
                :min i.min
                :max i.max
                :step (if (= kind "int") 1 0.01)
                :decimals (if (= kind "int") 0 2)
                :width gvr-proc-control-width :height gvr-row-height :font-size 9
                :on-change (lambda (v) (set-inlet! i v)))))))))

;; An expr card's inlet (docs/expr-process-spec.md §3.1): the body carries
;; no range, so the picker is unbounded — a drag moves 0.1 per row, Shift
;; ten times that, typing sets any value. A whole value shows no decimals
;; (an lfsr's taps = 46080, not "46080.00" overflowing the box); the explicit
;; :step keeps drags and arrow keys on 0.01 either way.
(def gvr-proc-expr-picker (key i)
  (let ((current i.value))
    (number-picker
      :key key :border-color :dim :background-color :bg
      :value current
      :drag :relative :drag-step 0.1 :step 0.01
      :decimals (if (= current (floor current)) 0 2)
      :width gvr-proc-control-width :height gvr-row-height :font-size 9
      :on-change (lambda (v) (set-inlet! i v)))))

;; Why an expr card's error dot is lit, as text for the inspector: a failed
;; commit from its edit buffer, a failed run, or a body with no compiled
;; class. nil when it runs clean.
(def gvr-proc-expr-error (self n p)
  (let ((committed (eseq.expr-buffer/commit-error self n.index p.proc-id)))
    (or committed
        (unless (= p.error "") p.error)
        (unless (= p.compile-error "") p.compile-error))))

;; Its own subtree: a run error changing on the scheduler re-runs the row,
;; not the card.
(def gvr-proc-expr-error-row (self n p)
  (when p.expr
    (subtree :key (str "graph-variable-reset-proc-expr-error-" n.index "-" p.proc-id)
      (let ((message (gvr-proc-expr-error self n p)))
        (v-stack :gap 0
          (when message
            (label message
              :width (- gvr-proc-card-width 1) :height gvr-row-height :font-size 7.5
              :color :toast-error :bg :transparent)))))))

;; Promote / edit as expr (docs/expr-process-spec.md §8): an expr card with
;; a body promotes to My processes under a name; a card whose class was
;; promoted from one turns back into an expr card. On any other card "as
;; expr" is dim and says why instead. Both stay eseq.expr-buffer's natives
;; (their result shows at once).
(def gvr-proc-expr-actions (self n p origin)
  (let ((id p.proc-id))
    (if p.expr
      (unless (= p.expr-source "")
        (button "promote…"
          :key (str "graph-variable-reset-proc-promote-" n.index "-" id)
          :width 5.2 :height gvr-row-height :padding 0.15 :font-size 7
          :background-color :transparent :border-color :process-lane-accent :color :process-lane-accent
          :on-click (lambda (event) (eseq.expr-buffer/open-promote self n.index id origin))))
      (button "as expr"
        :key (str "graph-variable-reset-proc-as-expr-" n.index "-" id)
        :width 5.2 :height gvr-row-height :padding 0.15 :font-size 7
        :background-color :transparent
        :border-color (if p.promoted-expr :process-lane-accent :mixer-strip-border)
        :color (if p.promoted-expr :process-lane-accent :dim)
        :on-click (lambda (event)
          (if p.promoted-expr
            (eseq.expr-buffer/edit-node-slot-as-expr self n.index id)
            (status (str "as expr: " p.as-expr-reason))))))))

;; Process p's inspector card: on/off, remove, inlet pickers, out mapping,
;; promote… / as expr. One function for both homes: the *processes* dock
;; (gvr-proc-inspector, `width` :fill, `origin` "dock") and, when the dock
;; is not showing this node, beside the bay (gvr-node-patch,
;; gvr-proc-card-width, "node"). `origin` is the mount of promote's name
;; modal.
(def gvr-proc-card (self g n p wired width origin)
  (let ((id p.proc-id)
        (enabled p.enabled)
        ;; Docked, the card is the dock's content: no second frame inside
        ;; the tile's, tighter rows, and promote… / as expr join the title
        ;; row so a card with a few inlets fits the inspector half.
        (docked (= origin "dock")))
    (box
      :key (str "graph-variable-reset-proc-card-" n.index "-" id)
      :width width
      :padding (if docked 0.1 0.5)
      :background-color (if docked :transparent :bg)
      :border-color (if docked :transparent (if enabled :process-lane-accent :mixer-strip-border))
      :corner-radius 6
      (v-stack :gap (if docked 0.15 0.3)
        (h-stack :gap 0.3 :align :center
          (label p.name :width 6.2 :height gvr-row-height :font-size 8 :color :foreground :bg :transparent)
          (button (if enabled "on" "off")
            :key (str "graph-variable-reset-proc-enable-" n.index "-" id)
            :width 2.0 :height gvr-row-height :padding 0.15 :font-size 7
            :background-color (if enabled :process-lane-accent :transparent)
            :border-color :process-lane-accent
            :color (if enabled :black :dim)
            :on-click (lambda (event) (set-process-enabled! p (not enabled))))
          (when docked (gvr-proc-expr-actions self n p origin)))
        (gvr-proc-expr-error-row self n p)
        (unless docked (gvr-proc-expr-actions self n p origin))
        (each p.inlets |i| (gvr-proc-inlet-row self g n p i wired))
        (each (filter (lambda (pt) pt.mappable) p.ports) |pt|
          (gvr-proc-map-row self n p pt))
        (gvr-proc-meter n p (if (number? width) (- width 1) (- gvr-proc-card-width 1)))
        ;; Docked, the dock draws delete at its own bottom right (the whole
        ;; inspector's corner); in the bay the card carries it.
        (unless docked
          (h-stack :width :fill :align :center
            (box :flex 1 :height 0.5 :bg :transparent)
            (gvr-proc-delete-button n p)))))))

;; Removes the card from node `n`'s chain. Reordering is by dragging cards,
;; so the card has no < > x buttons.
(def gvr-proc-delete-button (n p)
  (button "delete"
    :key (str "graph-variable-reset-proc-remove-" n.index "-" p.proc-id)
    :width 4.0 :height gvr-row-height :padding 0.1 :font-size 7
    :background-color :red :border-color :red :color :black
    :on-click (lambda (event) (remove-process! p))))

;; The *processes* dock's renderer (eseq.processes-buffer/register-node-inspector):
;; process `slot-id` of node `node` of instance `self`, full width, or nil
;; when it is gone.
(def gvr-proc-inspector (self node slot-id)
  (let ((g (graph-of self))
        (n (when g (nth g.nodes node)))
        (p (when n (eseq.sequencer/process-of n slot-id))))
    (when p
      (gvr-proc-card self g n p (gvr-proc-wired-inlets n) :fill "dock"))))

;; A live readout under the inlets for processes that have one:
;; lane-harmony's snap meter. Its scope is read inside the subtree, so a fire
;; re-runs the meter alone.
(def gvr-proc-meter (n p width)
  (when (= p.class-name "lane-harmony")
    (subtree :key (str "graph-variable-reset-proc-meter-" n.index "-" p.proc-id)
      (eseq.sequencer/harmony-snap-meter (str "graph-variable-reset-harmony-" n.index "-" p.proc-id)
        p width))))

;; A mappable port (rand/count/acc `out`, ...) writes onto the fire payload:
;; pick which field. Wire ports go through the bay's cables instead.
(def gvr-proc-map-row (self n p pt)
  (let ((mapped (if (= pt.target-step-param "") nil pt.target-step-param))
        (armed (gvr-map-port-active? self p.proc-id pt.name)))
    (h-stack :gap 0.4 :align :center
      (label (str pt.name " ->") :width 4.2 :height gvr-row-height :font-size 8 :h-align :right :color :process-lane-accent :bg :transparent)
      (label (if mapped (gvr-map-field-short mapped) (if armed "pick..." "unmapped"))
        :width 5.2 :height gvr-row-height :font-size 8 :v-align :center
        :color (if (or mapped armed) :process-lane-accent :dim) :bg :transparent)
      (button "map"
        :key (str "graph-variable-reset-proc-map-" n.index "-" p.proc-id "-" pt.name)
        :width 2.0 :height gvr-row-height :padding 0.15 :font-size 7
        :background-color (if armed :process-lane-accent :transparent)
        :border-color :process-lane-accent
        :color (if armed :black :process-lane-accent)
        :on-click (lambda (event) (gvr-map-arm self p.proc-id pt.name)))
      (when mapped
        (button "x"
          :key (str "graph-variable-reset-proc-unmap-" n.index "-" p.proc-id "-" pt.name)
          :width 1.4 :height gvr-row-height :padding 0.1 :font-size 7
          :background-color :transparent :border-color :dim :color :dim
          :on-click (lambda (event) (clear-port! pt)))))))

;; While a map is armed, a payload-backed picker turns into the target chip:
;; clicking it binds the armed port to `field`.
(def gvr-map-target (self n field key)
  (button (str "-> " (gvr-map-field-short field))
    :key (str key "-map-target")
    :width gvr-control-width :height gvr-row-height :padding 0.15 :font-size 7
    :background-color :process-map-arm-bg
    :border-color :process-lane-accent
    :color :process-lane-accent
    :on-click (lambda (event) (gvr-map-bind self n field))))

(def gvr-num-or-target (self n field key value lo hi stp dec on-change)
  (if (gvr-map-active? self)
    (gvr-map-target self n field key)
    (gvr-num key value lo hi stp dec on-change)))

;; Node n's param `name` (payload `field`), or its map target chip.
(def gvr-param-or-target (self n field name lo hi stp dec)
  (if (gvr-map-active? self)
    (gvr-map-target self n field (str "graph-variable-reset-" name "-" n.index))
    (gvr-param n name lo hi stp dec)))

;; The add menu: the library's classes that do something on a node (by
;; their node labels), then an "expr presets" heading over the expr preset
;; rows (docs/expr-process-spec.md §6.1). A preset row adds an expr card with
;; its body committed and inlets set (eseq.expr-buffer, through the natives:
;; the kinds list it at the host's next sync).
(def gvr-proc-preset-header "expr presets")
(def gvr-proc-add-card (self n)
  (let ((classes (filter (lambda (c) (not c.node-hidden)) process-library.classes))
        (class-labels (map (lambda (c) c.node-label) classes))
        (labels (append class-labels (list gvr-proc-preset-header) (eseq.expr-buffer/preset-labels)))
        (class-count (len class-labels)))
    (eseq.sequencer/lane-patch-add-menu-grouped
      (str "graph-variable-reset-proc-add-" n.index) "+  add process"
      labels (list class-count) "Filter processes…"
      (lambda (label)
        (let ((index (index-of labels label)))
          (if (< index class-count)
            (when (>= index 0) (add-process! n (nth classes index)))
            (eseq.expr-buffer/add-node-preset self n.index
              (eseq.expr-buffer/preset-named label))))))))

;; The process the inspector card shows: the bay's selection, else the first.
(def gvr-proc-selected (n)
  (or (eseq.sequencer/process-of n (eseq.sequencer/lane-patch-node-selected-id))
      (first n.processes)))

;; The node's patch: the shared lane patchbay (cards, ports, drag cables,
;; cable select + × / Backspace, fan-out) over the node's processes. The
;; selected process's inspector card lives in the *processes* dock while that
;; shows this node; otherwise (dock setting off, sidebar hidden) the same
;; card sits beside the bay. Wires pointing up the chain land next fire, as on a track. The bay's
;; namespace derives from the instance id, so two instances never share one.
(def gvr-node-patch (self g n)
  (let ((ns (eseq.sequencer/lane-patch-node-namespace self n.index))
        (selected (gvr-proc-selected n)))
    (v-stack :gap 0.4
      (label "process patch: runs on every fire before emit + scatter. veto mutes, writes ride on. drag a port onto an inlet to wire it"
        :width 70 :height 1.0 :font-size 8 :color :dim :bg :transparent)
      (h-stack :gap 0.6 :align :top
        (box :flex 1 :padding 0 :bg :transparent
          :key (str "graph-variable-reset-proc-bay-" n.index)
          (eseq.sequencer/lane-patchbay-node ns (gvr-proc-add-card self n)))
        (when (and selected (not (eseq.processes-buffer/docks? self n.index)))
          (gvr-proc-card self g n selected (gvr-proc-wired-inlets n) gvr-proc-card-width "node")))
      ;; Promote's name modal (eseq.expr-buffer), zero footprint closed.
      (subtree :key (str "graph-variable-reset-promote-modal-" n.index)
        (eseq.expr-buffer/promote-panel "node")))))

;; One row / column of the weight matrix `rows` (weight-rows) as a labeled
;; 1xN strip: node n's outgoing weights (`:out`) or incoming ones (`:in`).
(def gvr-edge-strip (g rows title n direction)
  (let ((count (len g.nodes))
        (out (= direction :out))
        (values (list (if out (nth rows n.index) (map (lambda (row) (nth row n.index)) rows)))))
    (v-stack :gap 0.2
      (label title :width 10 :height 0.9 :font-size 8 :color :dim :bg :transparent)
      (matrix
        :key (str "graph-variable-reset-edge-strip-" (if out "out" "in") "-" n.index)
        :rows 1
        :cols count
        :width (* count 2.2)
        :height 2.2
        :min 0 :max 1
        :background :mixer-strip-bg
        :color (rgba 0.14 0.3 0.9 1)
        :empty-fill-color (rgba 0.04 0.04 0.05 1)
        :stroke-color (rgba 0.36 0.62 0.57 1)
        :stroke-width 1.5
        :stroke-active-only true
        :value values
        :on-cell-change (lambda (r c v)
          (if out (set-weight! g n.index c v) (set-weight! g c n.index v)))))))

(def gvr-expanded-editor (self g n jakis menu)
  (let ((count (len g.nodes)))
    (box
      :padding gvr-row-panel-padding
      :border-color :mixer-strip-border
      :background-color :mixer-strip-bg :corner-radius 16
      (v-stack :gap 0.7
        (h-stack :gap 0.5 :align :center
          (button "all nodes"
            :key "graph-variable-reset-expanded-back"
            :width 5 :height gvr-row-height :padding 0.15 :font-size 7
            :background-color :transparent :border-color :dim :color :dim
            :on-click (lambda (event) (set! self.expanded-node -1)))
          (button "<"
            :key "graph-variable-reset-expanded-prev"
            :width 1.6 :height gvr-row-height :padding 0.1 :font-size 7
            :background-color :transparent :border-color :dim :color :dim
            :on-click (lambda (event) (gvr-expand-node self (max 0 (- n.index 1)))))
          (label (str "neuron " n.index) :width 6 :height 1.2 :font-size 11 :h-align :center :color :foreground :bg :transparent)
          (button ">"
            :key "graph-variable-reset-expanded-next"
            :width 1.6 :height gvr-row-height :padding 0.1 :font-size 7
            :background-color :transparent :border-color :dim :color :dim
            :on-click (lambda (event) (gvr-expand-node self (min (- count 1) (+ n.index 1))))))
        (v-stack :gap gvr-row-gap
          (gvr-header)
          (gvr-row self n jakis menu))
        ;; The strips read every weight: a subtree of their own, so a cell
        ;; drag re-runs them and not the patch below.
        (subtree :key "graph-variable-reset-edge-strips"
          (let ((rows (weight-rows g)))
            (h-stack :gap 1.5
              (gvr-edge-strip g rows (str "in  (from node -> " n.index ")") n :in)
              (gvr-edge-strip g rows (str "out (" n.index " -> to node)") n :out))))
        (gvr-node-patch self g n)))))

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

;; The kind's :view: the whole panel of instance `self`, empty until the
;; host publishes its graph (at its next sync). Playback is read only inside
;; the visualizers' subtrees, so a firing history or a track's notes never
;; rebuild the graph controls.
(def gvr-panel (self)
  (let ((g (graph-of self)))
    (if g (gvr-graph-panel self g) (nothing))))

(def gvr-graph-panel (self g)
  (let ((active-count (len g.nodes))
        (expanded self.expanded-node)
        (editing (when (>= expanded 0) (nth g.nodes expanded))))
    (box
      :padding 0.85
      :gap 0.6
      (v-stack :gap 0.5
        ;; ── sequencer-level config (on top) ──
        (box
          ;; Rack-owned: room for the corner chip past the spectrogram.
          :width (if g.owner 102 90.5)
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
              ;; Batch params: node 0's shows the value every node carries;
              ;; an edit sets them all (the threshold every node up to the
              ;; capacity, so a node that becomes active later carries it).
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
              :max-color (rgba 1.0 0.72 0.28 1))
            ;; Top-right corner of the config block, past the spectrogram.
            (box :flex 1 :height 1.0)
            (gvr-owner-rack-badge g)
            (box :width 1 :height 1.0)))

        (h-stack
          (let ((jakis (gvr-jakis g))
                (menu (gvr-route-menu (gvr-route-tracks-labels g) jakis)))
            (if editing
              (gvr-expanded-editor self g editing jakis menu)
              (box
                :padding gvr-row-panel-padding
                :border-color :mixer-strip-border
                :background-color :mixer-strip-bg :corner-radius 16
                (v-stack :gap 0.5
                  (v-stack :gap gvr-row-gap
                    (gvr-header)
                    (each g.nodes |n| (gvr-row self n jakis menu)))))))

          (gvr-column-matrix "graph-variable-reset-trigger-matrix" active-count 1 1 (lambda () g.triggers))
          (gvr-column-matrix "graph-variable-reset-energy-matrix" active-count 2 4 (lambda () g.energy))

          ;; The weights are read inside a subtree: a cell drag re-runs the
          ;; matrix, not the rows beside it.
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
                :on-cell-press (lambda (r c) (set! self.selected-neuron c))
                :on-cell-release (lambda (r c) (set! self.selected-neuron -1))
                :on-cell-change (lambda (r c v) (set-weight! g r c v))))))
        (box
          :debug-name "graph-variable-reset-piano-panel"
          :padding 1
          :background-color :mixer-strip-bg
          :border-color :mixer-strip-border
          :corner-radius 12
          (subtree :key "graph-variable-reset-piano"
            (let ((routed (route-tracks g)))
              (piano-keyboard
                :key "graph-variable-reset-piano"
                :notes-by-track (map (lambda (t) t.active-notes) routed)
                :track-colors (map (lambda (t) t.color) routed)
                :tracks (range 0 (len routed))
                :overlap-mode :loudest
                :press-depth self.piano-depth
                :start-note 12
                :key-count 80
                :width 84
                :height 3.5))))))))

;; The kind (docs/instance-kinds-spec.md §3). The host owns any number of
;; `neural` instances; each publishes this body under its own id with its own
;; overrides and renders (gvr-panel self) in its own buffer and tab. The
;; shared sequencer keymap keeps Backspace / Delete removing the cable selected
;; in a node's patch bay, and the arrows / RET driving the step grid.
(def-kind neural
  :sequencer (:shape (line :default 8 :min 1 :max 16)
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
        (dampening :float 0 1 :default 0))))
  :state ((expanded-node -1) (selected-neuron -1)
          (map-slot -1) (map-port "") (piano-depth 0.6))
  :view gvr-panel
  :keymap eseq.sequencer-keys/sequencer-keys
  :on-create gvr-init-ring-defaults)
