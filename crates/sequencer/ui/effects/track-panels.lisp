;; Track-level parameter, accumulator, and parameter-lock panels.
(def fx-track-bus-send-field (bus)
  (str "tp-bus-" bus "-send"))

(def fx-mute-group-value (label)
  (if (= label "1") 1
    (if (= label "2") 2
      (if (= label "3") 3
        (if (= label "4") 4
          (if (= label "5") 5
            (if (= label "6") 6
              (if (= label "7") 7
                (if (= label "8") 8
                  0)))))))))

(def fx-set-timebase (label)
  (do
    (cool-off-follow)
    (if (seq-has-selection?)
      (seq-plock-timebase label)
      (seq-set-timebase label))))

(def fx-track-param-plock-row (target)
  (nth (filter |row| (= (get row :target) target) SEQ.track-plocks) 0))

(def fx-track-param-plock-active? (target)
  (if (fx-track-param-plock-row target) true false))

(def fx-track-param-plock-default (target fallback)
  (let ((row (fx-track-param-plock-row target)))
    (if row (get row :default) fallback)))

(def fx-track-bus-send-control (send)
  (v-stack :align :center :gap 0.25
    (h-stack :gap 0.25 :align :baseline
      (label (substring (get send :name) 0 8) :font-size 9 :color :dim :bg :transparent)
      (number-picker
        :value (bind-seq (fx-track-bus-send-field (get send :bus-idx)))
        :min 0 :max 1 :decimals 2
        :noui true :font-size 9 :text-color :dim
        :on-change (lambda (v)
          (do
            (cool-off-follow)
            (host-command "set-track-bus-send"
              (dict :bus (get send :bus-idx) :amount v))))
        :width 4 :height 1))
    (box :width 8 :height 2
      (hslider :min 0 :max 1
        :value (bind-seq (fx-track-bus-send-field (get send :bus-idx)))
        :material (aqua-slider-material)
        :on-change (lambda (v)
          (do
            (cool-off-follow)
            (host-command "set-track-bus-send"
              (dict :bus (get send :bus-idx) :amount v))))))))

(def fx-plock-set-value (p v)
  (do
    (cool-off-follow)
    (host-command "set-track-plock-entry"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :rack-slot (get p :rack-slot)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :value v))))

(def fx-plock-set-option (p label)
  (do
    (cool-off-follow)
    (host-command "set-track-plock-entry-option"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :rack-slot (get p :rack-slot)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :label label))))

(def fx-plock-clear (p)
  (host-command "clear-track-plock-entry"
    (dict :target (get p :target)
          :step-idx (get p :step-idx)
          :rack-slot (get p :rack-slot)
          :slot-idx (get p :slot-idx)
          :param-idx (get p :param-idx)
          :target-track (get p :target-track)
          :network-id (get p :network-id)
          :neuron-idx (get p :neuron-idx))))

(defstate fx-selected-plock-row -1)

(def fx-plock-param-col-width 6.45)
(def fx-plock-lock-col-width 6.35)
(def fx-plock-def-col-width 4.25)
(def fx-plock-col-gap 0.22)

(def fx-plock-row-selected? ()
  (and (>= fx-selected-plock-row 0)
       (< fx-selected-plock-row (len SEQ.track-plocks))))

(def fx-selected-plock-row-preview? ()
  (and (fx-plock-row-selected?)
       (get (nth SEQ.track-plocks fx-selected-plock-row) :preview)))

(def fx-delete-selected-plock-row ()
  (if (fx-plock-row-selected?)
    (if (fx-selected-plock-row-preview?)
      (set! fx-selected-plock-row -1)
      (let ((idx fx-selected-plock-row)
            (next-count (- (len SEQ.track-plocks) 1)))
        (do
          (fx-plock-clear (nth SEQ.track-plocks idx))
          (set! fx-selected-plock-row
            (if (<= next-count 0)
              -1
              (min idx (- next-count 1)))))))
    nil))

(def fx-plock-chip-color (chip)
  (rgba (get chip :color-r) (get chip :color-g) (get chip :color-b) 1.0))

(def fx-plock-chip-label (chip)
  (if (get chip :display)
    (substring (get chip :display) 0 6)
    (get chip :label)))

(def fx-plock-chip-click (chip)
  (do
    (cool-off-follow)
    (set! fx-selected-plock-row -1)
    (if (seq-has-selection?)
      (host-command "stamp-plock-variant"
        (dict :label (get chip :label)
              :step (current-step)))
      (host-command "preview-plock-variant"
        (dict :label (get chip :label))))))

(def fx-plock-chip (chip)
  (let ((current (get chip :current))
      (def-chip (= (get chip :kind) "def"))
      (c (fx-plock-chip-color chip)))
    (box :key (str "track-plock-chip-" (get chip :kind) "-" (get chip :label))
      :height 1.12
      :align :baseline
      :padding 0.14
      :background-color (if current
        (rgba (get chip :color-r) (get chip :color-g) (get chip :color-b) 0.11)
        (rgba 1 1 1 0.025))
      :border-width (if current 0.75 0.35)
      :border-color (if current c (rgba 1 1 1 0.10))
      :corner-radius 5
      :on-click |x y r| (fx-plock-chip-click chip)
      (h-stack :gap 0.16 :align :baseline
        (box :width 0.18 :height 0.68
          :corner-radius 2
          :background-color (if def-chip :transparent c)
          :border-width (if def-chip 1 0)
          :border-color c)
        (label (fx-plock-chip-label chip)
          :font-size 8.6 :color (if current :black :dim) :bg :transparent)
        (label (str (get chip :count))
          :font-size 8.6 :color (if current :black :dark-gray) :bg :transparent)))))

(def fx-plock-domain-title (domain)
  (if (= domain "inst")
    "INST"
    (if (= domain "seq")
      "SEQ"
      (if (= domain "fx")
        "FX"
        "NEURAL"))))

(def fx-plock-domain-count (domain)
  (len (filter |p| (= (fx-plock-row-domain p) domain) SEQ.track-plocks)))

(def fx-plock-row-domain (p)
  (if (get p :domain)
    (get p :domain)
    (if (or (= (get p :target) "neural-instrument")
            (= (get p :target) "neural-effect"))
      "neural"
      (if (or (= (get p :target) "instrument")
              (= (get p :target) "rack-slot-param")
              (= (get p :target) "rack-slot-instrument"))
        "inst"
        (if (= (get p :target) "effect")
          "fx"
          "seq")))))

(def fx-plock-row-title (p)
  (if (= (get p :source) "neuron")
    (str (get p :label) " " (get p :name))
    (get p :name)))

(def fx-plock-row-key (idx suffix)
  (str "track-plock-row-" idx "-" suffix))

(def fx-plock-row-value (p)
  (if (get p :value-field)
    (bind-seq (get p :value-field))
    (get p :value)))

(def fx-plock-group-header (domain)
  (box :height 0.95
    (h-stack :gap 0.35 :align :center
      (label (fx-plock-domain-title domain)
        :font-size 8.5 :color :dim :bg :transparent :width 4.5)
      (box :height 0.05 :width :fill :background-color (rgba 1 1 1 0.10)))))

(def fx-plock-row (p idx)
  (subtree :key (str "track-plock-" idx "-" (get p :target) "-" (get p :step-idx) "-"
      (get p :slot-idx) "-" (get p :param-idx))
    (box :width :fill
      :height 1.14
      :align :baseline
      :padding 0.07
      :background-color (if (= fx-selected-plock-row idx)
        (rgba 0.27 0.78 0.86 0.18)
        (if (= (mod idx 2) 0) (rgba 1 1 1 0.025) :transparent))
      :border-width (if (= fx-selected-plock-row idx) 1 0)
      :border-color (rgba 0.27 0.78 0.86 0.55)
      :corner-radius 2
      :on-click |x y r| (set! fx-selected-plock-row idx)
      (h-stack :width :fill :gap fx-plock-col-gap :align :baseline
        (label (substring (fx-plock-row-title p) 0 12)
          :key (fx-plock-row-key idx "param")
          :font-size 9.2 :width fx-plock-param-col-width
          :color (if (= fx-selected-plock-row idx) :white :dim)
          :bg :transparent)
        (if (or (= (get p :source) "neuron") (get p :preview))
          (label (if (get p :text-value) (get p :text-value) (str (get p :value)))
            :key (fx-plock-row-key idx "lock")
            :font-size 9.2 :width fx-plock-lock-col-width
            :h-align :right :color :yellow :bg :transparent)
          (if (get p :options)
            (dropdown :value (get p :text-value)
              :options (get p :options)
              :key (fx-plock-row-key idx "lock")
              :on-change (lambda (v) (fx-plock-set-option p v))
              :width fx-plock-lock-col-width :height 0.98 :font-size 8.4)
            (number-picker :value (fx-plock-row-value p)
              :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 2
              :key (fx-plock-row-key idx "lock")
              :noui true :font-size 9.2 :text-color :yellow :text-align :right
              :on-change (lambda (v) (fx-plock-set-value p v))
              :width fx-plock-lock-col-width :height 1.0)))
        (label (if (get p :default-text) (get p :default-text) (str (get p :default)))
          :key (fx-plock-row-key idx "def")
          :font-size 9.2 :width fx-plock-def-col-width
          :h-align :right :color :dark-gray :bg :transparent)))))

(def fx-plock-group (domain)
  (if (> (fx-plock-domain-count domain) 0)
    (v-stack :gap 0.12
      (fx-plock-group-header domain)
      (each SEQ.track-plocks |p idx|
        (if (= (fx-plock-row-domain p) domain)
          (fx-plock-row p idx)
          (box :height 0))))
    (box :height 0)))

(def fx-track-plocks-panel ()
  (box :debug-name "track-plocks-panel" :padding 0.72
    (v-stack :gap 0.30
      (wrap :key "track-plock-variant-strip"
            :width :fill :gap 0.18 :row-gap 0.14 :align :start
        (each SEQ.track-plock-variants |chip idx|
          (fx-plock-chip chip)))
      (if (> (len SEQ.track-plocks) 0)
        (v-stack :key "track-plock-table" :width :fill :gap 0.1
          (h-stack :key "track-plock-table-header" :width :fill :gap fx-plock-col-gap
            (label "PARAM" :key "track-plock-header-param"
              :font-size 8.2 :width fx-plock-param-col-width :color :dark-gray :bg :transparent)
            (label "LOCK" :key "track-plock-header-lock"
              :font-size 8.2 :width fx-plock-lock-col-width :h-align :right
              :color :dark-gray :bg :transparent)
            (label "DEF" :key "track-plock-header-def"
              :font-size 8.2 :width fx-plock-def-col-width :h-align :right
              :color :dark-gray :bg :transparent))
          (fx-plock-group "inst")
          (fx-plock-group "seq")
          (fx-plock-group "fx")
          (fx-plock-group "neural"))
        (label (if (> (len SEQ.selected-neural-neurons) 0)
                 "no p-locks for selected neurons"
                 "No locks")
          :font-size 9 :color :dim :bg :transparent)))))

(def fx-step-param-value (mode)
  (let ((values (seqv-current-param-values mode))
        (step (current-step)))
    (if (< step (len values))
      (nth values step)
      0)))

(def fx-step-set-param (mode value)
  (do
    (cool-off-follow)
    (if (seq-has-selection?)
      (seq-set-step-param-plock
        (seqv-param-keyword mode)
        (seqv-step-param-value mode value))
      (seq-set-step-param
        (current-step)
        (seqv-param-keyword mode)
        (seqv-step-param-value mode value)))))

(def fx-step-set-sound (label)
  (fx-step-set-param 3 (seqv-drum-sound-transpose-for-label SEQ.current-track label)))

(def fx-step-param-min (mode)
  (if (= mode 3) -48
    (if (= mode 1) 0
      (seqv-param-min mode))))

(def fx-step-param-max (mode)
  (if (= mode 3) 48
    (if (= mode 1) 128
      (seqv-param-max mode))))

(def fx-step-param-picker (mode key width)
  (v-stack :align :center :gap 0.24
    (label (seqv-param-name mode) :font-size 8 :color :dim :bg :transparent)
    (number-picker
      :key (str "fx-step-param-" key)
      :value (bind-seq (str "fx-step-value-" key))
      :min (fx-step-param-min mode)
      :max (fx-step-param-max mode)
      :decimals (seqv-param-decimals mode)
      :noui true
      :font-size 10
      :text-color :white
      :on-change (lambda (v) (fx-step-set-param mode v))
      :width width
      :height 1.15)))

(def fx-step-sound-picker ()
  (v-stack :align :center :gap 0.24
    (label "Sound" :font-size 8 :color :dim :bg :transparent)
    (if (> (seqv-drum-sound-count SEQ.current-track) 0)
      (dropdown
        :key "fx-step-param-sound"
        :value (seqv-drum-sound-label-for-transpose SEQ.current-track (fx-step-param-value 3))
        :options (seqv-drum-sound-labels SEQ.current-track)
        :on-change (lambda (label) (fx-step-set-sound label))
        :width 8.8 :height 1.15 :font-size 8.2)
      (box :key "fx-step-param-sound-empty" :width 8.8 :height 1.15
        (label "No drum pads" :font-size 8 :color :dim :bg :transparent)))))

(def fx-step-track-badge ()
  (let ((track SEQ.current-track)
        (muted (mixer-v2-muted? SEQ.current-track)))
    (box
      :key "fx-step-track-badge"
      :width 3.65 :height 1.0
      :padding 0
      :background-color (rgba
        (mixer-v2-track-color-r track muted)
        (mixer-v2-track-color-g track muted)
        (mixer-v2-track-color-b track muted)
        1.0)
      (label (mixer-v2-track-collapsed-label track)
        :width 3.65
        :font-size 9
        :h-align :center
        :color (if muted :dim :black)
        :bg :transparent))))

(def fx-step-parameters-panel ()
  (box :debug-name "step-parameters-panel" :padding 0.75
    (box :padding 0.5 
      :background-color :mixer-strip-bg 
      :corner-radius 16
      :border-color :mixer-strip-border    (v-stack :gap 0.55
        (h-stack :gap 0.45 :align :start
          (fx-step-track-badge)
          (h-stack :key "fx-step-selection-summary" :gap 0.15 :align :center
            (number-label :key "fx-step-cursor-label"
              :value (bind-seq "fx-step-cursor-number")
              :prefix "step " :decimals 0 :width 3.3
              :font-size 8 :color :dim :bg :transparent)
            (label "·" :font-size 8 :color :dim :bg :transparent)
            (number-label :key "fx-step-selection-count-label"
              :value (bind-seq "fx-step-selection-count")
              :suffix " selected" :decimals 0 :width 5.0
              :font-size 8 :color :dim :bg :transparent)))
        (h-stack :gap 0.55 :align :center
          (if (seqv-track-drum-rack? SEQ.current-track)
            (fx-step-sound-picker)
            (fx-step-param-picker 3 "transpose" 4.2))
          (fx-step-param-picker 0 "velocity" 4.2)
          (fx-step-param-picker 1 "duration" 4.2))))))

(def fx-track-accumulator-panel ()
  (h-stack :debug-name "track-accumulator-panel" :padding 0.00 
    (box :padding 0.5
      :background-color :mixer-strip-bg
      :corner-radius 16
      :border-color :mixer-strip-border
      (h-stack :gap 0.55 :align :center
        (v-stack :align :center :gap 0.40
          (label "acc fn" :font-size 8 :color :dim :bg :transparent)
          (dropdown :key "fx-track-accumulator-function"
            :value SEQ.tp-accumulator
            :options SEQ.accumulator-options
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accumulator v)))
            :width 7.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.40
          (label "acc mode" :font-size 8 :color :dim :bg :transparent)
          (dropdown :key "fx-track-accumulator-mode"
            :value SEQ.tp-accum-mode
            :options SEQ.accum-mode-options
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-mode v)))
            :width 6.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.22
          (v-stack :gap 0.5 :align :center
            (label "acc lim" :font-size 8 :color :dim :bg :transparent)
            (number-picker :key "fx-track-accumulator-limit"
              :value SEQ.tp-accum-limit :min 0 :max 127 :decimals 0
              :noui false :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-limit v)))
              :width 5.2 :height 1.15)))))))

(def fx-track-parameters-panel ()
  (box :debug-name "track-parameters-strip" :padding 0.0
    (v-stack :gap 0.175
      (box :debug-name "track-primary-parameters-panel" :padding 0.5
        :background-color :mixer-strip-bg
        :corner-radius 16
        :border-color :mixer-strip-border
        (h-stack :gap 1.05 :align :center
          (v-stack :gap 0.5 :align :center
            (label "steps" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-num-steps :min 1 :max 256 :decimals 0
              :noui false :font-size 8 :text-color :white
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :num-steps v)))
              :width 4.2 :height 1.15))

          (v-stack :align :center :gap 0.34
            (label "poly" :font-size 8 :color :dim :bg :transparent)
            (button  (if SEQ.tp-poly "ON" "OFF") :width 3.2 :height 1.3
              :background-color (if SEQ.tp-poly  (rgba 0.95 0.48 0.18 1.0) '(rgba 0.1 0.1 0.1 1))
              :border-color :white
              :font-size 11
              :color (if SEQ.tp-poly :black :white)
              ;; Rack tracks: playback polyphony is per-slot (RackSlotSnapshot::max_polyphony),
              ;; never the track-level param below — route there instead, or this control
              ;; silently edits a value playback ignores.
              :on-click |x y r| (do (cool-off-follow)
                (if SEQ.tp-is-rack
                  (host-command "set-rack-slot-max-polyphony"
                    (dict :track SEQ.current-track :slot SEQ.tp-rack-slot-idx :value (if SEQ.tp-poly 1 4)))
                  (seq-set-track-param :poly (if SEQ.tp-poly 0 1))))
              )
            )
          (v-stack :gap 0.5 :align :center
            (label "voices" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-max-polyphony :min 1 :max 12 :decimals 0
              :noui false :font-size 8 :text-color :white
              :on-change (lambda (v) (do (cool-off-follow)
                (if SEQ.tp-is-rack
                  (host-command "set-rack-slot-max-polyphony"
                    (dict :track SEQ.current-track :slot SEQ.tp-rack-slot-idx :value v))
                  (seq-set-track-param :voices v))))
              :width 3.4 :height 1.15)
            )
          (v-stack :align :center :gap 0.40
            (label "scale" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-fts
              :options SEQ.fts-options
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-fts v)))
              :width 7.0 :height 1.25 :font-size 9))

          ))
      (box :debug-name "track-groove-parameters-panel" :padding 0.5
        :background-color :mixer-strip-bg
        :corner-radius 16
        :border-color :mixer-strip-border
        (h-stack :gap 1.05 :align :center
          (v-stack :align :center :gap 0.40
            (label "swg res" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-swing-resolution
              :key "fx-track-swing-resolution"
              :options '("1/16" "1/8" "1/4" "1/2")
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-swing-resolution v)))
              :plock-active (if (fx-track-param-plock-active? "swing-resolution") 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :width 5.0 :height 1.25 :font-size 9))
          (v-stack :align :center :gap 0.22
            (v-stack :gap 0.5 :align :center
              (label "swing" :font-size 8 :color :dim :bg :transparent)
              (number-picker :value SEQ.tp-swing :min 50 :max 75 :decimals 1
                :key "fx-track-swing"
                :noui false :font-size 8 :text-color :dim
                :plock-active (if (fx-track-param-plock-active? "swing") 1 0)
                :plock-default (fx-track-param-plock-default "swing" SEQ.tp-swing)
                :plock-color-r (param-plock-color-r)
                :plock-color-g (param-plock-color-g)
                :plock-color-b (param-plock-color-b)
                :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :swing v)))
                :width 5.2 :height 1.15))
            )

          (v-stack :align :center :gap 0.40
            (label "timebase" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-timebase
              :key "fx-track-timebase"
              :options seq-timebase-options
              :on-change (lambda (v) (fx-set-timebase v))
              :plock-active (if (fx-track-param-plock-active? "timebase") 1 0)
              :plock-color-r (param-plock-color-r)
              :plock-color-g (param-plock-color-g)
              :plock-color-b (param-plock-color-b)
              :width 6.0 :height 1.25 :font-size 9))

          (v-stack :align :center :gap 0.40
            (label "mute grp" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-mute-group
              :options SEQ.mute-group-options
              :on-change (lambda (v)
                (do
                  (cool-off-follow)
                  (seq-set-track-param :mute-group (fx-mute-group-value v))))
              :width 5.4 :height 1.25 :font-size 9))
          )))))
