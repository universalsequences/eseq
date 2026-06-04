;; Graph-mode 8x8 neural sequencer — a playground for the lisp node-graph DSL.
;;
;; Eight all-to-all nodes. Seed it by putting a trigger on track 0 (node 0 subscribes
;; to track 0). The :update rule shapes the emitted/propagated event in lisp: note
;; accumulates the per-node transpose around feedback loops, and velocity is scaled by
;; a per-node vel-decay each hop — the velocity analogue of the transpose cascade.
;;
;; All nodes route to track 0 by default so every firing is audible on one instrument;
;; change a node's route in your own copy if you want it to drive other tracks. The
;; control panel exposes per-node route / delay / transpose / vel-decay / resolution /
;; quantize plus the 8x8 connection-weight matrix.
;;
;; NOTE: each per-node knob is a *scalar* defstate bound directly (`:value g8-...-N`).
;; That direct binding is what makes the widgets live-editable — a single list-valued
;; state read by index does NOT bind per-widget in the reactive renderer.

(def-sequencer "neural-8x8-demo"
  :shape (line 8)
  :energy-decay 0.992
  :reset-every (bars 4)
  :seed-on-reset 0
  :max-poly 4
  :max-poly-selection :propagation

  (def-node nrn
    :resolution :16
    :delay 1
    :quantize :16
    :route 0
    :seed-from ()
    :reduce :sum
    :params ((threshold :float 0 4 :default 0.55)
             (transpose :int -48 48 :default 0)
             (vel-decay :float 0 2 :default 0.9)
             (dampening :float 0 1 :default 0.14)
             (recovery :float 0 1 :default 0.94))
    :state ((energy :leak (per-step :energy-decay)))
    ;; Fire when energy clears threshold. The else-branch returns nil (no fire).
    :update (if (>= (energy) (param :threshold))
              (do
                (dampen-incoming (param :dampening))
                (emit :note (+ (in-note) (param :transpose))
                      :vel  (* (in-vel) (param :vel-decay))))
              (recover-incoming (param :recovery))))

  (edges
    :from nrn
    :to nrn
    :topology (all-to-all)
    :gather (- (edge :weight) (edge :dampening))
    :params ((weight :float -1 1 :default 0.0)
             (dampening :float 0 1 :default 0))))

(def g8-name "neural-8x8-demo")

;; ── per-node scalar state (initialized to the node-prototype defaults) ──

(defstate g8-delay-0 1) (defstate g8-delay-1 1) (defstate g8-delay-2 1) (defstate g8-delay-3 1)
(defstate g8-delay-4 1) (defstate g8-delay-5 1) (defstate g8-delay-6 1) (defstate g8-delay-7 1)

(defstate g8-transp-0 0) (defstate g8-transp-1 0) (defstate g8-transp-2 0) (defstate g8-transp-3 0)
(defstate g8-transp-4 0) (defstate g8-transp-5 0) (defstate g8-transp-6 0) (defstate g8-transp-7 0)

(defstate g8-vel-0 0.9) (defstate g8-vel-1 0.9) (defstate g8-vel-2 0.9) (defstate g8-vel-3 0.9)
(defstate g8-vel-4 0.9) (defstate g8-vel-5 0.9) (defstate g8-vel-6 0.9) (defstate g8-vel-7 0.9)

(defstate g8-route-0 "Track 1") (defstate g8-route-1 "Track 1") (defstate g8-route-2 "Track 1") (defstate g8-route-3 "Track 1")
(defstate g8-route-4 "Track 1") (defstate g8-route-5 "Track 1") (defstate g8-route-6 "Track 1") (defstate g8-route-7 "Track 1")

(defstate g8-res-0 "16") (defstate g8-res-1 "16") (defstate g8-res-2 "16") (defstate g8-res-3 "16")
(defstate g8-res-4 "16") (defstate g8-res-5 "16") (defstate g8-res-6 "16") (defstate g8-res-7 "16")

(defstate g8-quant-0 "16") (defstate g8-quant-1 "16") (defstate g8-quant-2 "16") (defstate g8-quant-3 "16")
(defstate g8-quant-4 "16") (defstate g8-quant-5 "16") (defstate g8-quant-6 "16") (defstate g8-quant-7 "16")

;; Connection weights: a forward ring (i -> i+1) so a seed walks all eight nodes.
(defstate g8-weights
  (list
    (list 0 1 0 0 0 0 0 0)
    (list 0 0 1 0 0 0 0 0)
    (list 0 0 0 1 0 0 0 0)
    (list 0 0 0 0 1 0 0 0)
    (list 0 0 0 0 0 1 0 0)
    (list 0 0 0 0 0 0 1 0)
    (list 0 0 0 0 0 0 0 1)
    (list 1 0 0 0 0 0 0 0)))

(def g8-res-options (list "1" "2" "4" "8" "16" "32" "64"))
(def g8-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))
(def g8-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8"
        "Track 9" "Track 10" "Track 11" "Track 12" "Track 13" "Track 14" "Track 15" "Track 16"
        "Off"))

(def g8-route-index (route)
  (if (= route "Off")
    :off
    (if (= route "Track 1")
      0
      (if (= route "Track 2")
        1
        (if (= route "Track 3")
          2
          (if (= route "Track 4")
            3
            (if (= route "Track 5")
              4
              (if (= route "Track 6")
                5
                (if (= route "Track 7")
                  6
                  (if (= route "Track 8")
                    7
                    (if (= route "Track 9")
                      8
                      (if (= route "Track 10")
                        9
                        (if (= route "Track 11")
                          10
                          (if (= route "Track 12")
                            11
                            (if (= route "Track 13")
                              12
                              (if (= route "Track 14")
                                13
                                (if (= route "Track 15")
                                  14
                                  15)))))))))))))))))

(def g8-route-label (route)
  (if (= route 0)
    "Track 1"
    (if (= route 1)
      "Track 2"
      (if (= route 2)
        "Track 3"
        (if (= route 3)
          "Track 4"
          (if (= route 4)
            "Track 5"
            (if (= route 5)
              "Track 6"
              (if (= route 6)
                "Track 7"
                (if (= route 7)
                  "Track 8"
                  (if (= route 8)
                    "Track 9"
                    (if (= route 9)
                      "Track 10"
                      (if (= route 10)
                        "Track 11"
                        (if (= route 11)
                          "Track 12"
                          (if (= route 12)
                            "Track 13"
                            (if (= route 13)
                              "Track 14"
                              (if (= route 14)
                                "Track 15"
                                (if (= route 15)
                                  "Track 16"
                                  "Off")))))))))))))))))

;; ── apply helpers: push a value into the published graph's override layer ──

(def g8-apply-route  (i v) (graph-node  g8-name i :route (g8-route-index v)))
(def g8-apply-delay  (i v) (graph-node  g8-name i :delay v))
(def g8-apply-transp (i v) (graph-param g8-name i :transpose v))
(def g8-apply-vel    (i v) (graph-param g8-name i :vel-decay v))
(def g8-apply-res    (i v) (graph-node  g8-name i :resolution v))
(def g8-apply-quant  (i v) (graph-node  g8-name i :quantize v))

(def g8-apply-weights (w)
  (for-each
    (lambda (r)
      (for-each
        (lambda (c)
          (graph-edge g8-name :from r :to c :weight (nth (nth w r) c)))
        (range 8)))
    (range 8)))

;; Seed the override layer once at load: ring weights live, node 0 is the seed point.
(g8-apply-weights g8-weights)
(graph-node g8-name 0 :seed-from 0)

;; Pattern switches update the graph override layer outside these scalar defstates.
;; Refresh the UI controls from the resolved current-pattern graph before rendering.
(def g8-sync-node-0 ()
  (do
    (set! g8-route-0 (g8-route-label (graph-node-value g8-name 0 :route)))
    (set! g8-delay-0 (graph-node-value g8-name 0 :delay))
    (set! g8-transp-0 (graph-param-value g8-name 0 :transpose))
    (set! g8-vel-0 (graph-param-value g8-name 0 :vel-decay))
    (set! g8-res-0 (graph-node-value g8-name 0 :resolution))
    (set! g8-quant-0 (graph-node-value g8-name 0 :quantize))))

(def g8-sync-node-1 ()
  (do
    (set! g8-route-1 (g8-route-label (graph-node-value g8-name 1 :route)))
    (set! g8-delay-1 (graph-node-value g8-name 1 :delay))
    (set! g8-transp-1 (graph-param-value g8-name 1 :transpose))
    (set! g8-vel-1 (graph-param-value g8-name 1 :vel-decay))
    (set! g8-res-1 (graph-node-value g8-name 1 :resolution))
    (set! g8-quant-1 (graph-node-value g8-name 1 :quantize))))

(def g8-sync-node-2 ()
  (do
    (set! g8-route-2 (g8-route-label (graph-node-value g8-name 2 :route)))
    (set! g8-delay-2 (graph-node-value g8-name 2 :delay))
    (set! g8-transp-2 (graph-param-value g8-name 2 :transpose))
    (set! g8-vel-2 (graph-param-value g8-name 2 :vel-decay))
    (set! g8-res-2 (graph-node-value g8-name 2 :resolution))
    (set! g8-quant-2 (graph-node-value g8-name 2 :quantize))))

(def g8-sync-node-3 ()
  (do
    (set! g8-route-3 (g8-route-label (graph-node-value g8-name 3 :route)))
    (set! g8-delay-3 (graph-node-value g8-name 3 :delay))
    (set! g8-transp-3 (graph-param-value g8-name 3 :transpose))
    (set! g8-vel-3 (graph-param-value g8-name 3 :vel-decay))
    (set! g8-res-3 (graph-node-value g8-name 3 :resolution))
    (set! g8-quant-3 (graph-node-value g8-name 3 :quantize))))

(def g8-sync-node-4 ()
  (do
    (set! g8-route-4 (g8-route-label (graph-node-value g8-name 4 :route)))
    (set! g8-delay-4 (graph-node-value g8-name 4 :delay))
    (set! g8-transp-4 (graph-param-value g8-name 4 :transpose))
    (set! g8-vel-4 (graph-param-value g8-name 4 :vel-decay))
    (set! g8-res-4 (graph-node-value g8-name 4 :resolution))
    (set! g8-quant-4 (graph-node-value g8-name 4 :quantize))))

(def g8-sync-node-5 ()
  (do
    (set! g8-route-5 (g8-route-label (graph-node-value g8-name 5 :route)))
    (set! g8-delay-5 (graph-node-value g8-name 5 :delay))
    (set! g8-transp-5 (graph-param-value g8-name 5 :transpose))
    (set! g8-vel-5 (graph-param-value g8-name 5 :vel-decay))
    (set! g8-res-5 (graph-node-value g8-name 5 :resolution))
    (set! g8-quant-5 (graph-node-value g8-name 5 :quantize))))

(def g8-sync-node-6 ()
  (do
    (set! g8-route-6 (g8-route-label (graph-node-value g8-name 6 :route)))
    (set! g8-delay-6 (graph-node-value g8-name 6 :delay))
    (set! g8-transp-6 (graph-param-value g8-name 6 :transpose))
    (set! g8-vel-6 (graph-param-value g8-name 6 :vel-decay))
    (set! g8-res-6 (graph-node-value g8-name 6 :resolution))
    (set! g8-quant-6 (graph-node-value g8-name 6 :quantize))))

(def g8-sync-node-7 ()
  (do
    (set! g8-route-7 (g8-route-label (graph-node-value g8-name 7 :route)))
    (set! g8-delay-7 (graph-node-value g8-name 7 :delay))
    (set! g8-transp-7 (graph-param-value g8-name 7 :transpose))
    (set! g8-vel-7 (graph-param-value g8-name 7 :vel-decay))
    (set! g8-res-7 (graph-node-value g8-name 7 :resolution))
    (set! g8-quant-7 (graph-node-value g8-name 7 :quantize))))

(def g8-sync-weights ()
  (set! g8-weights
    (list
      (list (graph-edge-value g8-name 0 0 :weight) (graph-edge-value g8-name 0 1 :weight) (graph-edge-value g8-name 0 2 :weight) (graph-edge-value g8-name 0 3 :weight) (graph-edge-value g8-name 0 4 :weight) (graph-edge-value g8-name 0 5 :weight) (graph-edge-value g8-name 0 6 :weight) (graph-edge-value g8-name 0 7 :weight))
      (list (graph-edge-value g8-name 1 0 :weight) (graph-edge-value g8-name 1 1 :weight) (graph-edge-value g8-name 1 2 :weight) (graph-edge-value g8-name 1 3 :weight) (graph-edge-value g8-name 1 4 :weight) (graph-edge-value g8-name 1 5 :weight) (graph-edge-value g8-name 1 6 :weight) (graph-edge-value g8-name 1 7 :weight))
      (list (graph-edge-value g8-name 2 0 :weight) (graph-edge-value g8-name 2 1 :weight) (graph-edge-value g8-name 2 2 :weight) (graph-edge-value g8-name 2 3 :weight) (graph-edge-value g8-name 2 4 :weight) (graph-edge-value g8-name 2 5 :weight) (graph-edge-value g8-name 2 6 :weight) (graph-edge-value g8-name 2 7 :weight))
      (list (graph-edge-value g8-name 3 0 :weight) (graph-edge-value g8-name 3 1 :weight) (graph-edge-value g8-name 3 2 :weight) (graph-edge-value g8-name 3 3 :weight) (graph-edge-value g8-name 3 4 :weight) (graph-edge-value g8-name 3 5 :weight) (graph-edge-value g8-name 3 6 :weight) (graph-edge-value g8-name 3 7 :weight))
      (list (graph-edge-value g8-name 4 0 :weight) (graph-edge-value g8-name 4 1 :weight) (graph-edge-value g8-name 4 2 :weight) (graph-edge-value g8-name 4 3 :weight) (graph-edge-value g8-name 4 4 :weight) (graph-edge-value g8-name 4 5 :weight) (graph-edge-value g8-name 4 6 :weight) (graph-edge-value g8-name 4 7 :weight))
      (list (graph-edge-value g8-name 5 0 :weight) (graph-edge-value g8-name 5 1 :weight) (graph-edge-value g8-name 5 2 :weight) (graph-edge-value g8-name 5 3 :weight) (graph-edge-value g8-name 5 4 :weight) (graph-edge-value g8-name 5 5 :weight) (graph-edge-value g8-name 5 6 :weight) (graph-edge-value g8-name 5 7 :weight))
      (list (graph-edge-value g8-name 6 0 :weight) (graph-edge-value g8-name 6 1 :weight) (graph-edge-value g8-name 6 2 :weight) (graph-edge-value g8-name 6 3 :weight) (graph-edge-value g8-name 6 4 :weight) (graph-edge-value g8-name 6 5 :weight) (graph-edge-value g8-name 6 6 :weight) (graph-edge-value g8-name 6 7 :weight))
      (list (graph-edge-value g8-name 7 0 :weight) (graph-edge-value g8-name 7 1 :weight) (graph-edge-value g8-name 7 2 :weight) (graph-edge-value g8-name 7 3 :weight) (graph-edge-value g8-name 7 4 :weight) (graph-edge-value g8-name 7 5 :weight) (graph-edge-value g8-name 7 6 :weight) (graph-edge-value g8-name 7 7 :weight)))))

(def g8-sync-pattern-controls (current-pattern)
  (do
    current-pattern
    (g8-sync-node-0)
    (g8-sync-node-1)
    (g8-sync-node-2)
    (g8-sync-node-3)
    (g8-sync-node-4)
    (g8-sync-node-5)
    (g8-sync-node-6)
    (g8-sync-node-7)
    (g8-sync-weights)))

;; ── UI ──

(def g8-row-height 1.3)
(def g8-node-width 2.4)
(def g8-control-width 5.0)

(def g8-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width g8-control-width :height g8-row-height :font-size 9
    :on-change on-change))

(def g8-pick (key value options on-change)
  (dropdown
    :key key
    :value value :options options
    :width g8-control-width :height g8-row-height :font-size 9
    :on-change on-change))

(def g8-row (lbl route delay transp vel res quant on-route on-delay on-transp on-vel on-res on-quant)
  (h-stack :gap 0.4 :align :center
    (label lbl :width g8-node-width :height g8-row-height :font-size 9 :h-align :center :color :dim)
    (g8-pick (str "graph-8x8-route-" lbl) route g8-route-options on-route)
    (g8-num (str "graph-8x8-delay-" lbl) delay 0 16 1 0 on-delay)
    (g8-num (str "graph-8x8-transpose-" lbl) transp -48 48 1 0 on-transp)
    (g8-num (str "graph-8x8-vel-decay-" lbl) vel 0 2 0.01 2 on-vel)
    (g8-pick (str "graph-8x8-resolution-" lbl) res g8-res-options on-res)
    (g8-pick (str "graph-8x8-quantize-" lbl) quant g8-quant-options on-quant)))

(def g8-header ()
  (h-stack :gap 0.4 :align :center
    (label "node"   :width g8-node-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "route"  :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "delay"  :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "transp" :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "vel x"  :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "res"    :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "quant"  :width g8-control-width :height 1.0 :font-size 8 :h-align :center :color :dim)))

(def g8-panel (current-pattern)
  (do
    (g8-sync-pattern-controls current-pattern)
    (box
      :padding 0.85
      :gap 0.6
      :width 37
      :height 24
      (v-stack :gap 0.5
      (h-stack :gap 0.5 :align :center
        (label "8x8 graph" :width 8 :height 1.2 :font-size 11 :color :foreground)
        (label "per-node knobs + weights" :width 14 :height 1.2 :font-size 9 :color :dim))
      (v-stack :gap 0.2
        (g8-header)
        (g8-row "0" g8-route-0 g8-delay-0 g8-transp-0 g8-vel-0 g8-res-0 g8-quant-0
          (lambda (v) (do (set! g8-route-0 v)  (g8-apply-route 0 v)))
          (lambda (v) (do (set! g8-delay-0 v)  (g8-apply-delay 0 v)))
          (lambda (v) (do (set! g8-transp-0 v) (g8-apply-transp 0 v)))
          (lambda (v) (do (set! g8-vel-0 v)    (g8-apply-vel 0 v)))
          (lambda (v) (do (set! g8-res-0 v)    (g8-apply-res 0 v)))
          (lambda (v) (do (set! g8-quant-0 v)  (g8-apply-quant 0 v))))
        (g8-row "1" g8-route-1 g8-delay-1 g8-transp-1 g8-vel-1 g8-res-1 g8-quant-1
          (lambda (v) (do (set! g8-route-1 v)  (g8-apply-route 1 v)))
          (lambda (v) (do (set! g8-delay-1 v)  (g8-apply-delay 1 v)))
          (lambda (v) (do (set! g8-transp-1 v) (g8-apply-transp 1 v)))
          (lambda (v) (do (set! g8-vel-1 v)    (g8-apply-vel 1 v)))
          (lambda (v) (do (set! g8-res-1 v)    (g8-apply-res 1 v)))
          (lambda (v) (do (set! g8-quant-1 v)  (g8-apply-quant 1 v))))
        (g8-row "2" g8-route-2 g8-delay-2 g8-transp-2 g8-vel-2 g8-res-2 g8-quant-2
          (lambda (v) (do (set! g8-route-2 v)  (g8-apply-route 2 v)))
          (lambda (v) (do (set! g8-delay-2 v)  (g8-apply-delay 2 v)))
          (lambda (v) (do (set! g8-transp-2 v) (g8-apply-transp 2 v)))
          (lambda (v) (do (set! g8-vel-2 v)    (g8-apply-vel 2 v)))
          (lambda (v) (do (set! g8-res-2 v)    (g8-apply-res 2 v)))
          (lambda (v) (do (set! g8-quant-2 v)  (g8-apply-quant 2 v))))
        (g8-row "3" g8-route-3 g8-delay-3 g8-transp-3 g8-vel-3 g8-res-3 g8-quant-3
          (lambda (v) (do (set! g8-route-3 v)  (g8-apply-route 3 v)))
          (lambda (v) (do (set! g8-delay-3 v)  (g8-apply-delay 3 v)))
          (lambda (v) (do (set! g8-transp-3 v) (g8-apply-transp 3 v)))
          (lambda (v) (do (set! g8-vel-3 v)    (g8-apply-vel 3 v)))
          (lambda (v) (do (set! g8-res-3 v)    (g8-apply-res 3 v)))
          (lambda (v) (do (set! g8-quant-3 v)  (g8-apply-quant 3 v))))
        (g8-row "4" g8-route-4 g8-delay-4 g8-transp-4 g8-vel-4 g8-res-4 g8-quant-4
          (lambda (v) (do (set! g8-route-4 v)  (g8-apply-route 4 v)))
          (lambda (v) (do (set! g8-delay-4 v)  (g8-apply-delay 4 v)))
          (lambda (v) (do (set! g8-transp-4 v) (g8-apply-transp 4 v)))
          (lambda (v) (do (set! g8-vel-4 v)    (g8-apply-vel 4 v)))
          (lambda (v) (do (set! g8-res-4 v)    (g8-apply-res 4 v)))
          (lambda (v) (do (set! g8-quant-4 v)  (g8-apply-quant 4 v))))
        (g8-row "5" g8-route-5 g8-delay-5 g8-transp-5 g8-vel-5 g8-res-5 g8-quant-5
          (lambda (v) (do (set! g8-route-5 v)  (g8-apply-route 5 v)))
          (lambda (v) (do (set! g8-delay-5 v)  (g8-apply-delay 5 v)))
          (lambda (v) (do (set! g8-transp-5 v) (g8-apply-transp 5 v)))
          (lambda (v) (do (set! g8-vel-5 v)    (g8-apply-vel 5 v)))
          (lambda (v) (do (set! g8-res-5 v)    (g8-apply-res 5 v)))
          (lambda (v) (do (set! g8-quant-5 v)  (g8-apply-quant 5 v))))
        (g8-row "6" g8-route-6 g8-delay-6 g8-transp-6 g8-vel-6 g8-res-6 g8-quant-6
          (lambda (v) (do (set! g8-route-6 v)  (g8-apply-route 6 v)))
          (lambda (v) (do (set! g8-delay-6 v)  (g8-apply-delay 6 v)))
          (lambda (v) (do (set! g8-transp-6 v) (g8-apply-transp 6 v)))
          (lambda (v) (do (set! g8-vel-6 v)    (g8-apply-vel 6 v)))
          (lambda (v) (do (set! g8-res-6 v)    (g8-apply-res 6 v)))
          (lambda (v) (do (set! g8-quant-6 v)  (g8-apply-quant 6 v))))
        (g8-row "7" g8-route-7 g8-delay-7 g8-transp-7 g8-vel-7 g8-res-7 g8-quant-7
          (lambda (v) (do (set! g8-route-7 v)  (g8-apply-route 7 v)))
          (lambda (v) (do (set! g8-delay-7 v)  (g8-apply-delay 7 v)))
          (lambda (v) (do (set! g8-transp-7 v) (g8-apply-transp 7 v)))
          (lambda (v) (do (set! g8-vel-7 v)    (g8-apply-vel 7 v)))
          (lambda (v) (do (set! g8-res-7 v)    (g8-apply-res 7 v)))
          (lambda (v) (do (set! g8-quant-7 v)  (g8-apply-quant 7 v)))))
      (v-stack :gap 0.35
        (label "weights (from row -> to col)" :width 18 :height 1.0 :font-size 8 :color :dim)
        (matrix
          :key "graph-8x8-weight-matrix"
          :rows 8
          :cols 8
          :width 16
          :height 9
          :min 0
          :max 1
          :value g8-weights
          :on-change (lambda (weights)
            (do
              (set! g8-weights weights)
              (g8-apply-weights weights)))))))))

(effect-buffer "*8x8*" (g8-panel SEQ.current-pattern))
