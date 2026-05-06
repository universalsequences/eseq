;; metal-seq-mixer-v2.lisp — Horizontal DAW-style mixer.
;; Renders to *mixer* buffer. Loaded by metal-seq-grid.lisp.

(def track-peak (i)
  (reactive-get "SEQ" (str "track-peak-" i)))

(def bus-peak (i)
  (if (= i 0)
    (max SEQ.master-peak-l SEQ.master-peak-r)
    0.0))

(def mixer-v2-muted? (i)
  (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i)))

(def mixer-v2-strip-bg (selected muted)
  (if selected
    (rgba 0.18 0.23 0.30 1.0)
    (if muted
      (rgba 0.095 0.095 0.10 1.0)
      (rgba 0.135 0.135 0.14 1.0))))

(def mixer-v2-button-bg (active)
  (if active
    (rgba 0.95 0.48 0.18 1.0)
    (rgba 0.07 0.075 0.08 1.0)))

(def mixer-v2-arm-bg (active)
  (if active
    (rgba 0.95 0.20 0.18 1.0)
    (rgba 0.07 0.075 0.08 1.0)))

(def mixer-v2-pointer-volume (sy)
  (max 0.0 (min 1.0 (* 0.5 (- 1.0 sy)))))

(def mixer-v2-event-volume (event)
  (mixer-v2-pointer-volume (get event :sy)))

(defwidget mixer-v2-volume-triangle
  :width 1.35 :height 4.24
  :paint-margin 0.15
  :state (value)
  :shader
  (let ((marker-y (* (- 1.0 (* 2.0 value)) (* height 1.04))))
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.04)
        (material :color (rgba 0 0 0 0)))
      (sdf/fill
        (sdf/translate 0 marker-y
          (let ((tx x)
                (ty (* y aspect))
                (p1x -0.82) (p1y -0.22) (p2x -0.82) (p2y 0.22) (p3x 0.70) (p3y 0.0))
            (let ((d1 (- (* (- p2x p1x) (- ty p1y)) (* (- p2y p1y) (- tx p1x))))
                  (d2 (- (* (- p3x p2x) (- ty p2y)) (* (- p3y p2y) (- tx p2x))))
                  (d3 (- (* (- p1x p3x) (- ty p3y)) (* (- p1y p3y) (- tx p3x)))))
              (max (max d1 d2) d3))))
        (material :color (rgba 0.78 0.80 0.83 1.0))))))

(def mixer-v2-level-color (level)
  (if (> level 0.88)
    (rgba 0.95 0.18 0.16 1.0)
    (if (> level 0.70)
      (rgba 0.96 0.82 0.18 1.0)
      (rgba 0.10 0.85 0.30 1.0))))

(def mixer-v2-meter (level-l level-r)
  (mixer-meter
    :level-l level-l :level-r level-r
    :width 2.22 :height 4.24
    :font-size 7 :label-height 0.42 :label-top-inset 0.0
    :label-color :gray))

(def mixer-v2-track-meter (i)
  (subtree :key (str "mixer-v2-track-meter-" i)
    (mixer-v2-meter (track-peak i) (track-peak i))))

(def mixer-v2-bus-meter (i)
  (subtree :key (str "mixer-v2-bus-meter-" i)
    (mixer-v2-meter (bus-peak i) (bus-peak i))))

(def mixer-v2-track-meter-control (i)
  (box :width 3.65 :height 4.24
    :on-click (lambda (event) (seq-set-track-volume i (mixer-v2-event-volume event)))
    :on-drag (lambda (event) (seq-set-track-volume i (mixer-v2-event-volume event)))
    (h-stack :gap 0.06 :align :center
      (mixer-v2-volume-triangle
        :value (nth SEQ.track-volumes i)
        :on-click (lambda (sx sy region) (seq-set-track-volume i (mixer-v2-pointer-volume sy)))
        :on-drag (lambda (sx sy region) (seq-set-track-volume i (mixer-v2-pointer-volume sy))))
      (mixer-v2-track-meter i))))

(def mixer-v2-bus-meter-control (i)
  (box :width 3.65 :height 4.24
    :on-click (lambda (event) (seq-set-bus-volume i (mixer-v2-event-volume event)))
    :on-drag (lambda (event) (seq-set-bus-volume i (mixer-v2-event-volume event)))
    (h-stack :gap 0.06 :align :center
      (mixer-v2-volume-triangle
        :value (nth SEQ.bus-volumes i)
        :on-click (lambda (sx sy region) (seq-set-bus-volume i (mixer-v2-pointer-volume sy)))
        :on-drag (lambda (sx sy region) (seq-set-bus-volume i (mixer-v2-pointer-volume sy))))
      (mixer-v2-bus-meter i))))

(def mixer-v2-send-knob (track send)
  (knob-number :label (substring (get send :name) 0 7)
    :key (str "mixer-v2-track-" track "-send-" (get send :bus-idx))
    :value (get send :amount)
    :min 0 :max 1 :decimals 2
    :font-size 9 :label-font-size 8
    :text-color :gray :label-color :gray
    :width 4.7 :height 1.8
    :on-change (lambda (v)
      (host-command "set-track-bus-send"
        (dict :track track :bus (get send :bus-idx) :amount v)))))

(def mixer-v2-track-strip (i)
  (let ((selected (and (< selected-bus 0) (= SEQ.current-track i)))
        (muted (mixer-v2-muted? i))
        (sends (nth SEQ.track-bus-sends i)))
    (box :width 9.6 :height 12.4
      :background-color (mixer-v2-strip-bg selected muted)
      :border-width 1
      :border-color (if selected (rgba 0.26 0.48 0.86 1.0) (rgba 0.22 0.22 0.23 1.0))
      :padding 0.45
      :on-click |x y r| (do (set! selected-bus -1) (seq-set-track i))
      (v-stack :gap 0.25
        (dropdown :value (nth SEQ.track-outputs i)
          :key (str "mixer-v2-track-output-" i)
          :options SEQ.track-output-options
          :on-change (lambda (v)
            (host-command "set-track-output" (dict :track i :label v)))
          :width 8.5 :height 1.2 :font-size 10)
        (h-stack :gap 0.25
          (each sends |send send-idx|
            (mixer-v2-send-knob i send)))
        (h-stack :gap 0.45 :align :center
          (knob-number :label "pan"
            :key (str "mixer-v2-track-pan-" i)
            :value (nth SEQ.track-pans i)
            :min -1 :max 1 :decimals 2
            :font-size 9 :label-font-size 8
            :text-color :gray :label-color :gray
            :width 3.0 :height 2.0
            :on-change (lambda (v) (seq-set-track-pan i v)))
          (mixer-v2-track-meter-control i))
        (h-stack :gap 0.35
          (button (str (+ i 1))
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (if muted (rgba 0.10 0.10 0.11 1.0) (rgba 0.95 0.48 0.18 1.0))
            :color (if muted :gray :black)
            :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-track-mute i)))
          (button "S"
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (mixer-v2-button-bg (nth SEQ.track-solos i))
            :color (if (nth SEQ.track-solos i) :black :gray)
            :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-track-solo i)))
          (button "R"
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (mixer-v2-arm-bg (nth SEQ.record-armed i))
            :color (if (nth SEQ.record-armed i) :black :gray)
            :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-record-arm i))))
        (button (substring (nth SEQ.track-names i) 0 10)
          :width 8.5 :height 1.0 :padding 0 :font-size 10
          :background-color (if muted (rgba 0.09 0.09 0.09 1.0) (rgba 0.13 0.13 0.14 1.0))
          :color (if muted :dark-gray :gray)
          :on-click |x y r| (do (set! selected-bus -1) (seq-set-track i)))))))

(def mixer-v2-bus-label (i)
  (if (= i 0) "Main" (nth SEQ.bus-names i)))

(def mixer-v2-bus-mute-label (i)
  (if (= i 0) "M" (if (= i 1) "A" (if (= i 2) "B" (str i)))))

(def mixer-v2-has-mix-bus? ()
  (and (> (len SEQ.bus-names) 0) (= (nth SEQ.bus-names 0) "Mix")))

(def mixer-v2-display-bus-index (display-i)
  (if (or (not (mixer-v2-has-mix-bus?)) (<= (len SEQ.bus-names) 1))
    display-i
    (if (= display-i (- (len SEQ.bus-names) 1))
      0
      (+ display-i 1))))

(def mixer-v2-bus-strip (i)
  (let ((selected (= selected-bus i)))
    (box :width 8.7 :height 12.4
      :background-color (mixer-v2-strip-bg selected (nth SEQ.bus-mutes i))
      :border-width 1
      :border-color (if selected (rgba 0.26 0.48 0.86 1.0) (rgba 0.22 0.22 0.23 1.0))
      :padding 0.45
      :on-click |x y r| (do (seq-clear-selection) (set! selected-bus i))
      (v-stack :gap 0.4
        (box :height 3.0)
        (h-stack :gap 0.45 :align :center
          (box :width 3.0 :height 3.6)
          (mixer-v2-bus-meter-control i))
        (h-stack :gap 0.35
          (button (mixer-v2-bus-mute-label i)
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (if (nth SEQ.bus-mutes i) (rgba 0.10 0.10 0.11 1.0) (rgba 0.95 0.48 0.18 1.0))
            :color (if (nth SEQ.bus-mutes i) :gray :black)
            :on-click |x y r| (seq-toggle-bus-mute i))
          (button "S"
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (mixer-v2-button-bg (nth SEQ.bus-solos i))
            :color (if (nth SEQ.bus-solos i) :black :gray)
            :on-click |x y r| (seq-toggle-bus-solo i))
          (box :width 2.1 :height 1.0))
        (button (mixer-v2-bus-label i)
          :width 7.6 :height 1.0 :padding 0 :font-size 10
          :background-color (rgba 0.13 0.13 0.14 1.0)
          :color :white
          :on-click |x y r| (do (seq-clear-selection) (set! selected-bus i)))))))

(effect-buffer "*mixer*"
  (h-stack :padding 0.35 :gap 0.15
    (each (range 0 SEQ.num-tracks) |i|
      (subtree :key (str "mixer-v2-track-" i)
        (mixer-v2-track-strip i)))
    (box :width 1.2 :height 12.4)
    (each (range 0 (len SEQ.bus-names)) |display-i|
      (let ((i (mixer-v2-display-bus-index display-i)))
        (subtree :key (str "mixer-v2-bus-" i)
          (mixer-v2-bus-strip i))))))
