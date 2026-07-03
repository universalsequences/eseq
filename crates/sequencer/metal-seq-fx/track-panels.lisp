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
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :value v))))

(def fx-plock-set-option (p label)
  (do
    (cool-off-follow)
    (host-command "set-track-plock-entry-option"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :label label))))

(def fx-plock-clear (p)
  (do
    (cool-off-follow)
    (host-command "clear-track-plock-entry"
      (dict :target (get p :target)
            :step-idx (get p :step-idx)
            :slot-idx (get p :slot-idx)
            :param-idx (get p :param-idx)
            :target-track (get p :target-track)
            :network-id (get p :network-id)
            :neuron-idx (get p :neuron-idx)))))

(def fx-plock-row (p idx)
  (subtree :key (str "track-plock-" idx "-" (get p :target) "-" (get p :step-idx) "-"
                     (get p :slot-idx) "-" (get p :param-idx))
    (box :height 1.28
      (h-stack :gap 0.35 :align :center
        (label (get p :label) :font-size 9 :width 2.2 :color :yellow :bg :transparent)
        (label (substring (get p :group) 0 8) :font-size 9 :width 5.6 :color :dim :bg :transparent)
        (label (substring (get p :name) 0 9) :font-size 10 :width 5.9 :color :white :bg :transparent)
        (if (= (get p :source) "neuron")
          (label (if (get p :text-value) (get p :text-value) (str (get p :value)))
            :font-size 10 :width 5.2 :color :dim :bg :transparent)
          (if (get p :options)
            (dropdown :value (get p :text-value)
              :options (get p :options)
              :on-change (lambda (v) (fx-plock-set-option p v))
              :width 5.2 :height 1.1 :font-size 9)
            (number-picker :value (get p :value)
              :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 2
              :noui true :font-size 10 :text-color :dim
              :on-change (lambda (v) (fx-plock-set-value p v))
              :width 4.5 :height 1.05)))
        (button "x"
          :width 1.35 :height 1.05 :padding 0 :font-size 9
          :background-color :dark-gray :color :dim
          :on-click |x y r| (fx-plock-clear p))))))

(def fx-track-plocks-panel ()
  (box :debug-name "track-plocks-panel" :padding 0.75
    (v-stack :gap 0.35
      (h-stack :gap 0.35 :align :baseline
        (label "p-locks" :font-size 10 :color :white :bg :transparent)
        (label (str (len SEQ.track-plocks))
          :font-size 8 :color :dim :bg :transparent))
      (if (> (len SEQ.track-plocks) 0)
        (v-stack :gap 0.2
          (each SEQ.track-plocks |p idx|
            (fx-plock-row p idx)))
        (label (if (> (len SEQ.selected-neural-neurons) 0)
                 "no p-locks for selected neurons"
                 "no p-locks for selected steps")
          :font-size 9 :color :dim :bg :transparent)))))

(def fx-step-selected-count ()
  (len (filter (lambda (selected) selected) SEQ.selected-steps)))

(def fx-step-selection-title ()
  (let ((count (fx-step-selected-count)))
    (if (> count 0)
      (str count " steps")
      (str "step " (+ (current-step) 1)))))

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
      :value (fx-step-param-value mode)
      :min (fx-step-param-min mode)
      :max (fx-step-param-max mode)
      :decimals (seqv-param-decimals mode)
      :noui true
      :font-size 10
      :text-color :white
      :on-change (lambda (v) (fx-step-set-param mode v))
      :width width
      :height 1.15)))

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
          ;(label "step" :font-size 10 :color :white :bg :transparent)
          (label (fx-step-selection-title) :font-size 8 :color :dim :bg :transparent))
        (h-stack :gap 0.55 :align :center
          (fx-step-param-picker 3 "transpose" 4.2)
          (fx-step-param-picker 0 "velocity" 4.2)
          (fx-step-param-picker 1 "duration" 4.2))))))

(def fx-track-accumulator-panel ()
  (h-stack :debug-name "track-accumulator-panel" :padding 0.00 
    (box  :padding 0.5
      :background-color :mixer-strip-bg 
      :corner-radius 16
      :border-color :mixer-strip-border 
      (h-stack :gap 0.55 :align :center
        (v-stack :align :center :gap 0.40
          (label "acc fn" :font-size 8 :color :dim :bg :transparent)
          (dropdown :value SEQ.tp-accumulator
            :options SEQ.accumulator-options
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accumulator v)))
            :width 7.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.40
          (label "acc mode" :font-size 8 :color :dim :bg :transparent)
          (dropdown :value SEQ.tp-accum-mode
            :options SEQ.accum-mode-options
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-mode v)))
            :width 6.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.22
          (v-stack :gap 0.5 :align :center
            (label "acc lim" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-accum-limit :min 0 :max 127 :decimals 0
              :noui false :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-limit v)))
              :width 5.2 :height 1.15))
          )))))

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
              :options '("1/16" "1/8" "1/4" "1/2")
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-swing-resolution v)))
              :width 5.0 :height 1.25 :font-size 9))
          (v-stack :align :center :gap 0.22
            (v-stack :gap 0.5 :align :center
              (label "swing" :font-size 8 :color :dim :bg :transparent)
              (number-picker :value SEQ.tp-swing :min 50 :max 75 :decimals 1
                :noui false :font-size 8 :text-color :dim
                :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :swing v)))
                :width 5.2 :height 1.15))
            )

          (v-stack :align :center :gap 0.40
            (label "timebase" :font-size 8 :color :dim :bg :transparent)
            (dropdown :value SEQ.tp-timebase
              :key "fx-track-timebase"
              :options seq-timebase-options
              :on-change (lambda (v) (fx-set-timebase v))
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
