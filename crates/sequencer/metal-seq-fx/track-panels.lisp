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

(def fx-track-accumulator-panel ()
  (box :debug-name "track-accumulator-panel" :padding 0.75
    (h-stack :gap 0.55 :align :center
      (v-stack :align :center :gap 0.30
        (label "acc fn" :font-size 8 :color :dim :bg :transparent)
        (dropdown :value SEQ.tp-accumulator
          :options SEQ.accumulator-options
          :on-change (lambda (v) (do (cool-off-follow) (seq-set-accumulator v)))
          :width 7.0 :height 1.25 :font-size 9))
      (v-stack :align :center :gap 0.30
        (label "acc mode" :font-size 8 :color :dim :bg :transparent)
        (dropdown :value SEQ.tp-accum-mode
          :options SEQ.accum-mode-options
          :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-mode v)))
          :width 6.0 :height 1.25 :font-size 9))
      (v-stack :align :center :gap 0.22
        (h-stack :gap 0.2 :align :baseline
          (label "acc lim" :font-size 8 :color :dim :bg :transparent)
          (number-picker :value SEQ.tp-accum-limit :min 0 :max 127 :decimals 0
            :noui true :font-size 8 :text-color :dim
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-limit v)))
            :width 3.2 :height 0.85))
        (box :width 5.8 :height 1.2
          (hslider :min 0 :max 127
            :value SEQ.tp-accum-limit
            :material (aqua-slider-material)
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-limit v)))))))))

(def fx-track-parameters-panel ()
  (box :debug-name "track-parameters-strip" :padding 0.9
    (v-stack :gap 0.75
      (h-stack :gap 0.55 :align :center
        (v-stack :align :center :gap 0.22
          (h-stack :gap 0.2 :align :baseline
            (label "steps" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-num-steps :min 1 :max 256 :decimals 0
              :noui true :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :num-steps v)))
              :width 3.2 :height 0.85))
          (box :width 6.0 :height 1.2
            (hslider :min 1 :max 256
              :value SEQ.tp-num-steps
              :material (aqua-slider-material)
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :num-steps v))))))
        (v-stack :align :center :gap 0.24
          (label "poly" :font-size 8 :color :dim :bg :transparent)
          (box :width 3.2 :height 1.3
            :bg (if SEQ.tp-poly :blue :dark-gray)
            :on-click |x y r| (do (cool-off-follow) (seq-set-track-param :poly (if SEQ.tp-poly 0 1)))
            (label (if SEQ.tp-poly "ON" "OFF") :font-size 9 :color :white :bg :transparent)))
        (v-stack :align :center :gap 0.22
          (h-stack :gap 0.2 :align :baseline
            (label "voices" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-max-polyphony :min 1 :max 12 :decimals 0
              :noui true :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :voices v)))
              :width 2.4 :height 0.85))
          (box :width 4.8 :height 1.2
            (hslider :min 1 :max 12
              :value SEQ.tp-max-polyphony
              :material (aqua-slider-material)
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :voices v))))))
        (v-stack :align :center :gap 0.30
          (label "fts" :font-size 8 :color :dim :bg :transparent)
          (dropdown :value SEQ.tp-fts
            :options SEQ.fts-options
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-fts v)))
            :width 7.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.30
          (label "mute grp" :font-size 8 :color :dim :bg :transparent)
          (dropdown :value SEQ.tp-mute-group
            :options SEQ.mute-group-options
            :on-change (lambda (v)
              (do
                (cool-off-follow)
                (seq-set-track-param :mute-group (fx-mute-group-value v))))
            :width 5.4 :height 1.25 :font-size 9)))
      (h-stack :gap 0.55 :align :center
        (v-stack :align :center :gap 0.30
          (label "swg res" :font-size 8 :color :dim :bg :transparent)
          (dropdown :value SEQ.tp-swing-resolution
            :options '("1/16" "1/8" "1/4" "1/2")
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-swing-resolution v)))
            :width 5.0 :height 1.25 :font-size 9))
        (v-stack :align :center :gap 0.22
          (h-stack :gap 0.2 :align :baseline
            (label "swg" :font-size 8 :color :dim :bg :transparent)
            (number-picker :value SEQ.tp-swing :min 50 :max 75 :decimals 1
              :noui true :font-size 8 :text-color :dim
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :swing v)))
              :width 3.2 :height 0.85))
          (box :width 5.8 :height 1.2
            (hslider :min 50 :max 75
              :value SEQ.tp-swing
              :material (aqua-slider-material)
              :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :swing v))))))))))
