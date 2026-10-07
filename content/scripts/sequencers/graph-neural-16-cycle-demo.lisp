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
;; Tokens are the dropdowns' labels in any case: 1 2 4 8 16 32 64 2T..64T Prh, and
;; the graph-* natives' words (sixteenth, quarter-triplet, polyrhythm …). Other
;; tokens (`off` included) are dropped, so a half-typed field is safe; a quantize
;; field with no timebase left is off. The text typed into a field shows while the
;; node's cycle is still the one it set, so typing (including spaces) isn't
;; rewritten mid-keystroke; an undo, a scene switch or an init shows the cycle.
;;
;; The panel reads and edits the graph through the kinds (kind-bindings spec §14.2k,
;; §14.2s): every control edit is one undo entry, a drag's frames joining one. The
;; timing controls (dur x, swing) set every node's param at once through the graph-*
;; natives, unrecorded.
;;
;; Project scratch entrypoint:
;;   (load "content/scripts/sequencers/graph-neural-16-cycle-demo.lisp")
;;
;; Loading this file only publishes the graph/UI. It does not write graph overrides.
;; For a fresh demo patch, explicitly run:
;;   (script-init-fn)

(import eseq.kinds :refer (graph-of graph-param-named graph-timebase-options))
(import eseq.view-kit :refer (nothing))
(import eseq.graph-kit :refer (route-options node-route-label set-route-label! weight-rows
                               set-weight! column rack-name cycle-text cycle-labels
                               set-param-on-nodes!))

;; `def-sequencer` returns the instance handle; every graph-* native below takes
;; it, so this script also works when a drum rack owns it (routes then address
;; rack members and the handle stays unambiguous next to a project-owned copy).
(def g16c-name (def-sequencer "neural-16-cycle-demo"
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

(def g16c-node-count 16)
(def script-buffer-name "*16x16-cycle*")
;; Owned by a rack: routes address its members and the tab wears its name.
(def g16c-owner-rack (graph-owner g16c-name))
(def script-tab-label (if g16c-owner-rack (rack-name g16c-owner-rack) "16x16 cyc"))
(def script-sequencer-name "neural-16-cycle-demo")

;; The cycle text typed into the fields, one (dict :node :field :text :cycle)
;; per field: `cycle` is the node's cycle the text set (or left).
(def-kind g16c-view
  :key ()
  :state ((drafts (list))))

;; ── init helpers (explicit-only; loading the file does NOT call these) ──
;; They write through the graph-* natives, which answer at once: the graph is
;; not a kind instance until the host's next sync.

(def g16c-ring-weights ()
  (map
    (lambda (r)
      (map
        (lambda (c) (if (= c (if (= r (- g16c-node-count 1)) 0 (+ r 1))) 1 0))
        (range 0 g16c-node-count)))
    (range 0 g16c-node-count)))

(def g16c-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g16c-name :from r :to c :weight (nth (nth w r) c)))
        (range g16c-node-count)))
    (range g16c-node-count)))

(def g16c-init-ring-defaults ()
  (g16c-apply-weights (g16c-ring-weights))
  (graph-node g16c-name 0 :seed-from 0)
  ;; Showcase the feature: node 0 runs mostly 1/16 with a 1/4 break-out every 6th fire,
  ;; node 1 lurches on a 3-slot cycle that phases against it.
  (graph-node g16c-name 0 :resolution "16 16 16 16 16 4")
  (graph-node g16c-name 1 :resolution "16 8 16"))

(def script-init-fn ()
  (g16c-init-ring-defaults))

;; ── cycles ──

;; Node n's `field` cycle ("resolution" or "quantize").
(def g16c-cycle (n field)
  (if (= field "resolution") n.resolution-cycle n.quantize-cycle))

(def g16c-draft (n field)
  (first (filter (lambda (d) (and (= d.node n.index) (= d.field field))) g16c-view.drafts)))

;; What node n's `field` cycle field shows: the text typed into it while
;; the node's cycle is still the one it set, else the cycle.
(def g16c-cycle-shown (n field)
  (let ((cycle (g16c-cycle n field))
        (d (g16c-draft n field)))
    (if (and d (= d.cycle cycle)) d.text (cycle-text cycle))))

;; Keep the typed text and set the node's cycle to its labels (a
;; resolution's only when it names one; a quantize's off when it names none).
(def g16c-edit-cycle (n field text)
  (let ((labels (cycle-labels text graph-timebase-options)))
    (let ((cycle (if (empty? labels)
                   (if (= field "resolution") n.resolution-cycle (list "off"))
                   labels)))
      (set! g16c-view.drafts
        (cons (dict :node n.index :field field :text text :cycle cycle)
              (filter (lambda (d) (not (and (= d.node n.index) (= d.field field))))
                      g16c-view.drafts)))
      (if (= field "resolution")
        (unless (empty? labels) (set! n.resolution-cycle cycle))
        (set! n.quantize-cycle cycle)))))

;; ── UI ──

(def g16c-row-height 0.9)
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

(def g16c-pick (key value options on-change)
  (dropdown
    :key key
    :value value :options options
    :width g16c-dropdown-width :height g16c-row-height :font-size 9
    :on-change on-change))

;; Node n's param `name`, bound.
(def g16c-param (n name lo hi stp dec)
  (let ((p (graph-param-named n name)))
    (g16c-num (str "graph-16-" name "-" n.index) #'p.value lo hi stp dec
      (lambda (v) (set! p.value v)))))

;; A param every node carries alike: shows node 0's, sets every node's.
(def g16c-global-param (g key name lo hi stp dec)
  (let ((p (graph-param-named (first g.nodes) name)))
    (g16c-num key #'p.value lo hi stp dec
      (lambda (v) (set-param-on-nodes! g g16c-node-count name v)))))

;; Text field for node n's resolution or quantize cycle, in a subtree of its
;; own: a keystroke re-runs the fields, never the rows.
(def g16c-cycle-input (n field placeholder)
  (let ((key (str "graph-16c-" field "-" n.index)))
    (subtree :key key
      (text-input
        :key key
        :value (g16c-cycle-shown n field) :placeholder placeholder
        :width g16c-cycle-width :height g16c-row-height :font-size 9
        :on-change (lambda (text) (g16c-edit-cycle n field text))))))

(def g16c-row (n routes)
  (subtree :key (str "graph-16c-row-" n.index)
    (h-stack :gap 0.4 :align :center
      (label (str n.index) :width g16c-node-width :height g16c-row-height :font-size 9 :h-align :center :color :dim)
      (g16c-pick (str "graph-16-route-" n.index)
        (node-route-label n) routes
        (lambda (label) (set-route-label! n label)))
      (g16c-num (str "graph-16-delay-" n.index) #'n.delay 0 16 1 0
        (lambda (v) (set! n.delay v)))
      (g16c-param n "transpose" -48 48 1 0)
      (g16c-param n "transpose-reset" 0 1 1 0)
      (g16c-param n "vel-decay" 0 2 0.01 2)
      (g16c-param n "vel-reset" 0 1 1 0)
      (g16c-param n "state-reset" 0 1 1 0)
      (g16c-param n "dampening" 0 1 0.01 2)
      (g16c-param n "recovery" 0 1 0.01 2)
      (g16c-cycle-input n "resolution" "16 16 16 16 16 4")
      (g16c-cycle-input n "quantize" "off"))))

(def g16c-header-label (text width)
  (label text :width width :height 1.0 :font-size 8 :h-align :center :color :dim))

(def g16c-header ()
  (h-stack :gap 0.4 :align :center
    (g16c-header-label "node" g16c-node-width)
    (g16c-header-label "route" g16c-dropdown-width)
    (g16c-header-label "delay" g16c-control-width)
    (g16c-header-label "transp" g16c-control-width)
    (g16c-header-label "trn rst" g16c-control-width)
    (g16c-header-label "vel x" g16c-control-width)
    (g16c-header-label "vel rst" g16c-control-width)
    (g16c-header-label "state rst" g16c-control-width)
    (g16c-header-label "dampen" g16c-control-width)
    (g16c-header-label "recover" g16c-control-width)
    (g16c-header-label "res cycle" g16c-cycle-width)
    (g16c-header-label "quant cycle" g16c-cycle-width)))

(def g16c-config-label (text width)
  (label text :width width :height 1.2 :font-size 9 :h-align :right :color :dim))

;; A playback column (each node's trigger or energy), in a subtree of its
;; own so playback re-runs only it.
(def g16c-column-matrix (key width hi values)
  (v-stack :gap 0.35
    (box :height 2.5)
    (subtree :key key
      (matrix
        :key key
        :rows 16
        :cols 1
        :width width
        :height 17.5
        :min 0
        :max hi
        :value (column (values))))))

(def g16c-graph-panel (g)
  (box
    :padding 0.85
    :gap 0.6
    :width 42
    :height 47
    (v-stack
      (box
        :corner-radius 16
        :padding 1
        :width 82
        :background-color :mixer-strip-bg
        :border-color :mixer-strip-border
        (h-stack
          (v-stack :gap 0.5
            (h-stack :gap 0.6 :align :center
              (label "16x16 graph" :width 8 :height 1.2 :font-size 11 :color :foreground)
              (g16c-config-label "reset bars" 6)
              (g16c-num "graph-16-reset-bars" #'g.reset-bars 0 64 1 0
                (lambda (v) (set! g.reset-bars v)))
              (g16c-config-label "max poly" 6)
              (g16c-num "graph-16-max-poly" #'g.max-poly 0 16 1 0
                (lambda (v) (set! g.max-poly v))))
            (h-stack :gap 0.6 :align :center
              (label "timing" :width 8 :height 1.2 :font-size 9 :color :dim)
              (g16c-config-label "dur x" 6)
              (g16c-global-param g "graph-16-dur-factor" "dur-factor" 0 8 0.25 2)
              (g16c-config-label "swing" 6)
              (g16c-global-param g "graph-16-swing" "swing" 50 75 1 0)))
          (h-stack :gap 0.45
            (subtree :key "graph-16-dampening-matrix"
              (matrix
                :key "graph-16-dampening-matrix"
                :rows 16
                :cols 16
                :width 12
                :height 6
                :control :grid
                :background (rgba 0.1 0.1 0.1 .1)
                :fill :primary
                :min 0
                :max 1
                :value g.dampening))
            (subtree :key "graph-16c-event-view"
              (event-view
                :key "graph-16c-event-view"
                :events g.events
                :current-beat #'g.beat
                :renderer :isometric
                :x :transpose
                :x-min -24
                :background (rgba 0.1 0.1 0.1 0.1)
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
                :width 14
                :height 6))
            (spectrogram
              :key "graph-16c-master-spectrogram"
              :source :master
              :mode :waterfall
              :freq-scale :log
              :fft-size 2048
              :time-slices 180
              :min-db -64
              :max-db 0
              :smoothing 0.68
              :width 20
              :height 6.0
              :background-color (rgba 0.13 0.13 0.13 1.00)
              :min-color (rgba 0.05 0.05 0.11 1)
              :mid-color (rgba 0.16 0.66 0.88 1)
              :max-color (rgba 1.0 0.72 0.28 1)))))
      (h-stack
        (v-stack :gap 0.5
          (h-stack :gap 0.5 :align :center
            (label "per-node knobs" :width 14 :height 1.2 :font-size 9 :color :dim))
          (v-stack :gap 0.2
            (g16c-header)
            (let ((routes (route-options g)))
              (each g.nodes |n| (g16c-row n routes)))))
        (g16c-column-matrix "graph-16-trigger-matrix" 1 1 (lambda () g.triggers))
        (g16c-column-matrix "graph-16-energy-matrix" 2 4 (lambda () g.energy))
        (v-stack :gap 0.35
          (box :height 2.5)
          (subtree :key "graph-16-weight-matrix"
            (matrix
              :key "graph-16-weight-matrix"
              :rows 16
              :cols 16
              :width 45
              :height 17.5
              :min 0
              :max 1
              :value (weight-rows g)
              :on-cell-change (lambda (r c v) (set-weight! g r c v)))))))))

;; The panel, empty until the host publishes the graph (at its next sync).
(def g16c-panel ()
  (let ((g (graph-of g16c-name)))
    (if g (g16c-graph-panel g) (nothing))))

(effect-buffer "*16x16-cycle*" (g16c-panel))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
