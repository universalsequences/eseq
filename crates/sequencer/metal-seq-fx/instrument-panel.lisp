;; Instrument panel composition for sampler, rack, modulator, and synth tracks.
(def rack-panel-drop-on-rack (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((track (get target :track))
          (path (get payload :path))
          (name (get payload :name))
          (drag-type (get event :drag-type)))
      (if (= drag-type "sample")
        (if path
          (host-command "add-rack-sample-slot"
            (dict :track track :path path :preserve-browser-context true))
          (status "Drop a sample file, not a folder"))
        (if (= drag-type "instrument")
          (if name
            (host-command "add-rack-instrument-slot"
              (dict :track track :name name))
            (status "Drop an instrument, not a folder"))
          (status "Drop a sample or instrument"))))))

(def rack-slot-select (slot)
  (host-command "select-rack-slot"
    (dict :track (get slot :track) :slot (get slot :idx))))

(def rack-slot-delete-target-payload (slot)
  (dict :track (get slot :track) :slot (get slot :idx)))

(def rack-slot-select-delete-target (slot)
  (do
    (rack-slot-select slot)
    (seq-set-delete-target :rack-slot (rack-slot-delete-target-payload slot))))

(def rack-slot-delete-target-binding (slot)
  (bind-seq (str "rack-slot-delete-target-" (get slot :track) "-" (get slot :idx))))

(def rack-slot-delete-target? (slot)
  (rack-slot-delete-target-binding slot))

(def rack-slot-set-param-or-plock (slot param default-command v)
  (host-command (if (seq-has-selection?) "set-rack-slot-param-plock" default-command)
    (dict :track (get slot :track)
          :slot (get slot :idx)
          :param param
          :value v)))

(def rack-slot-set-gain (slot v)
  (rack-slot-set-param-or-plock slot "gain" "set-rack-slot-gain" v))

(def rack-slot-set-pan (slot v)
  (rack-slot-set-param-or-plock slot "pan" "set-rack-slot-pan" v))

(def rack-slot-set-base-note (slot v)
  (rack-slot-set-param-or-plock slot "base-note" "set-rack-slot-base-note" v))

(def rack-slot-set-max-polyphony (slot v)
  (rack-slot-set-param-or-plock slot "max-polyphony" "set-rack-slot-max-polyphony" v))

(def rack-slot-set-mute (slot v)
  (rack-slot-set-param-or-plock slot "mute" "set-rack-slot-mute" v))

(def rack-slot-set-solo (slot v)
  (rack-slot-set-param-or-plock slot "solo" "set-rack-slot-solo" v))

(def rack-slot-row (slot)
  (let ((delete-target (rack-slot-delete-target? slot))
        (selected (get slot :selected)))
    (box :key (str "rack-slot-row-" (get slot :idx))
         :width :fill
         :height 1.65
         :padding 0.18
         :selected delete-target
         :background-color (if selected
                             '(rgba 0.30 0.32 0.34 1.0)
                             '(rgba 0.06 0.065 0.074 0.0))
         :selected-background-color :fx-panel-header-selected-bg
         :border-width 1
         :border-color (if selected
                         '(rgba 0.48 0.50 0.52 1.0)
                         '(rgba 0.16 0.17 0.19 1.0))
         :selected-border-color :mixer-strip-selected-border
         :corner-radius 3
         :on-click |x y r| (rack-slot-select slot)
      (h-stack :width :fill :height :fill :gap 0.45 :align :center
        (label (str (+ (get slot :idx) 1))
          :font-size 10
          :color :gray
          :width 1.0
          :bg :transparent)
        (box :key (str "rack-slot-label-" (get slot :idx))
             :width 13 :height :fill :v-align :center :padding 0
             :selected delete-target
             :background-color :transparent
             :selected-background-color :fx-panel-header-selected-bg
             :corner-radius 3
             :on-click |x y r| (rack-slot-select-delete-target slot)
          (label (substring (get slot :display-name) 0 24)
            :font-size 10.5
            :color :white
            :active delete-target
            :active-color :white
            :bg :transparent))

        (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
          (label "T" :font-size 8.2 :color :dim :bg :transparent)
          (number-picker :value (get slot :base-note)
            :min (get slot :base-note-min) :max (get slot :base-note-max) :decimals 0
            :noui true :font-size 9.4
            :text-align :center :text-color :dim :edit-color :yellow
            :width 3.55 :height 0.84
            :on-change (lambda (v) (rack-slot-set-base-note slot v))))
        (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
          (label "G" :font-size 8.2 :color :dim :bg :transparent)
          (number-picker :value (get slot :gain)
            :min (get slot :gain-min) :max (get slot :gain-max) :decimals 2
            :noui true :font-size 9.4
            :text-align :center :text-color :dim :edit-color :yellow
            :width 3.55 :height 0.84
            :on-change (lambda (v) (rack-slot-set-gain slot v))))
        (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
          (label "P" :font-size 8.2 :color :dim :bg :transparent)
          (number-picker :value (get slot :pan)
            :min (get slot :pan-min) :max (get slot :pan-max) :decimals 2
            :noui true :font-size 9.4
            :text-align :center :text-color :dim :edit-color :yellow
            :width 3.55 :height 0.84
            :on-change (lambda (v) (rack-slot-set-pan slot v))))
        (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
          (label "V" :font-size 8.2 :color :dim :bg :transparent)
          (number-picker :value (get slot :max-polyphony)
            :min (get slot :max-polyphony-min) :max (get slot :max-polyphony-max) :decimals 0
            :noui true :font-size 9.4
            :text-align :center :text-color :dim :edit-color :yellow
            :width 3.55 :height 0.84
            :on-change (lambda (v) (rack-slot-set-max-polyphony slot v))))
        (button "M"
          :width 1.35 :height 1.02 :padding 0 :font-size 9
          :background-color (if (get slot :mute) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
          :color (if (get slot :mute) :black :dim)
          :on-click |x y r| (rack-slot-set-mute slot (not (get slot :mute))))
        (button "S"
          :width 1.35 :height 1.02 :padding 0 :font-size 9
          :background-color (if (get slot :solo) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
          :color (if (get slot :solo) :black :dim)
          :on-click |x y r| (rack-slot-set-solo slot (not (get slot :solo))))))))

(def rack-empty-selected-panel ()
  (box :debug-name "rack-empty-selected-panel"
       :width 34
       :height fx-fixed-panel-height
       :background "fx-panel-bg"
       :color :instrument-panel-bg
       :header :fx-panel-header-bg
       :selected-header :fx-panel-header-selected-bg
       :padding 0
       :selected 0
       :h-align :center
       :v-align :center
    (label "Drop an Instrument or Sample"
      :font-size 11 :color :dim :bg :transparent)))

(def rack-selected-instrument-panel (inst)
  (let ((selected (get inst :selected-instrument)))
    (if selected
      (instrument-panel selected)
      (rack-empty-selected-panel))))

(def rack-panel (inst)
  (box
    (v-stack :debug-name "rack-panel-vstack" :gap 0 :height :fill
      (box :debug-name "rack-header-box" :height 1 :padding 0 :v-align :center :h-align :start :width :fill
        (h-stack :debug-name "rack-header-row" :gap 0.6 :align :center :width :fill
          (fx-panel-header-leading-spacer)
          (label (substring (get inst :display-name) 0 16)
            :font-size 11 :color :white :bg :transparent)
          (label (get inst :routing)
            :font-size 9 :color :gray :bg :transparent)
          (box :flex 1 :height 0.15)))
      (fx-panel-body "rack-content-box"
        (v-stack :debug-name "rack-chain-list" :gap 0.025 :height fx-panel-body-content-height :width :fill
          (if (> (len (get inst :slots)) 0)
            (each (get inst :slots) |slot idx|
              (rack-slot-row slot))
            (box :width :fill :height 9 :h-align :center :v-align :center
              (label "Drop an Instrument or Sample"
                :font-size 11 :color :dim :bg :transparent))))))
    :debug-name "rack-panel"
    :drop-types (list "sample" "instrument")
    :drop-meta (dict :kind "rack-panel" :track (get inst :track))
    :drop-hover-border-color :mixer-strip-selected-border
    :on-drop (lambda (event) (rack-panel-drop-on-rack event))
    :background "fx-panel-bg"
    :color :instrument-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :padding 0
    :width 35.5
    :height fx-fixed-panel-height
    :selected 0))

(def rack-instrument-panel-row (inst)
  (h-stack :debug-name "rack-instrument-panel-row"
           :gap 0.2
           :height fx-fixed-panel-height
           :align :stretch
    (rack-panel inst)
    (rack-selected-instrument-panel inst)))

(def instrument-panel (inst)
  (if (= (get inst :type) "sampler")
    (sampler-panel inst)
    (if (= (get inst :type) "rack")
      (rack-instrument-panel-row inst)
      (if (= (get inst :type) "modulator")
        (modulator-panel inst)
        (box
          (v-stack :debug-name "instrument-panel-vstack" :gap 0 :height :fill
            (box :debug-name "instrument-header-box" :height 1 :padding 0 :v-align :center :h-align :start :width :fill
              (h-stack :debug-name "instrument-header-row" :gap 0.6 :align :center :width :fill
                (fx-panel-header-leading-spacer)
                (fx-enabled-toggle (enabled-param (get inst :synth)) false "instrument-enabled")
                (h-stack :v-align :center :height fx-panel-header-height :gap 2 :padding 0.1
                  (label (substring (get inst :display-name) 0 12)
                    :font-size 11  :color :white :bg :transparent)
                  (instrument-synth-button)
                  (instrument-mods-toggle-button))
                (box :flex 1 :height 0.15)
                (v-stack
                  (button "edit"
                    :background-color '(rgba 0.0 0.0 0.0 0.3)
                    :height 0.75
                    :debug-name "instrument-edit-button"
                    :font-size 10
                    :border-color '(rgba 0.1 0.1 0.1 0.8)
                    :on-click |x y r|
                    (host-command "enter-edit-instrument"
                      (dict :name (if (get inst :name) (get inst :name) SEQ.sidebar-instrument-name)))
                    ))
                (box :debug-name "instrument-preset-button" :padding 0.0 :width 4 :align :center
                  (v-stack
                    (box :width 1 :height 0.1)
                    (fx-mini-save-icon
                      :on-click |x y r| (sbrowser-enter-preset-save)
                      :active 0)))))
            (fx-panel-body "instrument-content-box"
              (instrument-synth-panel-body inst)))
          :debug-name "instrument-panel"
          :background "fx-panel-bg"
          :color :instrument-panel-bg
          :header :fx-panel-header-bg
          :selected-header :fx-panel-header-selected-bg
          :padding 0
          :height fx-fixed-panel-height
          :selected 0)))))
