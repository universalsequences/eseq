;; ui/sequencer.lisp — Project step sequencer view.
;; Renders to *sequencer* buffer. Shows every track's step grid laid out
;; vertically. Loaded by ui/main.lisp.

(load "@/ui/track-collapse.lisp")

(def seqv-track-peak (i)
  (bind-seq (str "track-peak-" i)))

(def seqv-track-volume-field (track)
  (str "track-" track "-volume"))

(def seqv-track-volume-binding (track)
  (bind-seq (seqv-track-volume-field track)))

(def seqv-track-volume-value (track)
  (if (< track (len SEQ.track-volumes))
    (nth SEQ.track-volumes track)
    1.0))

(def seqv-track-volume-from-event (track event)
  (let ((sx (get event :sx)))
    (if (= sx nil)
      (seqv-track-volume-value track)
      (max 0.0 (min 1.0 (* 0.5 (+ sx 1.0)))))))

(def seqv-set-track-volume-from-event (track event)
  (do
    (seqv-activate-track-for-edit track)
    (seq-set-track-volume track (seqv-track-volume-from-event track event))))

(def seqv-track-color-r-binding (track)
  (bind-seq-nth "track-color-r-effective" track))

(def seqv-track-color-g-binding (track)
  (bind-seq-nth "track-color-g-effective" track))

(def seqv-track-color-b-binding (track)
  (bind-seq-nth "track-color-b-effective" track))

(def seqv-track-name-max-chars 9)

(def seqv-track-name-display (name)
  (if (> (len name) seqv-track-name-max-chars)
    (str (substring name 0 (- seqv-track-name-max-chars 2)) "..")
    name))

(def seqv-muted? (i)
  (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i)))

(def seqv-track-selected-binding (i)
  (if (< selected-bus 0)
    (bind-seq (str "track-selected-" i))
    0))

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

(def seqv-track-index-for-id (track-id)
  (let ((matches
      (filter
        (lambda (i) (= (nth SEQ.track-ids i) track-id))
        (range 0 (len SEQ.track-ids)))))
    (if (> (len matches) 0)
      (nth matches 0)
      -1)))

(def seqv-project-cursor-step (track step)
  (mod step (max 1 (seqv-track-num-steps track))))

(def seqv-global-cursor-step-for-track (track)
  (seqv-project-cursor-step track cursor-step))

(def seqv-sync-track-cursor-to-global (track)
  (if (and (>= track 0) (< track (len SEQ.track-ids)))
    (seqv-set-cursor-step
      (seqv-track-id track)
      cursor-step)
    nil))

(def seqv-sync-all-track-cursors-to-global ()
  (for-each
    (lambda (track) (seqv-sync-track-cursor-to-global track))
    (range 0 (len SEQ.track-ids))))

(def seqv-select-track-for-edit (track)
  (do
    (set! selected-bus -1)
    (if (= SEQ.current-track track)
      nil
      (do
        (seq-clear-selection)
        (seq-show-fx-lower-panel)))
    (seq-set-track track)
    (seqv-sync-track-cursor-to-global track)))

(def seqv-activate-track-for-edit (track)
  (seqv-select-track-for-edit track))

(def seqv-open-piano-roll-for-track (track)
  (if (and (= lower-panel-buffer "*piano-roll*") (= SEQ.current-track track))
    (seq-show-fx-lower-panel)
    (do
      (seqv-activate-track-for-edit track)
      (seq-open-piano-roll-bottom-for-track track))))

(def seqv-track-expanded? (track-id)
  (reactive-get "SEQV" (seqv-expanded-track-field track-id)))

(def seqv-set-track-expanded (track-id expanded)
  (do
    (reactive-set "SEQV" (seqv-expanded-track-field track-id) expanded)
    (if expanded
      (let ((track (seqv-track-index-for-id track-id)))
        (if (>= track 0)
          (seqv-sync-expanded-step-slots-for track track-id)
          nil))
      (seqv-clear-expanded-step-slots track-id))
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
      (dict :id track-id :param-mode 0 :cursor-step nil))))

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
  (do
    (seqv-upsert-editor-state track-id
      (merge (seqv-editor-state-for track-id) :param-mode mode))
    (let ((track (seqv-track-index-for-id track-id)))
      (if (>= track 0)
        (seqv-sync-expanded-step-slots-for track track-id)
        nil))))

(def seqv-cursor-step (track-id)
  (let ((track (seqv-track-index-for-id track-id)))
    (if (>= track 0)
      (let ((stored-step (reactive-get "SEQV" (str "cursor-step-" track-id))))
        (if (= stored-step nil)
          (seqv-global-cursor-step-for-track track)
          (seqv-project-cursor-step track stored-step)))
      0)))

(def seqv-cursor-highlight-field (track step)
  (str "seqv-track-cursor-" track "-" step))

(def seqv-cursor-highlight-binding (track step)
  (bind "SEQV" (seqv-cursor-highlight-field track step)))

(def seqv-set-cursor-step (track-id step)
  (let ((track (seqv-track-index-for-id track-id)))
    (if (>= track 0)
      (let ((previous-step (seqv-cursor-step track-id))
          (projected-step (seqv-project-cursor-step track step)))
        (do
          (reactive-set "SEQV" (str "cursor-step-" track-id) projected-step)
          (if (= previous-step projected-step)
            nil
            (reactive-set "SEQV" (seqv-cursor-highlight-field track previous-step) false))
          (reactive-set "SEQV" (seqv-cursor-highlight-field track projected-step) true)
          (if (seqv-track-expanded? track-id)
            (seqv-sync-expanded-step-slots-for track track-id)
            nil)))
      nil)))

(def sequencer-cursor-step-changed (track step)
  (if (and (>= track 0) (< track (len SEQ.track-ids)))
    (seqv-set-cursor-step (seqv-track-id track) step)
    nil))

(def seqv-current-track-id ()
  (seqv-track-id SEQ.current-track))

(def seqv-current-track-expanded? ()
  (seqv-track-expanded? (seqv-current-track-id)))

(def seqv-current-selected-step ()
  (seqv-cursor-step (seqv-current-track-id)))

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
        (if (or (= key "t") (= key "T"))
          3
          (if (or (= key "p") (= key "P"))
            4
            (if (or (= key "s") (= key "S"))
              5
              (if (or (= key "x") (= key "X"))
                (if (> (len SEQ.process-lanes) 0) seqv-process-lane-mode-offset -1)
                -1))))))))

(def seqv-selected-drum-sound (track)
  (let ((selected
          (filter
            (lambda (sound)
              (> (reactive-value (seqv-drum-slot-selected-binding sound)) 0.5))
            (seqv-track-drum-sounds track))))
    (if (> (len selected) 0) (nth selected 0) nil)))

(def seqv-select-all-current-track-steps ()
  (let ((track SEQ.current-track))
    (do
      (set! selected-bus -1)
      (if (seqv-track-drum-rack? track)
        (let ((sound (seqv-selected-drum-sound track)))
          (if sound
            (seq-select-all-drum-rack-steps (get sound :transpose))
            nil))
        (select-all-steps)))))

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

(def seqv-drop-sample-on-track (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((path (get payload :path))
          (track (get target :track)))
      (if path
        (do
          (seqv-activate-track-for-edit track)
          (sbrowser-drop-sample-on-track event))
        (status "Drop a sample file, not a folder")))))

(def seqv-drop-on-track (event)
  (if (= (get event :drag-type) "sound")
    (sbrowser-drop-sound-on-track event)
    (if (= (get event :drag-type) "instrument")
      (sbrowser-drop-instrument-on-track event)
      (seqv-drop-sample-on-track event))))

(def seqv-drop-new-track (event)
  (let ((payload (get event :payload)))
    (let ((path (get payload :path))
          (name (get payload :name)))
      (if (= (get event :drag-type) "sound")
        (if path
          (host-command "add-track-from-sound" (dict :path path))
          (status "Drop a Sound item, not a folder"))
        (if (= (get event :drag-type) "instrument")
          (if name
            (do
              (set! sbrowser-loading-instrument-name name)
              (host-command "add-track-instrument" (dict :name name)))
            (status "Drop an instrument, not a folder"))
          (if path
            (host-command "add-track-sample" (dict :path path :preserve-browser-context true))
            (status "Drop a sample file, not a folder")))))))

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

(defwidget seqv-track-color-badge
  :width 0.68 :height 1.5
  :paint-margin 0.08
  :state (track-r track-g track-b)
  :bindable (track-r track-g track-b)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.28)
      (material
        :lighting (lighting :edge-min -0.38 :edge-max 0.45
          :light (vec3 0.0 -1.1 2.8) :shininess 54.0)
        :color
          (aqua-color
            (rgba (* track-r 0.55) (* track-g 0.55) (* track-b 0.55) 1.0)
            (rgba track-r track-g track-b 1.0))))))

(defwidget seqv-track-volume-meter
  :width 8.2 :height 1.05
  :paint-margin 0.16
  :state (level volume track-r track-g track-b)
  :bindable (level volume track-r track-g track-b)
  :shader
  (let ((lvl (min 1.0 (max 0.0 level)))
        (vol (min 1.0 (max 0.0 volume)))
        (track (sdf/rounded-rect width height 0.18))
        (green-end (min lvl 0.70))
        (yellow-end (min lvl 0.88))
        (red-end lvl)
        (marker-start (max 0.0 (- vol 0.012)))
        (marker-end (min 1.0 (+ vol 0.012))))
    (sdf/layer
      (sdf/fill track
        (material
          :lighting (lighting :edge-min -0.45 :edge-max 0.35
            :light (vec3 0.0 -1.4 2.2) :shininess 24.0)
          :color (rgba 0.035 0.040 0.050 0.2)))
      (if (> green-end 0.005)
        (sdf/fill
          (let ((__start 0.0)
                (__end green-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.30)
                (__radius (min 0.15 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.10 0.85 0.30 1.0)))
        (rgba 0 0 0 0))
      (if (> (- yellow-end 0.70) 0.005)
        (sdf/fill
          (let ((__start 0.70)
                (__end yellow-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.30)
                (__radius (min 0.15 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.96 0.82 0.18 1.0)))
        (rgba 0 0 0 0))
      (if (> (- red-end 0.88) 0.005)
        (sdf/fill
          (let ((__start 0.88)
                (__end red-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.30)
                (__radius (min 0.15 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.95 0.18 0.16 1.0)))
        (rgba 0 0 0 0))
      (sdf/fill
        (let ((__start marker-start)
              (__end marker-end)
              (__half_w (* 0.5 aspect (- __end __start)))
              (__half_h 0.44)
              (__radius 0.045))
          (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                (y (* 0.5 y)))
            (sdf/rounded-rect __half_w __half_h __radius)))
        (material
          :lighting (lighting :edge-min -0.30 :edge-max 0.45
            :light (vec3 0.0 -1.0 2.6) :shininess 64.0)
          :color
            (aqua-color
              (rgba (+ (* track-r 0.38) 0.45) (+ (* track-g 0.38) 0.45) (+ (* track-b 0.38) 0.45) 1.0)
              (rgba 0.95 0.96 0.98 1.0)))))))

(defwidget seqv-ellipsis-button
  :width 2.2 :height 1.2
  :state (expanded)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.98)
      (material
        :lighting (lighting :edge-min -0.45 :edge-max 0.4
          :light (vec3 0.1 -1.2 2.4) :shininess 24.0)
        :color 
        (if expanded 
          (rgba 0.18 0.18 0.20 1.0) 
          :bg)))
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
       (* (if (= active 1) 1.0 0.42)
          (aqua-color
            (if (= active 1)
              (rgba (* track-r 0.55) (* track-g 0.55) (* track-b 0.55) 1.0)
              (rgba
                (+ (* track-r 0.36) 0.06)
                (+ (* track-g 0.36) 0.06)
                (+ (* track-b 0.36) 0.08)
                0.85))
            (if (= active 1)
              (rgba track-r track-g track-b 1.0)
              (rgba
                (+ (* track-r 0.30) 0.04)
                (+ (* track-g 0.30) 0.04)
                (+ (* track-b 0.30) 0.08)
                0.85))))))

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
          (__half_h 0.32)
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
  :state (active odd plocked plock-kind selected duration hide track-r track-g track-b variant-r variant-g variant-b)
  :bindable (active plocked plock-kind selected duration hide track-r track-g track-b variant-r variant-g variant-b)
  :shader
  (if (= hide 1)
    (rgba 0 0 0 0)
    (let ((vcol (rgba variant-r variant-g variant-b 1.0))
        (seqcol (rgba 0.545 0.545 0.588 0.95))
        (border (if (= selected 1)
            (rgba 0.90 0.92 0.96 1.0)
            (if (= odd 1) 
              (rgba 0.28 0.28 0.28 1.0) 
              (rgba 0.18 0.18 0.18 1.0)))))
    (sdf/layer
      (sdf/fill
        (sdf/translate 0 0.0
          (sdf/rounded-rect (* 1.0 width) (* 1.00 height) 0.1))
        (material
          :lighting (lighting :edge-min -0.3 :edge-max 0.393
            :light (vec3 0.8 -1.8 4.5) :shininess 92.0)
          :color (if (= duration 1)
            (aqua-color
               (mix :white (rgba (* track-r 0.55) (* track-g 0.55) (* track-b 0.55) 0.5) (if (= selected 1) 0.8 1))
              (if (= selected 1) :white (rgba track-r track-g track-b 1)))
            (rgba 0 0 0 0))))
      (sdf/fill (sdf/circle (if (= selected 1) 0.76 0.75))
        (material
          :lighting (lighting :edge-min -0.3 :edge-max 1.0
            :light (vec3 0.3 -1.0 1.5) :shininess 92.0)
          :color (aqua-color border (rgba 0.9 0.1 0.5 1.0))))
      (sdf/fill (sdf/circle (if (= selected 1) 0.64 0.69))
        (material
          :color (if (= odd 1)
            (rgba 0.15 0.155 0.155 0.6)
            (rgba 0.015 0.016 0.025 0.8))))
      (sdf/fill
        (sdf/translate 0 0.82
          (sdf/rounded-rect 0.52 0.10 0.05))
        (material
          :color (if (= active 1)
            (if (= plock-kind 2)
              vcol
              (if (= plock-kind 1)
                seqcol
                (rgba 0 0 0 0)))
            (rgba 0 0 0 0))
          :shadow (shadow
            :color (if (= active 1)
              (if (= plock-kind 2)
                (rgba variant-r variant-g variant-b 0.70)
                (rgba 0 0 0 0))
              (rgba 0 0 0 0))
            :blur (if (= active 1)
              (if (= plock-kind 2) 0.12 0.0)
              0.0)
            :offset (vec2 0 0))))
      (sdf/fill (sdf/circle (if (= selected 1) 0.35 0.5))
        (material
          :lighting (lighting :edge-min -0.25 :edge-max 1.95
            :light (vec3 0.0 -1.0 2.5) :shininess 32.0)
          :color (if (= active 1)
            (aqua-color
              (rgba (* track-r 0.72) (* track-g 0.72) (* track-b 0.82) 1.0)
              (rgba track-r track-g track-b 1.0))
            (rgba 0 0 0 0))))))))

;; SEQ.song-track-governed carries one number per track (takes spec 10 UX):
;; 0 = the lane is not playing a take (pattern lanes stay fully editable —
;; jam with the step sequencer while the arrangement plays), 1 = the lane is
;; take-governed (dimmed steps + non-interactive grid + lit Back-to-Song
;; play button), 2 = a take lane the performer manually latched away
;; (editable again; the grey play button returns it to the song).
(def seqv-track-take-state (i)
  (let ((state (nth SEQ.song-track-governed i)))
    (if (= state nil) 0 state)))

(def seqv-track-song-governed? (i)
  (= (seqv-track-take-state i) 1))

;; Per-track take-lane indicator / Back-to-Song button: a play triangle that
;; sits lit green while a take governs the lane and grey while the lane is
;; manually latched (clicking then hands it back to the song). `take-state`
;; is the SEQ.song-track-governed value (0/1/2); at 0 the triangle renders
;; fully transparent — the box is ALWAYS in the layout so lanes flipping
;; between pattern and take never trigger a re-layout, only a repaint.
(defwidget seqv-back-to-song-icon
  :width 1.5 :height 1.5
  :state (take-state)
  :bindable (take-state)
  :shader
  (sdf/layer
    (sdf/fill
      (let ((p1x -0.32) (p1y -0.44) (p2x -0.32) (p2y 0.44) (p3x 0.5) (p3y 0.0))
        (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
              (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
              (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
          (max (max d1 d2) d3)))
      (material :color
        (if (= take-state 1)
          (rgba 0.35 0.82 0.40 1.0)
          (if (= take-state 2)
            (rgba 0.42 0.43 0.47 1.0)
            (rgba 0 0 0 0)))))))

(def seqv-mute-bg (active)
  (if active
    (rgba 0.08 0.09 0.10 1.0)
         (rgba 0.95 0.48 0.18 1.0)))

(def seqv-solo-bg (active)
  (if active
    (rgba 0.72 0.10 0.10 1.0)
    (rgba 0.08 0.09 0.10 1.0)))

;; Compact mixer track row — the common track actions plus an inline
;; meter/fader so the sequencer remains usable when the mixer is hidden.
;; The header lives in its own subtree so name/mute/solo/arm changes rerun
;; only this header instead of the whole track row (incl. its step grid).
(def seqv-track-header (i)
  (subtree :key (str "seqv-track-header-" (nth SEQ.track-ids i))
    (seqv-track-header-body i)))

(def seqv-track-volume-control (i)
  (v-stack (box :height 0.13 )
    (box
      :key (str "seqv-track-volume-control-" i)
      :width 8.2 :height 1.25
      :background "seqv-track-volume-meter"
      :level (seqv-track-peak i)
      :volume (seqv-track-volume-binding i)
      :track-r (seqv-track-color-r-binding i)
      :track-g (seqv-track-color-g-binding i)
      :track-b (seqv-track-color-b-binding i)
      :on-click (lambda (event) (seqv-set-track-volume-from-event i event))
      :on-drag (lambda (event) (seqv-set-track-volume-from-event i event))))
  )

(def seqv-track-header-body (i)
  (let ((name (nth SEQ.track-names i)))
    (box :background "seqv-track-container"
      :padding 0.4
      :on-click |x y r| (seqv-select-track-for-edit i)
      (h-stack :gap 0.4 :align :center
        (box
          :key (str "seqv-color-badge-" i)
          :width 0.68 :height 1.85
          :background "seqv-track-color-badge"
          :track-r (seqv-track-color-r-binding i)
          :track-g (seqv-track-color-g-binding i)
          :track-b (seqv-track-color-b-binding i)
          :on-click |x y r| (seqv-select-track-for-edit i))
        (box :width 2 :height 1.5
          :background "seqv-rec-arm-dot"
          :key (str "seqv-arm-" i)
          :active (if (nth SEQ.record-armed i) 1 0)
          :on-click |x y r| (do (seqv-activate-track-for-edit i) (seq-toggle-record-arm i)))
        (button (str (+ i 1))
          :key (str "seqv-mute-" i)
          :width 1.55 :height 1.2 :padding 0 :font-size 10
          :background-color (seqv-mute-bg (nth SEQ.track-mutes i))
          :color (if (nth SEQ.track-mutes i) :gray :black)
          :on-click |x y r| (do (seqv-activate-track-for-edit i) (seq-toggle-track-mute i)))
        (button "S"
          :key (str "seqv-solo-" i)
          :width 1.55 :height 1.2 :padding 0 :font-size 10
          :background-color (seqv-solo-bg (nth SEQ.track-solos i))
          :color (if (nth SEQ.track-solos i) :white :gray)
          :on-click |x y r| (do (seqv-activate-track-for-edit i) (seq-toggle-track-solo i)))
        (box :width 8.6 :height 1
          :key (str "seqv-select-" i)
          :background-color :transparent
          :on-click |x y r| (seqv-select-track-for-edit i)
          :on-double-click (lambda (evt) (seqv-open-piano-roll-for-track i))
          (badge (seqv-track-name-display name)
            :key (str "seqv-track-name-label-" i)
            :icon (seq-track-type-icon i)
            :font-size 11 :width 8.6 :height 1 :padding 0
            :h-align :left
            :background-color :transparent
            :border-color :transparent
            :highlight-color :transparent
            :shadow-color :transparent
            :color (if (or (nth SEQ.track-mutes i) (nth SEQ.track-muted-by-solo i))
                     :dark-gray
                     :dim)
            :bg :transparent))
        (seqv-track-volume-control i)
        ;; Take-lane indicator (takes spec 10 UX): green = a take governs
        ;; the lane (steps dim, grid read-only); grey = the performer
        ;; latched the lane away — click returns it to the song; invisible
        ;; on pattern lanes. Always laid out — the reactive take-state only
        ;; repaints the widget, so pattern<->take flips never re-layout.
        (box :width 2 :height 1.5
          :background "seqv-back-to-song-icon"
          :key (str "seqv-back-to-song-" i)
          :take-state (bind-seq-nth "song-track-governed" i)
          :on-click |x y r|
            (if (> (seqv-track-take-state i) 0)
              (seq-song-back-to-song-track i)
              nil))))))

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

;; The expanded step editor is a fixed-format 16-column control grid. Keeping
;; the dimensions named here lets the grid use an explicit row height without
;; duplicating geometry across the widget tree.
(def seqv-expanded-step-column-padding 0.25)
(def seqv-expanded-step-column-gap 0.5)
(def seqv-expanded-step-slider-height 4)
(def seqv-expanded-step-toggle-height 1.5)
(def seqv-expanded-step-label-height 1)
(def seqv-expanded-step-playhead-height 0.7)
(def seqv-expanded-step-row-height
  (+ (* 2 seqv-expanded-step-column-padding)
    seqv-expanded-step-slider-height
    seqv-expanded-step-toggle-height
    seqv-expanded-step-label-height
    seqv-expanded-step-playhead-height
    (* 3 seqv-expanded-step-column-gap)))

(def seqv-drag-track nil)
(def seqv-duration-drag-source nil)
(def seqv-drum-drag-pad nil)

(def seqv-duration-edge? (evt)
  (let ((sx (get evt :sx)))
    (and (not (= sx nil)) (> sx 0.48))))

(def seqv-set-duration-from-drag (track source step)
  (do
    (seq-set-track track)
    (seq-set-step-param source :duration (max 1 (min 32 (+ (- step source) 1))))))

(def seqv-step-select-drag-start (track step evt)
  (if (seqv-track-song-governed? track)
    nil
    (do
      (set! selected-bus -1)
      (seq-set-track track)
      (set! seqv-drag-track track)
      (step-select-drag-start step evt))))

(def seqv-step-select-drag-over (track step evt)
  (if (seqv-track-song-governed? track)
    nil
    (if (and (= seqv-drag-track track) (not (= seqv-duration-drag-source nil)))
      (seqv-set-duration-from-drag track seqv-duration-drag-source step)
      (if (= seqv-drag-track track)
        (do
          (seq-set-track track)
          (step-select-drag-over-for-track track step evt))
        nil))))

;; Song-governed lanes are non-interactive (takes spec 10 UX): while the
;; arrangement holds launch authority the Seq grid is a dimmed read-only view
;; of the session pattern — edits would silently target a pattern the lane is
;; not playing.
(def seqv-step-pointer-down (track step evt)
  (if (seqv-track-song-governed? track)
    nil
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
            (set-track-cursor-step step)
            (seqv-set-duration-from-drag track step step))
          (step-pointer-down-for-track track step evt use-selection))))))

(def seqv-step-double-click (track step evt)
  (if (seqv-track-song-governed? track)
    nil
    (do
      (seq-set-track track)
      (step-double-click-for-track track step evt))))

(def seqv-step-pointer-up (track step evt)
  (do
    (if (and (= seqv-drag-track track)
          (= seqv-duration-drag-source nil)
          (not (seqv-track-song-governed? track)))
      (do
        (seq-set-track track)
        (step-pointer-up step evt))
      nil)
    (set! seqv-drag-track nil)
    (set! seqv-duration-drag-source nil)))

(def seqv-set-drum-duration-from-drag (track pad-note source step)
  (do
    (seq-set-track track)
    (seq-set-drum-lane-step-duration
      track
      pad-note
      source
      (max 1 (min 32 (+ (- step source) 1))))))

(def seqv-drum-step-select-drag-over (track pad-note step evt)
  (if (seqv-track-song-governed? track)
    nil
    (if (and (= seqv-drag-track track)
          (= seqv-drum-drag-pad pad-note)
          (not (= seqv-duration-drag-source nil)))
      (seqv-set-drum-duration-from-drag
        track pad-note seqv-duration-drag-source step)
      (if (and (= seqv-drag-track track) (= seqv-drum-drag-pad pad-note))
        (drum-step-select-drag-over track pad-note step evt)
        nil))))

(def seqv-drum-step-pointer-down (track slot-idx pad-note step evt)
  (if (seqv-track-song-governed? track)
    nil
    (do
      (seqv-select-drum-slot-index track slot-idx)
      (set! selected-bus -1)
      (seq-set-track track)
      (set! seqv-drag-track track)
      (set! seqv-drum-drag-pad pad-note)
      (if (and (seq-drum-lane-step-active? track pad-note step)
            (not (selection-click? evt))
            (seqv-duration-edge? evt))
        (do
          (set! seqv-duration-drag-source step)
          (set! step-click-pending nil)
          (set! step-drag-anchor nil)
          (set! step-move-last nil)
          (cool-off-follow)
          (drum-step-set-cursor track pad-note step)
          (seqv-set-drum-duration-from-drag track pad-note step step))
        (drum-step-pointer-down track pad-note step evt)))))

(def seqv-drum-step-pointer-up (track pad-note step evt)
  (do
    (if (and (= seqv-drag-track track)
          (= seqv-drum-drag-pad pad-note)
          (= seqv-duration-drag-source nil)
          (not (seqv-track-song-governed? track)))
      (drum-step-pointer-up track pad-note step evt)
      nil)
    (set! seqv-drag-track nil)
    (set! seqv-drum-drag-pad nil)
    (set! seqv-duration-drag-source nil)))

(def seqv-drum-step-double-click (track slot-idx pad-note step evt)
  (if (seqv-track-song-governed? track)
    nil
    (do
      (seqv-select-drum-slot-index track slot-idx)
      (seq-set-track track)
      (drum-step-double-click track pad-note step evt))))

;; Single tight step button (no slider, no number).
(def seqv-track-step-value (lists track step fallback)
  (let ((track-list (if (< track (len lists)) (nth lists track) '())))
    (if (< step (len track-list))
      (nth track-list step)
      fallback)))

(def seqv-step-odd (step)
  (let ((odd1 (mod (floor (/ step 4)) 2))
      (odd2 (mod (floor (/ step 32)) 2)))
    (if (= odd2 1) (if (= odd1 1) 0 1) odd1)))

(def seqv-step-cell (track step visible)
  ;; Step cells use the step-color channels: same as the track color but
  ;; additionally dimmed while the lane is take-governed (the header keeps
  ;; its full color — only the actual steps go translucent).
  (let ((track-r (bind-seq-nth "step-color-r-effective" track))
      (track-g (bind-seq-nth "step-color-g-effective" track))
      (track-b (bind-seq-nth "step-color-b-effective" track))
      (plock-kind (bind-seq (str "seq-track-step-plock-kind-" track "-" step)))
      (variant-r (bind-seq (str "seq-track-step-variant-r-" track "-" step)))
      (variant-g (bind-seq (str "seq-track-step-variant-g-" track "-" step)))
      (variant-b (bind-seq (str "seq-track-step-variant-b-" track "-" step))))
    (box
      :width 3.05 :height 1.55
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
      :on-double-click (lambda (evt)
        (if visible
          (seqv-step-double-click track step evt)
          nil))
      :active (seqv-cursor-highlight-binding track step)
      :selected (seqv-track-selected-binding track)
      :hide (if visible 0 1)
      :background "cursor-highlight"
      (box
        :width 3.05 :height 1.55
        :align :center
        :odd (seqv-step-odd step)
        :active (bind-seq (str "seq-track-step-active-" track "-" step))
        :plocked (bind-seq (str "seq-track-step-plocked-" track "-" step))
        :plock-kind plock-kind
        :selected (bind-seq (str "seq-track-step-selected-" track "-" step))
        :duration (bind-seq (str "seq-track-step-duration-" track "-" step))
        :hide (if visible 0 1)
        :track-r track-r :track-g track-g :track-b track-b
        :variant-r variant-r :variant-g variant-g :variant-b variant-b
        :background "seqv-step-shell"))))

(def seqv-drum-lane-step-cell (track slot-idx pad-note step visible)
  (let ((track-r (bind-seq-nth "step-color-r-effective" track))
      (track-g (bind-seq-nth "step-color-g-effective" track))
      (track-b (bind-seq-nth "step-color-b-effective" track))
      (plock-kind (bind-seq (str "seq-track-step-plock-kind-" track "-" step)))
      (variant-r (bind-seq (str "seq-track-step-variant-r-" track "-" step)))
      (variant-g (bind-seq (str "seq-track-step-variant-g-" track "-" step)))
      (variant-b (bind-seq (str "seq-track-step-variant-b-" track "-" step))))
    (box
      :width 3.05 :height 1.55
      :key (str "seqv-drum-lane-step-" track "-" pad-note "-" step)
      :debug-name "seqv-drum-lane-step"
      :on-mouse-down (lambda (evt)
        (if visible
          (seqv-drum-step-pointer-down track slot-idx pad-note step evt)
          nil))
      :on-drag (lambda (evt)
        (if visible
          (seqv-drum-step-select-drag-over track pad-note step evt)
          nil))
      :on-mouse-up (lambda (evt)
        (if visible
          (seqv-drum-step-pointer-up track pad-note step evt)
          nil))
      :on-double-click (lambda (evt)
        (if visible
          (seqv-drum-step-double-click track slot-idx pad-note step evt)
          nil))
      :active (if (and (= drum-step-cursor-track track)
                    (= drum-step-cursor-pad pad-note))
        (seqv-cursor-highlight-binding track step)
        0)
      :selected (seqv-track-selected-binding track)
      :hide (if visible 0 1)
      :background "cursor-highlight"
      (box
        :width 3.05 :height 1.55
        :align :center
        :odd (seqv-step-odd step)
        :active (bind-seq (str "drum-lane-step-active-" track "-" pad-note "-" step))
        :plocked (bind-seq (str "seq-track-step-plocked-" track "-" step))
        :plock-kind plock-kind
        :selected (bind-seq
          (str "drum-lane-step-selected-" track "-" pad-note "-" step))
        :duration (bind-seq
          (str "drum-lane-step-duration-" track "-" pad-note "-" step))
        :hide (if visible 0 1)
        :track-r track-r :track-g track-g :track-b track-b
        :variant-r variant-r :variant-g variant-g :variant-b variant-b
        :background "seqv-step-shell"))))

(def seqv-playhead-row (track track-id row)
  (box
    :key (str "seqv-playhead-row-" track-id "-" row)
    :width 48.8 :height 0.24
    :background "seqv-playhead-row-bar"
    :col (bind-seq (str "track-playhead-row-" track "-" row))))

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

(def seqv-current-step (track track-id)
  (seqv-cursor-step track-id))

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

(def seqv-sync-expanded-step-slots-for (track track-id)
  (seqv-sync-expanded-step-slots
    track
    track-id
    (seqv-visible-page track track-id)
    (seqv-param-mode track-id)
    (seqv-current-step track track-id)))

(def seqv-slot-field (name track-id slot)
  (str "seqv-slot-" name "-" track-id "-" slot))

(def seqv-slot-param-field (kind track-id mode slot)
  (str "seqv-slot-param-" kind "-" track-id "-" mode "-" slot))

(def seqv-slot-page-active-field (track-id page)
  (str "seqv-page-active-" track-id "-" page))

(def seqv-slot-step-index-binding (track-id slot)
  (bind-seq (seqv-slot-field "step-index" track-id slot)))

(def seqv-slot-step-index-value (track-id slot)
  (reactive-value (seqv-slot-step-index-binding track-id slot)))

(def seqv-slot-visible-binding (track-id slot)
  (bind-seq (seqv-slot-field "visible" track-id slot)))

(def seqv-slot-visible? (track-id slot)
  (> (reactive-value (seqv-slot-visible-binding track-id slot)) 0.5))

(def seqv-slot-label-binding (track-id slot)
  (bind-seq (seqv-slot-field "step-label" track-id slot)))

(def seqv-slot-active-binding (track-id slot)
  (bind-seq (seqv-slot-field "active" track-id slot)))

(def seqv-slot-plocked-binding (track-id slot)
  (bind-seq (seqv-slot-field "plocked" track-id slot)))

(def seqv-slot-selected-binding (track-id slot)
  (bind-seq (seqv-slot-field "selected" track-id slot)))

(def seqv-slot-playhead-binding (track-id slot)
  (bind-seq (seqv-slot-field "playhead-active" track-id slot)))

(def seqv-slot-cursor-binding (track-id slot)
  (bind-seq (seqv-slot-field "cursor-active" track-id slot)))

(def seqv-slot-param-slider-binding (track-id mode slot)
  (bind-seq (seqv-slot-param-field "slider" track-id mode slot)))

(def seqv-slot-param-haptic-binding (track-id mode slot)
  (bind-seq (seqv-slot-param-field "haptic" track-id mode slot)))

(def seqv-page-active-binding (track-id page)
  (bind-seq (seqv-slot-page-active-field track-id page)))

(def seqv-step-active-binding (track step)
  (bind-seq (str "seq-track-step-active-" track "-" step)))

(def seqv-step-plocked-binding (track step)
  (bind-seq (str "seq-track-step-plocked-" track "-" step)))

(def seqv-step-selected-binding (track step)
  (bind-seq (str "seq-track-step-selected-" track "-" step)))

(def seqv-step-param-slider-binding (track mode step)
  (bind-seq (str "seq-track-step-param-slider-" track "-" mode "-" step)))

(def seqv-step-param-haptic-binding (track mode step)
  (bind-seq (str "seq-track-step-param-haptic-" track "-" mode "-" step)))

(def seqv-expanded-sync-current-label (track track-id)
  (nth SEQ.sync-labels
    (floor (+ 0.5 (seqv-param-value-at track 5 (seqv-current-step track track-id))))))

(def seqv-set-expanded-cursor (track track-id step)
  (do
    (set-track-cursor-step step)))

(def seqv-expanded-step-click (track track-id step evt)
  (if (seqv-expanded-step-visible? track track-id (- step (seqv-page-offset track track-id)))
    (do
      (seqv-activate-track-for-edit track)
      (cool-off-follow)
      (seqv-set-expanded-cursor track track-id step)
      (if (selection-click? evt)
        (step-select-drag-start step evt)
        (seq-clear-selection)))
    nil))

(def seqv-expanded-step-drag (track track-id step evt)
  (do
    (step-select-drag-over-for-track-no-cursor track step evt)))

(def seqv-expanded-step-pointer-down (track track-id step evt)
  (let ((use-selection (= SEQ.current-track track)))
    (do
      (seqv-activate-track-for-edit track)
      (seqv-set-expanded-cursor track track-id step)
      (step-pointer-down-for-track track step evt use-selection))))

(def seqv-expanded-step-pointer-up (track track-id step evt)
  (do
    (seqv-activate-track-for-edit track)
    (seqv-set-expanded-cursor track track-id step)
    (step-pointer-up step evt)))

(def seqv-expanded-slot-click (track track-id slot evt)
  (let ((step (seqv-slot-step-index-value track-id slot)))
    (if (>= step 0)
      (seqv-expanded-step-click track track-id step evt)
      nil)))

(def seqv-expanded-slot-drag (track track-id slot evt)
  (let ((step (seqv-slot-step-index-value track-id slot)))
    (if (>= step 0)
      (seqv-expanded-step-drag track track-id step evt)
      nil)))

(def seqv-expanded-slot-pointer-down (track track-id slot evt)
  (let ((step (seqv-slot-step-index-value track-id slot)))
    (if (>= step 0)
      (seqv-expanded-step-pointer-down track track-id step evt)
      nil)))

(def seqv-expanded-slot-pointer-up (track track-id slot evt)
  (let ((step (seqv-slot-step-index-value track-id slot)))
    (if (>= step 0)
      (seqv-expanded-step-pointer-up track track-id step evt)
      nil)))

(def seqv-expanded-step-double-click (track track-id step evt)
  (do
    (seqv-activate-track-for-edit track)
    (step-double-click-for-track track step evt)))

(def seqv-expanded-slot-double-click (track track-id slot evt)
  (let ((step (seqv-slot-step-index-value track-id slot)))
    (if (>= step 0)
      (seqv-expanded-step-double-click track track-id step evt)
      nil)))

(def seqv-set-expanded-slot-param (track track-id slot mode slider-value)
  (let ((step (seqv-slot-step-index-value track-id slot)))
    (if (>= step 0)
      (seqv-set-expanded-step-param track track-id step mode slider-value)
      nil)))

(def seqv-set-expanded-slot-sound (track track-id slot sound-index)
  (seqv-set-expanded-slot-param
    track track-id slot 3
    (seqv-drum-sound-transpose-at-index track sound-index)))

(def seqv-set-expanded-step-param (track track-id step mode slider-value)
  (do
    (seqv-activate-track-for-edit track)
    (cool-off-follow)
    (seqv-set-expanded-cursor track track-id step)
    (if (seqv-process-lane-mode? mode)
      (seq-set-process-lane-from-step
        track
        mode
        step
        (seqv-track-step-slider-param-value track mode slider-value))
      (seq-set-step-param-from-step
        step
        (seqv-param-keyword mode)
        (seqv-step-slider-param-value mode slider-value)))))

(def seqv-set-expanded-current-param (track track-id mode value)
  (do
    (seqv-activate-track-for-edit track)
    (cool-off-follow)
    (set-track-cursor-step (seqv-current-step track track-id))
    (if (seqv-process-lane-mode? mode)
      (seq-set-process-lane-from-step
        track
        mode
        (seqv-current-step track track-id)
        (seqv-track-step-param-value track mode value))
      (seq-set-step-param-from-step
        (seqv-current-step track track-id)
        (seqv-param-keyword mode)
        (seqv-step-param-value mode value)))))

(def seqv-set-expanded-current-sound (track track-id label)
  (seqv-set-expanded-current-param
    track track-id 3
    (seqv-drum-sound-transpose-for-label track label)))

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
      (set-track-cursor-step step))))

(def seqv-double-track-pattern (track track-id)
  (do
    (seqv-activate-track-for-edit track)
    (cool-off-follow)
    (seq-double-track-pattern)
    (seqv-sync-all-track-cursors-to-global)))

(def seqv-halve-track-pattern (track track-id)
  (do
    (seqv-activate-track-for-edit track)
    (cool-off-follow)
    (seq-halve-track-pattern)
    (seqv-sync-all-track-cursors-to-global)))

(def seqv-param-tab-width (mode)
  7.8)

(def seqv-clip-label (text max-chars)
  (let ((s (str text)))
    (if (> (len s) max-chars)
      (str (substring s 0 (- max-chars 2)) "..")
      s)))

(def seqv-param-header-name (track mode)
  (if (seqv-process-lane-mode? mode)
    (seqv-clip-label (seqv-track-param-name track mode) 28)
    (if (seqv-drum-sound-mode? track mode) "Sound" (seqv-param-name mode))))

(def seqv-param-header-width (mode)
  (if (seqv-process-lane-mode? mode) 17.8 6.4))

(def seqv-param-tab (track track-id mode tab-label)
  (box :width (seqv-param-tab-width mode) :height 2
    :key (str "seqv-expanded-param-tab-" track-id "-" mode)
    :bg (if (= (seqv-param-mode track-id) mode) (seqv-param-color mode) :dark-gray)
    :on-click |x y r| (do (seqv-activate-track-for-edit track) (seqv-set-param-mode track-id mode))
    (label tab-label :font-size 12
      :color (if (= (seqv-param-mode track-id) mode) :primary :dim)
      :bg :transparent)))

(def seqv-process-lane-option-label (track lane-idx)
  (let ((lane (nth (seqv-track-process-lanes track) lane-idx)))
    (str (+ lane-idx 1) " " (get lane :short-label))))

(def seqv-process-lane-options (track)
  (append
    (list "none")
    (map
      (lambda (lane-idx) (seqv-process-lane-option-label track lane-idx))
      (range 0 (len (seqv-track-process-lanes track))))))

(def seqv-process-lane-selector-value (track mode)
  (if (seqv-process-lane-mode? mode)
    (let ((lane-idx (seqv-process-lane-index mode)))
      (if (and (>= lane-idx 0) (< lane-idx (len (seqv-track-process-lanes track))))
        (seqv-process-lane-option-label track lane-idx)
        "none"))
    "none"))

(def seqv-process-lane-selector-index (track label)
  (if (= label "none")
    -1
    (reduce |acc lane-idx|
      (if (= label (seqv-process-lane-option-label track lane-idx)) lane-idx acc)
      -1
      (range 0 (len (seqv-track-process-lanes track))))))

(def seqv-selected-process-lane (track mode)
  (if (seqv-process-lane-mode? mode)
    (let ((lane-idx (seqv-process-lane-index mode)))
      (if (and (>= lane-idx 0) (< lane-idx (len (seqv-track-process-lanes track))))
        (nth (seqv-track-process-lanes track) lane-idx)
        nil))
    nil))

(def seqv-select-process-lane-option (track track-id label)
  (let ((lane-idx (seqv-process-lane-selector-index track label)))
    (do
      (seqv-activate-track-for-edit track)
      (if (>= lane-idx 0)
        (seqv-set-param-mode track-id (+ seqv-process-lane-mode-offset lane-idx))
        (if (seqv-process-lane-mode? (seqv-param-mode track-id))
          (seqv-set-param-mode track-id 3)
          nil)))))

(def seqv-process-lane-selector (track track-id mode)
  (dropdown
    :value (seqv-process-lane-selector-value track mode)
    :key (str "seqv-expanded-process-lane-selector-" track-id)
    :options (seqv-process-lane-options track)
    :on-change (lambda (v) (seqv-select-process-lane-option track track-id v))
    :width 14.8 :height 1.45 :font-size 10))

(def seqv-expanded-track-quick-controls (track track-id)
  (let ((mode (seqv-param-mode track-id)))
    (v-stack 
      (box :height 0.4 :width 1)
      (h-stack :gap 0.55 :align :center
        (box :width (seqv-param-header-width mode) :height 1.3
          :key (str "seqv-expanded-step-summary-" track-id)
          (label (seqv-param-header-name track mode)
            :font-size 11 :width (seqv-param-header-width mode) :color :white :bg :transparent))
        (if (= mode 5)
          (box :width 8 :height 1.3
            :key (str "seqv-expanded-sync-label-" track-id)
            (label (seqv-expanded-sync-current-label track track-id)
              :font-size 11 :color :white :bg :transparent))
          (if (seqv-drum-sound-mode? track mode)
            (if (> (seqv-drum-sound-count track) 0)
              (dropdown :key (str "seqv-expanded-sound-picker-" track-id)
                :value (seqv-drum-sound-label-for-transpose track
                  (seqv-param-value-at track mode (seqv-current-step track track-id)))
                :options (seqv-drum-sound-labels track)
                :on-change (lambda (label) (seqv-set-expanded-current-sound track track-id label))
                :width 13 :height 1.3 :font-size 9)
              (box :width 13 :height 1.3
                :key (str "seqv-expanded-no-sounds-" track-id)
                (label "No drum pads" :font-size 9 :color :dim :bg :transparent)))
            (number-picker :key (str "seqv-expanded-param-number-picker-" track-id)
              :border-color :white
              :value (seqv-param-value-at track mode (seqv-current-step track track-id))
              :min (seqv-track-param-min track mode) :max (seqv-track-param-max track mode) :decimals (seqv-track-param-decimals track mode)
              :on-change (lambda (v) (seqv-set-expanded-current-param track track-id mode v))
              :width 8 :height 1.3 :font-size 11)))
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
                  :active (seqv-page-active-binding track-id page)
                  :style pattern-control-style
                  :on-click |x y r| (seqv-goto-page track track-id page)
                  (v-stack :align :center
                    (label (fmt " {} " (+ page 1))
                      :font-size 11
                      :active (seqv-page-active-binding track-id page)
                      :active-color :white
                      :color :dim
                      :bg :transparent)))))))))))

(def seqv-expanded-track-editor (track track-id)
  (let ((mode (seqv-param-mode track-id))
        (sound-mode (seqv-drum-sound-mode? track (seqv-param-mode track-id)))
        (sound-count (seqv-drum-sound-count track)))
    (box :padding 0.25
      (box 
        :background-color (rgba 0.1 0.1 0.1 0.2) :corner-radius 8
        (v-stack :width :fill :padding 0.35 :gap 0.1
          (h-stack :gap 0.5
            (box :width 1)
            (seqv-param-tab track track-id 0 "vel")
            (seqv-param-tab track track-id 1 "dur")
            (seqv-param-tab track track-id 3 (if (seqv-track-drum-rack? track) "sound" "tpose"))
            (seqv-param-tab track track-id 4 "pan")
            (seqv-param-tab track track-id 5 "sync")
            (seqv-param-tab track track-id 6 "delay")
            (seqv-process-lane-selector track track-id mode)
            (h-stack :align :center :gap 0.35
              (dropdown :value (seqv-track-timebase track)
                :key (str "seqv-expanded-timebase-" track-id)
                :options seqv-timebase-options
                :on-change (lambda (v) (seqv-set-expanded-timebase track v))
                :width 6 :height 1.45 :font-size 10)))
          
          (grid
            :cols 16
            :col-width 4
            :row-height seqv-expanded-step-row-height
            :align :stretch
            (each (range 0 page-size) |i|
              (box :padding seqv-expanded-step-column-padding
                :key (str "seqv-expanded-step-column-" track-id "-" i)
                :background "cursor-highlight"
                :active (seqv-slot-cursor-binding track-id i)
                :selected (seqv-track-selected-binding track)
                :on-click (lambda (evt)
                  (seqv-expanded-slot-click track track-id i evt))
                :on-drag (lambda (evt)
                  (seqv-expanded-slot-drag track track-id i evt))
                (v-stack :align :center :gap seqv-expanded-step-column-gap
                  (let ((active-ref (seqv-slot-active-binding track-id i))
                      (plocked-ref (seqv-slot-plocked-binding track-id i))
                      (selected-ref (seqv-slot-selected-binding track-id i))
                      (track-r (seqv-expanded-track-color-r track))
                      (track-g (seqv-expanded-track-color-g track))
                      (track-b (seqv-expanded-track-color-b track)))
                    (list
                      (if sound-mode
                        (if (> sound-count 0)
                          (vslider :height seqv-expanded-step-slider-height
                            :key (str "seqv-expanded-step-sound-slider-" track-id "-" i)
                            :width 1
                            :min 0 :max (- sound-count 1)
                            :origin 0
                            :value (seqv-drum-sound-index-for-transpose track
                              (reactive-value (seqv-slot-param-slider-binding track-id mode i)))
                            :haptic-value (seqv-drum-sound-index-for-transpose track
                              (reactive-value (seqv-slot-param-haptic-binding track-id mode i)))
                            :haptic-min 0 :haptic-max (- sound-count 1)
                            :haptic-pivot-position 1 :haptic-pivot-value (- sound-count 1)
                            :haptic-exponent 1
                            :items (seqv-drum-sound-short-labels track)
                            :font-size 11
                            :color :white
                            :fill (seqv-expanded-slider-fill track)
                            :dot-color :dark-gray
                            :active active-ref
                            :track-r track-r
                            :track-g track-g
                            :track-b track-b
                            :material (seqv-aqua-slider-track-material)
                            :on-change (lambda (v)
                              (seqv-set-expanded-slot-sound track track-id i v)))
                          (box :height seqv-expanded-step-slider-height :width 3
                            :key (str "seqv-expanded-step-no-sound-" track-id "-" i)
                            (label "No pad" :font-size 8 :color :dim :bg :transparent)))
                        (vslider :height seqv-expanded-step-slider-height
                          :key (str "seqv-expanded-step-slider-" track-id "-" i)
                          :width (if (= mode 5) 2 1)
                          :min (seqv-track-param-slider-min track mode) :max (seqv-track-param-slider-max track mode)
                          :origin (seqv-track-param-origin track mode)
                          :value (seqv-slot-param-slider-binding track-id mode i)
                          :haptic-value (seqv-slot-param-haptic-binding track-id mode i)
                          :haptic-min (seqv-track-param-min track mode)
                          :haptic-max (seqv-track-param-max track mode)
                          :haptic-pivot-position (seqv-param-haptic-pivot-position mode)
                          :haptic-pivot-value (seqv-track-param-haptic-pivot-value track mode)
                          :haptic-exponent (seqv-param-haptic-exponent mode)
                          :items (if (= mode 5) SEQ.sync-labels '())
                          :font-size 11
                          :color :white
                          :fill (seqv-expanded-slider-fill track)
                          :dot-color :dark-gray
                          :active active-ref
                          :track-r track-r
                          :track-g track-g
                          :track-b track-b
                          :material (seqv-aqua-slider-track-material)
                          :on-change (lambda (v)
                            (seqv-set-expanded-slot-param track track-id i mode v))))
                      (box
                        :key (str "seqv-expanded-step-toggle-" track-id "-" i)
                        :active active-ref
                        :plocked plocked-ref
                        :selected selected-ref
                        :background "aqua-button"
                        :align :center :width 3 :height seqv-expanded-step-toggle-height
                        :on-mouse-down (lambda (evt)
                          (seqv-expanded-slot-pointer-down track track-id i evt))
                        :on-drag (lambda (evt)
                          (seqv-expanded-slot-drag track track-id i evt))
                        :on-mouse-up (lambda (evt)
                          (seqv-expanded-slot-pointer-up track track-id i evt))
                        :on-double-click (lambda (evt)
                          (seqv-expanded-slot-double-click track track-id i evt))
                        (metal-track-tick
                          :active active-ref
                          :plocked plocked-ref
                          :selected selected-ref
                          :track-r track-r
                          :track-g track-g
                          :track-b track-b))))
                  (number-label
                    :key (str "seqv-expanded-step-label-" track-id "-" i)
                    :value (seqv-slot-label-binding track-id i)
                    :active (seqv-slot-selected-binding track-id i)
                    :active-color :yellow
                    :decimals 0
                    :width 2.8
                    :height seqv-expanded-step-label-height
                    :h-align :center
                    :font-size 10 :bg :transparent
                    :color :dim)
                  (subtree :key (str "seqv-expanded-step-playhead-probe-" track-id "-" i)
                    (step-playhead-dot
                      :active (seqv-slot-playhead-binding track-id i)))))))))
    )
  )
)

(def seqv-track-grid (track-idx)
  (let ((num-steps (nth SEQ.track-num-steps track-idx))
      (rows (max 1 (floor (/ (+ num-steps (- sequencer-row-width 1)) sequencer-row-width)))))
    (box :padding 0.15
      (box :background-color :buffer-bg
        (v-stack :gap -0.04
          (box :width 0.1 :height 0.342 :bg :transparent)
          (each (range 0 rows) |row|
            (v-stack :gap -0.16
              (h-stack :gap 0.0
                (each (range 0 sequencer-row-width) |col|
                  (let ((step (+ (* row sequencer-row-width) col)))
                    (seqv-step-cell
                      track-idx
                      step
                      (< step num-steps)))))
              (seqv-playhead-row track-idx (nth SEQ.track-ids track-idx) row)))))))
  )

(def seqv-drum-slot-name-max-chars 14)

(def seqv-drum-slot-name-display (name)
  (if (> (len name) seqv-drum-slot-name-max-chars)
    (str (substring name 0 (- seqv-drum-slot-name-max-chars 2)) "..")
    name))

;; Slot gain spans 0..2 (unity at 1), so the meter shows gain/2 and drags map
;; the widget's -1..1 sx straight onto the gain range.
(def seqv-drum-slot-gain-max 2.0)

(def seqv-drum-slot-set-gain-from-event (track-idx sound event)
  (let ((sx (get event :sx)))
    (if (= sx nil)
      nil
      (host-command "set-rack-slot-gain"
        (dict :track track-idx
          :slot (get sound :slot-idx)
          :value (max 0.0 (min seqv-drum-slot-gain-max (+ sx 1.0))))))))

;; bind-seq is a float binding: Bool publishes arrive as 1.0/0.0 and `not`
;; doesn't negate numbers, so compare against 0.5 to get a real boolean.
(def seqv-drum-slot-flag (sound field-key)
  (let ((field (get sound field-key)))
    (if field (> (reactive-value (bind-seq field)) 0.5) false)))

(def seqv-drum-slot-selected-binding (sound)
  (bind-seq (get sound :selected-field)))

(def seqv-select-drum-slot-index (track-idx slot-idx)
  (do
    (seqv-select-track-for-edit track-idx)
    (host-command "select-rack-slot" (dict :track track-idx :slot slot-idx))))

(def seqv-select-drum-slot (track-idx sound)
  (seqv-select-drum-slot-index track-idx (get sound :slot-idx)))

(def seqv-drum-slot-toggle (label-text track-idx sound param command active)
  (button label-text
    :key (str "seqv-drum-slot-" param "-" track-idx "-" (get sound :slot-idx))
    
    :width 1.55 :height 1.2    
    :padding 0 :font-size 10
    ;:border-color :transparent
    :background-color (if active (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
    :color (if active :black :dim)
    :on-click |x y r| (do
      (seqv-select-drum-slot track-idx sound)
      (host-command command
        (dict :track track-idx :slot (get sound :slot-idx) :value (not active))))))

;; A drum pad's note is its identity in the sequencer, just as a regular
;; track's number is its identity. Its enabled state means the pad is audible,
;; so the visual state intentionally inverts the underlying mute flag.
(def seqv-drum-slot-mute-button (track-idx sound muted)
  (button (get sound :pad-label)
    :key (str "seqv-drum-slot-mute-" track-idx "-" (get sound :slot-idx))
    :width 1.9 :height 1.2
    :padding 0 :font-size 10
    :background-color (seqv-mute-bg muted)
    :color (if muted :gray :black)
    :on-click |x y r| (do
      (seqv-select-drum-slot track-idx sound)
      (host-command "set-rack-slot-mute"
        (dict :track track-idx :slot (get sound :slot-idx) :value (not muted))))))

(def seqv-drum-slot-volume-control (track-idx sound)
  (let ((gain-field (get sound :gain-field))
      (peak-field (get sound :peak-field))
      (gain (if gain-field (reactive-value (bind-seq gain-field)) 1.0)))
    (box
      :key (str "seqv-drum-slot-volume-" track-idx "-" (get sound :slot-idx))
      :width 8.2 :height 1.1
      :background "seqv-track-volume-meter"
      :level (if peak-field (bind-seq peak-field) 0)
      :volume (* 0.5 gain)
      :track-r (seqv-track-color-r-binding track-idx)
      :track-g (seqv-track-color-g-binding track-idx)
      :track-b (seqv-track-color-b-binding track-idx)
      :on-click (lambda (event) (do
        (seqv-select-drum-slot track-idx sound)
        (seqv-drum-slot-set-gain-from-event track-idx sound event)))
      :on-drag (lambda (event) (do
        (seqv-select-drum-slot track-idx sound)
        (seqv-drum-slot-set-gain-from-event track-idx sound event))))))

(def seqv-drum-track-grid (track-idx)
  (let ((num-steps (nth SEQ.track-num-steps track-idx))
      (rows (max 1 (floor (/ (+ num-steps (- sequencer-row-width 1)) sequencer-row-width))))
      (sounds (seqv-track-drum-sounds track-idx)))
    (box :padding 0.15
      (box :background-color :buffer-bg
        (v-stack :gap 0.5
          (each sounds |sound|
            (let ((pad-note (get sound :transpose))
                (muted (seqv-drum-slot-flag sound :mute-field))
                (soloed (seqv-drum-slot-flag sound :solo-field)))
              (box
                :key (str "seqv-drum-lane-" track-idx "-" pad-note)
                :debug-name "seqv-drum-slot-lane"
                :selected (seqv-drum-slot-selected-binding sound)
                :background-color :transparent
                :selected-background-color (rgba 0.12 0.17 0.20 0.55)
                :border-width 1
                :border-color :transparent
                :selected-border-color (rgba 0.30 0.74 0.88 0.62)
                :on-click |x y r| (seqv-select-drum-slot track-idx sound)
                (h-stack
                  :padding 0.5
                  :gap 0.45
                  :align :top
                ;; Per-slot gutter mirrors the track-header order: note/mute, solo,
                ;; name, volume. The note control is bright while its pad is audible.
                (box :width 2.65)
                (h-stack :align :baseline :gap 0.45
                  (seqv-drum-slot-mute-button track-idx sound muted)
                  (seqv-drum-slot-toggle "S" track-idx sound "solo" "set-rack-slot-solo" soloed)
                  (label
                    (seqv-drum-slot-name-display (get sound :name))
                    :key (str "seqv-drum-lane-label-" track-idx "-" pad-note)
                    :debug-name "seqv-drum-slot-label"
                    :width 8.6
                    ;:height 1.55
                    :bg :transparent
                    :font-size 10
                    :h-align :left
                    :color (if muted :dark-gray :dim))
                  (seqv-drum-slot-volume-control track-idx sound)
                  (box :width 0.3))
                (v-stack :gap -0.04
                  (each (range 0 rows) |row|
                    (v-stack :gap -0.16
                      (h-stack :gap 0.0
                        (each (range 0 sequencer-row-width) |col|
                          (let ((step (+ (* row sequencer-row-width) col)))
                            (seqv-drum-lane-step-cell
                              track-idx
                              (get sound :slot-idx)
                              pad-note
                              step
                              (< step num-steps)))))
                      (seqv-playhead-row
                        track-idx
                        (nth SEQ.track-ids track-idx)
                        row))))))))))))
  )

(effect-buffer "*sequencer*"
  (v-stack :padding 0.00 :gap 0.0
    (each (seq-visible-track-indices) |i|
      (subtree :key (str "sequencer-track-" (nth SEQ.track-ids i))
        ;; :muted is a binding (not a value read) so mute/solo changes update
        ;; the row chrome without rerunning this subtree.
        (box :width :fill
          :key (str "sequencer-track-drop-" i)
          :selected (seqv-track-selected-binding i)
          :muted (bind-seq-nth "track-muted-effective" i)
          :background-color :buffer-bg
          :selected-background-color :mixer-strip-selected-bg
          :muted-background-color :mixer-strip-muted-bg
          :border-width 2
          :corner-radius 10
          :border-color :mixer-strip-border
          :selected-border-color :mixer-strip-selected-border
          :muted-border-color :mixer-strip-border
          :drop-hover-border-color :mixer-strip-selected-border
          :drop-types (if (seq-track-replaceable-instrument? i)
            (list "sample" "instrument" "sound")
            (if (seq-track-sound-replaceable? i) (list "sample" "sound") (list "sample")))
          :drop-meta (dict :kind "track" :track i)
          :on-drop (lambda (event) (seqv-drop-on-track event))
          :padding 0.0145
          :on-click |x y r| (seqv-select-track-for-edit i)
          (if (seqv-track-expanded? (nth SEQ.track-ids i))
            (v-stack 
              :width :fill :gap 0.2
              (h-stack :width :fill :gap 0.6 :align :start
                (seqv-track-header i)
                (seqv-expanded-track-quick-controls i (nth SEQ.track-ids i))
                (box :flex 1 :width 0 :height 0.1 :bg :transparent)
                (seqv-track-actions i))
              (seqv-expanded-track-editor i (nth SEQ.track-ids i)))
            (if (seqv-track-drum-rack? i)
              ;; Drum racks put the rack header on its own row so the slot
              ;; lanes get the full width for their per-slot gutters.
              (v-stack :width :fill :gap 0.2
                (h-stack :width :fill :gap 0.6 :align :start
                  (seqv-track-header i)
                  (box :flex 1 :width 0 :height 0.1 :bg :transparent)
                  (seqv-track-actions i))
                (seqv-drum-track-grid i))
              (h-stack :width :fill :gap 0.6 :align :start
                (seqv-track-header i)
                (seqv-track-grid i)
                (box :flex 1 :width 0 :height 0.1 :bg :transparent)
                (seqv-track-actions i)))))))
    (box :key "sequencer-new-track-drop-zone"
      :width :fill :height 2.4 :flex 1
      :background-color :transparent
      :drop-hover-background-color :mixer-control-bg
      :border-width 1
      :border-color :transparent
      :drop-hover-border-color :mixer-strip-selected-border
      :corner-radius 10
      :drop-types (list "sample" "instrument" "sound")
      :drop-meta (dict :kind "new-sample-track")
      :on-drop (lambda (event) (seqv-drop-new-track event))
      (label ""
        :font-size 1
        :color :transparent
        :bg :transparent))))

(set-buffer-mode-for "*sequencer*" "seq-grid-mode")

(sequencer-cursor-step-changed SEQ.current-track cursor-step)
