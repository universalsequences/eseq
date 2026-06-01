;; Create an enabled 8-neuron network and route its neurons to visible tracks 1-8.
;; The weight matrix is a simple ring: neuron 0 feeds 1, 1 feeds 2, ... 7 feeds 0.
;; Track 1 / Step 1 resets neural state so this behaves as one seeded phrase per pattern loop.
;; Neural route indices are zero-based internally: route 0 is visible Track 1.

(def neural-8x8-track-router-name "8x8-track-router2")
(def neural-8x8-track-router-row-height 1.5)
(def neural-8x8-track-router-matrix-width 26)
(def neural-8x8-track-router-matrix-height 12)
(defstate neural-8x8-track-router-id 0)
(defstate neural-8x8-track-router-reset-bars 4)
(defstate neural-8x8-track-router-energy-decay 0.994)
(defstate neural-8x8-track-router-max-poly 2)
(defstate neural-8x8-track-router-max-poly-selection "deterministic")
(defstate neural-8x8-track-router-threshold 1)

(def neural-8x8-track-router-max-poly-selection-options
  (list "deterministic" "propagation" "random"))

(def neural-8x8-track-router-route-options
  (list "Track 1" "Track 2" "Track 3" "Track 4" "Track 5" "Track 6" "Track 7" "Track 8" "Off"))

(def neural-8x8-track-router-quantize-options
  (list "off" "1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))

(defstate neural-8x8-track-router-weights
  (list
    (list 0 1 0 0 0 0 0 0)
    (list 0 0 1 0 0 0 0 0)
    (list 0 0 0 1 0 0 0 0)
    (list 0 0 0 0 1 0 0 0)
    (list 0 0 0 0 0 1 0 0)
    (list 0 0 0 0 0 0 1 0)
    (list 0 0 0 0 0 0 0 1)
    (list 1 0 0 0 0 0 0 0)))

(def neural-8x8-track-router-zero-row ()
  (list 0 0 0 0 0 0 0 0))

(def neural-8x8-track-router-zero-column-row ()
  (list 0))

(def neural-8x8-track-router-zero-matrix ()
  (list
    (neural-8x8-track-router-zero-row)
    (neural-8x8-track-router-zero-row)
    (neural-8x8-track-router-zero-row)
    (neural-8x8-track-router-zero-row)
    (neural-8x8-track-router-zero-row)
    (neural-8x8-track-router-zero-row)
    (neural-8x8-track-router-zero-row)
    (neural-8x8-track-router-zero-row)))

(def neural-8x8-track-router-zero-column-matrix ()
  (list
    (neural-8x8-track-router-zero-column-row)
    (neural-8x8-track-router-zero-column-row)
    (neural-8x8-track-router-zero-column-row)
    (neural-8x8-track-router-zero-column-row)
    (neural-8x8-track-router-zero-column-row)
    (neural-8x8-track-router-zero-column-row)
    (neural-8x8-track-router-zero-column-row)
    (neural-8x8-track-router-zero-column-row)))

(def neural-8x8-track-router-visible-row (row)
  (if (> (len row) 7)
    (list (nth row 0) (nth row 1) (nth row 2) (nth row 3) (nth row 4) (nth row 5) (nth row 6) (nth row 7))
    (neural-8x8-track-router-zero-row)))

(def neural-8x8-track-router-visible-matrix (matrix)
  (if (> (len matrix) 7)
    (list
      (neural-8x8-track-router-visible-row (nth matrix 0))
      (neural-8x8-track-router-visible-row (nth matrix 1))
      (neural-8x8-track-router-visible-row (nth matrix 2))
      (neural-8x8-track-router-visible-row (nth matrix 3))
      (neural-8x8-track-router-visible-row (nth matrix 4))
      (neural-8x8-track-router-visible-row (nth matrix 5))
      (neural-8x8-track-router-visible-row (nth matrix 6))
      (neural-8x8-track-router-visible-row (nth matrix 7)))
    (neural-8x8-track-router-zero-matrix)))

(def neural-8x8-track-router-visible-column-row (row)
  (if (> (len row) 0)
    (list (nth row 0))
    (neural-8x8-track-router-zero-column-row)))

(def neural-8x8-track-router-visible-column-matrix (matrix)
  (if (> (len matrix) 7)
    (list
      (neural-8x8-track-router-visible-column-row (nth matrix 0))
      (neural-8x8-track-router-visible-column-row (nth matrix 1))
      (neural-8x8-track-router-visible-column-row (nth matrix 2))
      (neural-8x8-track-router-visible-column-row (nth matrix 3))
      (neural-8x8-track-router-visible-column-row (nth matrix 4))
      (neural-8x8-track-router-visible-column-row (nth matrix 5))
      (neural-8x8-track-router-visible-column-row (nth matrix 6))
      (neural-8x8-track-router-visible-column-row (nth matrix 7)))
    (neural-8x8-track-router-zero-column-matrix)))

(defstate neural-8x8-track-router-route-0 "Track 1")
(defstate neural-8x8-track-router-route-1 "Track 2")
(defstate neural-8x8-track-router-route-2 "Track 3")
(defstate neural-8x8-track-router-route-3 "Track 4")
(defstate neural-8x8-track-router-route-4 "Track 5")
(defstate neural-8x8-track-router-route-5 "Track 6")
(defstate neural-8x8-track-router-route-6 "Track 7")
(defstate neural-8x8-track-router-route-7 "Track 8")

(defstate neural-8x8-track-router-delay-0 1)
(defstate neural-8x8-track-router-delay-1 1)
(defstate neural-8x8-track-router-delay-2 1)
(defstate neural-8x8-track-router-delay-3 1)
(defstate neural-8x8-track-router-delay-4 1)
(defstate neural-8x8-track-router-delay-5 1)
(defstate neural-8x8-track-router-delay-6 1)
(defstate neural-8x8-track-router-delay-7 1)

(defstate neural-8x8-track-router-quantize-0 "off")
(defstate neural-8x8-track-router-quantize-1 "off")
(defstate neural-8x8-track-router-quantize-2 "off")
(defstate neural-8x8-track-router-quantize-3 "off")
(defstate neural-8x8-track-router-quantize-4 "off")
(defstate neural-8x8-track-router-quantize-5 "off")
(defstate neural-8x8-track-router-quantize-6 "off")
(defstate neural-8x8-track-router-quantize-7 "off")

(defstate neural-8x8-track-router-transpose-0 0)
(defstate neural-8x8-track-router-transpose-1 0)
(defstate neural-8x8-track-router-transpose-2 0)
(defstate neural-8x8-track-router-transpose-3 0)
(defstate neural-8x8-track-router-transpose-4 0)
(defstate neural-8x8-track-router-transpose-5 0)
(defstate neural-8x8-track-router-transpose-6 0)
(defstate neural-8x8-track-router-transpose-7 0)

(defstate neural-8x8-track-router-dampening-0 0)
(defstate neural-8x8-track-router-dampening-1 0)
(defstate neural-8x8-track-router-dampening-2 0)
(defstate neural-8x8-track-router-dampening-3 0)
(defstate neural-8x8-track-router-dampening-4 0)
(defstate neural-8x8-track-router-dampening-5 0)
(defstate neural-8x8-track-router-dampening-6 0)
(defstate neural-8x8-track-router-dampening-7 0)

(defstate neural-8x8-track-router-recovery-0 0.98)
(defstate neural-8x8-track-router-recovery-1 0.98)
(defstate neural-8x8-track-router-recovery-2 0.98)
(defstate neural-8x8-track-router-recovery-3 0.98)
(defstate neural-8x8-track-router-recovery-4 0.98)
(defstate neural-8x8-track-router-recovery-5 0.98)
(defstate neural-8x8-track-router-recovery-6 0.98)
(defstate neural-8x8-track-router-recovery-7 0.98)

(def neural-8x8-track-router-route-index (route)
  (if (= route "Off")
    false
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
                  7)))))))))

(def neural-8x8-track-router-route-label (route)
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
                  "Off")))))))))

(def neural-8x8-track-router-quantize-label (quantize)
  (if (not quantize)
    "off"
    (let ((q (str quantize)))
      (if (= q ":1")
        "1"
        (if (= q ":2")
          "2"
          (if (= q ":4")
            "4"
            (if (= q ":8")
              "8"
              (if (= q ":16")
                "16"
                (if (= q ":32")
                  "32"
                  (if (= q ":64")
                    "64"
                    (if (= q ":2T")
                      "2T"
                      (if (= q ":4T")
                        "4T"
                        (if (= q ":8T")
                          "8T"
                          (if (= q ":16T")
                            "16T"
                            (if (= q ":32T")
                              "32T"
                              (if (= q ":64T")
                                "64T"
                                "Prh"))))))))))))))))

(def neural-8x8-track-router-quantize-value (quantize)
  (if (= quantize "off")
    false
    quantize))

(def neural-8x8-track-router-max-poly-selection-label (selection)
  (let ((s (str selection)))
    (if (= s ":random")
      "random"
      (if (= s "random")
        "random"
        (if (= s ":propagation")
          "propagation"
          (if (= s "propagation")
            "propagation"
            "deterministic"))))))

(def neural-8x8-track-router-apply-network (network reset-bars energy-decay max-poly max-poly-selection)
  (neural-set network
    :reset-bars reset-bars
    :energy-decay energy-decay
    :max-poly max-poly
    :max-poly-selection max-poly-selection))

(def neural-8x8-track-router-apply-threshold (network threshold)
  (do
    (neural-neuron network 0 :threshold threshold)
    (neural-neuron network 1 :threshold threshold)
    (neural-neuron network 2 :threshold threshold)
    (neural-neuron network 3 :threshold threshold)
    (neural-neuron network 4 :threshold threshold)
    (neural-neuron network 5 :threshold threshold)
    (neural-neuron network 6 :threshold threshold)
    (neural-neuron network 7 :threshold threshold)))

(def neural-8x8-track-router-apply-neuron (network idx route delay quantize transpose dampening recovery)
  (neural-neuron network idx
    :route (neural-8x8-track-router-route-index route)
    :delay delay
    :quantize (neural-8x8-track-router-quantize-value quantize)
    :transpose transpose
    :dampening dampening
    :recovery recovery))

(def neural-8x8-track-router-existing-network ()
  (let ((matches
          (filter |network|
            (= (get network :name) neural-8x8-track-router-name)
            (neural-list))))
    (if (> (len matches) 0)
      (first matches)
      false)))

(def neural-8x8-track-router-load-network-state (network)
  (let ((neurons (get network :neurons)))
    (do
      (set! neural-8x8-track-router-weights (get network :weights))
      (set! neural-8x8-track-router-reset-bars (get network :reset-bars))
      (set! neural-8x8-track-router-energy-decay (get network :energy-decay))
      (set! neural-8x8-track-router-max-poly (get network :max-poly))
      (set! neural-8x8-track-router-max-poly-selection (neural-8x8-track-router-max-poly-selection-label (get network :max-poly-selection)))
      (set! neural-8x8-track-router-threshold (get (nth neurons 0) :threshold))
      (set! neural-8x8-track-router-route-0 (neural-8x8-track-router-route-label (get (nth neurons 0) :route)))
      (set! neural-8x8-track-router-route-1 (neural-8x8-track-router-route-label (get (nth neurons 1) :route)))
      (set! neural-8x8-track-router-route-2 (neural-8x8-track-router-route-label (get (nth neurons 2) :route)))
      (set! neural-8x8-track-router-route-3 (neural-8x8-track-router-route-label (get (nth neurons 3) :route)))
      (set! neural-8x8-track-router-route-4 (neural-8x8-track-router-route-label (get (nth neurons 4) :route)))
      (set! neural-8x8-track-router-route-5 (neural-8x8-track-router-route-label (get (nth neurons 5) :route)))
      (set! neural-8x8-track-router-route-6 (neural-8x8-track-router-route-label (get (nth neurons 6) :route)))
      (set! neural-8x8-track-router-route-7 (neural-8x8-track-router-route-label (get (nth neurons 7) :route)))
      (set! neural-8x8-track-router-delay-0 (get (nth neurons 0) :delay))
      (set! neural-8x8-track-router-delay-1 (get (nth neurons 1) :delay))
      (set! neural-8x8-track-router-delay-2 (get (nth neurons 2) :delay))
      (set! neural-8x8-track-router-delay-3 (get (nth neurons 3) :delay))
      (set! neural-8x8-track-router-delay-4 (get (nth neurons 4) :delay))
      (set! neural-8x8-track-router-delay-5 (get (nth neurons 5) :delay))
      (set! neural-8x8-track-router-delay-6 (get (nth neurons 6) :delay))
      (set! neural-8x8-track-router-delay-7 (get (nth neurons 7) :delay))
      (set! neural-8x8-track-router-quantize-0 (neural-8x8-track-router-quantize-label (get (nth neurons 0) :quantize)))
      (set! neural-8x8-track-router-quantize-1 (neural-8x8-track-router-quantize-label (get (nth neurons 1) :quantize)))
      (set! neural-8x8-track-router-quantize-2 (neural-8x8-track-router-quantize-label (get (nth neurons 2) :quantize)))
      (set! neural-8x8-track-router-quantize-3 (neural-8x8-track-router-quantize-label (get (nth neurons 3) :quantize)))
      (set! neural-8x8-track-router-quantize-4 (neural-8x8-track-router-quantize-label (get (nth neurons 4) :quantize)))
      (set! neural-8x8-track-router-quantize-5 (neural-8x8-track-router-quantize-label (get (nth neurons 5) :quantize)))
      (set! neural-8x8-track-router-quantize-6 (neural-8x8-track-router-quantize-label (get (nth neurons 6) :quantize)))
      (set! neural-8x8-track-router-quantize-7 (neural-8x8-track-router-quantize-label (get (nth neurons 7) :quantize)))
      (set! neural-8x8-track-router-transpose-0 (get (nth neurons 0) :transpose))
      (set! neural-8x8-track-router-transpose-1 (get (nth neurons 1) :transpose))
      (set! neural-8x8-track-router-transpose-2 (get (nth neurons 2) :transpose))
      (set! neural-8x8-track-router-transpose-3 (get (nth neurons 3) :transpose))
      (set! neural-8x8-track-router-transpose-4 (get (nth neurons 4) :transpose))
      (set! neural-8x8-track-router-transpose-5 (get (nth neurons 5) :transpose))
      (set! neural-8x8-track-router-transpose-6 (get (nth neurons 6) :transpose))
      (set! neural-8x8-track-router-transpose-7 (get (nth neurons 7) :transpose))
      (set! neural-8x8-track-router-dampening-0 (get (nth neurons 0) :dampening))
      (set! neural-8x8-track-router-dampening-1 (get (nth neurons 1) :dampening))
      (set! neural-8x8-track-router-dampening-2 (get (nth neurons 2) :dampening))
      (set! neural-8x8-track-router-dampening-3 (get (nth neurons 3) :dampening))
      (set! neural-8x8-track-router-dampening-4 (get (nth neurons 4) :dampening))
      (set! neural-8x8-track-router-dampening-5 (get (nth neurons 5) :dampening))
      (set! neural-8x8-track-router-dampening-6 (get (nth neurons 6) :dampening))
      (set! neural-8x8-track-router-dampening-7 (get (nth neurons 7) :dampening))
      (set! neural-8x8-track-router-recovery-0 (get (nth neurons 0) :dampening-recovery))
      (set! neural-8x8-track-router-recovery-1 (get (nth neurons 1) :dampening-recovery))
      (set! neural-8x8-track-router-recovery-2 (get (nth neurons 2) :dampening-recovery))
      (set! neural-8x8-track-router-recovery-3 (get (nth neurons 3) :dampening-recovery))
      (set! neural-8x8-track-router-recovery-4 (get (nth neurons 4) :dampening-recovery))
      (set! neural-8x8-track-router-recovery-5 (get (nth neurons 5) :dampening-recovery))
      (set! neural-8x8-track-router-recovery-6 (get (nth neurons 6) :dampening-recovery))
      (set! neural-8x8-track-router-recovery-7 (get (nth neurons 7) :dampening-recovery))
      network)))

(def neural-8x8-track-router-load-default-state ()
  (do
    (set! neural-8x8-track-router-weights
      (list
        (list 0 1 0 0 0 0 0 0)
        (list 0 0 1 0 0 0 0 0)
        (list 0 0 0 1 0 0 0 0)
        (list 0 0 0 0 1 0 0 0)
        (list 0 0 0 0 0 1 0 0)
        (list 0 0 0 0 0 0 1 0)
        (list 0 0 0 0 0 0 0 1)
        (list 1 0 0 0 0 0 0 0)))
    (set! neural-8x8-track-router-reset-bars 4)
    (set! neural-8x8-track-router-energy-decay 0.994)
    (set! neural-8x8-track-router-max-poly 2)
    (set! neural-8x8-track-router-max-poly-selection "deterministic")
    (set! neural-8x8-track-router-threshold 1)
    (set! neural-8x8-track-router-route-0 "Track 1")
    (set! neural-8x8-track-router-route-1 "Track 2")
    (set! neural-8x8-track-router-route-2 "Track 3")
    (set! neural-8x8-track-router-route-3 "Track 4")
    (set! neural-8x8-track-router-route-4 "Track 5")
    (set! neural-8x8-track-router-route-5 "Track 6")
    (set! neural-8x8-track-router-route-6 "Track 7")
    (set! neural-8x8-track-router-route-7 "Track 8")
    (set! neural-8x8-track-router-delay-0 1)
    (set! neural-8x8-track-router-delay-1 1)
    (set! neural-8x8-track-router-delay-2 1)
    (set! neural-8x8-track-router-delay-3 1)
    (set! neural-8x8-track-router-delay-4 1)
    (set! neural-8x8-track-router-delay-5 1)
    (set! neural-8x8-track-router-delay-6 1)
    (set! neural-8x8-track-router-delay-7 1)
    (set! neural-8x8-track-router-quantize-0 "off")
    (set! neural-8x8-track-router-quantize-1 "off")
    (set! neural-8x8-track-router-quantize-2 "off")
    (set! neural-8x8-track-router-quantize-3 "off")
    (set! neural-8x8-track-router-quantize-4 "off")
    (set! neural-8x8-track-router-quantize-5 "off")
    (set! neural-8x8-track-router-quantize-6 "off")
    (set! neural-8x8-track-router-quantize-7 "off")
    (set! neural-8x8-track-router-transpose-0 0)
    (set! neural-8x8-track-router-transpose-1 0)
    (set! neural-8x8-track-router-transpose-2 0)
    (set! neural-8x8-track-router-transpose-3 0)
    (set! neural-8x8-track-router-transpose-4 0)
    (set! neural-8x8-track-router-transpose-5 0)
    (set! neural-8x8-track-router-transpose-6 0)
    (set! neural-8x8-track-router-transpose-7 0)
    (set! neural-8x8-track-router-dampening-0 0)
    (set! neural-8x8-track-router-dampening-1 0)
    (set! neural-8x8-track-router-dampening-2 0)
    (set! neural-8x8-track-router-dampening-3 0)
    (set! neural-8x8-track-router-dampening-4 0)
    (set! neural-8x8-track-router-dampening-5 0)
    (set! neural-8x8-track-router-dampening-6 0)
    (set! neural-8x8-track-router-dampening-7 0)
    (set! neural-8x8-track-router-recovery-0 0.98)
    (set! neural-8x8-track-router-recovery-1 0.98)
    (set! neural-8x8-track-router-recovery-2 0.98)
    (set! neural-8x8-track-router-recovery-3 0.98)
    (set! neural-8x8-track-router-recovery-4 0.98)
    (set! neural-8x8-track-router-recovery-5 0.98)
    (set! neural-8x8-track-router-recovery-6 0.98)
    (set! neural-8x8-track-router-recovery-7 0.98)))

(def neural-8x8-track-router-global-controls ()
  (h-stack :gap 0.5 :align :center
    (label "bars" :width 2.8 :height 1.2 :font-size 9 :color :dim)
    (number-picker
      :value neural-8x8-track-router-reset-bars
      :min 0.25
      :max 64
      :step 0.25
      :decimals 2
      :on-change (lambda (reset-bars)
        (do
          (set! neural-8x8-track-router-reset-bars reset-bars)
          (neural-8x8-track-router-apply-network neural-8x8-track-router-id reset-bars neural-8x8-track-router-energy-decay neural-8x8-track-router-max-poly neural-8x8-track-router-max-poly-selection)))
      :width 4.8
      :height 1.2
      :font-size 9)
    (label "decay" :width 3.4 :height 1.2 :font-size 9 :color :dim)
    (number-picker
      :value neural-8x8-track-router-energy-decay
      :min 0
      :max 1
      :step 0.001
      :decimals 3
      :on-change (lambda (energy-decay)
        (do
          (set! neural-8x8-track-router-energy-decay energy-decay)
          (neural-8x8-track-router-apply-network neural-8x8-track-router-id neural-8x8-track-router-reset-bars energy-decay neural-8x8-track-router-max-poly neural-8x8-track-router-max-poly-selection)))
      :width 4.8
      :height 1.2
      :font-size 9)
    (label "poly" :width 2.8 :height 1.2 :font-size 9 :color :dim)
    (number-picker
      :value neural-8x8-track-router-max-poly
      :min 1
      :max 32
      :step 1
      :decimals 0
      :on-change (lambda (max-poly)
        (do
          (set! neural-8x8-track-router-max-poly max-poly)
          (neural-8x8-track-router-apply-network neural-8x8-track-router-id neural-8x8-track-router-reset-bars neural-8x8-track-router-energy-decay max-poly neural-8x8-track-router-max-poly-selection)))
      :width 4.2
      :height 1.2
      :font-size 9)
    (label "pick" :width 2.8 :height 1.2 :font-size 9 :color :dim)
    (dropdown
      :value neural-8x8-track-router-max-poly-selection
      :options neural-8x8-track-router-max-poly-selection-options
      :on-change (lambda (max-poly-selection)
        (do
          (set! neural-8x8-track-router-max-poly-selection max-poly-selection)
          (neural-8x8-track-router-apply-network neural-8x8-track-router-id neural-8x8-track-router-reset-bars neural-8x8-track-router-energy-decay neural-8x8-track-router-max-poly max-poly-selection)))
      :width 9.2
      :height 1.2
      :font-size 9)
    (label "thresh" :width 4.2 :height 1.2 :font-size 9 :color :dim)
    (number-picker
      :value neural-8x8-track-router-threshold
      :min 0
      :max 4
      :step 0.01
      :decimals 2
      :on-change (lambda (threshold)
        (do
          (set! neural-8x8-track-router-threshold threshold)
          (neural-8x8-track-router-apply-threshold neural-8x8-track-router-id threshold)))
      :width 4.8
      :height 1.2
      :font-size 9)))

(def neural-8x8-track-router-control-row (row-label route delay quantize transpose dampening recovery on-route on-delay on-quantize on-transpose on-dampening on-recovery)
  (box :height neural-8x8-track-router-row-height
    (h-stack :gap 0.4 :align :center
      (label row-label :width 1.2 :height 1.2 :font-size 9 :color :dim)
      (dropdown
        :value route
        :options neural-8x8-track-router-route-options
        :on-change on-route
        :width 6.4
        :height 1.2
        :font-size 9)
      (number-picker
        :value delay
        :min 0
        :max 16
        :step 1
        :decimals 0
        :on-change on-delay
        :width 4.2
        :height 1.2
        :font-size 9)
      (dropdown
        :value quantize
        :options neural-8x8-track-router-quantize-options
        :on-change on-quantize
        :width 4.8
        :height 1.2
        :font-size 9)
      (number-picker
        :value transpose
        :min -48
        :max 48
        :step 1
        :decimals 0
        :on-change on-transpose
        :width 4.2
        :height 1.2
        :font-size 9)
      (number-picker
        :value dampening
        :min 0
        :max 1
        :step 0.01
        :decimals 2
        :on-change on-dampening
        :width 4.2
        :height 1.2
        :font-size 9)
      (number-picker
        :value recovery
        :min 0
        :max 1
        :step 0.01
        :decimals 2
        :on-change on-recovery
        :width 4.2
        :height 1.2
        :font-size 9))))

(def neural-8x8-track-router-apply-neuron-0 ()
  (neural-8x8-track-router-apply-neuron neural-8x8-track-router-id 0 neural-8x8-track-router-route-0 neural-8x8-track-router-delay-0 neural-8x8-track-router-quantize-0 neural-8x8-track-router-transpose-0 neural-8x8-track-router-dampening-0 neural-8x8-track-router-recovery-0))

(def neural-8x8-track-router-apply-neuron-1 ()
  (neural-8x8-track-router-apply-neuron neural-8x8-track-router-id 1 neural-8x8-track-router-route-1 neural-8x8-track-router-delay-1 neural-8x8-track-router-quantize-1 neural-8x8-track-router-transpose-1 neural-8x8-track-router-dampening-1 neural-8x8-track-router-recovery-1))

(def neural-8x8-track-router-apply-neuron-2 ()
  (neural-8x8-track-router-apply-neuron neural-8x8-track-router-id 2 neural-8x8-track-router-route-2 neural-8x8-track-router-delay-2 neural-8x8-track-router-quantize-2 neural-8x8-track-router-transpose-2 neural-8x8-track-router-dampening-2 neural-8x8-track-router-recovery-2))

(def neural-8x8-track-router-apply-neuron-3 ()
  (neural-8x8-track-router-apply-neuron neural-8x8-track-router-id 3 neural-8x8-track-router-route-3 neural-8x8-track-router-delay-3 neural-8x8-track-router-quantize-3 neural-8x8-track-router-transpose-3 neural-8x8-track-router-dampening-3 neural-8x8-track-router-recovery-3))

(def neural-8x8-track-router-apply-neuron-4 ()
  (neural-8x8-track-router-apply-neuron neural-8x8-track-router-id 4 neural-8x8-track-router-route-4 neural-8x8-track-router-delay-4 neural-8x8-track-router-quantize-4 neural-8x8-track-router-transpose-4 neural-8x8-track-router-dampening-4 neural-8x8-track-router-recovery-4))

(def neural-8x8-track-router-apply-neuron-5 ()
  (neural-8x8-track-router-apply-neuron neural-8x8-track-router-id 5 neural-8x8-track-router-route-5 neural-8x8-track-router-delay-5 neural-8x8-track-router-quantize-5 neural-8x8-track-router-transpose-5 neural-8x8-track-router-dampening-5 neural-8x8-track-router-recovery-5))

(def neural-8x8-track-router-apply-neuron-6 ()
  (neural-8x8-track-router-apply-neuron neural-8x8-track-router-id 6 neural-8x8-track-router-route-6 neural-8x8-track-router-delay-6 neural-8x8-track-router-quantize-6 neural-8x8-track-router-transpose-6 neural-8x8-track-router-dampening-6 neural-8x8-track-router-recovery-6))

(def neural-8x8-track-router-apply-neuron-7 ()
  (neural-8x8-track-router-apply-neuron neural-8x8-track-router-id 7 neural-8x8-track-router-route-7 neural-8x8-track-router-delay-7 neural-8x8-track-router-quantize-7 neural-8x8-track-router-transpose-7 neural-8x8-track-router-dampening-7 neural-8x8-track-router-recovery-7))

(def neural-8x8-track-router-create-default-network ()
  (do
    (neural-8x8-track-router-load-default-state)
    (let ((network
            (neural-create
              :name neural-8x8-track-router-name
              :neurons 8
              :enabled true
              :weights neural-8x8-track-router-weights)))
      (let ((id (get network :id)))
        (set! neural-8x8-track-router-id id)
        (neural-8x8-track-router-apply-network id neural-8x8-track-router-reset-bars neural-8x8-track-router-energy-decay neural-8x8-track-router-max-poly neural-8x8-track-router-max-poly-selection)
        (neural-8x8-track-router-apply-threshold id neural-8x8-track-router-threshold)
        (neural-8x8-track-router-apply-neuron-0)
        (neural-8x8-track-router-apply-neuron-1)
        (neural-8x8-track-router-apply-neuron-2)
        (neural-8x8-track-router-apply-neuron-3)
        (neural-8x8-track-router-apply-neuron-4)
        (neural-8x8-track-router-apply-neuron-5)
        (neural-8x8-track-router-apply-neuron-6)
	        (neural-8x8-track-router-apply-neuron-7)
	        (neural-describe id)))))

(neural-reset-step :track 0 :step 0 false)

(def neural-8x8-track-router-panel (reactive-networks)
  (let ((reactive-network-count (len reactive-networks)))
    (let ((existing (neural-8x8-track-router-existing-network)))
      (let ((network
              (if existing
                (if (= (get existing :num-neurons) 8)
                  (do
                    (if (get existing :enabled)
                      existing
                      (neural-enable (get existing :id) true))
                    (neural-8x8-track-router-load-network-state (neural-describe (get existing :id))))
                  (do
                    (neural-delete (get existing :id))
                    (neural-8x8-track-router-create-default-network)))
                (neural-8x8-track-router-create-default-network))))
        (let ((id (get network :id)))
          (set! neural-8x8-track-router-id id)

      (v-stack :gap 0.5 :padding 1
        (neural-8x8-track-router-global-controls)
        (h-stack :gap 1 :align :start
          (v-stack :gap 0
            (neural-8x8-track-router-control-row
              "1" neural-8x8-track-router-route-0 neural-8x8-track-router-delay-0 neural-8x8-track-router-quantize-0 neural-8x8-track-router-transpose-0 neural-8x8-track-router-dampening-0 neural-8x8-track-router-recovery-0
              (lambda (route) (do (set! neural-8x8-track-router-route-0 route) (neural-8x8-track-router-apply-neuron-0)))
              (lambda (delay) (do (set! neural-8x8-track-router-delay-0 delay) (neural-8x8-track-router-apply-neuron-0)))
              (lambda (quantize) (do (set! neural-8x8-track-router-quantize-0 quantize) (neural-8x8-track-router-apply-neuron-0)))
              (lambda (transpose) (do (set! neural-8x8-track-router-transpose-0 transpose) (neural-8x8-track-router-apply-neuron-0)))
              (lambda (dampening) (do (set! neural-8x8-track-router-dampening-0 dampening) (neural-8x8-track-router-apply-neuron-0)))
              (lambda (recovery) (do (set! neural-8x8-track-router-recovery-0 recovery) (neural-8x8-track-router-apply-neuron-0))))
            (neural-8x8-track-router-control-row
              "2" neural-8x8-track-router-route-1 neural-8x8-track-router-delay-1 neural-8x8-track-router-quantize-1 neural-8x8-track-router-transpose-1 neural-8x8-track-router-dampening-1 neural-8x8-track-router-recovery-1
              (lambda (route) (do (set! neural-8x8-track-router-route-1 route) (neural-8x8-track-router-apply-neuron-1)))
              (lambda (delay) (do (set! neural-8x8-track-router-delay-1 delay) (neural-8x8-track-router-apply-neuron-1)))
              (lambda (quantize) (do (set! neural-8x8-track-router-quantize-1 quantize) (neural-8x8-track-router-apply-neuron-1)))
              (lambda (transpose) (do (set! neural-8x8-track-router-transpose-1 transpose) (neural-8x8-track-router-apply-neuron-1)))
              (lambda (dampening) (do (set! neural-8x8-track-router-dampening-1 dampening) (neural-8x8-track-router-apply-neuron-1)))
              (lambda (recovery) (do (set! neural-8x8-track-router-recovery-1 recovery) (neural-8x8-track-router-apply-neuron-1))))
            (neural-8x8-track-router-control-row
              "3" neural-8x8-track-router-route-2 neural-8x8-track-router-delay-2 neural-8x8-track-router-quantize-2 neural-8x8-track-router-transpose-2 neural-8x8-track-router-dampening-2 neural-8x8-track-router-recovery-2
              (lambda (route) (do (set! neural-8x8-track-router-route-2 route) (neural-8x8-track-router-apply-neuron-2)))
              (lambda (delay) (do (set! neural-8x8-track-router-delay-2 delay) (neural-8x8-track-router-apply-neuron-2)))
              (lambda (quantize) (do (set! neural-8x8-track-router-quantize-2 quantize) (neural-8x8-track-router-apply-neuron-2)))
              (lambda (transpose) (do (set! neural-8x8-track-router-transpose-2 transpose) (neural-8x8-track-router-apply-neuron-2)))
              (lambda (dampening) (do (set! neural-8x8-track-router-dampening-2 dampening) (neural-8x8-track-router-apply-neuron-2)))
              (lambda (recovery) (do (set! neural-8x8-track-router-recovery-2 recovery) (neural-8x8-track-router-apply-neuron-2))))
            (neural-8x8-track-router-control-row
              "4" neural-8x8-track-router-route-3 neural-8x8-track-router-delay-3 neural-8x8-track-router-quantize-3 neural-8x8-track-router-transpose-3 neural-8x8-track-router-dampening-3 neural-8x8-track-router-recovery-3
              (lambda (route) (do (set! neural-8x8-track-router-route-3 route) (neural-8x8-track-router-apply-neuron-3)))
              (lambda (delay) (do (set! neural-8x8-track-router-delay-3 delay) (neural-8x8-track-router-apply-neuron-3)))
              (lambda (quantize) (do (set! neural-8x8-track-router-quantize-3 quantize) (neural-8x8-track-router-apply-neuron-3)))
              (lambda (transpose) (do (set! neural-8x8-track-router-transpose-3 transpose) (neural-8x8-track-router-apply-neuron-3)))
              (lambda (dampening) (do (set! neural-8x8-track-router-dampening-3 dampening) (neural-8x8-track-router-apply-neuron-3)))
              (lambda (recovery) (do (set! neural-8x8-track-router-recovery-3 recovery) (neural-8x8-track-router-apply-neuron-3))))
            (neural-8x8-track-router-control-row
              "5" neural-8x8-track-router-route-4 neural-8x8-track-router-delay-4 neural-8x8-track-router-quantize-4 neural-8x8-track-router-transpose-4 neural-8x8-track-router-dampening-4 neural-8x8-track-router-recovery-4
              (lambda (route) (do (set! neural-8x8-track-router-route-4 route) (neural-8x8-track-router-apply-neuron-4)))
              (lambda (delay) (do (set! neural-8x8-track-router-delay-4 delay) (neural-8x8-track-router-apply-neuron-4)))
              (lambda (quantize) (do (set! neural-8x8-track-router-quantize-4 quantize) (neural-8x8-track-router-apply-neuron-4)))
              (lambda (transpose) (do (set! neural-8x8-track-router-transpose-4 transpose) (neural-8x8-track-router-apply-neuron-4)))
              (lambda (dampening) (do (set! neural-8x8-track-router-dampening-4 dampening) (neural-8x8-track-router-apply-neuron-4)))
              (lambda (recovery) (do (set! neural-8x8-track-router-recovery-4 recovery) (neural-8x8-track-router-apply-neuron-4))))
            (neural-8x8-track-router-control-row
              "6" neural-8x8-track-router-route-5 neural-8x8-track-router-delay-5 neural-8x8-track-router-quantize-5 neural-8x8-track-router-transpose-5 neural-8x8-track-router-dampening-5 neural-8x8-track-router-recovery-5
              (lambda (route) (do (set! neural-8x8-track-router-route-5 route) (neural-8x8-track-router-apply-neuron-5)))
              (lambda (delay) (do (set! neural-8x8-track-router-delay-5 delay) (neural-8x8-track-router-apply-neuron-5)))
              (lambda (quantize) (do (set! neural-8x8-track-router-quantize-5 quantize) (neural-8x8-track-router-apply-neuron-5)))
              (lambda (transpose) (do (set! neural-8x8-track-router-transpose-5 transpose) (neural-8x8-track-router-apply-neuron-5)))
              (lambda (dampening) (do (set! neural-8x8-track-router-dampening-5 dampening) (neural-8x8-track-router-apply-neuron-5)))
              (lambda (recovery) (do (set! neural-8x8-track-router-recovery-5 recovery) (neural-8x8-track-router-apply-neuron-5))))
            (neural-8x8-track-router-control-row
              "7" neural-8x8-track-router-route-6 neural-8x8-track-router-delay-6 neural-8x8-track-router-quantize-6 neural-8x8-track-router-transpose-6 neural-8x8-track-router-dampening-6 neural-8x8-track-router-recovery-6
              (lambda (route) (do (set! neural-8x8-track-router-route-6 route) (neural-8x8-track-router-apply-neuron-6)))
              (lambda (delay) (do (set! neural-8x8-track-router-delay-6 delay) (neural-8x8-track-router-apply-neuron-6)))
              (lambda (quantize) (do (set! neural-8x8-track-router-quantize-6 quantize) (neural-8x8-track-router-apply-neuron-6)))
              (lambda (transpose) (do (set! neural-8x8-track-router-transpose-6 transpose) (neural-8x8-track-router-apply-neuron-6)))
              (lambda (dampening) (do (set! neural-8x8-track-router-dampening-6 dampening) (neural-8x8-track-router-apply-neuron-6)))
              (lambda (recovery) (do (set! neural-8x8-track-router-recovery-6 recovery) (neural-8x8-track-router-apply-neuron-6))))
            (neural-8x8-track-router-control-row
              "8" neural-8x8-track-router-route-7 neural-8x8-track-router-delay-7 neural-8x8-track-router-quantize-7 neural-8x8-track-router-transpose-7 neural-8x8-track-router-dampening-7 neural-8x8-track-router-recovery-7
              (lambda (route) (do (set! neural-8x8-track-router-route-7 route) (neural-8x8-track-router-apply-neuron-7)))
              (lambda (delay) (do (set! neural-8x8-track-router-delay-7 delay) (neural-8x8-track-router-apply-neuron-7)))
              (lambda (quantize) (do (set! neural-8x8-track-router-quantize-7 quantize) (neural-8x8-track-router-apply-neuron-7)))
              (lambda (transpose) (do (set! neural-8x8-track-router-transpose-7 transpose) (neural-8x8-track-router-apply-neuron-7)))
              (lambda (dampening) (do (set! neural-8x8-track-router-dampening-7 dampening) (neural-8x8-track-router-apply-neuron-7)))
              (lambda (recovery) (do (set! neural-8x8-track-router-recovery-7 recovery) (neural-8x8-track-router-apply-neuron-7)))))
          (matrix
            :rows 8
            :cols 1
            :width 1
            :height neural-8x8-track-router-matrix-height
            :min 0
            :max 1
            :value (neural-8x8-track-router-visible-column-matrix SEQ.neural-trigger-matrix))
          (matrix
            :rows 8
            :cols 1
            :width 0.8
            :height neural-8x8-track-router-matrix-height
            :min 0
            :max 4
            :value (neural-8x8-track-router-visible-column-matrix SEQ.neural-energy-matrix))
          (matrix
            :rows 8
            :cols 8
            :width neural-8x8-track-router-matrix-width
            :height neural-8x8-track-router-matrix-height
            :min 0
            :max 1
            :value neural-8x8-track-router-weights
            :on-change (lambda (weights)
              (do
                (set! neural-8x8-track-router-weights weights)
                (neural-weights neural-8x8-track-router-id weights))))
          (matrix
            :rows 8
            :cols 8
            :width 8
            :height 4
            :min 0
            :max 1
            :value (neural-8x8-track-router-visible-matrix SEQ.neural-dampening-matrix)))))))))

(effect-buffer "*matrix*" (neural-8x8-track-router-panel SEQ.neural-networks))

(neural-describe neural-8x8-track-router-id)
