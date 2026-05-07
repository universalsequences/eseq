;; metal-seq-mixer-v2.lisp — Horizontal DAW-style mixer.
;; Renders to *mixer* buffer. Loaded by metal-seq-grid.lisp.

(def track-peak (i)
  (reactive-get "SEQ" (str "track-peak-" i)))

(def bus-peak (i)
  (if (= (nth SEQ.bus-names i) "Mix")
    (max SEQ.master-peak-l SEQ.master-peak-r)
    (reactive-get "SEQ" (str "bus-peak-" i))))

(def mixer-v2-muted? (i)
  (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i)))

(def mixer-v2-strip-bg (selected muted)
  (if selected
    :mixer-strip-selected-bg
    (if muted
      :mixer-strip-muted-bg
      :mixer-strip-bg)))

(def mixer-v2-strip-border (selected)
  (if selected
    :mixer-strip-selected-border
    :mixer-strip-border))

(def mixer-v2-button-bg (active)
  (if active
    (rgba 0.95 0.48 0.18 1.0)
    :mixer-control-bg))

(def mixer-v2-arm-bg (active)
  (if active
    (rgba 0.95 0.20 0.18 1.0)
    :mixer-control-bg))

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
                (p1x -0.82) (p1y -0.15) (p2x -0.82) (p2y 0.15) (p3x 0.70) (p3y 0.0))
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
    :label-color :dim))

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

(def mixer-v2-send-label (name)
  (if (= name "Bus A")
    "A"
    (if (= name "Bus B")
      "B"
      (substring name 0 3))))

(def mixer-v2-send-knob (track send)
  (knob-number :label (mixer-v2-send-label (get send :name))
    :key (str "mixer-v2-track-" track "-send-" (get send :bus-idx))
    :value (get send :amount)
    :min 0 :max 1 :decimals 2
    :font-size 9 :label-font-size 8
    :text-color :dim :label-color :dim
    :width 4.7 :height 2.15 :knob-size 1.34
    :on-change (lambda (v)
      (host-command "set-track-bus-send"
        (dict :track track :bus (get send :bus-idx) :amount v)))))

(def mixer-v2-track-strip (i)
  (let ((selected (and (< selected-bus 0) (= SEQ.current-track i)))
        (muted (mixer-v2-muted? i))
        (sends (nth SEQ.track-bus-sends i)))
    (box :width 10.9 :height 11.0
      :background-color (mixer-v2-strip-bg selected muted)
      :border-width 2
      :border-color (mixer-v2-strip-border selected)
      :padding 0.45
      :on-click |x y r| (do (set! selected-bus -1) (seq-set-track i))
      (v-stack :gap 0.25
        (dropdown :value (nth SEQ.track-outputs i)
          :key (str "mixer-v2-track-output-" i)
          :options SEQ.track-output-options
          :on-change (lambda (v)
            (host-command "set-track-output" (dict :track i :label v)))
          :width 9.8 :height 1.2 :font-size 10)
        (h-stack :gap 0.05
          (each sends |send send-idx|
            (mixer-v2-send-knob i send)))
        (h-stack :gap 0.45 :align :center
          (knob-number :label "pan"
            :key (str "mixer-v2-track-pan-" i)
            :value (nth SEQ.track-pans i)
            :min -1 :max 1 :decimals 2
            :font-size 9 :label-font-size 8
            :text-color :dim :label-color :dim
            :width 3.9 :height 2.35 :knob-size 1.48
            :on-change (lambda (v) (seq-set-track-pan i v)))
          (mixer-v2-track-meter-control i))
        (h-stack :gap 0.35
          (button (str (+ i 1))
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (if muted :mixer-control-bg (rgba 0.95 0.48 0.18 1.0))
            :color (if muted :dim :black)
            :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-track-mute i)))
          (button "S"
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (mixer-v2-button-bg (nth SEQ.track-solos i))
            :color (if (nth SEQ.track-solos i) :black :dim)
            :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-track-solo i)))
          (button "R"
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (mixer-v2-arm-bg (nth SEQ.record-armed i))
            :color (if (nth SEQ.record-armed i) :black :dim)
            :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-record-arm i))))
        (button (substring (nth SEQ.track-names i) 0 10)
          :width 9.8 :height 1.0 :padding 0 :font-size 10
          :background-color (if muted :mixer-label-muted-bg :mixer-label-bg)
          :color (if muted :dark-gray :dim)
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

(def mixer-v2-bus-display-index (bus-i)
  (if (or (not (mixer-v2-has-mix-bus?)) (<= (len SEQ.bus-names) 1))
    bus-i
    (if (= bus-i 0)
      (- (len SEQ.bus-names) 1)
      (- bus-i 1))))

(def mixer-v2-channel-count ()
  (+ SEQ.num-tracks (len SEQ.bus-names)))

(def mixer-v2-current-channel-index ()
  (if (and (>= selected-bus 0) (< selected-bus (len SEQ.bus-names)))
    (+ SEQ.num-tracks (mixer-v2-bus-display-index selected-bus))
    (min (max SEQ.current-track 0) (- (max SEQ.num-tracks 1) 1))))

(def mixer-v2-select-channel-index (idx)
  (let ((clamped (min (max idx 0) (- (max (mixer-v2-channel-count) 1) 1))))
    (if (< clamped SEQ.num-tracks)
      (do
        (set! selected-bus -1)
        (seq-set-track clamped))
      (do
        (seq-clear-selection)
        (set! selected-bus (mixer-v2-display-bus-index (- clamped SEQ.num-tracks)))))))

(def mixer-v2-select-prev-channel ()
  (do
    (mixer-v2-select-channel-index (- (mixer-v2-current-channel-index) 1))
    true))

(def mixer-v2-select-next-channel ()
  (do
    (mixer-v2-select-channel-index (+ (mixer-v2-current-channel-index) 1))
    true))

(def mixer-v2-delete-selected-track ()
  (if (and (< selected-bus 0) (> SEQ.num-tracks 0) (< SEQ.current-track SEQ.num-tracks))
    (do
      (host-command "delete-track" (dict :track SEQ.current-track))
      true)
    false))

(def mixer-v2-handle-key (key text)
  (if (= key "LEFT")
    (mixer-v2-select-prev-channel)
    (if (= key "RIGHT")
      (mixer-v2-select-next-channel)
      (if (or (= key "BS") (= key "Delete"))
        (mixer-v2-delete-selected-track)
        false))))

(def mixer-v2-bus-strip (i)
  (let ((selected (= selected-bus i)))
    (box :width 9.3 :height 11.0
      :background-color (mixer-v2-strip-bg selected (nth SEQ.bus-mutes i))
      :border-width 2
      :border-color (mixer-v2-strip-border selected)
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
            :background-color (if (nth SEQ.bus-mutes i) :mixer-control-bg (rgba 0.95 0.48 0.18 1.0))
            :color (if (nth SEQ.bus-mutes i) :dim :black)
            :on-click |x y r| (seq-toggle-bus-mute i))
          (button "S"
            :width 2.1 :height 1.0 :padding 0 :font-size 10
            :background-color (mixer-v2-button-bg (nth SEQ.bus-solos i))
            :color (if (nth SEQ.bus-solos i) :black :dim)
            :on-click |x y r| (seq-toggle-bus-solo i))
          (box :width 2.1 :height 1.0))
        (button (mixer-v2-bus-label i)
          :width 8.2 :height 1.0 :padding 0 :font-size 10
          :background-color :mixer-label-bg
          :color :white
          :on-click |x y r| (do (seq-clear-selection) (set! selected-bus i)))))))

(effect-buffer "*mixer*"
  (h-stack :padding 0.005 :gap 0.0
    (each (range 0 SEQ.num-tracks) |i|
      (subtree :key (str "mixer-v2-track-" i)
        (mixer-v2-track-strip i)))
    (box :width 1.2 :height 11.0)
    (each (range 0 (len SEQ.bus-names)) |display-i|
      (let ((i (mixer-v2-display-bus-index display-i)))
        (subtree :key (str "mixer-v2-bus-" i)
          (mixer-v2-bus-strip i))))))

(define-mode "seq-mixer-mode" :read-only true :on-key "mixer-v2-handle-key")
(mode-bind-key "seq-mixer-mode" "LEFT" "mixer-v2-select-prev-channel")
(mode-bind-key "seq-mixer-mode" "RIGHT" "mixer-v2-select-next-channel")
(mode-bind-key "seq-mixer-mode" "BS" "mixer-v2-delete-selected-track")
(mode-bind-key "seq-mixer-mode" "Delete" "mixer-v2-delete-selected-track")
(set-buffer-mode-for "*mixer*" "seq-mixer-mode")
