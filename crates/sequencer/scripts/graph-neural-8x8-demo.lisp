;; Graph-mode 8x8 neural sequencer — a playground for the lisp node-graph DSL.
;;
;; Eight all-to-all nodes. Seed it by putting a trigger on track 0 (node 0 subscribes
;; to track 0). The :update rule shapes the emitted/propagated event in lisp: note
;; accumulates the per-node transpose around feedback loops, and velocity is scaled by
;; a per-node vel-decay each hop — the velocity analogue of the transpose cascade.
;;
;; All nodes route to track 0 by default so every firing is audible on one instrument;
;; change a node's route in your own copy if you want it to drive other tracks. The
;; control panel exposes per-node delay / transpose / vel-decay / resolution / quantize
;; plus the 8x8 connection-weight matrix.
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
(def g8-quant-options (list "off" "1" "2" "4" "8" "16" "32" "64"))

;; ── apply helpers: push a value into the published graph's override layer ──

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

;; ── UI ──

(def g8-row-height 1.3)

(def g8-num (key value lo hi stp dec on-change)
  (number-picker
    :key key
    :value value :min lo :max hi :step stp :decimals dec
    :width 4.2 :height g8-row-height :font-size 9
    :on-change on-change))

(def g8-pick (key value options on-change)
  (dropdown
    :key key
    :value value :options options
    :width 4.2 :height g8-row-height :font-size 9
    :on-change on-change))

(def g8-row (lbl delay transp vel res quant on-delay on-transp on-vel on-res on-quant)
  (h-stack :gap 0.4 :align :center
    (label lbl :width 2.4 :height g8-row-height :font-size 9 :h-align :center :color :dim)
    (g8-num (str "graph-8x8-delay-" lbl) delay 0 16 1 0 on-delay)
    (g8-num (str "graph-8x8-transpose-" lbl) transp -48 48 1 0 on-transp)
    (g8-num (str "graph-8x8-vel-decay-" lbl) vel 0 2 0.01 2 on-vel)
    (g8-pick (str "graph-8x8-resolution-" lbl) res g8-res-options on-res)
    (g8-pick (str "graph-8x8-quantize-" lbl) quant g8-quant-options on-quant)))

(def g8-header ()
  (h-stack :gap 0.4 :align :center
    (label "node"   :width 2.4 :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "delay"  :width 4.2 :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "transp" :width 4.2 :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "vel x"  :width 4.2 :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "res"    :width 4.2 :height 1.0 :font-size 8 :h-align :center :color :dim)
    (label "quant"  :width 4.2 :height 1.0 :font-size 8 :h-align :center :color :dim)))

(def g8-panel ()
  (box
    :padding 0.85
    :gap 0.6
    :width 27
    :height 24
    (v-stack :gap 0.5
      (h-stack :gap 0.5 :align :center
        (label "8x8 graph" :width 8 :height 1.2 :font-size 11 :color :foreground)
        (label "per-node knobs + weights" :width 14 :height 1.2 :font-size 9 :color :dim))
      (v-stack :gap 0.2
        (g8-header)
        (g8-row "0" g8-delay-0 g8-transp-0 g8-vel-0 g8-res-0 g8-quant-0
          (lambda (v) (do (set! g8-delay-0 v)  (g8-apply-delay 0 v)))
          (lambda (v) (do (set! g8-transp-0 v) (g8-apply-transp 0 v)))
          (lambda (v) (do (set! g8-vel-0 v)    (g8-apply-vel 0 v)))
          (lambda (v) (do (set! g8-res-0 v)    (g8-apply-res 0 v)))
          (lambda (v) (do (set! g8-quant-0 v)  (g8-apply-quant 0 v))))
        (g8-row "1" g8-delay-1 g8-transp-1 g8-vel-1 g8-res-1 g8-quant-1
          (lambda (v) (do (set! g8-delay-1 v)  (g8-apply-delay 1 v)))
          (lambda (v) (do (set! g8-transp-1 v) (g8-apply-transp 1 v)))
          (lambda (v) (do (set! g8-vel-1 v)    (g8-apply-vel 1 v)))
          (lambda (v) (do (set! g8-res-1 v)    (g8-apply-res 1 v)))
          (lambda (v) (do (set! g8-quant-1 v)  (g8-apply-quant 1 v))))
        (g8-row "2" g8-delay-2 g8-transp-2 g8-vel-2 g8-res-2 g8-quant-2
          (lambda (v) (do (set! g8-delay-2 v)  (g8-apply-delay 2 v)))
          (lambda (v) (do (set! g8-transp-2 v) (g8-apply-transp 2 v)))
          (lambda (v) (do (set! g8-vel-2 v)    (g8-apply-vel 2 v)))
          (lambda (v) (do (set! g8-res-2 v)    (g8-apply-res 2 v)))
          (lambda (v) (do (set! g8-quant-2 v)  (g8-apply-quant 2 v))))
        (g8-row "3" g8-delay-3 g8-transp-3 g8-vel-3 g8-res-3 g8-quant-3
          (lambda (v) (do (set! g8-delay-3 v)  (g8-apply-delay 3 v)))
          (lambda (v) (do (set! g8-transp-3 v) (g8-apply-transp 3 v)))
          (lambda (v) (do (set! g8-vel-3 v)    (g8-apply-vel 3 v)))
          (lambda (v) (do (set! g8-res-3 v)    (g8-apply-res 3 v)))
          (lambda (v) (do (set! g8-quant-3 v)  (g8-apply-quant 3 v))))
        (g8-row "4" g8-delay-4 g8-transp-4 g8-vel-4 g8-res-4 g8-quant-4
          (lambda (v) (do (set! g8-delay-4 v)  (g8-apply-delay 4 v)))
          (lambda (v) (do (set! g8-transp-4 v) (g8-apply-transp 4 v)))
          (lambda (v) (do (set! g8-vel-4 v)    (g8-apply-vel 4 v)))
          (lambda (v) (do (set! g8-res-4 v)    (g8-apply-res 4 v)))
          (lambda (v) (do (set! g8-quant-4 v)  (g8-apply-quant 4 v))))
        (g8-row "5" g8-delay-5 g8-transp-5 g8-vel-5 g8-res-5 g8-quant-5
          (lambda (v) (do (set! g8-delay-5 v)  (g8-apply-delay 5 v)))
          (lambda (v) (do (set! g8-transp-5 v) (g8-apply-transp 5 v)))
          (lambda (v) (do (set! g8-vel-5 v)    (g8-apply-vel 5 v)))
          (lambda (v) (do (set! g8-res-5 v)    (g8-apply-res 5 v)))
          (lambda (v) (do (set! g8-quant-5 v)  (g8-apply-quant 5 v))))
        (g8-row "6" g8-delay-6 g8-transp-6 g8-vel-6 g8-res-6 g8-quant-6
          (lambda (v) (do (set! g8-delay-6 v)  (g8-apply-delay 6 v)))
          (lambda (v) (do (set! g8-transp-6 v) (g8-apply-transp 6 v)))
          (lambda (v) (do (set! g8-vel-6 v)    (g8-apply-vel 6 v)))
          (lambda (v) (do (set! g8-res-6 v)    (g8-apply-res 6 v)))
          (lambda (v) (do (set! g8-quant-6 v)  (g8-apply-quant 6 v))))
        (g8-row "7" g8-delay-7 g8-transp-7 g8-vel-7 g8-res-7 g8-quant-7
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
              (g8-apply-weights weights))))))))

(effect-buffer "*8x8*" (g8-panel))
