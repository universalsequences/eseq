;; Compact graph-mode neural sequencer for hands-on testing.
;;
;; Seed it from track 0. Node 0 is the only external seed point; the graph then
;; routes accepted firings across the first four tracks through per-pattern
;; graph-node overrides at the bottom of this file. The 4x4 matrix in *4x4*
;; controls the complete graph.

(def-sequencer "neural-4x4-demo"
  :shape (line 4)
  :energy-decay 0.992
  :reset-every (bars 4)
  :seed-on-reset 0
  :max-poly 3
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
             (dampening :float 0 1 :default 0.14)
             (recovery :float 0 1 :default 0.94))
    :state ((energy :leak (per-step :energy-decay)))
    :update (if (>= (node-state self :energy)
                    (node-param self :threshold))
              (do
                (dampen-incoming self (node-param self :dampening))
                true)
              (do
                (recover-incoming self (node-param self :recovery))
                false)))

  (edges
    :from nrn
    :to nrn
    :topology (all-to-all)
    :gather (- (edge :weight) (edge :dampening))
    :params ((weight :float -1 1 :default 0.62)
             (dampening :float 0 1 :default 0))))

;; Demo plocks. These are sparse current-pattern graph overrides, not edits to the
;; published graph definition above.

(graph-node "neural-4x4-demo" 0  :delay 1 :route 0 :seed-from 0)
(graph-node "neural-4x4-demo" 1  :delay 1 :route 1)
(graph-node "neural-4x4-demo" 2  :delay 2 :route 2)
(graph-node "neural-4x4-demo" 3  :delay 1 :route 3)

(def graph-4x4-demo-name "neural-4x4-demo")
(def graph-4x4-demo-row-height 1.35)
(def graph-4x4-demo-delay-width 4.4)
(def graph-4x4-demo-matrix-width 13)
(def graph-4x4-demo-matrix-height 8)

(defstate graph-4x4-demo-delay-0 1)
(defstate graph-4x4-demo-delay-1 1)
(defstate graph-4x4-demo-delay-2 2)
(defstate graph-4x4-demo-delay-3 1)

(defstate graph-4x4-demo-weights
  (list
    (list 0.62 0.62 0.62 0.62)
    (list 0.62 0.62 0.62 0.62)
    (list 0.62 0.62 0.62 0.62)
    (list 0.62 0.62 0.62 0.62)))

(def graph-4x4-demo-apply-delay (idx delay)
  (graph-node graph-4x4-demo-name idx :delay delay))

(def graph-4x4-demo-apply-weights (weights)
  (do
    (graph-edge graph-4x4-demo-name :from 0 :to 0 :weight (nth (nth weights 0) 0))
    (graph-edge graph-4x4-demo-name :from 0 :to 1 :weight (nth (nth weights 0) 1))
    (graph-edge graph-4x4-demo-name :from 0 :to 2 :weight (nth (nth weights 0) 2))
    (graph-edge graph-4x4-demo-name :from 0 :to 3 :weight (nth (nth weights 0) 3))
    (graph-edge graph-4x4-demo-name :from 1 :to 0 :weight (nth (nth weights 1) 0))
    (graph-edge graph-4x4-demo-name :from 1 :to 1 :weight (nth (nth weights 1) 1))
    (graph-edge graph-4x4-demo-name :from 1 :to 2 :weight (nth (nth weights 1) 2))
    (graph-edge graph-4x4-demo-name :from 1 :to 3 :weight (nth (nth weights 1) 3))
    (graph-edge graph-4x4-demo-name :from 2 :to 0 :weight (nth (nth weights 2) 0))
    (graph-edge graph-4x4-demo-name :from 2 :to 1 :weight (nth (nth weights 2) 1))
    (graph-edge graph-4x4-demo-name :from 2 :to 2 :weight (nth (nth weights 2) 2))
    (graph-edge graph-4x4-demo-name :from 2 :to 3 :weight (nth (nth weights 2) 3))
    (graph-edge graph-4x4-demo-name :from 3 :to 0 :weight (nth (nth weights 3) 0))
    (graph-edge graph-4x4-demo-name :from 3 :to 1 :weight (nth (nth weights 3) 1))
    (graph-edge graph-4x4-demo-name :from 3 :to 2 :weight (nth (nth weights 3) 2))
    (graph-edge graph-4x4-demo-name :from 3 :to 3 :weight (nth (nth weights 3) 3))))

(def graph-4x4-demo-delay-control (row-label idx value on-change)
  (h-stack :gap 0.35 :align :center
    (label row-label :width 1.4 :height graph-4x4-demo-row-height :font-size 9 :h-align :center :color :dim)
    (number-picker
      :key (str "graph-4x4-delay-" idx)
      :value value
      :min 0
      :max 16
      :step 1
      :decimals 0
      :on-change on-change
      :width graph-4x4-demo-delay-width
      :height graph-4x4-demo-row-height
      :font-size 9)))

(def graph-4x4-demo-panel ()
  (box
    :padding 0.75
    :gap 0.55
    :width 28
    :height 13.5
    (v-stack :gap 0.55
      (h-stack :gap 0.5 :align :center
        (label "4x4 graph" :width 8 :height 1.2 :font-size 11 :color :foreground)
        (label "delays + weights" :width 12 :height 1.2 :font-size 9 :color :dim))
      (h-stack :gap 1.0 :align :start
        (v-stack :gap 0.35
          (label "delay" :width 6 :height 1.0 :font-size 8 :color :dim)
          (graph-4x4-demo-delay-control "0" 0 graph-4x4-demo-delay-0
            (lambda (delay)
              (do
                (set! graph-4x4-demo-delay-0 delay)
                (graph-4x4-demo-apply-delay 0 delay))))
          (graph-4x4-demo-delay-control "1" 1 graph-4x4-demo-delay-1
            (lambda (delay)
              (do
                (set! graph-4x4-demo-delay-1 delay)
                (graph-4x4-demo-apply-delay 1 delay))))
          (graph-4x4-demo-delay-control "2" 2 graph-4x4-demo-delay-2
            (lambda (delay)
              (do
                (set! graph-4x4-demo-delay-2 delay)
                (graph-4x4-demo-apply-delay 2 delay))))
          (graph-4x4-demo-delay-control "3" 3 graph-4x4-demo-delay-3
            (lambda (delay)
              (do
                (set! graph-4x4-demo-delay-3 delay)
                (graph-4x4-demo-apply-delay 3 delay)))))
        (v-stack :gap 0.35
          (label "weights" :width graph-4x4-demo-matrix-width :height 1.0 :font-size 8 :color :dim)
          (matrix
            :key "graph-4x4-weight-matrix"
            :rows 4
            :cols 4
            :width graph-4x4-demo-matrix-width
            :height graph-4x4-demo-matrix-height
            :min 0
            :max 1
            :value graph-4x4-demo-weights
            :on-change (lambda (weights)
              (do
                (set! graph-4x4-demo-weights weights)
                (graph-4x4-demo-apply-weights weights)))))))))

(effect-buffer "*4x4*" (graph-4x4-demo-panel))
