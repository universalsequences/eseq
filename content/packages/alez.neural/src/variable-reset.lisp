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
;;   instance's graph overrides, written with `graph-*` and read back with the
;;   tracked `graph-*-value` reads / `bind-graph` handles, so an edit re-renders
;;   exactly its readers with no echo and no cache;
;; - view (expanded node, selected neuron, add-process class, map arming,
;;   piano depth) is the kind's `:state`, one cell per instance;
;; - nothing is global, so two instances never share a selection or a cache.
;;
;; A fresh instance gets the ring patch from :on-create (gvr-init-ring-defaults);
;; call (alez.neural.variable-reset/gvr-init-ring-defaults inst) to write it again.

(module alez.neural.variable-reset)

(export gvr-panel gvr-init-ring-defaults gvr-expand-node gvr-map-arm gvr-edit-config
        gvr-node-count)

(def gvr-min-node-count 1)
(def gvr-max-node-count 16)

;; ── dropdown option lists (order is the index space the dropdowns map into) ──

(def gvr-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def gvr-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))

;; Owned by a rack: routes address its members. `self.owner` is `:project` or
;; the rack's group id.
(def gvr-owner-rack (self)
  (let ((owner self.owner))
    (if (number? owner) owner nil)))
;; "attached to <rack>" chip in the config block's top-right corner, dressed
;; like the sample browser's tag chips. Resolved at render so a rack rename shows at once;
;; SEQ.groups is read only to re-render on that change.
(def gvr-owner-rack-badge (self)
  (let ((rack (gvr-owner-rack self)))
    (if rack
      (let ((groups SEQ.groups)
          (gidx (eseq.drum-rack-v2/group-index-by-id rack)))
        (if (>= gidx 0)
          (h-stack
            (button (str "attached to " (substring (eseq.drum-rack-v2/group-name gidx) 0 14))
              :key "graph-variable-reset-owner-rack"
              :variant :ghost
              :background-color :mixer-control-bg
              :color :dimmer
              :border-color :none
              :height 1.0 :padding 0.8532 :font-size 12.0 :corner-radius 13))
          nil))
      nil)))
(def gvr-route-tracks (self)
  ;; Read live so a member that joins the rack later shows up; SEQ.groups and
  ;; self.owner are read only to re-render when the membership or owner moves.
  (let ((groups SEQ.groups) (owner self.owner)) (graph-route-tracks self)))
;; Route option n is track n (project-owned) or rack member n (rack-owned);
;; "Off" is always last. Either way the option index IS the route value.
(def gvr-route-options (self)
  (let ((tracks (gvr-route-tracks self)))
    (if tracks
      (append
        (map (lambda (track) (str (+ track 1) " " (nth SEQ.track-names track))) tracks)
        (list "Off"))
      (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
            "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
            "Off"))))
;; Colors parallel to the route options: a rack-owned instance colors by the
;; member's track, so the panel's track-colors are re-indexed through the members.
(def gvr-route-track-colors (self track-colors)
  (let ((tracks (gvr-route-tracks self)))
    (if tracks
      (map (lambda (track) (nth track-colors track)) tracks)
      track-colors)))
(def gvr-max-poly-selection-options
  (list "deterministic" "propagation" "random" "markov" "loudest" "lowest-transpose" "highest-transpose" "seed-first"))
;; Neural-group assignment (docs/neural-groups-spec.md §3.1). The stored value IS the
;; dropdown index (group A = 0), so the numeric bind-graph handle seeds it directly.
(def gvr-group-options (list "A" "B" "C" "D"))
(def gvr-route-off-index (self) (- (len (gvr-route-options self)) 1))
(def gvr-route-off-color (list 0.20 0.21 0.23))

(def gvr-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

;; Route dropdown label -> the internal route the engine stores (:off or a track index).
(def gvr-route->internal (self label)
  (if (= label "Off") :off (gvr-index-of (gvr-route-options self) label)))

;; Node n's route as an option index: a tracked read, so a route edit (or a
;; pattern switch) re-renders its readers. Unrouted, or past the last
;; member, is "Off".
(def gvr-route-option-index (self n)
  (let ((route (graph-node-value self n :route))
        (off (gvr-route-off-index self)))
    (if (number? route) (if (< route off) (round route) off) off)))

(def gvr-route-color-valid? (track-colors route-index off-index)
  (and (>= route-index 0) (< route-index (len track-colors)) (< route-index off-index)))

(def gvr-color-channel (color channel fallback)
  (if (< channel (len color)) (nth color channel) fallback))

(def gvr-route-color-channel (track-colors route-index off-index channel)
  (if (gvr-route-color-valid? track-colors route-index off-index)
    (gvr-color-channel (nth track-colors route-index) channel (nth gvr-route-off-color channel))
    (nth gvr-route-off-color channel)))

;; ── graph reads (tracked: a write re-renders only what read the field) ──

(def gvr-node-count (self)
  (max gvr-min-node-count
    (min gvr-max-node-count
      (round (graph-config-value self :node-count)))))

;; The active NxN weights, one tracked read per cell.
(def gvr-read-weights (self)
  (let ((count (gvr-node-count self)))
    (map
      (lambda (r) (map (lambda (c) (graph-edge-value self r c :weight)) (range 0 count)))
      (range 0 count))))

(def gvr-zero-row (count)
  (map (lambda (n) 0) (range 0 count)))

(def gvr-zero-matrix (count)
  (map (lambda (n) (gvr-zero-row count)) (range 0 count)))

(def gvr-zero-column-matrix (count)
  (map (lambda (n) (list 0)) (range 0 count)))

(def gvr-viz (self visualizations)
  (let ((id self.id)
        (hits (filter (lambda (viz) (= (get viz :id) id)) visualizations)))
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

;; ── fresh-instance defaults ──
;; The kind's :on-create (spec §11: a ring cannot be an `edges` default, whose
;; params are scalars). Writes the ring n -> n+1 at full weight and lets node 0
;; seed from its routed track, so a new instance plays as soon as that track
;; does. Only the ring cells are written; every other edge keeps the kind's
;; default weight 0.

(def gvr-init-ring-defaults (self)
  (let ((count (gvr-node-count self)))
    (do
      (for-each
        (lambda (r) (graph-edge self :from r :to (mod (+ r 1) count) :weight 1))
        (range 0 count))
      (graph-node self 0 :seed-from :route))))

;; ── edit helpers: a `graph-*` write persists the override and re-renders its
;;    readers (tracked reads and numeric `bind-graph` handles alike) ──

(def gvr-edit-global-param (self field v)
  (for-each
    (lambda (n) (graph-param self n field v))
    (range 0 (gvr-node-count self))))

;; Every node up to the capacity, so a node that becomes active later
;; already carries the value.
(def gvr-edit-capacity-param (self field v)
  (for-each
    (lambda (n) (graph-param self n field v))
    (range 0 gvr-max-node-count)))

(def gvr-edit-seed-route (self n enabled)
  (graph-node self n :seed-from (if enabled :route :off)))

(def gvr-edit-reset-seed (self n enabled)
  (graph-node self n :seed-on-reset (if enabled 1 0)))

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

;; Timing batch edits: scale every active node's delay by a factor label
;; ("1/4" .. "4"), or shift every node's resolution / quantize by octaves.
(def gvr-apply-delay-factor (self label)
  (let ((factor (gvr-factor-value label)))
    (for-each
      (lambda (n)
        (let ((current (graph-node-value self n :delay)))
          (graph-node self n :delay
            (if (<= current 0)
              0
              (max 1 (round (* current factor)))))))
      (range 0 (gvr-node-count self)))))

(def gvr-apply-timebase-factor (self label)
  (let ((shift (gvr-factor-shift label)))
    (for-each
      (lambda (n)
        (let ((res (gvr-scale-res-label (graph-node-value self n :resolution) shift))
              (quant (gvr-scale-quant-label (graph-node-value self n :quantize) shift)))
          (do
            (graph-node self n :resolution res)
            (graph-node self n :quantize quant))))
      (range 0 (gvr-node-count self)))))

;; Sequencer-level config is per-pattern like the node/edge overrides.
(def gvr-edit-config (self field v)
  (graph-config self field v))

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

(def gvr-pick-sized (key value-index options width on-change)
  (dropdown
    :key key
    :value-index value-index :options options
    :badge-color :transparent
    :bg-color :mixer-strip-bg
    :border-color :mixer-strip-selected-bg
    :width width :height gvr-row-height :font-size 6
    :on-change on-change))

(def gvr-pick (key value-index options on-change)
  (gvr-pick-sized key value-index options gvr-control-width on-change))

(def gvr-reset-value (self n field)
  (>= (graph-param-value self n field) 1))

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

(def gvr-toggle (key value on-change)
  (gvr-toggle-sized key gvr-control-width value on-change))

(def gvr-seed-toggle (key value on-change)
  (gvr-toggle-sized key gvr-seed-control-width value on-change))

(def gvr-seed-route-value (self n)
  (>= (graph-node-value self n :seed-route) 1))

(def gvr-reset-seed-value (self n)
  (>= (graph-node-value self n :seed-on-reset) 1))

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

;; The node's route color: computed from the tracked route read, so a route
;; edit repaints it with no bound echo field.
(def gvr-route-bar (self n track-colors)
  (let ((route-index (gvr-route-option-index self n))
        (off-index (gvr-route-off-index self)))
    (box
      :key (str "graph-variable-reset-route-color-" n)
      :width gvr-route-bar-width
      :height gvr-row-height
      :background "gvr-route-color-strip"
      :active (if (gvr-route-color-valid? track-colors route-index off-index) 1 0)
      :track-r (gvr-route-color-channel track-colors route-index off-index 0)
      :track-g (gvr-route-color-channel track-colors route-index off-index 1)
      :track-b (gvr-route-color-channel track-colors route-index off-index 2))))

(def gvr-row (self n track-colors)
  (box
    :key (str "graph-variable-reset-row-" n)
    :height gvr-row-height
    :padding 0
    :selected (= self.selected-neuron n)
    :background-color :transparent
    :selected-background-color :mixer-strip-selected-bg
    :corner-radius 4
    (h-stack :gap 0.4 :align :center
      (gvr-route-bar self n track-colors)
      (label (str n) :width gvr-node-width :height gvr-row-height :font-size 9 :h-align :center :color :dim :bg :transparent)
      (gvr-pick (str "graph-variable-reset-route-" n)
        (gvr-route-option-index self n) (gvr-route-options self)
        (lambda (v) (graph-node self n :route (gvr-route->internal self v))))
      (gvr-pick-sized (str "graph-variable-reset-group-" n)
        (bind-graph self n :group) gvr-group-options gvr-group-width
        (lambda (v) (graph-node self n :group (gvr-index-of gvr-group-options v))))
      (gvr-seed-toggle (str "graph-variable-reset-seed-route-" n)
        (gvr-seed-route-value self n)
        (lambda (v) (gvr-edit-seed-route self n v)))
      (gvr-seed-toggle (str "graph-variable-reset-reset-seed-" n)
        (gvr-reset-seed-value self n)
        (lambda (v) (gvr-edit-reset-seed self n v)))
      (gvr-num-or-target self n "delay" (str "graph-variable-reset-delay-" n)
        (bind-graph self n :delay) 0 16 1 0
        (lambda (v) (graph-node self n :delay v)))
      (gvr-num-or-target self n "transpose" (str "graph-variable-reset-transpose-" n)
        (bind-graph self n :transpose) -48 48 1 0
        (lambda (v) (graph-param self n :transpose v)))
      (gvr-toggle (str "graph-variable-reset-transpose-reset-" n)
        (gvr-reset-value self n :transpose-reset)
        (lambda (v) (graph-param self n :transpose-reset (if v 1 0))))
      (gvr-num-or-target self n "velocity" (str "graph-variable-reset-vel-decay-" n)
        (bind-graph self n :vel-decay) 0 2 0.01 2
        (lambda (v) (graph-param self n :vel-decay v)))
      (gvr-toggle (str "graph-variable-reset-vel-reset-" n)
        (gvr-reset-value self n :vel-reset)
        (lambda (v) (graph-param self n :vel-reset (if v 1 0))))
      (gvr-num (str "graph-variable-reset-dampening-" n)
        (bind-graph self n :dampening) 0 1 0.01 2
        (lambda (v) (graph-param self n :dampening v)))
      (gvr-num (str "graph-variable-reset-recovery-" n)
        (bind-graph self n :recovery) 0 1 0.01 2
        (lambda (v) (graph-param self n :recovery v)))
      (gvr-pick (str "graph-variable-reset-resolution-" n)
        (gvr-index-of gvr-res-options (graph-node-value self n :resolution)) gvr-res-options
        (lambda (v) (graph-node self n :resolution v)))
      (gvr-pick (str "graph-variable-reset-quantize-" n)
        (gvr-index-of gvr-quant-options (graph-node-value self n :quantize)) gvr-quant-options
        (lambda (v) (graph-node self n :quantize v)))
      (gvr-expand-button self n)
      (gvr-sounding self n))))

;; What the node is sounding right now: one chip per open gate, so overlapping
;; notes on a poly route all show, each as opaque as its velocity. Element
;; bindings (not a SEQ read), so a note starting or ending repaints this widget
;; alone.
(def gvr-sounding-width 12)

(def gvr-sounding (self n)
  (let ((notes (bind-graph-node-notes self n)))
    (number-list
      :key (str "graph-variable-reset-sounding-" n)
      :count (get notes :count)
      :values (get notes :values)
      :levels (get notes :levels)
      :signed true
      :chip-width 2.2
      :gap 0.2
      :chip-color :mixer-strip-selected-bg
      :font-size 8
      :width gvr-sounding-width
      :height gvr-row-height)))

(def gvr-header ()
  (h-stack :gap 0.4 :align :center
    (label "" :width gvr-route-bar-width :height 1.0 :font-size 1 :bg :transparent)
    (label "node"   :width gvr-node-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "route"  :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "grp"    :width gvr-group-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
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
    (label "quant"  :width gvr-control-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "proc"   :width gvr-expand-width :height 1.0 :font-size 8 :h-align :center :color :dim :bg :transparent)
    (label "playing" :width gvr-sounding-width :height 1.0 :font-size 8 :h-align :left :color :dim :bg :transparent)))


;; ── Expanded neuron editor ─────────────────────────────────────────────────
;; One node's full editor in place of the row grid: its compact row, its
;; in/out edges as two strips of the weight matrix, and its process PATCH —
;; the slots that run on every fire before the payload is emitted and
;; scattered (docs/graph-node-processes-spec.md). Every inlet is a knob on a
;; node (no lanes); a connectable port wires into a LATER slot's inlet, so
;; `rand -> cmp -> veto` composes here exactly as in the track patch bay.

(def gvr-expand-width 2.4)
(def gvr-proc-card-width 15)
(def gvr-proc-control-width 6.2)

(def gvr-expand-button (self n)
  (let ((expanded self.expanded-node)
        (patched (>= (len (gvr-node-patch-slots self n)) 1)))
    (button (if (= expanded n) "close" "edit")
      :key (str "graph-variable-reset-expand-" n)
      :width gvr-expand-width :height gvr-row-height :padding 0.15 :font-size 7
      :background-color (if patched :effect-mode-on-bg :transparent)
      :border-color :effect-mode-on-bg
      :color (if patched :control-on-fg :dim)
      :on-click (lambda (event)
        (gvr-expand-node self (if (= self.expanded-node n) -1 n))))))

;; Scripting entry: open node n's expanded editor on instance `self` (-1 =
;; back to all nodes). Registers the node with the shared lane patchbay so its
;; cables route to the graph-node-process-* natives
;; (docs/graph-node-processes-spec.md §6).
(def gvr-expand-node (self n)
  (do
    (if (>= n 0) (eseq.sequencer/lane-patch-register-node self n) nil)
    (eseq.sequencer/lane-patch-node-select -1)
    (gvr-map-clear self)
    (set! self.expanded-node n)))

;; Track-typed process inlets (lane-harmony :source, xpose-by-track :source, ...)
;; store a real track index. The dropdown shows the route labels minus "Off",
;; then one "nrn k" entry per active node: a neuron source is stored as
;; -(k+1) (docs/graph-node-processes-spec.md §4; lane-harmony reads it via
;; (neuron k :chord/:key)). A rack-owned instance maps member positions back
;; to their tracks.
(def gvr-track-inlet-track-options (self)
  (let ((opts (gvr-route-options self)))
    (map (lambda (i) (nth opts i)) (range 0 (- (len opts) 1)))))
(def gvr-track-inlet-options (self)
  (append (gvr-track-inlet-track-options self)
          (map (lambda (k) (str "nrn " k)) (range 0 (gvr-node-count self)))))
(def gvr-track-inlet-index (self value)
  (if (< value 0)
    (+ (len (gvr-track-inlet-track-options self)) (- -1 value))
    (if (gvr-route-tracks self) (gvr-index-of (gvr-route-tracks self) value) value)))
(def gvr-track-inlet-value (self index)
  (let ((tracks (len (gvr-track-inlet-track-options self))))
    (if (>= index tracks)
      (- -1 (- index tracks))
      (if (gvr-route-tracks self) (nth (gvr-route-tracks self) index) index))))

;; The node's patch, and the bay's view of it (per slot, which in ports have
;; a cable). Both reads are tracked: every graph-node-process-* edit, from
;; this panel or from a cable in the bay, re-renders them.
(def gvr-node-patch-slots (self n)
  (graph-node-process-chain self n))

(def gvr-node-patch-entries (self n)
  (graph-node-lane-patch self n))

;; The picker's classes: node-flavoured labels over the library's class
;; names (SEQ.process-library is read so a library reload re-renders).
(def gvr-proc-classes ()
  (do SEQ.process-library (graph-node-process-classes)))
(def gvr-proc-class-labels () (map (lambda (c) (get c :label)) (gvr-proc-classes)))
(def gvr-proc-class-at (index)
  (let ((classes (gvr-proc-classes)))
    (if (> (len classes) 0) (get (nth classes (min index (- (len classes) 1))) :class) nil)))

;; Map arming (spec §6): a mappable port armed here lights the neuron row's
;; transp / vel x pickers; clicking one binds the port to that payload field.
;; The armed slot / port are view state of this instance.
(def gvr-map-active? (self) (>= self.map-slot 0))
(def gvr-map-port-active? (self slot-id port-name)
  (and (= self.map-slot slot-id) (= self.map-port port-name)))
(def gvr-map-clear (self)
  (do (set! self.map-slot -1) (set! self.map-port "")))
;; Scripting entry: arm (or disarm) process slot `slot-id`'s `port-name`.
(def gvr-map-arm (self slot-id port-name)
  (if (gvr-map-port-active? self slot-id port-name)
    (gvr-map-clear self)
    (do (set! self.map-slot slot-id) (set! self.map-port port-name))))
(def gvr-map-bind (self n field)
  (if (gvr-map-active? self)
    (do
      (graph-node-process-map self n self.map-slot self.map-port field)
      (gvr-map-clear self))
    nil))
(def gvr-map-field-labels (list "transpose" "velocity" "duration" "delay"))
(def gvr-map-field-short (field)
  (if (= field "transpose") "tpose" (if (= field "velocity") "vel" (if (= field "duration") "dur" field))))

(def gvr-proc-number (v fallback)
  (if (number? v) v (if (= v true) 1 fallback)))

;; Inlets a cable lands on: (list (list instance-id inlet) ...), from the bay
;; entries so fan-out cables count too.
(def gvr-proc-wired-inlets (entries)
  (reduce
    (lambda (acc entry)
      (append acc
        (map (lambda (port) (list (get entry :instance-id) (get port :name)))
             (filter (lambda (port) (> (len (get port :writers)) 0)) (get entry :in-ports)))))
    (list)
    entries))

(def gvr-proc-inlet-wired? (wired id inlet-name)
  (> (len (filter (lambda (w) (and (= (nth w 0) id) (= (nth w 1) inlet-name))) wired)) 0))

(def gvr-proc-inlet-row (self n slot inlet wired)
  (let ((id (get slot :instance-id))
        (name (get inlet :name))
        (kind (get inlet :kind))
        (key (str "graph-variable-reset-proc-" n "-" id "-" name))
        (current (gvr-proc-number (get inlet :value) 0))
        (lo (gvr-proc-number (get inlet :min) 0))
        (hi (gvr-proc-number (get inlet :max) 1))
        (set-inlet (lambda (v) (graph-node-process-inlet self n id name v))))
    (h-stack :gap 0.4 :align :center
      (label name :width 4.2 :height gvr-row-height :font-size 8 :h-align :right :color :dim :bg :transparent)
      (if (gvr-proc-inlet-wired? wired id name)
        (label "wired" :width gvr-proc-control-width :height gvr-row-height :font-size 8 :h-align :center :color :accent :bg :transparent)
        (if (= kind "gate")
          (gvr-toggle-sized key gvr-proc-control-width (>= current 0.5)
            (lambda (v) (set-inlet (if v 1 0))))
          (if (= kind "enum")
            (dropdown
              :key key
              :value-index (floor current) :options (get inlet :options)
              :badge-color :transparent :bg-color :bg :border-color :mixer-strip-selected-bg
              :width gvr-proc-control-width :height gvr-row-height :font-size 6
              :on-change (lambda (v) (set-inlet (gvr-index-of (get inlet :options) v))))
            (if (= kind "track")
              (let ((opts (gvr-track-inlet-options self)))
                (dropdown
                  :key key
                  :value-index (gvr-track-inlet-index self (floor current)) :options opts
                  :badge-color :transparent :bg-color :bg :border-color :mixer-strip-selected-bg
                  :width gvr-proc-control-width :height gvr-row-height :font-size 6
                  :on-change (lambda (v) (set-inlet (gvr-track-inlet-value self (gvr-index-of opts v))))))
            (number-picker
              :key key :border-color :dim :background-color :bg
              :value current
              :min (if (= kind "track") 0 lo)
              :max (if (= kind "track") 63 hi)
              :step (if (or (= kind "int") (= kind "track")) 1 0.01)
              :decimals (if (or (= kind "int") (= kind "track")) 0 2)
              :width gvr-proc-control-width :height gvr-row-height :font-size 9
              :on-change set-inlet))))))))

(def gvr-proc-card (self n slots index wired)
  (let ((slot (nth slots index))
        (id (get slot :instance-id))
        (enabled (get slot :enabled)))
    (box
      :key (str "graph-variable-reset-proc-card-" n "-" id)
      :width gvr-proc-card-width
      :padding 0.5
      :background-color :bg
      :border-color (if enabled :process-lane-accent :mixer-strip-border)
      :corner-radius 6
      (v-stack :gap 0.3
        (h-stack :gap 0.3 :align :center
          (label (get slot :label) :width 6.2 :height gvr-row-height :font-size 8 :color :foreground :bg :transparent)
          (button (if enabled "on" "off")
            :key (str "graph-variable-reset-proc-enable-" n "-" id)
            :width 2.0 :height gvr-row-height :padding 0.15 :font-size 7
            :background-color (if enabled :process-lane-accent :transparent)
            :border-color :process-lane-accent
            :color (if enabled :black :dim)
            :on-click (lambda (event) (graph-node-process-enable self n id (not enabled))))
          (button "<"
            :key (str "graph-variable-reset-proc-left-" n "-" id)
            :width 1.4 :height gvr-row-height :padding 0.1 :font-size 7
            :background-color :transparent :border-color :dim :color :dim
            :on-click (lambda (event) (graph-node-process-move self n id -1)))
          (button ">"
            :key (str "graph-variable-reset-proc-right-" n "-" id)
            :width 1.4 :height gvr-row-height :padding 0.1 :font-size 7
            :background-color :transparent :border-color :dim :color :dim
            :on-click (lambda (event) (graph-node-process-move self n id 1)))
          (button "x"
            :key (str "graph-variable-reset-proc-remove-" n "-" id)
            :width 1.4 :height gvr-row-height :padding 0.1 :font-size 7
            :background-color :transparent :border-color :dim :color :dim
            :on-click (lambda (event) (graph-node-process-remove self n id))))
        (each (get slot :inlet-defs) |inlet| (gvr-proc-inlet-row self n slot inlet wired))
        (each (filter (lambda (port) (get port :mappable)) (get slot :ports)) |port|
          (gvr-proc-map-row self n slot port))
        (gvr-proc-meter n slot)))))

;; A live readout under the inlets for slots that have one: lane-harmony's
;; snap meter. Its scope is read inside the subtree, so a fire repaints the
;; meter alone.
(def gvr-proc-meter (n slot)
  (let ((id (get slot :instance-id)))
    (if (= (get slot :class) "lane-harmony")
      (subtree :key (str "graph-variable-reset-proc-meter-" n "-" id)
        (eseq.sequencer/harmony-snap-meter (str "graph-variable-reset-harmony-" n "-" id)
          (eseq.sequencer/process-scope-cells-for id)
          (- gvr-proc-card-width 1)))
      nil)))

;; A mappable port (rand/count/acc `out`, ...) writes onto the fire payload:
;; pick which field. Wire ports go through the bay's cables instead.
(def gvr-proc-map-row (self n slot port)
  (let ((id (get slot :instance-id))
        (port-name (get port :name))
        (mapped (get port :mapped-to)))
    (let ((armed (gvr-map-port-active? self id port-name)))
      (h-stack :gap 0.4 :align :center
        (label (str port-name " ->") :width 4.2 :height gvr-row-height :font-size 8 :h-align :right :color :process-lane-accent :bg :transparent)
        (label (if mapped (gvr-map-field-short mapped) (if armed "pick..." "unmapped"))
          :width 4.0 :height gvr-row-height :font-size 8 :v-align :center
          :color (if (or mapped armed) :process-lane-accent :dim) :bg :transparent)
        (button "map"
          :key (str "graph-variable-reset-proc-map-" n "-" id "-" port-name)
          :width 2.0 :height gvr-row-height :padding 0.15 :font-size 7
          :background-color (if armed :process-lane-accent :transparent)
          :border-color :process-lane-accent
          :color (if armed :black :process-lane-accent)
          :on-click (lambda (event) (gvr-map-arm self id port-name)))
        (if mapped
          (button "x"
            :key (str "graph-variable-reset-proc-unmap-" n "-" id "-" port-name)
            :width 1.4 :height gvr-row-height :padding 0.1 :font-size 7
            :background-color :transparent :border-color :dim :color :dim
            :on-click (lambda (event) (graph-node-process-map self n id port-name nil)))
          nil)))))

;; While a map is armed, a payload-backed picker turns into the target chip:
;; clicking it binds the armed port to `field`.
(def gvr-num-or-target (self n field key value lo hi stp dec on-change)
  (if (gvr-map-active? self)
    (button (str "-> " (gvr-map-field-short field))
      :key (str key "-map-target")
      :width gvr-control-width :height gvr-row-height :padding 0.15 :font-size 7
      :background-color :process-map-arm-bg
      :border-color :process-lane-accent
      :color :process-lane-accent
      :on-click (lambda (event) (gvr-map-bind self n field)))
    (gvr-num key value lo hi stp dec on-change)))

(def gvr-proc-add-card (self n)
  (let ((classes (gvr-proc-class-labels)))
    (box
      :key (str "graph-variable-reset-proc-add-" n)
      :width gvr-proc-card-width
      :padding 0.5
      :background-color :transparent
      :border-color :mixer-strip-border
      :corner-radius 6
      (v-stack :gap 0.3
        (dropdown
          :key (str "graph-variable-reset-proc-add-class-" n)
          :value-index (min self.add-class (- (len classes) 1)) :options classes
          :badge-color :transparent :bg-color :bg :border-color :mixer-strip-selected-bg
          :width (- gvr-proc-card-width 1) :height gvr-row-height :font-size 9
          :on-change (lambda (v) (set! self.add-class (gvr-index-of classes v))))
        (button "+ add process"
          :key (str "graph-variable-reset-proc-add-button-" n)
          :width (- gvr-proc-card-width 1) :height gvr-row-height :padding 0.15 :font-size 7
          :background-color :transparent :border-color :process-lane-accent :color :process-lane-accent
          :on-click (lambda (event)
            (graph-node-process-add self n (gvr-proc-class-at self.add-class))))))))

;; The slot the inspector card shows: the bay's selection, else the first.
(def gvr-proc-selected-index (slots)
  (let ((selected (eseq.sequencer/lane-patch-node-selected-id))
        (hits (filter (lambda (i) (= (get (nth slots i) :instance-id) selected)) (range 0 (len slots)))))
    (if (> (len hits) 0) (nth hits 0) (if (> (len slots) 0) 0 -1))))

;; The node's patch: the shared lane patchbay (cards, ports, drag cables,
;; cable select + × / Backspace, fan-out) over this node's chain, with the
;; selected slot's inspector card (on/off, order, remove, inlet knobs) beside
;; it. Wires pointing up the chain land next fire, as on a track. The bay's
;; namespace derives from the instance id, so two instances never share one.
(def gvr-node-patch (self n)
  (let ((slots (gvr-node-patch-slots self n))
        (entries (gvr-node-patch-entries self n))
        (ns (eseq.sequencer/lane-patch-node-namespace self n)))
    (let ((wired (gvr-proc-wired-inlets entries))
          (index (gvr-proc-selected-index slots)))
      (v-stack :gap 0.4
        (label "process patch: runs on every fire before emit + scatter. veto mutes, writes ride on. drag a port onto an inlet to wire it"
          :width 70 :height 1.0 :font-size 8 :color :dim :bg :transparent)
        (h-stack :gap 0.6 :align :top
          (box :flex 1 :padding 0 :bg :transparent
            :key (str "graph-variable-reset-proc-bay-" n)
            (eseq.sequencer/lane-patchbay-node ns (gvr-proc-add-card self n)))
          (if (>= index 0) (gvr-proc-card self n slots index wired) nil))))))

;; One row / column of the weight matrix as a labeled 1xN strip.
(def gvr-edge-strip (self title n active-count direction)
  (let ((values (if (= direction :out)
                  (list (map (lambda (c) (graph-edge-value self n c :weight)) (range 0 active-count)))
                  (list (map (lambda (r) (graph-edge-value self r n :weight)) (range 0 active-count))))))
    (v-stack :gap 0.2
      (label title :width 10 :height 0.9 :font-size 8 :color :dim :bg :transparent)
      (matrix
        :key (str "graph-variable-reset-edge-strip-" (if (= direction :out) "out" "in") "-" n)
        :rows 1
        :cols active-count
        :width (* active-count 2.2)
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
          (let ((from (if (= direction :out) n c))
                (to (if (= direction :out) c n)))
            (graph-edge self :from from :to to :weight v)))))))

(def gvr-expanded-editor (self n active-count track-colors)
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
          :on-click (lambda (event) (gvr-expand-node self (max 0 (- n 1)))))
        (label (str "neuron " n) :width 6 :height 1.2 :font-size 11 :h-align :center :color :foreground :bg :transparent)
        (button ">"
          :key "graph-variable-reset-expanded-next"
          :width 1.6 :height gvr-row-height :padding 0.1 :font-size 7
          :background-color :transparent :border-color :dim :color :dim
          :on-click (lambda (event) (gvr-expand-node self (min (- active-count 1) (+ n 1))))))
      (v-stack :gap gvr-row-gap
        (gvr-header)
        (gvr-row self n track-colors))
      (h-stack :gap 1.5
        (gvr-edge-strip self (str "in  (from node -> " n ")") n active-count :in)
        (gvr-edge-strip self (str "out (" n " -> to node)") n active-count :out))
      (gvr-node-patch self n))))

;; The kind's :view: the whole panel of instance `self`. Every read below is
;; tracked (graph reads, `self.*` view cells, SEQ fields), so the host re-runs
;; it only for what changed; playback activity is read only inside the
;; visualizers' subtrees, so a firing history or a track's notes never
;; rebuild the graph controls.
(def gvr-panel (self)
  (let ((active-count (gvr-node-count self))
        (track-colors (gvr-route-track-colors self SEQ.track-colors))
        (rack (gvr-owner-rack self))
        (expanded self.expanded-node))
    (box
      :padding 0.85
      :gap 0.6
      (v-stack :gap 0.5
        ;; ── sequencer-level config (on top) ──
        (box
          ;; Rack-owned: room for the corner chip past the spectrogram.
          :width (if rack 102 90.5)
          :background-color :mixer-strip-bg :border-color :mixer-strip-border :padding 1 :corner-radius 16

          (h-stack
            (v-stack
              (h-stack :gap 0.6 :align :center
                (label "variable graph" :width 8 :height 1.2 :font-size 11 :color :foreground :bg :transparent)
                (label "nodes" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                (gvr-num "graph-variable-reset-node-count"
                  (bind-graph-config self :node-count) 1 16 1 0
                  (lambda (v) (gvr-edit-config self :node-count v))))
              (h-stack :gap 0.6 :align :center
                (label "reset bars" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                (gvr-num "graph-variable-reset-reset-bars"
                  (bind-graph-config self :reset-bars) 0 64 1 0
                  (lambda (v) (gvr-edit-config self :reset-bars v))))
              (h-stack :gap 0.6 :align :center
                (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                (gvr-num "graph-variable-reset-max-poly"
                  (bind-graph-config self :max-poly) 0 16 1 0
                  (lambda (v) (gvr-edit-config self :max-poly v))))
              (h-stack :gap 0.6 :align :center
                (label "poly mode" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                (gvr-pick-sized "graph-variable-reset-max-poly-selection"
                  (gvr-index-of gvr-max-poly-selection-options (graph-config-value self :max-poly-selection))
                  gvr-max-poly-selection-options 9.5
                  (lambda (v) (gvr-edit-config self :max-poly-selection v))))
              ;; Batch params: node 0's handle shows the value every node
              ;; carries (the edit writes them all, which echoes node 0).
              (h-stack :gap 0.6 :align :center
                (label "threshold" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                (gvr-num "graph-variable-reset-threshold"
                  (bind-graph self 0 :threshold) 0 4 0.01 2
                  (lambda (v) (gvr-edit-capacity-param self :threshold v))))
              (h-stack :gap 0.6 :align :center
                (label "global trn" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                (gvr-num "graph-variable-reset-global-transpose"
                  (bind-graph self 0 :global-transpose) -48 48 1 0
                  (lambda (v) (gvr-edit-global-param self :global-transpose v)))
                (label "dur x" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim :bg :transparent)
                (gvr-num "graph-variable-reset-dur-factor"
                  (bind-graph self 0 :dur-factor) 0 8 0.25 2
                  (lambda (v) (gvr-edit-global-param self :dur-factor v)))))
            (subtree :key "graph-variable-reset-dampening-matrix"
              (let ((viz (gvr-viz self SEQ.graph-visualizations)))
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
                  :value (gvr-viz-matrix viz :dampening-matrix (gvr-zero-matrix active-count) active-count active-count))))

            (subtree :key "graph-variable-reset-event-view"
              (let ((viz (gvr-viz self SEQ.graph-visualizations)))
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
                  :background :bg
                  :width 16
                  :height 7)))
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
            (gvr-owner-rack-badge self)
            (box :width 1 :height 1.0)))

        (h-stack
          (if (and (>= expanded 0) (< expanded active-count))
            (gvr-expanded-editor self expanded active-count track-colors)
            (box
              :padding gvr-row-panel-padding
              :border-color :mixer-strip-border
              :background-color :mixer-strip-bg :corner-radius 16
              (v-stack :gap 0.5
                (v-stack :gap gvr-row-gap
                  (gvr-header)
                  (each (range 0 active-count) |n| (gvr-row self n track-colors))))))

          (v-stack :gap gvr-matrix-column-gap
            (label "" :width 0.1 :height (gvr-matrix-header-spacer-height) :font-size 1 :bg :transparent)
            (subtree :key "graph-variable-reset-trigger-matrix"
              (let ((viz (gvr-viz self SEQ.graph-visualizations)))
                (matrix
                  :key "graph-variable-reset-trigger-matrix"
                  :rows active-count
                  :cols 1
                  :width 1
                  :height (gvr-matrix-data-height active-count)
                  :min 0
                  :max 1
                  :value (gvr-viz-matrix viz :trigger-matrix (gvr-zero-column-matrix active-count) active-count 1)))))

          (v-stack :gap gvr-matrix-column-gap
            (label "" :width 0.1 :height (gvr-matrix-header-spacer-height) :font-size 1 :bg :transparent)
            (subtree :key "graph-variable-reset-energy-matrix"
              (let ((viz (gvr-viz self SEQ.graph-visualizations)))
                (matrix
                  :key "graph-variable-reset-energy-matrix"
                  :rows active-count
                  :cols 1
                  :width 2
                  :height (gvr-matrix-data-height active-count)
                  :min 0
                  :max 4
                  :value (gvr-viz-matrix viz :energy-matrix (gvr-zero-column-matrix active-count) active-count 1)))))

          ;; The weights are N² tracked reads; the subtree keeps a cell drag
          ;; from re-running the rows beside it.
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
                :value (gvr-read-weights self)
                :on-cell-press (lambda (r c) (set! self.selected-neuron c))
                :on-cell-release (lambda (r c) (set! self.selected-neuron -1))
                :on-cell-change (lambda (r c v) (graph-edge self :from r :to c :weight v))))))
        (box
          :debug-name "graph-variable-reset-piano-panel"
          :padding 1
          :background-color :mixer-strip-bg
          :border-color :mixer-strip-border
          :corner-radius 12
          (subtree :key "graph-variable-reset-piano"
            (piano-keyboard
              :key "graph-variable-reset-piano"
              :notes-by-track SEQ.track-active-notes
              :track-colors track-colors
              :tracks (range 0 active-count)
              :overlap-mode :loudest
              :press-depth self.piano-depth
              :start-note 12
              :key-count 80
              :width 84
              :height 3.5)))))))

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
  :state ((expanded-node -1) (selected-neuron -1) (add-class 0)
          (map-slot -1) (map-port "") (piano-depth 0.6))
  :view gvr-panel
  :keymap eseq.sequencer-keys/sequencer-keys
  :on-create gvr-init-ring-defaults)
