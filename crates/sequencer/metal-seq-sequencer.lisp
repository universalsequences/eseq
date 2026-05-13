;; metal-seq-sequencer.lisp — Project step sequencer view.
;; Renders to *sequencer* buffer. Shows every track's step grid laid out
;; vertically. Loaded by metal-seq-grid.lisp.

(def seqv-track-peak (i)
  (reactive-get "SEQ" (str "track-peak-" i)))

(def seqv-muted? (i)
  (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i)))

(def seqv-track-color (i)
  (if (< i (len SEQ.track-colors))
    (nth SEQ.track-colors i)
    (list 0.34 0.48 0.98)))

(def seqv-track-color-r (i muted)
  (let ((r (nth (seqv-track-color i) 0)))
    (if muted (+ (* r 0.34) (* 0.10 0.66)) r)))

(def seqv-track-color-g (i muted)
  (let ((g (nth (seqv-track-color i) 1)))
    (if muted (+ (* g 0.34) (* 0.10 0.66)) g)))

(def seqv-track-color-b (i muted)
  (let ((b (nth (seqv-track-color i) 2)))
    (if muted (+ (* b 0.34) (* 0.11 0.66)) b)))

(def seqv-row-bg (selected muted)
  (if selected
    :mixer-strip-selected-bg
    (if muted
      :mixer-strip-muted-bg
      :mixer-strip-bg)))

(def seqv-row-border (selected)
  (if selected
    :mixer-strip-selected-border
    :mixer-strip-border))

(defwidget seqv-track-container
  :width 1.5 :height 1.5
  :state ()
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.45)
      (rgba 0 0 0 0))))

(defwidget seqv-rec-arm-dot
  :width 1.5 :height 1.5
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.8)
      (material
        :lighting (lighting :edge-min -0.35 :edge-max 0.5
          :light (vec3 0.0 -1.0 3.5) :shininess 82.0)
        :color
        (* (if (= active 1) 1.0 (+ 0.2 (smoothstep -0.4 0.1 d)))
          (aqua-color
            (rgba
              (if (= active 1) 0.85 0.5)
              (if (= active 1) 0.05 0.5)
              (if (= active 1) 0.05 0.5)
              1.0)
            (rgba 0.99 0.15 0.15 1.0)))))))

(defwidget seqv-playhead-row-bar
  :width 48.8 :height 0.24
  :paint-margin 0.18
  :state (col)
  :bindable (col)
  :shader
  (if (< col 0)
    (rgba 0 0 0 0)
    (let ((step-w (/ 1.0 16.0))
          (center (/ (+ col 0.5) 16.0))
          (trail-start (max 0.0 (- center (* step-w 1.55))))
          (start (- center (* step-w 0.46)))
          (end (+ center (* step-w 0.46)))
          (trail-half-w (* 0.5 aspect (- end trail-start)))
          (__half_w (* 0.5 aspect (- end start)))
          (__half_h 0.12)
          (__radius 0.07))
      (sdf/layer
        (sdf/fill
          (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ trail-start end)))))
                (y (* 0.5 y)))
            (sdf/rounded-rect trail-half-w 0.09 0.06))
          (material
            :color
            (rgba 0.32 0.48 1.0
              (* 0.42
                (smoothstep trail-start center (/ (+ x aspect) (* 2.0 aspect)))
                (smoothstep 0.82 0.0 (abs y))))))
        (sdf/fill
          (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ start end)))))
                (y (* 0.5 y)))
            (sdf/rounded-rect __half_w __half_h __radius))
          (material
            :color
            (mix
              (rgba 0.20 0.42 1.0 0.38)
              (rgba 0.82 0.92 1.0 1.0)
              (smoothstep 0.85 0.0 (abs y)))
            :shadow (shadow
              :color (rgba 0.25 0.45 1.0 0.72)
              :blur 0.12
              :offset (vec2 0 0))))))))

(defwidget seqv-step-shell
  :width 1.5 :height 1.5
  :state (active plocked selected duration track-r track-g track-b)
  :bindable (active plocked selected duration track-r track-g track-b)
  :shader
  (let ((border (if (= selected 1)
          (rgba 1.0 0.86 0.22 1.0)
          (if (= plocked 1)
            (rgba 0.45 0.42 0.62 1.0)
            (rgba 0.31 0.32 0.40 1.0)))))
    (sdf/layer
      (sdf/fill
        (sdf/translate 0 0.0 (sdf/rounded-rect (* 2.0 width) (* 1.00 height) 0.0)
          )
        (material
          :lighting (lighting :edge-min -0.5 :edge-max 0.3
            :light (vec3 0.1 -1.8 2.5) :shininess 92.0)
          :color (if (= duration 1)
            (aqua-color  
              (rgba (* track-r 0.55) (* track-g 0.55) (* track-b 0.55) 0.5)
              (rgba track-r track-g track-b 1))
            (rgba 0 0 0 0))))
      (sdf/fill (sdf/rounded-rect (* 0.65 width) (* 0.65 height) 0.65)
        (material
          :lighting (lighting :edge-min -0.3 :edge-max 1.0
            :light (vec3 0.3 -1.0 1.5) :shininess 92.0)
          :color (aqua-color border (rgba 0.9 0.1 0.5 1.0))))
      (sdf/fill (sdf/rounded-rect (* 0.53 width) (* 0.53 height) 0.52)
        (material
          :color (if (= active 1) (rgba 0.05 0.055 0.075 1.0) (rgba 0.015 0.016 0.025 1.0))))
      (sdf/fill
        (sdf/translate 0 0.70
          (sdf/circle 0.16))
        (material
          :color (if (= plocked 1)
            (rgba 0.82 0.84 0.88 0.95)
            (rgba 0 0 0 0)))))))

(defwidget seqv-step-dot
  :width 1.0 :height 0.5
  :state (active plocked selected track-r track-g track-b)
  :bindable (active plocked selected track-r track-g track-b)
  :shader
  (if (= active 1)
    (sdf/layer
      (sdf/fill (sdf/rounded-rect width height 0.88)
        (material
          :lighting (lighting :edge-min -0.35 :edge-max 0.5
            :light (vec3 0.0 -1.0 2.5) :shininess 32.0)
          :color
          (aqua-color
            (rgba (* track-r 0.72) (* track-g 0.72) (* track-b 0.82) 1.0)
            (rgba track-r track-g track-b 1.0)))))
    (rgba 0 0 0 0)))

(def seqv-mute-bg (active)
  (if active
    (rgba 0.08 0.09 0.10 1.0)
    (rgba 0.115 0.130 0.144 1.0)))

(def seqv-solo-bg (active)
  (if active
    (rgba 0.72 0.10 0.10 1.0)
    (rgba 0.08 0.09 0.10 1.0)))

;; Compact mixer track row — same controls as metal-seq-mixer.lisp but no
;; volume slider/meter or delete button (sequencer view stays focused on steps).
(def seqv-track-header (i)
  (let ((name (nth SEQ.track-names i)))
    (box :background "seqv-track-container"
      :padding 0.4
      (h-stack :gap 0.5 :align :center
        (box :width 2 :height 1.5
          :background "seqv-rec-arm-dot"
          :key (str "seqv-arm-" i)
          :active (if (nth SEQ.record-armed i) 1 0)
          :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-record-arm i)))
        (button (str (+ i 1))
          :key (str "seqv-mute-" i)
          :width 1.55 :height 1.2 :padding 0 :font-size 10
          :background-color (seqv-mute-bg (nth SEQ.track-mutes i))
          :color (if (nth SEQ.track-mutes i) :gray :blue)
          :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-track-mute i)))
        (button "S"
          :key (str "seqv-solo-" i)
          :width 1.55 :height 1.2 :padding 0 :font-size 10
          :background-color (seqv-solo-bg (nth SEQ.track-solos i))
          :color (if (nth SEQ.track-solos i) :white :gray)
          :on-click |x y r| (do (set! selected-bus -1) (seq-toggle-track-solo i)))
        (box :width 8.6 :height 1
          :key (str "seqv-select-" i)
          :bg (if (and (< selected-bus 0) (= SEQ.current-track i)) :blue :dark-gray)
          :on-click |x y r| (do (set! selected-bus -1) (seq-set-track i))
          (label (substring name 0 12) :font-size 11 :width 8.6
            :color (if (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i))
                     :dark-gray
                     (if (and (< selected-bus 0) (= SEQ.current-track i)) :white :dim))
            :bg :transparent))))))

(def sequencer-row-width 16)

(defstate seqv-drag-track nil)
(defstate seqv-duration-drag-source nil)

(def seqv-duration-edge? (evt)
  (let ((sx (get evt :sx)))
    (and (not (= sx nil)) (> sx 0.48))))

(def seqv-set-duration-from-drag (track source step)
  (do
    (seq-set-track track)
    (seq-set-step-param source :duration (max 1 (min 32 (+ (- step source) 1))))))

(def seqv-step-select-drag-start (track step evt)
  (do
    (set! selected-bus -1)
    (seq-set-track track)
    (set! seqv-drag-track track)
    (step-select-drag-start step evt)))

(def seqv-step-select-drag-over (track step evt)
  (if (and (= seqv-drag-track track) (not (= seqv-duration-drag-source nil)))
    (seqv-set-duration-from-drag track seqv-duration-drag-source step)
    (if (= seqv-drag-track track)
      (do
        (seq-set-track track)
        (step-select-drag-over step evt))
      nil)))

(def seqv-step-pointer-down (track step evt)
  (do
    (set! selected-bus -1)
    (seq-set-track track)
    (set! seqv-drag-track track)
    (if (and (seq-track-step-active? track step) (not (selection-click? evt)) (seqv-duration-edge? evt))
      (do
        (set! seqv-duration-drag-source step)
        (set! step-click-pending nil)
        (set! step-drag-anchor nil)
        (set! step-move-last nil)
        (cool-off-follow)
        (set! cursor-step step)
        (seqv-set-duration-from-drag track step step))
      (step-pointer-down step evt))))

(def seqv-step-pointer-up (track step evt)
  (do
    (if (and (= seqv-drag-track track) (= seqv-duration-drag-source nil))
      (do
        (seq-set-track track)
        (step-pointer-up step evt))
      nil)
    (set! seqv-drag-track nil)
    (set! seqv-duration-drag-source nil)))

;; Single tight step button (no slider, no number).
(def seqv-step-cell (track step visible)
  (let ((track-r (seqv-track-color-r track (seqv-muted? track)))
        (track-g (seqv-track-color-g track (seqv-muted? track)))
        (track-b (seqv-track-color-b track (seqv-muted? track))))
  (box :width 3.05 :height 1.45 :align :center :padding 0.03
    :key (str "seqv-step-cell-" track "-" step)
    :on-mouse-down (lambda (evt)
      (if visible
        (seqv-step-pointer-down track step evt)
        nil))
    :on-drag (lambda (evt)
      (if visible
        (seqv-step-select-drag-over track step evt)
        nil))
    :on-mouse-up (lambda (evt)
      (if visible
        (seqv-step-pointer-up track step evt)
        nil))
    (box
      :active (bind-seq (str "seq-track-step-active-" track "-" step))
      :plocked (bind-seq (str "seq-track-step-plocked-" track "-" step))
      :selected (bind-seq (str "seq-track-step-selected-" track "-" step))
      :duration (bind-seq (str "seq-track-step-duration-" track "-" step))
      :track-r track-r :track-g track-g :track-b track-b
      :background "seqv-step-shell"
      :align :center :width 3.08 :height 1.41
      (box :width 1.62 :height 1.38 :align :center
        (seqv-step-dot
          :active (bind-seq (str "seq-track-step-active-" track "-" step))
          :plocked (bind-seq (str "seq-track-step-plocked-" track "-" step))
          :selected (bind-seq (str "seq-track-step-selected-" track "-" step))
          :track-r track-r :track-g track-g :track-b track-b))))))

(def seqv-playhead-row (track track-id row)
  (box
    :key (str "seqv-playhead-row-" track-id "-" row)
    :width 48.8 :height 0.24
    :background "seqv-playhead-row-bar"
    :col (bind-seq (str "track-playhead-row-" track "-" row))))

(def seqv-track-grid (track-idx)
  (let ((num-steps (nth SEQ.track-num-steps track-idx))
        (rows (max 1 (floor (/ (+ num-steps (- sequencer-row-width 1)) sequencer-row-width)))))
    (v-stack :gap -0.04
      (box :width 0.1 :height 0.12 :bg :transparent)
      (each (range 0 rows) |row|
        (v-stack :gap -0.16
          (h-stack :gap 0.0
            (each (range 0 sequencer-row-width) |col|
              (let ((step (+ (* row sequencer-row-width) col)))
                (seqv-step-cell
                  track-idx
                  step
                  (< step num-steps)))))
          (seqv-playhead-row track-idx (nth SEQ.track-ids track-idx) row))))))

(effect-buffer "*sequencer*"
  (v-stack :padding 0.00 :gap 0.0
    (each (range 0 (len SEQ.track-names)) |i|
      (subtree :key (str "sequencer-track-" (nth SEQ.track-ids i))
        (let ((selected (and (< selected-bus 0) (= SEQ.current-track i)))
            (muted (seqv-muted? i)))
          (box :width :fill
            :background-color (seqv-row-bg selected muted)
            :border-width 2
            :corner-radius 10
            :border-color (seqv-row-border selected)
            :padding 0.45
            :on-click |x y r| (do (set! selected-bus -1) (seq-set-track i))
            (h-stack :gap 0.6 :align :start
              (seqv-track-header i)
              (seqv-track-grid i))))))))

(set-buffer-mode-for "*sequencer*" "seq-grid-mode")
