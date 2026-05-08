;; metal-seq-sequencer.lisp — Project step sequencer view.
;; Renders to *sequencer* buffer. Shows every track's step grid laid out
;; vertically. Loaded by metal-seq-grid.lisp.

(def seqv-track-peak (i)
  (reactive-get "SEQ" (str "track-peak-" i)))

(def seqv-muted? (i)
  (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i)))

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
          :light (vec3 0.0 -1.0 1.5) :shininess 82.0)
        :color
        (* (if (= active 1) 1.0 (+ 0.2 (smoothstep -0.4 0.1 d)))
          (aqua-color
            (rgba
              (if (= active 1) 0.85 0.5)
              (if (= active 1) 0.05 0.5)
              (if (= active 1) 0.05 0.5)
              1.0)
            (rgba 0.99 0.15 0.15 1.0)))))))

(defwidget seqv-playhead-lane
  :width 1.5 :height 1.5
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.32)
      (material
        :color (if (= active 1)
                 (rgba 0.55 0.65 1.0 0.58)
                 (rgba 0 0 0 0))
        :shadow (shadow
          :color (if (= active 1) (rgba 0.50 0.58 1.0 0.7) (rgba 0 0 0 0))
          :blur 0.22
          :offset (vec2 0 0))))))

(defwidget seqv-step-shell
  :width 1.5 :height 1.5
  :state (active plocked selected)
  :shader
  (let ((border (if (= selected 1)
                  (rgba 1.0 0.86 0.22 1.0)
                  (if (= plocked 1)
                    (rgba 0.95 0.22 0.72 1.0)
                    (rgba 0.31 0.32 0.40 1.0)))))
    (sdf/layer
      (sdf/fill (sdf/circle 1.0)
        (material
          :color border
          :shadow (shadow
            :color (if (= selected 1)
                     (rgba 1.0 0.78 0.16 0.48)
                     (if (= plocked 1) (rgba 0.85 0.14 0.60 0.42) (rgba 0 0 0 0.35)))
            :blur (if (= selected 1) 0.20 (if (= plocked 1) 0.15 0.09))
            :offset (vec2 0 0.02))))
      (sdf/fill (sdf/circle 0.72)
        (material
          :color (if (= active 1) (rgba 0.05 0.055 0.075 1.0) (rgba 0.015 0.016 0.025 1.0)))))))

(defwidget seqv-step-dot
  :width 1.5 :height 1.5
  :state (active plocked selected)
  :shader
  (if (= active 1)
    (sdf/layer
      (sdf/fill (sdf/circle 0.48)
        (material
          :lighting (lighting :edge-min -0.35 :edge-max 0.5
            :light (vec3 0.0 -1.0 1.5) :shininess 32.0)
          :color
          (aqua-color
            (if (= plocked 1) (rgba 0.75 0.15 0.5 1.0) (rgba 0.3 0.3 0.85 1.0))
            (if (= plocked 1) (rgba 0.4 0.135 0.95 1.0) (rgba 0.90 0.50 0.82 1.0))))))
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

(def seqv-step-select-drag-start (track step evt)
  (do
    (set! selected-bus -1)
    (seq-set-track track)
    (set! seqv-drag-track track)
    (step-select-drag-start step evt)))

(def seqv-step-select-drag-over (track step evt)
  (if (= seqv-drag-track track)
    (do
      (seq-set-track track)
      (step-select-drag-over step evt))
    nil))

(def seqv-step-pointer-down (track step evt)
  (do
    (set! selected-bus -1)
    (seq-set-track track)
    (set! seqv-drag-track track)
    (step-pointer-down step evt)))

(def seqv-step-pointer-up (track step evt)
  (do
    (if (= seqv-drag-track track)
      (do
        (seq-set-track track)
        (step-pointer-up step evt))
      nil)
    (set! seqv-drag-track nil)))

;; Single tight step button (no slider, no number).
(def seqv-step-cell (track step active visible playhead? selected? plocked?)
  (box :width 2.55 :height 1.45 :align :center :padding 0.03
    :background "seqv-playhead-lane"
    :active (if playhead? 1 0)
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
      :active (if visible (if active 1 0) 0)
      :plocked (if visible (if plocked? 1 0) 0)
      :selected (if visible (if selected? 1 0) 0)
      :background "seqv-step-shell"
      :align :center :width 2.08 :height 1.31
      (box :width 1.62 :height 0.98 :align :center
        (seqv-step-dot :active (if visible (if active 1 0) 0)
          :plocked (if visible (if plocked? 1 0) 0)
          :selected (if visible (if selected? 1 0) 0))))))

(def seqv-track-grid (track-idx)
  (let ((steps (nth SEQ.track-steps track-idx))
        (plocks (nth SEQ.track-step-has-plocks track-idx))
        (num-steps (nth SEQ.track-num-steps track-idx))
        (playhead (nth SEQ.track-playheads track-idx))
        (rows (max 1 (floor (/ (+ num-steps (- sequencer-row-width 1)) sequencer-row-width)))))
    (v-stack :gap 0.1
      (box :width 0.1 :height 0.12 :bg :transparent)
      (each (range 0 rows) |row|
        (h-stack :gap 0.1
          (each (range 0 sequencer-row-width) |col|
            (let ((step (+ (* row sequencer-row-width) col)))
              (seqv-step-cell
                track-idx
                step
                (if (< step (len steps)) (nth steps step) 0)
                (< step num-steps)
                (and SEQ.playing (= step playhead))
                (and (= track-idx SEQ.current-track)
                  (< step (len SEQ.selected-steps))
                  (nth SEQ.selected-steps step))
                (and (< step (len plocks)) (nth plocks step))))))))))

(effect-buffer "*sequencer*"
  (v-stack :padding 0.3 :gap 0.0
    (each (range 0 (len SEQ.track-names)) |i|
      (subtree :key (str "sequencer-track-" i)
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
