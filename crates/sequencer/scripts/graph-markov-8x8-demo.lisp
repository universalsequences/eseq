;; Graph-mode 8x8 Markov sequencer.
;;
;; This is the probabilistic-transition counterpart to graph-neural-8x8-demo.lisp:
;; each fired state emits once, chooses exactly one outgoing edge with probability
;; proportional to that row's weight values, and schedules the chosen target after the
;; source state's delay. Put a trigger on track 0 to seed state 0; the seed step
;; itself plays normally and starts the chain.
;;
;; Project scratch entrypoint:
;;   (load "crates/sequencer/scripts/graph-markov-8x8-demo.lisp")
;;
;; Loading this file publishes the graph/UI only. For a fresh patch, run:
;;   (script-init-fn)

(def-sequencer "markov-8x8-demo"
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
    :params ((weight :float 0 1 :default 0.0))))

(def m8-name "markov-8x8-demo")
(def m8-node-count 8)
(def script-buffer-name "*markov-8x8*")
(def script-tab-label "Markov 8x8")

(def m8-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def m8-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def m8-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
        "Off"))

(def m8-index-of (xs item)
  (let ((hits (filter (lambda (i) (= (nth xs i) item)) (range 0 (len xs)))))
    (if (> (len hits) 0) (nth hits 0) 0)))

(def m8-route->internal (label)
  (if (= label "Off") :off (m8-index-of m8-route-options label)))

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

(defstate m8-weights (list))

(def m8-read-edge-matrix (field)
  (map
    (lambda (r) (map (lambda (c) (graph-edge-value m8-name r c field)) (range 0 m8-node-count)))
    (range 0 m8-node-count)))

(def m8-set-cell (matrix r c v)
  (set-nth matrix r (set-nth (nth matrix r) c v)))

(def m8-zero-row ()
  (map (lambda (n) 0) (range 0 m8-node-count)))

(def m8-zero-matrix ()
  (map (lambda (n) (m8-zero-row)) (range 0 m8-node-count)))

(def m8-zero-column-matrix ()
  (map (lambda (n) (list 0)) (range 0 m8-node-count)))

(def m8-viz (visualizations)
  (let ((hits (filter (lambda (viz) (= (get viz :name) m8-name)) visualizations)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def m8-viz-matrix (viz field fallback)
  (if viz
    (let ((value (get viz field)))
      (if value value fallback))
    fallback))

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
  (do
    (set! m8-weights (m8-ring-weights))
    (m8-apply-edge-matrix :weight m8-weights)
    (m8-apply-node-delays (m8-node-delays))
    (graph-node m8-name 0 :seed-from 0)
    (graph-param m8-name 0 :transpose 0)
    (graph-param m8-name 1 :transpose 2)
    (graph-param m8-name 2 :transpose 3)
    (graph-param m8-name 3 :transpose 5)
    (graph-param m8-name 4 :transpose 7)
    (graph-param m8-name 5 :transpose 10)
    (graph-param m8-name 6 :transpose 12)
    (graph-param m8-name 7 :transpose -5)))

(def script-init-fn ()
  (m8-init-defaults))

(def m8-edit-num (n field v)
  (do
    (reactive-set "GRAPH" (graph-key m8-name n field) v)
    (graph-node m8-name n field v)))

(def m8-edit-param (n field v)
  (do
    (reactive-set "GRAPH" (graph-key m8-name n field) v)
    (graph-param m8-name n field v)))

(def m8-edit-enum (n field options label internal)
  (do
    (reactive-set "GRAPH" (graph-key m8-name n field) (m8-index-of options label))
    (graph-node m8-name n field internal)))

(def m8-edit-config (field v)
  (do
    (reactive-set "GRAPH" (graph-config-key m8-name field) v)
    (graph-config m8-name field v)))

(def m8-row-height 1.3)
(def m8-node-width 1.4)
(def m8-control-width 7.0)

(def m8-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width m8-control-width :height m8-row-height :font-size 9
    :on-change on-change))

(def m8-pick (key value-index options on-change)
  (dropdown
    :key key
    :value-index value-index :options options
    :width m8-control-width :height m8-row-height :font-size 9
    :on-change on-change))

(def m8-row (n)
  (h-stack :gap 0.4 :align :center
    (label (str n) :width m8-node-width :height m8-row-height :font-size 9 :h-align :center :color :dim)
    (m8-pick (str "markov-8x8-route-" n)
      (bind-graph m8-name n :route m8-route-options) m8-route-options
      (lambda (v) (m8-edit-enum n :route m8-route-options v (m8-route->internal v))))
    (m8-num (str "markov-8x8-delay-" n)
      (bind-graph m8-name n :delay) 0 16 1 0
      (lambda (v) (m8-edit-num n :delay v)))
    (m8-num (str "markov-8x8-transpose-" n)
      (bind-graph m8-name n :transpose) -48 48 1 0
      (lambda (v) (m8-edit-param n :transpose v)))
    (m8-num (str "markov-8x8-vel-scale-" n)
      (bind-graph m8-name n :vel-scale) 0 2 0.01 2
      (lambda (v) (m8-edit-param n :vel-scale v)))
    (m8-pick (str "markov-8x8-resolution-" n)
      (bind-graph m8-name n :resolution m8-res-options) m8-res-options
      (lambda (v) (m8-edit-enum n :resolution m8-res-options v v)))
    (m8-pick (str "markov-8x8-quantize-" n)
      (bind-graph m8-name n :quantize m8-quant-options) m8-quant-options
      (lambda (v) (m8-edit-enum n :quantize m8-quant-options v v)))))

(def m8-header ()
  (h-stack :gap 0.4 :align :center
    (label "node" :width m8-node-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "route" :width m8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "delay" :width m8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "transp" :width m8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "vel x" :width m8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "res" :width m8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "quant" :width m8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)))

(def m8-panel (current-pattern graph-visualizations)
  (do
    current-pattern
    (set! m8-weights (m8-read-edge-matrix :weight))
    (let ((viz (m8-viz graph-visualizations)))
      (box
        :padding 0.85
        :gap 0.6
        :width 42
        :height 43
        (v-stack :gap 0.55
          (h-stack :gap 0.6 :align :center
            (label "8x8 markov" :width 8 :height 1.2 :font-size 11 :color :foreground)
            (label "max poly" :width 6 :height 1.2 :font-size 9 :h-align :right :color :dim)
            (m8-num "markov-8x8-max-poly"
              (bind-graph-config m8-name :max-poly) 0 16 1 0
              (lambda (v) (m8-edit-config :max-poly v))))
          (h-stack
            (v-stack :gap 0.5
              (label "per-state controls" :width 14 :height 1.2 :font-size 9 :color :dim)
              (v-stack :gap 0.2
                (m8-header)
                (each (range 0 m8-node-count) |n| (m8-row n))))
            (v-stack :gap 0.35
              (label "trig" :width 2 :height 2.5 :font-size 8 :color :dim)
              (matrix
                :key "markov-8x8-trigger-matrix"
                :rows 8
                :cols 1
                :width 1
                :height 12
                :min 0
                :max 1
                :value (m8-viz-matrix viz :trigger-matrix (m8-zero-column-matrix))))
            (v-stack :gap 0.35
              (label "energy" :width 3 :height 2.5 :font-size 8 :color :dim)
              (matrix
                :key "markov-8x8-energy-matrix"
                :rows 8
                :cols 1
                :width 2
                :height 12
                :min 0
                :max 4
                :value (m8-viz-matrix viz :energy-matrix (m8-zero-column-matrix)))))
          (v-stack :gap 0.35
            (label "transition weights (row -> col)" :width 18 :height 1.3 :font-size 8 :color :dim)
            (matrix
              :key "markov-8x8-weight-matrix"
              :rows 8
              :cols 8
              :width 26
              :height 12
              :min 0
              :max 1
              :value m8-weights
              :on-cell-change (lambda (r c v)
                (do
                  (set! m8-weights (m8-set-cell m8-weights r c v))
                  (graph-edge m8-name :from r :to c :weight v))))))))))

(effect-buffer "*markov-8x8*" (m8-panel SEQ.current-pattern SEQ.graph-visualizations))
(seq-register-step-sequencer-tab script-tab-label script-buffer-name)
