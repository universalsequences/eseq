;; Create an enabled 8-neuron network and route its neurons to visible tracks 1-8.
;; The weight matrix is a simple ring: neuron 0 feeds 1, 1 feeds 2, ... 7 feeds 0.
;; Track 1 / Step 1 resets neural state so this behaves as one seeded phrase per pattern loop.
;;
;; Loading makes sure the network exists through the neural-* natives, which
;; answer at once. The panel reads and edits it through the kinds
;; (kind-bindings spec §14.2q): the `network` named below and its `neuron`s,
;; their playback (triggers, energy, dampening) and the step-editing
;; selection. Every edit is one undo entry, a drag's frames joining one: a
;; `set!` of a field, or for the threshold `set-neural-thresholds!`, which
;; sets every neuron's at once.

(import eseq.kinds :refer (tracks networks set-neural-thresholds!
                           graph-quantize-options graph-max-poly-selection-options))

(def router-name "8x8-track-router2")
(def router-size 8)
(def router-row-height 1.5)
(def router-control-gap 0.4)
(def router-row-label-width 1.2)
(def router-route-width 7.68)
(def router-delay-width 5.04)
(def router-quantize-width 5.76)
(def router-transpose-width 5.04)
(def router-dampening-width 5.04)
(def router-recovery-width 5.04)
(def router-matrix-width 26)
(def router-matrix-height 12)

;; The ring the network starts with: neuron i feeds i + 1, the last feeds 0.
(def router-ring
  (map (lambda (from)
         (map (lambda (to) (if (= to (mod (+ from 1) router-size)) 1 0))
              (range 0 router-size)))
       (range 0 router-size)))

;; ── the network ────────────────────────────────────────────────────────────

;; The network this script made earlier (the natives' description), or nil.
(def router-existing ()
  (first (filter (lambda (n) (= (get n :name) router-name)) (neural-list))))

;; A fresh network: the ring, neuron i on track i, a 4-bar phrase. Returns
;; its id. (Not Off past the project's tracks: at load, before the host's
;; next push, (tracks) can still be empty or another project's.)
(def router-create ()
  (let ((id (get (neural-create :name router-name :neurons router-size :enabled true
                   :weights router-ring)
                 :id)))
    (neural-set id :reset-bars 4 :energy-decay 0.994 :max-poly 2
      :max-poly-selection "deterministic")
    (for-each
      (lambda (i)
        (neural-neuron id i :route i :threshold 1 :delay 1 :quantize false :transpose 0
          :dampening 0 :recovery 0.98))
      (range 0 router-size))
    id))

;; The network, enabled, with its 8 neurons (one of another size is made
;; anew); returns its description.
(def router-ensure ()
  (let ((existing (router-existing)))
    (if (and existing (= (get existing :num-neurons) router-size))
      (let ((id (get existing :id)))
        (unless (get existing :enabled) (neural-enable id true))
        (neural-describe id))
      (do
        (when existing (neural-delete (get existing :id)))
        (neural-describe (router-create))))))

;; The network as the kinds show it: nil until the host publishes it.
(def router-network ()
  (first (filter (lambda (nw) (= nw.name router-name)) (networks))))

;; ── routes: "Track 1" … and Off ───────────────────────────────────────────

(def router-track-label (t) (str "Track " (+ t.index 1)))

(def router-route-options ()
  (append (map router-track-label (tracks)) (list "Off")))

(def router-route-label (nr)
  (let ((t nr.route)) (if t (router-track-label t) "Off")))

;; The track a route label names; nil for Off.
(def router-route-track (label)
  (first (filter (lambda (t) (= (router-track-label t) label)) (tracks))))

;; ── controls ───────────────────────────────────────────────────────────────

(def router-number (value lo hi step decimals width on-change)
  (number-picker
    :value value :min lo :max hi :step step :decimals decimals
    :on-change on-change
    :width width :height 1.2 :font-size 9))

(def router-dropdown (value options width on-change)
  (dropdown
    :value value :options options
    :on-change on-change
    :width width :height 1.2 :font-size 9))

(def router-dim-label (text width)
  (label text :width width :height 1.2 :font-size 9 :color :dim))

;; One threshold for every neuron, shown as the first one's.
(def router-threshold (nw)
  (let ((n0 (first nw.neurons))) (if n0 n0.threshold 0)))

(def router-global-controls (nw)
  (h-stack :gap 0.5 :align :center
    (router-dim-label "bars" 2.8)
    (router-number nw.reset-bars 0.25 64 0.25 2 4.8
      (lambda (v) (set! nw.reset-bars v)))
    (router-dim-label "decay" 3.4)
    (router-number nw.energy-decay 0 1 0.001 3 4.8
      (lambda (v) (set! nw.energy-decay v)))
    (router-dim-label "poly" 2.8)
    (router-number nw.max-poly 1 32 1 0 4.2
      (lambda (v) (set! nw.max-poly v)))
    (router-dim-label "pick" 2.8)
    (router-dropdown nw.max-poly-selection graph-max-poly-selection-options 9.2
      (lambda (v) (set! nw.max-poly-selection v)))
    (router-dim-label "thresh" 4.2)
    (router-number (router-threshold nw) 0 4 0.01 2 4.8
      (lambda (v) (set-neural-thresholds! nw v)))))

(def router-column-label (key text width)
  (label text :key key :width width :height 1.0 :font-size 8 :h-align :center :color :dim))

(def router-control-header ()
  (h-stack :gap router-control-gap :align :center
    (router-column-label "neural-router-column-label-index" "" router-row-label-width)
    (router-column-label "neural-router-column-label-route" "route" router-route-width)
    (router-column-label "neural-router-column-label-delay" "delay" router-delay-width)
    (router-column-label "neural-router-column-label-quantize" "quant" router-quantize-width)
    (router-column-label "neural-router-column-label-transpose" "transp" router-transpose-width)
    (router-column-label "neural-router-column-label-dampening" "damp" router-dampening-width)
    (router-column-label "neural-router-column-label-recovery" "recov" router-recovery-width)))

;; A neuron's row; clicking its number selects it for step editing (the row
;; lights from the bound selection).
(def router-control-row (nr)
  (let ((row-label (str (+ nr.index 1))))
    (box
      :key (str "neural-router-row-" row-label)
      :height router-row-height
      :selected #'nr.selected
      :background-color :transparent
      :selected-background-color :fx-panel-header-selected-bg
      :corner-radius 4
      (h-stack :gap router-control-gap :align :center
        (box
          :key (str "neural-router-row-label-" row-label)
          :width router-row-label-width :height 1.2 :padding 0
          :on-click (lambda (event) (set! nr.selected true))
          (label row-label
            :width router-row-label-width :height 1.2 :font-size 9 :h-align :center
            :color :dim))
        (router-dropdown (router-route-label nr) (router-route-options) router-route-width
          (lambda (label) (set! nr.route (router-route-track label))))
        (router-number nr.delay 0 16 1 0 router-delay-width
          (lambda (v) (set! nr.delay v)))
        (router-dropdown nr.quantize graph-quantize-options router-quantize-width
          (lambda (q) (set! nr.quantize q)))
        (router-number nr.transpose -48 48 1 0 router-transpose-width
          (lambda (v) (set! nr.transpose v)))
        (router-number nr.dampening-amount 0 1 0.01 2 router-dampening-width
          (lambda (v) (set! nr.dampening-amount v)))
        (router-number nr.dampening-recovery 0 1 0.01 2 router-recovery-width
          (lambda (v) (set! nr.dampening-recovery v)))))))

;; ── matrices ───────────────────────────────────────────────────────────────
;; The playback matrices read live fields, each in a subtree of its own, so
;; playback re-runs only them.

;; One value per neuron, as a column: its trigger or its energy.
(def router-column-matrix (key nw width hi read)
  (subtree :key key
    (matrix
      :rows nw.neuron-count :cols 1
      :width width :height router-matrix-height
      :min 0 :max hi
      :value (map (lambda (nr) (list (read nr))) nw.neurons))))

(def router-weights (nw)
  (matrix
    :rows nw.neuron-count :cols nw.neuron-count
    :width router-matrix-width :height router-matrix-height
    :min 0 :max 1
    :value nw.weights
    :on-change (lambda (weights) (set! nw.weights weights))))

;; Each neuron's edges' live dampening, drawn over the weights.
(def router-dampening (nw)
  (subtree :key "neural-router-dampening"
    (matrix
      :rows nw.neuron-count :cols nw.neuron-count
      :width router-matrix-width :height router-matrix-height
      :min 0 :max 1
      :control :grid
      :background :transparent
      :fill :white
      :value (map (lambda (nr) nr.dampening) nw.neurons))))

;; ── panel ──────────────────────────────────────────────────────────────────

;; Clicking the panel's background clears the step-editing selection. Without
;; the network (deleted, or another scene) it offers to create it.
(def router-panel ()
  (let ((nw (router-network)))
    (if nw
      (box :on-click (lambda (event) (neural-clear-selection))
        (v-stack :gap 0.5 :padding 1
          (router-global-controls nw)
          (v-stack :gap 0
            (router-control-header)
            (h-stack :gap 1 :align :start
              (v-stack :gap 0 (each nw.neurons |nr| (router-control-row nr)))
              (router-column-matrix "neural-router-triggers" nw 1 1 (lambda (nr) nr.trigger))
              (router-column-matrix "neural-router-energy" nw 0.8 4 (lambda (nr) nr.energy))
              (router-weights nw)
              (router-dampening nw)))))
      (h-stack :gap 0.5 :align :center :padding 1
        (router-dim-label (str "no " router-name " network in this scene") 23)
        (button "Create" :key "neural-router-create" :width 5.0 :height 1.2 :font-size 9
          :on-click (lambda (event) (router-ensure)))))))

(neural-reset-step :track 0 :step 0 false)

(effect-buffer "*matrix*" (router-panel))

(router-ensure)
