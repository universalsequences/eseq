;; Graph-mode 8x8 Markov sequencer.
;;
;; This is the probabilistic-transition counterpart to graph-neural-8x8-demo.lisp:
;; each fired state emits once, chooses exactly one outgoing edge with probability
;; proportional to that row's weight values, and schedules the chosen target after the
;; source state's delay. Put a trigger on track 0 to seed state 0; the seed step
;; itself plays normally and starts the chain.
;;
;; The panel reads and edits the graph through the kinds (kind-bindings spec §14.2k):
;; every control edit is one undo entry, a drag's frames joining one.
;;
;; Project scratch entrypoint:
;;   (load "content/scripts/sequencers/graph-markov-8x8-demo.lisp")
;;
;; Loading this file publishes the graph/UI only. For a fresh patch, run:
;;   (script-init-fn)

(import eseq.kinds :refer (graph-of graph-param-named graph-quantize-options))
(import eseq.view-kit :refer (nothing))
(import eseq.graph-kit :refer (route-options node-route-label set-route-label! weight-rows
                               set-weight! column rack-name res-options))

;; `def-sequencer` returns the instance handle; every graph-* native below takes
;; it, so this script also works when a drum rack owns it (routes then address
;; rack members and the handle stays unambiguous next to a project-owned copy).
(def m8-name (def-sequencer "markov-8x8-demo"
  :shape (line 8)
  :energy-decay 1
  :reset-every 0
  :seed-on-reset 0
  :max-poly 4
  :max-poly-selection :deterministic
  :duration (steps 1)

  (def-node state
    :resolution :16
    :delay 1
    :quantize :16
    :route 0
    :seed-from ()
    :reduce :max
    :event :newest
    :params ((transpose :int -48 48 :default 0)
      (vel-scale :float 0 2 :default 1.0))
    :state ((energy :leak (per-step :energy-decay)))
    :update (if (> (input) 0)
      (emit :note (+ (in-note) (param :transpose))
        :vel (* (in-vel) (param :vel-scale))
        :dur (seed))
      false))

  (edges
    :from state
    :to state
    :topology (all-to-all)
    :distribution :weighted-choice
    :gather (edge :weight)
    :params ((weight :float 0 1 :default 0.0)))))

(def m8-node-count 8)
(def script-buffer-name "*markov-8x8*")
;; Owned by a rack: routes address its members and the tab wears its name.
(def m8-owner-rack (graph-owner m8-name))
(def script-tab-label (if m8-owner-rack (rack-name m8-owner-rack) "Markov 8x8"))
(def script-sequencer-name "markov-8x8-demo")

;; ── init helpers (explicit-only; loading the file does NOT call these) ──
;; They write through the graph-* natives, which answer at once: the graph is
;; not a kind instance until the host's next sync.

(def m8-ring-weights ()
  (list
    (list 0.05 0.65 0.15 0.00 0.10 0.00 0.05 0.00)
    (list 0.00 0.10 0.55 0.15 0.00 0.15 0.05 0.00)
    (list 0.20 0.00 0.10 0.50 0.10 0.00 0.10 0.00)
    (list 0.00 0.25 0.00 0.10 0.45 0.10 0.00 0.10)
    (list 0.10 0.00 0.20 0.00 0.10 0.45 0.10 0.05)
    (list 0.00 0.15 0.00 0.20 0.00 0.10 0.45 0.10)
    (list 0.25 0.00 0.10 0.00 0.15 0.00 0.10 0.40)
    (list 0.55 0.10 0.00 0.10 0.00 0.15 0.00 0.10)))

(def m8-node-delays ()
  (list 1 1 2 1 3 2 1 4))

(def m8-apply-edge-matrix (field matrix)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge m8-name :from r :to c field (nth (nth matrix r) c)))
        (range m8-node-count)))
    (range m8-node-count)))

(def m8-apply-node-delays (delays)
  (for-each
    (lambda (n)
      (graph-node m8-name n :delay (nth delays n)))
    (range m8-node-count)))

(def m8-init-defaults ()
  (m8-apply-edge-matrix :weight (m8-ring-weights))
  (m8-apply-node-delays (m8-node-delays))
  (graph-node m8-name 0 :seed-from 0)
  (graph-param m8-name 0 :transpose 0)
  (graph-param m8-name 1 :transpose 2)
  (graph-param m8-name 2 :transpose 3)
  (graph-param m8-name 3 :transpose 5)
  (graph-param m8-name 4 :transpose 7)
  (graph-param m8-name 5 :transpose 10)
  (graph-param m8-name 6 :transpose 12)
  (graph-param m8-name 7 :transpose -5))

(def script-init-fn ()
  (m8-init-defaults))

;; ── UI ──

(def m8-row-height 1.3)
(def m8-node-width 1.4)
(def m8-control-width 7.0)

(def m8-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width m8-control-width :height m8-row-height :font-size 9
    :on-change on-change))

(def m8-pick (key value options on-change)
  (dropdown
    :key key
    :value value :options options
    :width m8-control-width :height m8-row-height :font-size 9
    :on-change on-change))

;; Node n's param `name`, bound.
(def m8-param (n name lo hi stp dec)
  (let ((p (graph-param-named n name)))
    (m8-num (str "markov-8x8-" name "-" n.index) #'p.value lo hi stp dec
      (lambda (v) (set! p.value v)))))

(def m8-row (n routes)
  (subtree :key (str "markov-8x8-row-" n.index)
    (h-stack :gap 0.4 :align :center
      (label (str n.index) :width m8-node-width :height m8-row-height :font-size 9 :h-align :center :color :dim)
      (m8-pick (str "markov-8x8-route-" n.index)
        (node-route-label n) routes
        (lambda (label) (set-route-label! n label)))
      (m8-num (str "markov-8x8-delay-" n.index) #'n.delay 0 16 1 0
        (lambda (v) (set! n.delay v)))
      (m8-param n "transpose" -48 48 1 0)
      (m8-param n "vel-scale" 0 2 0.01 2)
      (m8-pick (str "markov-8x8-resolution-" n.index) n.resolution res-options
        (lambda (v) (set! n.resolution v)))
      (m8-pick (str "markov-8x8-quantize-" n.index) n.quantize graph-quantize-options
        (lambda (v) (set! n.quantize v))))))

(def m8-header-label (text width)
  (label text :width width :height 1.0 :font-size 8 :h-align :center :color :dim))

(def m8-header ()
  (h-stack :gap 0.4 :align :center
    (m8-header-label "node" m8-node-width)
    (m8-header-label "route" m8-control-width)
    (m8-header-label "delay" m8-control-width)
    (m8-header-label "transp" m8-control-width)
    (m8-header-label "vel x" m8-control-width)
    (m8-header-label "res" m8-control-width)
    (m8-header-label "quant" m8-control-width)))

;; A playback column (each node's trigger or energy), in a subtree of its
;; own so playback re-runs only it.
(def m8-column-matrix (title title-width key width hi values)
  (v-stack :gap 0.35
    (label title :width title-width :height 2.5 :font-size 8 :color :dim)
    (subtree :key key
      (matrix
        :key key
        :rows 8
        :cols 1
        :width width
        :height 12
        :min 0
        :max hi
        :value (column (values))))))

(def m8-graph-panel (g)
  (box
    :padding 0.85
    :gap 0.6
    :width 42
    :height 43
    (v-stack :gap 0.55
      (h-stack :gap 0.6 :align :center
        (label "8x8 markov" :width 8 :height 1.2 :font-size 11 :color :foreground)
        (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
        (m8-num "markov-8x8-max-poly" #'g.max-poly 0 16 1 0
          (lambda (v) (set! g.max-poly v))))
      (h-stack
        (v-stack :gap 0.5
          (label "per-state controls" :width 14 :height 1.2 :font-size 9 :color :dim)
          (v-stack :gap 0.2
            (m8-header)
            (let ((routes (route-options g)))
              (each g.nodes |n| (m8-row n routes)))))
        (m8-column-matrix "trig" 2 "markov-8x8-trigger-matrix" 1 1 (lambda () g.triggers))
        (m8-column-matrix "energy" 3 "markov-8x8-energy-matrix" 2 4 (lambda () g.energy)))
      (v-stack :gap 0.35
        (label "transition weights (row -> col)" :width 18 :height 1.3 :font-size 8 :color :dim)
        (subtree :key "markov-8x8-weight-matrix"
          (matrix
            :key "markov-8x8-weight-matrix"
            :rows 8
            :cols 8
            :width 26
            :height 12
            :min 0
            :max 1
            :value (weight-rows g)
            :on-cell-change (lambda (r c v) (set-weight! g r c v))))))))

;; The panel, empty until the host publishes the graph (at its next sync).
(def m8-panel ()
  (let ((g (graph-of m8-name)))
    (if g (m8-graph-panel g) (nothing))))

(effect-buffer "*markov-8x8*" (m8-panel))
(eseq.seq-step-tabs/seq-register-script-step-sequencer-tab script-tab-label script-buffer-name script-sequencer-name "")
