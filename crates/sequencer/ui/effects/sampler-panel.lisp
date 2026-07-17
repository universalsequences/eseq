;; Sampler instrument panel state and controls.
(defstate sampler-view-start 0.0)
(defstate sampler-view-duration 0)
(defstate sampler-cursor-time 0.0)
(defstate sampler-active-marker "none")

(def sampler-reset-view ()
  (set! sampler-view-start 0.0)
  (set! sampler-view-duration 0)
  (set! sampler-cursor-time 0.0)
  (set! sampler-active-marker "none"))

(def sampler-set-start-end (inst start-seconds end-seconds duration)
  (if (> duration 0)
    (do
      (fx-set-instrument-value
        (instrument-target-param-dict inst 2)
        (* 100 (/ start-seconds duration)))
      (fx-set-instrument-value
        (instrument-target-param-dict inst 3)
        (* 100 (/ end-seconds duration))))))

(def sampler-clamp-start (next-start duration)
  (max 0 (min next-start (max 0 (- duration sampler-view-duration)))))

(def sampler-clamp-duration (next-duration duration)
  (max 0.001 (min next-duration (max 0.001 duration))))

(def handle-sampler-waveform-action (inst event duration)
  (match event.type
    :set-cursor
    (set! sampler-cursor-time event.time)
    :set-selection
    (sampler-set-start-end inst event.start event.end duration)
    :begin-marker-drag
    (set! sampler-active-marker (if (= event.marker :start) "start" "end"))
    :end-marker-drag
    (set! sampler-active-marker "none")
    :clear-selection
    (sampler-set-start-end inst 0 duration duration)
    :scroll-view
    (set! sampler-view-start (sampler-clamp-start (+ sampler-view-start event.delta-time) duration))
    :zoom-view
    (let ((cur-duration (if (= sampler-view-duration 0) duration sampler-view-duration)))
      (let ((anchor-ratio (/ (- event.anchor-time sampler-view-start) cur-duration))
            (next-duration (sampler-clamp-duration (/ cur-duration event.factor) duration)))
        (set! sampler-view-duration next-duration)
        (set! sampler-view-start (sampler-clamp-start (- event.anchor-time (* anchor-ratio next-duration)) duration))))
    _
    nil))

(def sampler-panel-drop-types (inst)
  (if (instrument-rack-target? inst)
    (list "sound")
    (list "sample" "instrument" "sound")))

(def sampler-panel-drop-meta (inst)
  (if (instrument-rack-target? inst)
    (dict :kind "rack-selected-sampler"
          :track (get inst :rack-track)
          :slot (get inst :rack-slot))
    (dict :kind "sampler-panel" :track (get inst :track))))

(def sampler-param-knob (p key)
  (instrument-param-mod-wrapper p (str key "-mod-wrapper")
    (subtree :key (str key (instrument-param-control-key-mode p))
      (knob-number :label (substring (get p :name) 0 12)
        :value (fx-param-value p)
        :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 1
        :base-value (instrument-param-base-value-prop p)
        :base-min (instrument-param-base-min-prop p) :base-max (instrument-param-base-max-prop p)
        :mod-range-0-slot (instrument-param-knob-mod-slot-prop p 0) :mod-range-0-depth (instrument-param-knob-mod-depth-prop p 0)
        :mod-range-1-slot (instrument-param-knob-mod-slot-prop p 1) :mod-range-1-depth (instrument-param-knob-mod-depth-prop p 1)
        :mod-range-2-slot (instrument-param-knob-mod-slot-prop p 2) :mod-range-2-depth (instrument-param-knob-mod-depth-prop p 2)
        :mod-range-3-slot (instrument-param-knob-mod-slot-prop p 3) :mod-range-3-depth (instrument-param-knob-mod-depth-prop p 3)
        :mod-range-4-slot (instrument-param-knob-mod-slot-prop p 4) :mod-range-4-depth (instrument-param-knob-mod-depth-prop p 4)
        :mod-range-5-slot (instrument-param-knob-mod-slot-prop p 5) :mod-range-5-depth (instrument-param-knob-mod-depth-prop p 5)
        :mod-range-6-slot (instrument-param-knob-mod-slot-prop p 6) :mod-range-6-depth (instrument-param-knob-mod-depth-prop p 6)
        :mod-range-7-slot (instrument-param-knob-mod-slot-prop p 7) :mod-range-7-depth (instrument-param-knob-mod-depth-prop p 7)
        :mod-range-8-slot (instrument-param-knob-mod-slot-prop p 8) :mod-range-8-depth (instrument-param-knob-mod-depth-prop p 8)
        :mod-range-9-slot (instrument-param-knob-mod-slot-prop p 9) :mod-range-9-depth (instrument-param-knob-mod-depth-prop p 9)
        :selected-mod-slot (instrument-selected-mod-slot-prop p)
        :font-size 10.5 :label-font-size 10
        :text-color (param-plock-text-color false p) :label-color :dim
        :plock-active (if (param-plock-active? false p) 1 0)
        :plock-default (param-plock-default false p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.7 :height 2.8
        :knob-size 1.7
        :on-change (lambda (v) (instrument-set-param-control-value p v))))))


(def sampler-param-number-picker (p key)
  (instrument-param-mod-wrapper p (str key "-mod-wrapper")
    (subtree :key (str key (instrument-param-control-key-mode p))
      (h-stack :align :baseline :gap 0.35
        (label (sampler-param-display-name p) :font-size 10 :color :white :bg :transparent)
        (number-picker
          :value (fx-param-value p)
          :noui true
          :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 1
          :font-size 10.5
          :text-color (param-plock-text-color false p) :edit-color :yellow
          :plock-active (if (param-plock-active? false p) 1 0)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :text-align :left
          :width 4.0 :height 1.0
          :on-change (lambda (v) (instrument-set-param-control-value p v)))))))

(def sampler-param-button (p key)
  (subtree :key key
    (v-stack :align :center :gap 0.2
      (label (substring (get p :name) 0 12) :font-size 10 :color :dim :bg :transparent)
      (button (if (fx-param-on? p) "ON" "OFF")
        :width 3.2 :height 1.5 :padding 0 :font-size 10
        :background-color (if (fx-param-on? p) (rgba 0.95 0.48 0.18 1.0) (rgba 0.1 0.1 0.1 0.5))
        :color (if (fx-param-on? p) :black :dim)
        :plock-active (if (param-plock-active? false p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :on-click |x y r| (fx-set-instrument-value p (if (fx-param-on? p) 0 1))))))

(def sampler-param-dropdown (p key)
  (subtree :key key
    (v-stack :align :center :gap 0.5
      (label (substring (get p :name) 0 12) :font-size 10 :color :dim :bg :transparent)
      (dropdown :value (get p :text-value)
        :options (get p :options)
        
        :bg-color '(rgba 0.1 0.1 0.1 0.3) ;:instrument-control-bg
        ;:text-color accent
       ; :chevron-color :accent
        ;:badge-color (rgba 0.16 0.17 0.20 1.0)
        :border-color :gray
        :border-width 0.05  
        :plock-active (if (param-plock-active? false p) 1 0)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :on-change (lambda (v) (fx-set-instrument-option p v))
        :width 5.8 :height 1.0 :font-size 9))))

(def sampler-gate-button ()
  (v-stack :align :center :gap 0.2
    (label "gate" :font-size 10 :color :dim :bg :transparent)
    (button (if SEQ.tp-gate "ON" "OFF")
      :width 3.2 :height 1.5 :padding 0 :font-size 10
      :background-color (if SEQ.tp-gate (rgba 0.95 0.48 0.18 1.0) (rgba 0.1 0.1 0.1 0.5))
      :color (if SEQ.tp-gate :black :dim)
      :on-click |x y r| (do (cool-off-follow) (seq-set-track-param :gate (if SEQ.tp-gate 0 1))))))

(def sampler-param-control (p)
  (let ((key (if (get p :idx)
          (str "sampler-param-" (get p :idx))
          (str "sampler-param-" (get p :name)))))
    (if (get p :boolean)
      (sampler-param-button p key)
      (if (get p :options)
        (sampler-param-dropdown p key)
        (sampler-param-knob p key)))))

(def sampler-param-control-number-picker (p)
  (let ((key (if (get p :idx)
          (str "sampler-param-" (get p :idx))
          (str "sampler-param-" (get p :name)))))
    (if (get p :boolean)
      (sampler-param-button p key)
      (if (get p :options)
        (sampler-param-dropdown p key)
        (sampler-param-number-picker p key)))))

(def sampler-param-by-name (params name)
  (nth (filter |p| (= (get p :name) name) params) 0))

(def sampler-base-note-param? (p)
  (= (get p :control) "base-note"))

(def sampler-param-display-name (p)
  (if (sampler-base-note-param? p)
    "base"
    (get p :name)))

(def sampler-small-params (params)
  (filter |p|
    (let ((name (get p :name)))
      (or (sampler-base-note-param? p)
          (= name "attack")
          (= name "release")
          (= name "start")
          (= name "end")))
    params))

(def sampler-main-params (params)
  (filter |p|
    (let ((name (get p :name)))
      (and (not (= name "enabled"))
        (not (= name "warp"))
        (not (= name "mode"))
        (not (= name "bpm"))
        (not (= name "preserve"))
        (not (= name "fill"))
        (not (sampler-base-note-param? p))
        (not (= name "attack"))
        (not (= name "release"))
        (not (= name "start"))
        (not (= name "end"))
        (not (= name "decay"))))
    params))

(def sampler-bpm-control (p)
  (h-stack :gap 0.65 :align :start
    (instrument-param-mod-wrapper p "sampler-param-bpm-mod-wrapper"
      (subtree :key (str "sampler-param-bpm" (instrument-param-control-key-mode p))
        (knob-number :label "bpm"
          :value (fx-param-value p)
          :min (instrument-param-control-min p) :max (instrument-param-control-max p) :decimals 1
          :base-value (instrument-param-base-value-prop p)
          :base-min (instrument-param-base-min-prop p) :base-max (instrument-param-base-max-prop p)
          :mod-range-0-slot (instrument-param-knob-mod-slot-prop p 0) :mod-range-0-depth (instrument-param-knob-mod-depth-prop p 0)
          :mod-range-1-slot (instrument-param-knob-mod-slot-prop p 1) :mod-range-1-depth (instrument-param-knob-mod-depth-prop p 1)
          :mod-range-2-slot (instrument-param-knob-mod-slot-prop p 2) :mod-range-2-depth (instrument-param-knob-mod-depth-prop p 2)
          :mod-range-3-slot (instrument-param-knob-mod-slot-prop p 3) :mod-range-3-depth (instrument-param-knob-mod-depth-prop p 3)
          :mod-range-4-slot (instrument-param-knob-mod-slot-prop p 4) :mod-range-4-depth (instrument-param-knob-mod-depth-prop p 4)
          :mod-range-5-slot (instrument-param-knob-mod-slot-prop p 5) :mod-range-5-depth (instrument-param-knob-mod-depth-prop p 5)
          :mod-range-6-slot (instrument-param-knob-mod-slot-prop p 6) :mod-range-6-depth (instrument-param-knob-mod-depth-prop p 6)
          :mod-range-7-slot (instrument-param-knob-mod-slot-prop p 7) :mod-range-7-depth (instrument-param-knob-mod-depth-prop p 7)
          :mod-range-8-slot (instrument-param-knob-mod-slot-prop p 8) :mod-range-8-depth (instrument-param-knob-mod-depth-prop p 8)
          :mod-range-9-slot (instrument-param-knob-mod-slot-prop p 9) :mod-range-9-depth (instrument-param-knob-mod-depth-prop p 9)
          :selected-mod-slot (instrument-selected-mod-slot-prop p)
          :font-size 10.5 :label-font-size 10
          :text-color (param-plock-text-color false p) :label-color :dim
          :plock-active (if (param-plock-active? false p) 1 0)
          :plock-default (param-plock-default false p)
          :plock-color-r (param-plock-color-r)
          :plock-color-g (param-plock-color-g)
          :plock-color-b (param-plock-color-b)
          :width 4.0 :height 2.70
          :knob-size 1.7
          :on-change (lambda (v) (instrument-set-param-control-value p v)))))
    (v-stack :gap 0.12 :align :center
      (box :height 0.02)
      (v-stack :gap 0.2
        (button "1/2"
          :width 2.25 :height 1.02 :padding 0 :font-size 8
          :background-color :mixer-control-bg :color :dim
          :on-click |x y r| (fx-set-instrument-value p (min 400 (* (get p :value) 2))))
        (button "2x"
          :width 2.25 :height 1.02 :padding 0 :font-size 8
          :background-color :mixer-control-bg :color :dim
          :on-click |x y r| (fx-set-instrument-value p (max 20 (/ (get p :value) 2))))))))

(def sampler-param-pickers (params inst)
  (h-stack :debug-name "sampler-small-param-row" :gap 0.85 :padding 0.55 :align :center
    (each (sampler-small-params params) |p pi|
      (sampler-param-control-number-picker p))))

(def sampler-param-knobs (params inst)
  (h-stack :debug-name "sampler-main-param-row" :gap 0.85 :padding 0.15 :align :start
    (sampler-gate-button)
    (each (sampler-main-params params) |p pi|
      (sampler-param-control p))
    (sampler-param-control (sampler-param-by-name params "warp"))
    (sampler-param-control (sampler-param-by-name params "mode"))
    (sampler-param-control (sampler-param-by-name params "preserve"))
    (sampler-param-control (sampler-param-by-name params "fill"))
    (sampler-param-control (sampler-param-by-name params "decay"))
    (sampler-bpm-control (sampler-param-by-name params "bpm"))))

(def sampler-selection-start-prop (inst)
  (if (get inst :start-time-field)
    (bind-seq (get inst :start-time-field))
    (get inst :start-time)))

(def sampler-selection-end-prop (inst)
  (if (get inst :end-time-field)
    (bind-seq (get inst :end-time-field))
    (get inst :end-time)))

(def sampler-panel-content (inst)
  (let ((body
        (v-stack
          (box :background-color :instrument-control-bg :corner-radius 10
            (v-stack :gap 0.0
              (box :height 0.3)
              (if (get inst :buffer)
                (subtree :key (str "sampler-waveform-" (get inst :buffer))
                  (box :width 81 :height 4.4
                    (waveform
                      :height 3.6
                      :header-height 0.3
                      :ruler-font-size 8
                      :ruler-color :dim
                      :ruler-bg :black
                      :grid-major-color :black
                      :grid-minor-color :black
                      :bg :instrument-control-bg
                      :focusable true
                      :marker-selection true
                      :active-marker sampler-active-marker
                      :marker-color :dim
                      :active-marker-color :widget-knob-filled
                      :waveform-color :yellow
                      :inactive-waveform-color '(rgba 0.25 0.25 0.25 1)
                      :buffer (get inst :buffer)
                      :view-start sampler-view-start
                      :view-duration (if (= sampler-view-duration 0) (get inst :duration) sampler-view-duration)
                      :cursor-time sampler-cursor-time
                      :playhead-time (bind-seq "sampler-playhead")
                      :selection-start (sampler-selection-start-prop inst)
                      :selection-end (sampler-selection-end-prop inst)
                      :time-ruler (dict :mode :seconds)
                      :on-action |event| (handle-sampler-waveform-action inst event (get inst :duration)))))
                (box :width 70 :height 4.2 :h-align :center :v-align :center
                  (label "No sample" :font-size 12 :color :dim :bg :transparent)))
              (sampler-param-pickers (get inst :synth) inst)))
          (sampler-param-knobs (get inst :synth) inst))))
    (if instrument-mods-open
      (h-stack :debug-name "sampler-mods-inline-body" :height :fill :gap 0.45 :align :stretch
        (instrument-mod-control-panel inst)
        body)
      body)))

(def sampler-panel (inst)
  (box :background "fx-panel-bg" :color :instrument-panel-bg :header :fx-panel-header-bg :selected-header :fx-panel-header-selected-bg :selected 0 :padding 0
    :height fx-fixed-panel-height
    :debug-name "sampler-panel"
    :drop-types (sampler-panel-drop-types inst)
    :drop-meta (sampler-panel-drop-meta inst)
    :drop-hover-border-color :mixer-strip-selected-border
    :on-drop (lambda (event) (sbrowser-drop-sound-on-track event))
    (v-stack :gap 0 :height :fill
      (box :debug-name "sampler-header-box" :width :fill :height 1 :padding 0 :v-align :center :h-align :start
        (h-stack :gap 0.5 :align :center :width :fill
          (fx-panel-header-leading-spacer)
          (fx-enabled-toggle (enabled-param (get inst :synth)) false "sampler-enabled")
          (label "Sampler" :font-size 11 :color :white :bg :transparent)
          (instrument-synth-button)
          (instrument-mods-toggle-button)
          (box :flex 1 :height 0.15)
          (if (= (get inst :rack-slot) nil)
            (button "Group Rack"
              :width 6.2 :height 0.78 :padding 0.1 :font-size 8
              :on-click |x y r|
                (host-command "group-track-to-instrument-rack"
                  (dict :track (get inst :track))))
            (box :width 0 :height 0))
          (instrument-header-actions-menu inst)
          (box :width 0.25 :height 0.1)))
      (fx-panel-body "sampler-panel-content"
        (sampler-panel-content inst)))))
