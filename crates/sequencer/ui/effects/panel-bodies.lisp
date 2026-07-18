;; Instrument, MIDI FX, and audio FX panel body selection.
(def instrument-key-note-names '("C" "C#" "D" "D#" "E" "F" "F#" "G" "G#" "A" "A#" "B"))
(def instrument-key-count 12)
(def instrument-key-panel-padding 0.35)
(def instrument-key-white-width 2.9)
(def instrument-key-black-width 1.85)
(def instrument-key-white-height 3.25)
(def instrument-key-black-height 2.45)
(def instrument-key-strip-height 0.34)
(def instrument-key-activity-height 0.42)
(def instrument-key-button-gap 0.08)
(def instrument-key-row-width
  (+ (* 7 instrument-key-white-width)
     (* 5 instrument-key-black-width)
     (* (- instrument-key-count 1) instrument-key-button-gap)))
(def instrument-key-panel-width (+ instrument-key-row-width (* 2 instrument-key-panel-padding) 0.1))
(def instrument-key-panel-outer-width (+ instrument-key-panel-width 4))

(def instrument-key-note-number (idx)
  (+ (* (+ instrument-key-lock-octave 1) 12) idx))

(def instrument-key-note-name (idx)
  (nth instrument-key-note-names idx))

(def instrument-key-black? (idx)
  (or (= idx 1) (= idx 3) (= idx 6) (= idx 8) (= idx 10)))

(def instrument-key-width (idx)
  (if (instrument-key-black? idx) instrument-key-black-width instrument-key-white-width))

(def instrument-key-height (idx)
  (if (instrument-key-black? idx) instrument-key-black-height instrument-key-white-height))

(def instrument-key-label-height (idx)
  (- (instrument-key-height idx) instrument-key-strip-height instrument-key-activity-height))

(def instrument-key-note-selected? (note)
  (fx-list-contains? instrument-key-lock-selected-notes note))

(def instrument-key-note-active? (note)
  (fx-list-contains? SEQ.instrument-active-notes note))

(def instrument-key-param-has-lock? (p note)
  (if (instrument-param-key-lock-row p note) true false))

(def instrument-key-note-has-lock? (inst note)
  (> (len (filter |p| (instrument-key-param-has-lock? p note) (get inst :synth))) 0))

(def instrument-key-note-variant-row (inst note)
  (nth
    (filter |row| (= (get row :note) note)
      (if (get inst :key-lock-note-variants) (get inst :key-lock-note-variants) '()))
    0))

(def instrument-key-note-variant-color (row alpha)
  (rgba (get row :color-r) (get row :color-g) (get row :color-b) alpha))

(def instrument-key-strip-color (inst note variant-row)
  (if variant-row
    (instrument-key-note-variant-color variant-row 1.0)
    (if (instrument-key-note-has-lock? inst note)
      (rgba 0.72 0.72 0.76 0.92)
      :transparent)))

(def instrument-key-base-color (idx selected)
  (if (instrument-key-black? idx)
    (if selected (rgba 0.10 0.10 0.11 1.0) (rgba 0.135 0.135 0.14 0.8))
    (if selected (rgba 1.0 0.94 0.70 1.0) (rgba 1.0 1.0 1.0 0.9))))

(def instrument-key-text-color (idx selected)
  (if (instrument-key-black? idx)
    (if selected :yellow :white)
    :black))

(def instrument-key-border-color (inst idx note variant-row selected)
  (if selected
    (rgba 0.95 0.74 0.22 1.0)
    (if variant-row
      (instrument-key-note-variant-color variant-row 0.85)
      (if (instrument-key-black? idx)
        (rgba 1 1 1 0.13)
        (if (instrument-key-note-has-lock? inst note)
          (rgba 0.46 0.46 0.50 0.9)
          (rgba 0.0 0.0 0.0 0.55))))))

(def instrument-key-lock-variant-items (inst)
  (if (get inst :key-lock-variants) (get inst :key-lock-variants) '()))

(def instrument-key-lock-chip-color (chip alpha)
  (rgba (get chip :color-r) (get chip :color-g) (get chip :color-b) alpha))

(def instrument-key-lock-chip-label (chip)
  (if (get chip :display)
    (substring (get chip :display) 0 6)
    (get chip :label)))

(def instrument-key-lock-chip-note-matches? (inst chip note)
  (let ((row (instrument-key-note-variant-row inst note))
        (def-chip (= (get chip :kind) "def")))
    (if def-chip
      (if row false true)
      (if row (= (get row :label) (get chip :label)) false))))

(def instrument-key-lock-chip-current? (inst chip)
  (if (instrument-key-lock-has-selection?)
    (= (len (filter |note| (instrument-key-lock-chip-note-matches? inst chip note)
              instrument-key-lock-selected-notes))
       (len instrument-key-lock-selected-notes))
    (= (get chip :kind) "def")))

(def instrument-key-lock-chip-click (chip)
  (do
    (cool-off-follow)
    (host-command "stamp-key-lock-variant"
      (dict :label (get chip :label)
            :notes instrument-key-lock-selected-notes))))

(def instrument-key-lock-chip (inst chip)
  (let ((current (instrument-key-lock-chip-current? inst chip))
      (def-chip (= (get chip :kind) "def"))
      (c (instrument-key-lock-chip-color chip 1.0)))
    (box :key (str "instrument-key-lock-chip-" (get chip :kind) "-" (get chip :label))
      :height 1.08
      :align :baseline
      :padding 0.12
      :background-color (if current
        (instrument-key-lock-chip-color chip 0.12)
        (rgba 1 1 1 0.025))
      :border-width (if current 0.75 0.35)
      :border-color (if current c (rgba 1 1 1 0.10))
      :corner-radius 5
      :on-click |x y r| (instrument-key-lock-chip-click chip)
      (h-stack :gap 0.14 :align :baseline
        (box :width 0.18 :height 0.64
          :corner-radius 2
          :background-color (if def-chip :transparent c)
          :border-width (if def-chip 1 0)
          :border-color c)
        (label (instrument-key-lock-chip-label chip)
          :font-size 8.4 :color (if current :white :dim) :bg :transparent)
        (label (str (get chip :count))
          :font-size 8.4 :color (if current :white :dark-gray) :bg :transparent)))))

(def instrument-key-select-note (note)
  (let ((already-selected (instrument-key-note-selected? note))
        (next
          (if (instrument-key-note-selected? note)
            (filter |selected| (not (= selected note)) instrument-key-lock-selected-notes)
            (append instrument-key-lock-selected-notes (list note)))))
    (do
      (set! instrument-key-lock-selected-notes next)
      (if (and instrument-key-lock-audition (not already-selected))
        (host-command "audition-instrument-key" (dict :note note))
        false))))

(def instrument-key-clear-selected ()
  (if (> (len instrument-key-lock-selected-notes) 0)
    (host-command "stamp-key-lock-variant"
      (dict :label "def" :notes instrument-key-lock-selected-notes))
    false))

(def instrument-key-button (inst idx)
  (let ((note (instrument-key-note-number idx))
        (name (instrument-key-note-name idx))
        (variant-row (instrument-key-note-variant-row inst note))
        (selected (instrument-key-note-selected? note)))
    (box
      :key (str "instrument-key-" note)
      :width (instrument-key-width idx)
      :height (instrument-key-height idx)
      :padding 0
      :background-color (instrument-key-base-color idx selected)
      :border-width 1
      :border-color (instrument-key-border-color inst idx note variant-row selected)
      :corner-radius 4
      :on-click |x y r| (instrument-key-select-note note)
      (v-stack :width :fill :height :fill :gap 0 :align :center
        (label name
          :width :fill
          :height (instrument-key-label-height idx)
          :font-size (if (instrument-key-black? idx) 9.0 10.4)
          :h-align :center
          :color (instrument-key-text-color idx selected)
          :bg :transparent)
        (box
          :key (str "instrument-key-activity-row-" note)
          :debug-name (str "instrument-key-activity-row-" note)
          :width :fill
          :height instrument-key-activity-height
          :padding 0
          :h-align :center
          :v-align :center
          :background-color :transparent
          (if (instrument-key-note-active? note)
            (label "●"
              :key (str "instrument-key-activity-" note)
              :debug-name (str "instrument-key-activity-" note)
              :width 0.6
              :height instrument-key-activity-height
              :font-size 6.0
              :h-align :center
              :bg :transparent
              :color (rgba 1.0 0.72 0.10 1.0))
            (box
              :key (str "instrument-key-activity-" note)
              :debug-name (str "instrument-key-activity-" note)
              :width 0.6
              :height instrument-key-activity-height
              :background-color :transparent)))
        (box
          :key (str "instrument-key-strip-" note)
          :width :fill
          :height instrument-key-strip-height
          :background-color (instrument-key-strip-color inst note variant-row)
          :corner-radius 1)))))

(def instrument-key-lock-control-panel (inst)
  (box :width instrument-key-panel-outer-width :background-color :black :corner-radius 16 :padding 1
    (v-stack  :debug-name "instrument-key-lock-control-panel" :width instrument-key-panel-width :height fx-panel-body-content-height :gap 0.35 :padding instrument-key-panel-padding
      (h-stack :gap 0.35 :height 1.2 :align :baseline
        (button "<" :width 2 :height 1.1 :padding 0 :font-size 10
          :on-click |x y r| (set! instrument-key-lock-octave (max -1 (- instrument-key-lock-octave 1))))
        (label (str "OCT " instrument-key-lock-octave) :font-size 10 :width 5.2 :color :dim :bg :transparent)
        (button ">" :width 2 :height 1.1 :padding 0 :font-size 10
          :on-click |x y r| (set! instrument-key-lock-octave (min 8 (+ instrument-key-lock-octave 1))))
        (button "audition" :width 8 :height 1.1 :padding 0 :font-size 10
          :background-color (if instrument-key-lock-audition
            (rgba 0.95 0.74 0.22 1.0)
            (rgba 0.12 0.12 0.13 0.70))
          :color (if instrument-key-lock-audition :black :dim)
          :border-color (rgba 0.0 0.0 0.0 0.65)
          :on-click |x y r| (set! instrument-key-lock-audition (not instrument-key-lock-audition))))
      (box :padding 0.5 :background-color :gray :corner-radius 12
      (h-stack :debug-name "instrument-key-row" :gap instrument-key-button-gap :height instrument-key-white-height :width instrument-key-row-width :align :start
        (each (range instrument-key-count) |idx|
          (instrument-key-button inst idx))))
      (wrap :key "instrument-key-lock-variant-strip"
        :width :fill :gap 0.18 :row-gap 0.14 :align :start
        (each (instrument-key-lock-variant-items inst) |chip idx|
          (instrument-key-lock-chip inst chip)))
      (button "clear key" :width 6.5 :height 1.1 :padding 0 :font-size 10
        :background-color (rgba 0.12 0.12 0.13 0.70)
        :border-color (rgba 0.0 0.0 0.0 0.65)
        :on-click |x y r| (instrument-key-clear-selected)))))

(def instrument-synth-panel-body (inst)
  (do
    (set! custom-ui-current-kind "instrument")
    (let ((custom (custom-instrument-synth-ui inst)))
      (let ((body
              (if custom
                (box custom
                  :debug-name "custom-synth-wrapper" :padding 0
                  :h-align :start :v-align :stretch)
                (box (fx-param-grid (get inst :synth) false)
                  :debug-name "fallback-synth-wrapper"))))
        (if (= instrument-panel-tab 1)
          (h-stack :debug-name "instrument-keys-inline-body" :height :fill :gap 0.45 :align :stretch
            (instrument-key-lock-control-panel inst)
            body)
          (if instrument-mods-open
          (h-stack :debug-name "instrument-mods-inline-body" :height :fill :gap 0.45 :align :stretch
            (instrument-mod-control-panel inst)
            body)
          body))))))

(def midi-fx-panel-body (fx)
  (let ((custom (custom-midi-fx-ui fx)))
    (if custom
      (box
        (v-stack :gap 0.25 custom)
        :debug-name "custom-midi-fx-wrapper" :padding 0 :h-align :start :v-align :start)
      (box (fx-param-grid (get fx :params) fx)
        :debug-name "fallback-midi-fx-wrapper"))))

(def audio-fx-panel-body (fx params)
  (let ((builtin-ui (builtin-audio-fx-ui fx)))
    (let ((body
            (if builtin-ui
              builtin-ui
              (do
                (set! custom-ui-current-kind "audio-fx")
                (let ((custom (custom-audio-fx-ui fx)))
                  (if custom
                    (box
                      (v-stack :gap 0.25 custom)
                      :debug-name "custom-audio-fx-wrapper" :padding 0 :h-align :start :v-align :start)
                    (fx-param-grid params fx)))))))
      (if (effect-mods-active? fx)
        (h-stack :debug-name "effect-mods-inline-body" :height fx-panel-body-content-height :gap 0.45 :align :stretch
          (effect-mod-control-panel fx)
          body)
        body))))

(def fx-panel-selected? (fx)
  (do
    SEQ.delete-target-version
    (if (get fx :rack-fx)
      (seq-delete-target? :fx-effect
        (dict :chain "rack"
              :track (get fx :track-idx)
              :rack-slot (get fx :rack-slot)
              :effect-slot (get fx :slot-idx)))
      (if (get fx :midi-fx)
      (seq-delete-target? :fx-effect (dict :chain "midi" :slot (get fx :slot-idx)))
      (if (get fx :bus-fx)
        (seq-delete-target? :fx-effect
          (dict :chain "bus" :bus (get fx :bus-idx) :slot (get fx :slot-idx)))
        (seq-delete-target? :fx-effect (dict :chain "audio" :slot (get fx :slot-idx))))))))

(def fx-panel-header-bg (selected)
  (if selected :fx-panel-header-selected-bg :fx-panel-header-bg))
