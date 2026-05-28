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
      :buffer-bg)))

(def seqv-row-border (selected)
  (if selected
    :mixer-strip-selected-border
    :mixer-strip-border))

(def seqv-timebase-options
  '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))

(def seqv-track-timebase (i)
  (if (< i (len SEQ.track-timebases))
    (nth SEQ.track-timebases i)
    "16"))

(def seqv-set-row-timebase (track label)
  (let ((plock-selected
      (and (< selected-bus 0) (= SEQ.current-track track) (seq-has-selection?))))
    (do
      (seqv-activate-track-for-edit track)
      (cool-off-follow)
      (if plock-selected
        (seq-plock-timebase label)
        (seq-set-timebase label)))))

(defstate seqv-expanded-track-ids '())

(defstate seqv-track-editor-state '())

(def seqv-list-contains? (xs item)
  (> (len (filter (lambda (x) (= x item)) xs)) 0))

(def seqv-list-remove (xs item)
  (filter (lambda (x) (not (= x item))) xs))

(def seqv-expanded-track-field (track-id)
  (str "track-expanded-" track-id))

(def seqv-track-id (track)
  (if (< track (len SEQ.track-ids))
    (nth SEQ.track-ids track)
    track))

(def seqv-activate-track-for-edit (track)
  (do
    (set! selected-bus -1)
    (if (= SEQ.current-track track)
      nil
      (seq-clear-selection))
    (seq-set-track track)))

(def seqv-track-expanded? (track-id)
  (reactive-get "SEQV" (seqv-expanded-track-field track-id)))

(def seqv-set-track-expanded (track-id expanded)
  (do
    (reactive-set "SEQV" (seqv-expanded-track-field track-id) expanded)
    (set! seqv-expanded-track-ids
      (if expanded
        (if (seqv-list-contains? seqv-expanded-track-ids track-id)
          seqv-expanded-track-ids
          (append seqv-expanded-track-ids (list track-id)))
        (seqv-list-remove seqv-expanded-track-ids track-id)))))

(def seqv-editor-state-for (track-id)
  (let ((matches (filter
      (lambda (state) (= (get state :id) track-id))
      seqv-track-editor-state)))
    (if (> (len matches) 0)
      (nth matches 0)
      (dict :id track-id :param-mode 0 :cursor-step 0))))

(def seqv-upsert-editor-state (track-id next-state)
  (if (seqv-list-contains? (map (lambda (state) (get state :id)) seqv-track-editor-state) track-id)
    (set! seqv-track-editor-state
      (map
        (lambda (state)
          (if (= (get state :id) track-id) next-state state))
        seqv-track-editor-state))
    (set! seqv-track-editor-state (append seqv-track-editor-state (list next-state)))))

(def seqv-param-mode (track-id)
  (get (seqv-editor-state-for track-id) :param-mode))

(def seqv-set-param-mode (track-id mode)
  (seqv-upsert-editor-state track-id
    (merge (seqv-editor-state-for track-id) :param-mode mode)))

(def seqv-cursor-step (track-id)
  (get (seqv-editor-state-for track-id) :cursor-step))

(def seqv-set-cursor-step (track-id step)
  (seqv-upsert-editor-state track-id
    (merge (seqv-editor-state-for track-id) :cursor-step step)))

(def seqv-current-track-id ()
  (seqv-track-id SEQ.current-track))

(def seqv-current-track-expanded? ()
  (seqv-track-expanded? (seqv-current-track-id)))

(def seqv-current-selected-step ()
  (min cursor-step (- (max 1 (seqv-track-num-steps SEQ.current-track)) 1)))

(def seqv-current-param-mode ()
  (seqv-param-mode (seqv-current-track-id)))

(def seqv-current-number-picker-key ()
  (str "seqv-expanded-param-number-picker-" (seqv-current-track-id)))

(def seqv-select-current-param-mode (mode)
  (seqv-set-param-mode (seqv-current-track-id) mode))

(def seqv-param-mode-for-key (key)
  (if (not (= (len key) 1))
    -1
    (if (or (= key "v") (= key "V"))
      0
      (if (or (= key "d") (= key "D"))
        1
        (if (or (= key "a") (= key "A"))
          2
          (if (or (= key "t") (= key "T"))
            3
            (if (or (= key "p") (= key "P"))
              4
              (if (or (= key "s") (= key "S"))
                5
                -1))))))))

(def seqv-select-all-current-track-steps ()
  (do
    (set! selected-bus -1)
    (select-all-steps)))

(def seqv-collapse-all-tracks ()
  (do
    (for-each
      (lambda (track-id) (seqv-set-track-expanded track-id false))
      seqv-expanded-track-ids)
    (set! seqv-expanded-track-ids '())))

(def seqv-toggle-current-track-expanded ()
  (let ((track-id (seqv-current-track-id)))
    (do
      (set! selected-bus -1)
      (seqv-set-track-expanded track-id (not (seqv-track-expanded? track-id))))))

(def seqv-handle-key (key text)
  (let ((mode (seqv-param-mode-for-key key)))
    (if (>= mode 0)
      (do (seqv-select-current-param-mode mode) true)
      (if (= key "LEFT")
        (do (cursor-left) true)
        (if (= key "RIGHT")
          (do (cursor-right) true)
          (if (= key "C-a")
            (do (seqv-select-all-current-track-steps) true)
            (if (or (= key "C-h") (= key "C-H"))
              (do (seqv-collapse-all-tracks) true)
              (if (or (= key "BS") (= key "Delete"))
                (do (delete-selected-steps) true)
                (if (= key "RET")
                  (do (cursor-toggle) true)
                  false)))))))))

(def seqv-track-menu-click (track)
  (let ((track-id (seqv-track-id track)))
    (do
      (seqv-activate-track-for-edit track)
      (seqv-set-track-expanded track-id (not (seqv-track-expanded? track-id))))))

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

(defwidget seqv-ellipsis-button
  :width 2.2 :height 1.2
  :state (expanded)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.98)
      (material
        :lighting (lighting :edge-min -0.45 :edge-max 0.4
          :light (vec3 0.1 -1.2 2.4) :shininess 24.0)
        :color (if expanded (rgba 0.18 0.18 0.20 1.0) :mixer-control-bg)))
    (sdf/fill
      (sdf/translate -0.48 0
        (sdf/circle 0.12))
      (material :color (rgba 0.60 0.62 0.68 1.0)))
    (sdf/fill (sdf/circle 0.12)
      (material :color (rgba 0.60 0.62 0.68 1.0)))
    (sdf/fill
      (sdf/translate 0.48 0
        (sdf/circle 0.12))
      (material :color (rgba 0.60 0.62 0.68 1.0)))))

(defmacro seqv-aqua-slider-track-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.8413
       :light (vec3 -0.1 -0.61 3.5) :shininess 81.0)
     :color
       (aqua-color
         (rgba (* track-r 0.55) (* track-g 0.55) (* track-b 0.55) 1.0)
         (rgba track-r track-g track-b 1.0))))

(defmacro seqv-aqua-slider-track-muted-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.8413
       :light (vec3 -0.1 -0.61 2.4) :shininess 38.0)
     :color
       (* 0.42
          (aqua-color
            (rgba
              (+ (* track-r 0.36) 0.06)
              (+ (* track-g 0.36) 0.06)
              (+ (* track-b 0.36) 0.08)
              0.85)
            (rgba
              (+ (* track-r 0.30) 0.04)
              (+ (* track-g 0.30) 0.04)
              (+ (* track-b 0.30) 0.08)
              0.85)))))

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
  :state (active odd plocked selected duration track-r track-g track-b)
  :bindable (active plocked selected duration track-r track-g track-b)
  :shader
  (let ((border (if (= selected 1)
          (rgba 1.0 0.86 0.22 1.0)
          (if (= plocked 1)
            (rgba 0.25 0.22 0.22 1.0)
            (if (= odd 1) (rgba 0.28 0.28 0.28 1.0) (rgba 0.18 0.18 0.18 1.0))))))
    (sdf/layer
      (sdf/fill
        (sdf/translate 0 0.0
          (sdf/rounded-rect (* 2.0 width) (* 1.00 height) 0.0))
        (material
          :lighting (lighting :edge-min -0.5 :edge-max 0.3
            :light (vec3 0.1 -1.8 2.5) :shininess 92.0)
          :color (if (= duration 1)
            (aqua-color
              (rgba (* track-r 0.55) (* track-g 0.55) (* track-b 0.55) 0.5)
              (rgba track-r track-g track-b 1))
            (rgba 0 0 0 0))))
      (sdf/fill (sdf/circle 0.65)
        (material
          :lighting (lighting :edge-min -0.3 :edge-max 1.0
            :light (vec3 0.3 -1.0 1.5) :shininess 92.0)
          :color (aqua-color border (rgba 0.9 0.1 0.5 1.0))))
      (sdf/fill (sdf/circle 0.53)
        (material
          :color (if (= odd 1) 
            (rgba 0.15 0.155 0.155 1.0) 
            (rgba 0.015 0.016 0.025 1.0))))
      (sdf/fill
        (sdf/translate 0 0.70
          (sdf/circle 0.16))
        (material
          :color (if (= plocked 1)
            (rgba 0.82 0.84 0.88 0.95)
            (rgba 0 0 0 0))))
      (sdf/fill (sdf/circle 0.36)
        (material
          :lighting (lighting :edge-min -0.35 :edge-max 0.5
            :light (vec3 0.0 -1.0 2.5) :shininess 32.0)
          :color (if (= active 1)
            (aqua-color
              (rgba (* track-r 0.72) (* track-g 0.72) (* track-b 0.82) 1.0)
              (rgba track-r track-g track-b 1.0))
            (rgba 0 0 0 0)))))))

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

(def seqv-track-actions (i)
  (h-stack :gap 0.35 :padding 0.85
    (box
      :key (str "seqv-expand-" i)
      :width 3.5 :height 1.0
      :background "seqv-ellipsis-button"
      :expanded (if (seqv-track-expanded? (nth SEQ.track-ids i)) 1 0)
      :on-click |x y r| (seqv-track-menu-click i))
    (box :width 0.1 :height 0.0)
    ))

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
        (step-select-drag-over-for-track track step evt))
      nil)))

(def seqv-step-pointer-down (track step evt)
  (let ((use-selection (= SEQ.current-track track)))
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
        (step-pointer-down-for-track track step evt use-selection)))))

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
  (let (
      (odd1 (mod (floor (/ step 4)) 2))
      (odd2 (mod (floor (/ step 32)) 2))
      (odd (if (= odd2 1) (if (= odd1 1) 0 1) odd1))
      (track-r (seqv-track-color-r track (seqv-muted? track)))
      (track-g (seqv-track-color-g track (seqv-muted? track)))
      (track-b (seqv-track-color-b track (seqv-muted? track))))
    (box
      :width 3.05 :height 1.45
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
      :odd odd
      :active (bind-seq (str "seq-track-step-active-" track "-" step))
      :plocked (bind-seq (str "seq-track-step-plocked-" track "-" step))
      :selected (bind-seq (str "seq-track-step-selected-" track "-" step))
      :duration (bind-seq (str "seq-track-step-duration-" track "-" step))
      :track-r track-r :track-g track-g :track-b track-b
      :background "seqv-step-shell")))

(def seqv-playhead-row (track track-id row)
  (box
    :key (str "seqv-playhead-row-" track-id "-" row)
    :width 48.8 :height 0.24
    :background "seqv-playhead-row-bar"
    :col (bind-seq (str "track-playhead-row-" track "-" row))))

(def seqv-track-list (lists track)
  (if (< track (len lists))
    (nth lists track)
    '()))

(def seqv-track-num-steps (track)
  (if (< track (len SEQ.track-num-steps))
    (nth SEQ.track-num-steps track)
    16))

(def seqv-expanded-track-color-r (track)
  (nth (seqv-track-color track) 0))

(def seqv-expanded-track-color-g (track)
  (nth (seqv-track-color track) 1))

(def seqv-expanded-track-color-b (track)
  (nth (seqv-track-color track) 2))

(def seqv-expanded-slider-fill (track)
  (rgba (seqv-expanded-track-color-r track) (seqv-expanded-track-color-g track) (seqv-expanded-track-color-b track) 1.0))

(def seqv-expanded-slider-muted-fill (track)
  (rgba
    (+ (* (seqv-expanded-track-color-r track) 0.30) (* 0.08 0.70))
    (+ (* (seqv-expanded-track-color-g track) 0.30) (* 0.08 0.70))
    (+ (* (seqv-expanded-track-color-b track) 0.30) (* 0.12 0.70))
    0.50))

(def seqv-expanded-slider-muted-dot (track)
  (rgba
    (+ (* (seqv-expanded-track-color-r track) 0.28) (* 0.25 0.72))
    (+ (* (seqv-expanded-track-color-g track) 0.28) (* 0.25 0.72))
    (+ (* (seqv-expanded-track-color-b track) 0.28) (* 0.30 0.72))
    0.55))

(def seqv-param-values (track mode)
  (if (= mode 0) (seqv-track-list SEQ.track-velocities track)
    (if (= mode 1) (seqv-track-list SEQ.track-durations track)
      (if (= mode 2) (seqv-track-list SEQ.track-auxas track)
        (if (= mode 3) (seqv-track-list SEQ.track-transposes track)
          (if (= mode 4) (seqv-track-list SEQ.track-pans track)
            (seqv-track-list SEQ.track-syncs track)))))))

(def seqv-param-value-at (track mode step)
  (let ((values (seqv-param-values track mode)))
    (if (< step (len values))
      (nth values step)
      0)))

(def seqv-param-min (mode)
  (if (= mode 0) 0
    (if (= mode 1) 0
      (if (= mode 2) 0
        (if (= mode 3) -12
          (if (= mode 4) -1
            0))))))

(def seqv-param-max (mode)
  (if (= mode 0) 1
    (if (= mode 1) 32
      (if (= mode 2) 16
        (if (= mode 3) 12
          (if (= mode 4) 1
            (- (len SEQ.sync-labels) 1)))))))

(def seqv-param-slider-min (mode)
  (if (= mode 1) 0 (seqv-param-min mode)))

(def seqv-param-slider-max (mode)
  (if (= mode 1) 1 (seqv-param-max mode)))

(def seqv-param-slider-value (track mode step)
  (if (= mode 1)
    (duration-slider-position (seqv-param-value-at track mode step))
    (seqv-param-value-at track mode step)))

(def seqv-param-haptic-pivot-position (mode)
  (if (= mode 1) 0.5 1))

(def seqv-param-haptic-pivot-value (mode)
  (if (= mode 1) 2 (seqv-param-max mode)))

(def seqv-param-haptic-exponent (mode)
  (if (= mode 1) 4 1))

(def seqv-param-keyword (mode)
  (if (= mode 0) :velocity
    (if (= mode 1) :duration
      (if (= mode 2) :aux-a
        (if (= mode 3) :transpose
          (if (= mode 4) :pan
            :sync))))))

(def seqv-param-color (mode)
  (if (= mode 0) :blue
    (if (= mode 1) :green
      (if (= mode 2) :magenta
        (if (= mode 3) :yellow
          (if (= mode 4) :red
            :green))))))

(def seqv-param-name (mode)
  (if (= mode 0) "Velocity"
    (if (= mode 1) "Duration"
      (if (= mode 2) "Aux A"
        (if (= mode 3) "Transpose"
          (if (= mode 4) "Pan"
            "Sync"))))))

(def seqv-param-origin (mode)
  (if (= mode 3) 0
    (if (= mode 4) 0
      (if (= mode 5) 0
        (seqv-param-min mode)))))

(def seqv-param-decimals (mode)
  (if (= mode 3) 0 2))

(def seqv-step-param-value (mode value)
  (if (= mode 3)
    (round value)
    value))

(def seqv-step-slider-param-value (mode value)
  (if (= mode 1)
    (duration-slider-value value)
    (seqv-step-param-value mode value)))

(def seqv-current-step (track track-id)
  (if (= track SEQ.current-track)
    (seqv-current-selected-step)
    (min (seqv-cursor-step track-id) (- (max 1 (seqv-track-num-steps track)) 1))))

(def seqv-page-count (track)
  (max 1 (floor (/ (+ (seqv-track-num-steps track) (- page-size 1)) page-size))))

(def seqv-current-page (track track-id)
  (min (floor (/ (seqv-current-step track track-id) page-size)) (- (seqv-page-count track) 1)))

(def seqv-playhead-page (track)
  (let ((page (reactive-get "SEQ" (str "track-playhead-page-" track))))
    (min
      (if page page 0)
      (- (seqv-page-count track) 1))))

(def seqv-visible-page (track track-id)
  (if (and SEQ.playing SEQ.auto-follow (not (seq-has-selection?)))
    (seqv-playhead-page track)
    (seqv-current-page track track-id)))

(def seqv-page-offset (track track-id)
  (* (seqv-visible-page track track-id) page-size))

(def seqv-expanded-step-index (track track-id i)
  (+ (seqv-page-offset track track-id) i))

(def seqv-expanded-step-visible? (track track-id i)
  (< (seqv-expanded-step-index track track-id i) (seqv-track-num-steps track)))

(def seqv-step-active? (track step)
  (let ((steps (seqv-track-list SEQ.track-steps track)))
    (if (< step (len steps)) (nth steps step) false)))

(def seqv-step-plocked? (track step)
  (let ((plocks (seqv-track-list SEQ.track-step-has-plocks track)))
    (if (< step (len plocks)) (nth plocks step) false)))

(def seqv-expanded-step-selected? (track step)
  (and (= SEQ.current-track track) (nth SEQ.selected-steps step)))

(def seqv-expanded-sync-current-label (track track-id)
  (nth SEQ.sync-labels
    (floor (+ 0.5 (seqv-param-value-at track 5 (seqv-current-step track track-id))))))

(def seqv-set-expanded-cursor (track track-id step)
  (do
    (seqv-set-cursor-step track-id step)
    (if (= SEQ.current-track track)
      (set! cursor-step step)
      nil)))

(def seqv-expanded-step-click (track track-id step evt)
  (if (seqv-expanded-step-visible? track track-id (- step (seqv-page-offset track track-id)))
    (do
      (seqv-activate-track-for-edit track)
      (cool-off-follow)
      (seqv-set-expanded-cursor track track-id step)
      (set! cursor-step step)
      (if (selection-click? evt)
        (step-select-drag-start step evt)
        (seq-clear-selection)))
    nil))

(def seqv-expanded-step-drag (track track-id step evt)
  (do
    (seqv-activate-track-for-edit track)
    (seqv-set-expanded-cursor track track-id step)
    (set! cursor-step step)
    (step-select-drag-over-for-track track step evt)))

(def seqv-expanded-step-pointer-down (track track-id step evt)
  (let ((use-selection (= SEQ.current-track track)))
    (do
      (seqv-activate-track-for-edit track)
      (seqv-set-expanded-cursor track track-id step)
      (set! cursor-step step)
      (step-pointer-down-for-track track step evt use-selection))))

(def seqv-expanded-step-pointer-up (track track-id step evt)
  (do
    (seqv-activate-track-for-edit track)
    (seqv-set-expanded-cursor track track-id step)
    (set! cursor-step step)
    (step-pointer-up step evt)))

(def seqv-set-expanded-step-param (track track-id step mode slider-value)
  (do
    (seqv-activate-track-for-edit track)
    (cool-off-follow)
    (seqv-set-expanded-cursor track track-id step)
    (set! cursor-step step)
    (seq-set-step-param-from-step
      step
      (seqv-param-keyword mode)
      (seqv-step-slider-param-value mode slider-value))))

(def seqv-set-expanded-current-param (track track-id mode value)
  (do
    (seqv-activate-track-for-edit track)
    (cool-off-follow)
    (set! cursor-step (seqv-current-step track track-id))
    (seq-set-step-param-from-step
      (seqv-current-step track track-id)
      (seqv-param-keyword mode)
      (seqv-step-param-value mode value))))

(def seqv-set-expanded-timebase (track label)
  (let ((plock-selected
      (and (< selected-bus 0) (= SEQ.current-track track) (seq-has-selection?))))
    (do
      (seqv-activate-track-for-edit track)
      (cool-off-follow)
      (if plock-selected
        (seq-plock-timebase label)
        (seq-set-timebase label)))))

(def seqv-goto-page (track track-id page)
  (let ((step (min (* page page-size) (- (max 1 (seqv-track-num-steps track)) 1))))
    (do
      (seqv-activate-track-for-edit track)
      (cool-off-follow)
      (seqv-set-cursor-step track-id step)
      (set! cursor-step step))))

(def seqv-double-track-pattern (track track-id)
  (do
    (seqv-activate-track-for-edit track)
    (cool-off-follow)
    (seq-double-track-pattern)
    (seqv-set-cursor-step track-id (min (seqv-current-step track track-id) (- (max 1 (seqv-track-num-steps track)) 1)))))

(def seqv-halve-track-pattern (track track-id)
  (do
    (seqv-activate-track-for-edit track)
    (cool-off-follow)
    (seq-halve-track-pattern)
    (seqv-set-cursor-step track-id (min (seqv-current-step track track-id) (- (max 1 (seqv-track-num-steps track)) 1)))))

(def seqv-param-tab (track track-id mode tab-label)
  (box :width 8 :height 2
    :key (str "seqv-expanded-param-tab-" track-id "-" mode)
    :bg (if (= (seqv-param-mode track-id) mode) (seqv-param-color mode) :dark-gray)
    :on-click |x y r| (do (seqv-activate-track-for-edit track) (seqv-set-param-mode track-id mode))
    (label tab-label :font-size 12
      :color (if (= (seqv-param-mode track-id) mode) :primary :dim)
      :bg :transparent)))

(def seqv-expanded-track-quick-controls (track track-id)
  (let ((mode (seqv-param-mode track-id)))
    (v-stack (box :height 0.2 :width 1)
    (h-stack :gap 0.55 :align :center
      (box :width 9.4 :height 1.3
        :key (str "seqv-expanded-step-summary-" track-id)
        (label (seqv-param-name mode)
          :font-size 11 :width 9.4 :color :dim :bg :transparent))
      (if (= mode 5)
        (box :width 8 :height 1.3
          :key (str "seqv-expanded-sync-label-" track-id)
          (label (seqv-expanded-sync-current-label track track-id)
            :font-size 11 :color :white :bg :transparent))
        (number-picker :key (str "seqv-expanded-param-number-picker-" track-id)
          :value (seqv-param-value-at track mode (seqv-current-step track track-id))
          :min (seqv-param-min mode) :max (seqv-param-max mode) :decimals (seqv-param-decimals mode)
          :on-change (lambda (v) (seqv-set-expanded-current-param track track-id mode v))
          :width 8 :height 1.3 :font-size 11))
      (h-stack :gap 0.4 :align :center
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :key (str "seqv-expanded-half-" track-id)
          :on-click |x y r| (seqv-halve-track-pattern track track-id)
          (v-stack :align :center
            (label "-"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :key (str "seqv-expanded-double-" track-id)
          :on-click |x y r| (seqv-double-track-pattern track track-id)
          (v-stack :align :center
            (label "+"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "transport-btn-bg" :padding 0.2 :height 1.4
          :key (str "seqv-expanded-pages-" track-id)
          (h-stack :gap 0.1 :align :center
            (each (range 0 (seqv-page-count track)) |page|
              (box :width page-button-width :height 1.1
                :key (str "seqv-expanded-page-" track-id "-" page)
                :background "pattern-pill-bg"
                :active (if (= page (seqv-visible-page track track-id)) 1 0)
                :style pattern-control-style
                :on-click |x y r| (seqv-goto-page track track-id page)
                (v-stack :align :center
                  (label (fmt " {} " (+ page 1))
                    :font-size 11
                    :color (if (= page (seqv-visible-page track track-id)) :white :dim)
                    :bg :transparent)))))))))))

(def seqv-expanded-track-editor (track track-id)
  (let ((mode (seqv-param-mode track-id)))
    (v-stack :width :fill :padding 0.35 :gap 0.1
      (h-stack :gap 0.5
        (seqv-param-tab track track-id 0 "vel")
        (seqv-param-tab track track-id 1 "dur")
        (seqv-param-tab track track-id 2 "aux_a")
        (seqv-param-tab track track-id 3 "xpose")
        (seqv-param-tab track track-id 4 "pan")
        (seqv-param-tab track track-id 5 "syn")
        (h-stack :align :center :gap 0.35
          (dropdown :value (seqv-track-timebase track)
            :key (str "seqv-expanded-timebase-" track-id)
            :options seqv-timebase-options
            :on-change (lambda (v) (seqv-set-expanded-timebase track v))
            :width 6 :height 1.45 :font-size 10)))

      (grid :cols 16 :col-width 4
        (each (range 0 page-size) |i|
          (let ((step (seqv-expanded-step-index track track-id i))
                (visible (seqv-expanded-step-visible? track track-id i)))
            (box :padding 0.25
              :key (str "seqv-expanded-step-column-" track-id "-" i)
              :background (if visible
                (if (and (= track SEQ.current-track) (= (seqv-current-step track track-id) step))
                  "cursor-highlight"
                  nil)
                nil)
              :on-click (lambda (evt)
                (if visible (seqv-expanded-step-click track track-id step evt) nil))
              :on-drag (lambda (evt)
                (if visible (seqv-expanded-step-drag track track-id step evt) nil))
              (v-stack :align :center :gap 0.5
                (let ((step-on (and visible (seqv-step-active? track step))))
                  (if step-on
                    (vslider :height 4
                      :key (str "seqv-expanded-step-slider-" track-id "-" i)
                      :width (if (= mode 5) 2 1)
                      :min (seqv-param-slider-min mode) :max (seqv-param-slider-max mode)
                      :origin (seqv-param-origin mode)
                      :value (seqv-param-slider-value track mode step)
                      :haptic-value (seqv-param-value-at track mode step)
                      :haptic-min (seqv-param-min mode)
                      :haptic-max (seqv-param-max mode)
                      :haptic-pivot-position (seqv-param-haptic-pivot-position mode)
                      :haptic-pivot-value (seqv-param-haptic-pivot-value mode)
                      :haptic-exponent (seqv-param-haptic-exponent mode)
                      :items (if (= mode 5) SEQ.sync-labels '())
                      :font-size 11
                      :color :white
                      :fill (seqv-expanded-slider-fill track)
                      :dot-color :dark-gray
                      :track-r (seqv-expanded-track-color-r track)
                      :track-g (seqv-expanded-track-color-g track)
                      :track-b (seqv-expanded-track-color-b track)
                      :material (seqv-aqua-slider-track-material)
                      :on-change (lambda (v)
                        (if visible
                          (seqv-set-expanded-step-param track track-id step mode v)
                          nil)))
                    (vslider :height 4
                      :key (str "seqv-expanded-step-slider-" track-id "-" i)
                      :width (if (= mode 5) 2 1)
                      :min (seqv-param-slider-min mode) :max (seqv-param-slider-max mode)
                      :origin (seqv-param-origin mode)
                      :value (seqv-param-slider-value track mode step)
                      :haptic-value (seqv-param-value-at track mode step)
                      :haptic-min (seqv-param-min mode)
                      :haptic-max (seqv-param-max mode)
                      :haptic-pivot-position (seqv-param-haptic-pivot-position mode)
                      :haptic-pivot-value (seqv-param-haptic-pivot-value mode)
                      :haptic-exponent (seqv-param-haptic-exponent mode)
                      :items (if (= mode 5) SEQ.sync-labels '())
                      :font-size 11
                      :color :dim
                      :fill (seqv-expanded-slider-muted-fill track)
                      :dot-color (seqv-expanded-slider-muted-dot track)
                      :track-r (seqv-expanded-track-color-r track)
                      :track-g (seqv-expanded-track-color-g track)
                      :track-b (seqv-expanded-track-color-b track)
                      :material (seqv-aqua-slider-track-muted-material)
                      :on-change (lambda (v)
                        (if visible
                          (seqv-set-expanded-step-param track track-id step mode v)
                          nil)))))
                (box
                  :key (str "seqv-expanded-step-toggle-" track-id "-" i)
                  :active (if visible (if (seqv-step-active? track step) 1 0) 0)
                  :plocked (if visible (if (seqv-step-plocked? track step) 1 0) 0)
                  :selected (if visible (if (seqv-expanded-step-selected? track step) 1 0) 0)
                  :background "aqua-button"
                  :align :center :width 3 :height 1.5
                  :on-mouse-down (lambda (evt)
                    (if visible
                      (seqv-expanded-step-pointer-down track track-id step evt)
                      nil))
                  :on-drag (lambda (evt)
                    (if visible
                      (seqv-expanded-step-drag track track-id step evt)
                      nil))
                  :on-mouse-up (lambda (evt)
                    (if visible
                      (seqv-expanded-step-pointer-up track track-id step evt)
                      nil))
                  (metal-track-tick
                    :active (if visible (if (seqv-step-active? track step) 1 0) 0)
                    :plocked (if visible (if (seqv-step-plocked? track step) 1 0) 0)
                    :selected (if visible (if (seqv-expanded-step-selected? track step) 1 0) 0)
                    :track-r (seqv-expanded-track-color-r track)
                    :track-g (seqv-expanded-track-color-g track)
                    :track-b (seqv-expanded-track-color-b track)))
                (label (if visible (str (+ step 1)) "")
                  :key (str "seqv-expanded-step-label-" track-id "-" i)
                  :width 2.8
                  :h-align :center
                  :font-size 10 :bg :transparent
                  :color (if visible
                          (if (seqv-expanded-step-selected? track step)
                            :yellow
                            :dim)
                          :dim))
                (subtree :key (str "seqv-expanded-step-playhead-probe-" track-id "-" i)
                  (step-playhead-dot
                    :active (bind-seq (str "track-playhead-active-" track "-" step))))))))))))

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
            (if (seqv-track-expanded? (nth SEQ.track-ids i))
              (v-stack :width :fill :gap 0.2
                (h-stack :width :fill :gap 0.6 :align :start
                  (seqv-track-header i)
                  (seqv-expanded-track-quick-controls i (nth SEQ.track-ids i))
                  (box :flex 1 :width 0 :height 0.1 :bg :transparent)
                  (seqv-track-actions i))
                (seqv-expanded-track-editor i (nth SEQ.track-ids i)))
              (h-stack :width :fill :gap 0.6 :align :start
                (seqv-track-header i)
                (seqv-track-grid i)
                (box :flex 1 :width 0 :height 0.1 :bg :transparent)
                (seqv-track-actions i)))))))))

(set-buffer-mode-for "*sequencer*" "seq-grid-mode")
