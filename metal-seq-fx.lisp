;; metal-seq-fx.lisp — Effect chain UI for Metal Sequencer
;; Renders to *fx* buffer. Loaded by metal-seq-grid.lisp.

(defstate instrument-panel-tab 0)
(defstate instrument-source-tab 0)
;; These are temporary render-context globals used by generated custom synth UI.
;; They must NOT be defstate: custom UI functions set them while rendering, and
;; writing reactive state during measurement/layout can perturb the layout.
(def synth-ui-current-inst false)
(def synth-ui-current-name "")

(defwidget fx-panel-bg
  :width 1 :height 1
  :shader
  (let ((panel (sdf/rounded-rect (* 1 width) (* 1 height) 0.065))
      ;; Use derivatives to convert a real pixel height into the shader's
      ;; normalized/SDF y-space. This keeps the header bar visually constant
      ;; as panels get taller/shorter.
      (header-h (* 41 (fwidth y)))
      (header-bottom (+ (- height) header-h))
      (header-shape (max panel (- y header-bottom))))
    (sdf/layer
      
      (sdf/fill (sdf/rounded-rect width height 0.065)
        (material
          :lighting
          (lighting
            :edge-min -0.00852
            :edge-max 0.1862
            :light (vec3 -0.313 -0.411531 0.109565)
            :shininess 71.0)
          :color
          (let ((base
                (mix (rgba 0.5 0.5 0.5 1.0) 
                  (mix 
                    (rgba 0.16 0.16 0.16 1) 
                    (rgba 0.2 0.2 0.2 1) 
                    (smoothstep 0 0.001 header-shape))
                  (smoothstep 0 -0.01623 d)))
              (lit (+ 0.5 (* 0.5 diffuse)))
              (shine (* 1 specular)))
            (+ (* base (rgba lit lit lit 1.0)) (rgba shine shine shine 0.0)))))
      )))
  

(defwidget compile-progress
  :width 12 :height 0.3
  :state (active)
  :shader
  (if (= active 0)
    (rgba 0 0 0 0)
    (let ((bar-w 0.3)
          (pos (fract (* 0.5 itime)))
          (bar-x (- (* pos (+ 1 bar-w)) (/ bar-w 2)))
          (d-bar (- (abs (- x bar-x)) (/ bar-w 2)))
          (bg (sdf/rounded-rect width height 0.06))
          (mask (max bg (- d-bar))))
      (sdf/layer
        (sdf/fill bg
          (material :color (rgba 0.15 0.15 0.17 1)))
          (sdf/fill mask
          (material :color
            (mix
              (rgba 0.3 0.5 1.0 1)
              (rgba 0.2 0.35 0.8 1)
              (smoothstep -0.02 0.02 d-bar))))))))

(def fx-set-instrument-value (p v)
  (if (= (get p :control) "base-note")
    (host-command "set-instrument-base-note" (dict :value v))
    (host-command
      (if (seq-has-selection?) "set-instrument-plock" "set-instrument-param")
      (dict :param-idx (get p :idx) :value v))))

(def fx-set-instrument-option (p label)
  (host-command
    (if (seq-has-selection?) "set-instrument-plock-option" "set-instrument-param-option")
    (dict :param-idx (get p :idx) :label label)))

(def fx-set-effect-value (fx p v)
  (if (seq-has-selection?)
    (seq-set-effect-plock (get fx :slot-idx) (get p :idx) v)
    (seq-set-effect-param (get fx :slot-idx) (get p :idx) v)))

(def fx-param-row (p fx subtree-key)
  (subtree :key subtree-key
    (box :height 1.25
      (h-stack :gap 0.45 :align :center
        (box :width 13.2 :height 1.25
          (h-stack :gap 0.25 :align :baseline
            (label (substring (get p :name) 0 9) :font-size 12 :width 7
                   :color :gray :bg :transparent)
            (if (get p :boolean)
              (box :width 5.5 :height 1.25 :align :center
                   :bg :transparent
                   :on-click |x y r|
                     (if fx
                       (fx-set-effect-value fx p (if (> (get p :value) 0.5) 0 1))
                       (fx-set-instrument-value p (if (> (get p :value) 0.5) 0 1)))
                (label (if (> (get p :value) 0.5) "ON" "OFF")
                       :font-size 13 :width 5.5
                       :color :white :bg :transparent))
              (if (get p :options)
              (dropdown :value (get p :text-value)
                :options (get p :options)
                :on-change (lambda (v)
                  (if fx
                    (host-command
                      (if (seq-has-selection?) "set-effect-plock-option" "set-effect-param-option")
                      (dict :slot-idx (get fx :slot-idx) :param-idx (get p :idx) :label v))
                    (fx-set-instrument-option p v)))
                :width 5.8 :height 1.2 :font-size 13)
              (number-picker :value (get p :value)
                :min (get p :min) :max (get p :max) :decimals 2
                :noui true :font-size 12 :text-color :gray
                :on-change (lambda (v)
                  (if fx
                    (fx-set-effect-value fx p v)
                    (fx-set-instrument-value p v)))
                :width 5.2 :height 1.1)))))
        (if (or (get p :options) (get p :boolean))
          (label "" :width 7.8 :bg :transparent)
          (hslider :width 7.8 :min (get p :min) :max (get p :max)
                   :value (get p :value)
                   :material (aqua-slider-material)
                   :on-change (lambda (v)
                     (if fx
                       (fx-set-effect-value fx p v)
                       (fx-set-instrument-value p v)))))))))

(def fx-param-grid (params fx)
  (h-stack :gap 1.5
    (each (chunks params 6) |chunk ci|
      (v-stack :gap 0.25
        (each chunk |p pi|
          (fx-param-row p fx
            (if fx
              (str "fx-slot-" (get fx :slot-idx) "-param-" (get p :idx))
              (str "instrument-tab-" instrument-panel-tab "-chunk-" ci "-param-" (get p :idx)))))))))

(def instrument-sources-grid (sections)
  (h-stack :gap 2
    (each sections |section si|
      (v-stack :gap 0.25
        (label (get section :name) :font-size 14 :color :white :bg :transparent)
        (each (get section :params) |p pi|
          (fx-param-row p false
            (str "instrument-source-" si "-param-" (get p :idx))))))))

(def instrument-source-tabs (inst)
  (if (> (len (get inst :sources)) 0)
    (tabs :items (get inst :source-names)
      :bind instrument-source-tab
      :compact true
      :gap 0.75
      :tab-padding 0.5
      :header-height 1.2
      (each (get inst :sources) |section si|
        (fx-param-grid (get section :params) false)))
    (instrument-sources-grid (get inst :sources))))

(defwidget header
  :shader
  (rgba 1 1 1 1))

(defwidget fx-mini-save-icon
  :width 2.0 :height 1.0
  :paint-margin 0.25
  :state (active)
  :shader
  (let ((fg-col (rgba 0.92 0.92 0.96 1.0))
        (bg-col (if (= active 1)
          (rgba 0.00 0.35 0.82 1.0)
          (rgba 0.18 0.18 0.20 1.0))))
    (sdf/layer
      (sdf/fill
        (sdf/rounded-rect width height 0.22)
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 -0.42
          (sdf/rounded-rect 0.30 0.20 0.08))
        (material :color fg-col))
      (sdf/fill
        (sdf/translate 0.16 -0.42
          (sdf/rounded-rect 0.10 0.16 0.06))
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 0.27
          (sdf/rounded-rect 0.34 0.22 0.08))
        (material :color fg-col)))))

(def fx-panel (title params fx)
  (box :background "fx-panel-bg" :padding 0
    (v-stack :gap 0
      (box :height 1.0 :padding 0 :v-align :center :h-align :start
        (h-stack :gap 0.5 :align :center
          (box :width 0.75 :height 0)
          (label title :font-size 13 :color :white :bg :transparent)
          ;; Only show edit button for custom dgenlisp effects (not built-in Filter/Delay)
          (if (and (not (= title "Filter")) (not (= title "Delay")))
            (box :bg :dark-gray :width 4 :height 1.0 :align :center
              :on-click |x y r|
              (host-command "enter-edit-effect"
                (dict :name title :slot (get fx :slot-idx)))
              (label "edit" :font-size 8 :color :gray :bg :transparent))
            (box))))
      (box :padding 1
        (fx-param-grid params fx)))))

(def instrument-tab-button (text idx width)
  (box :width width :height 1.2 :align :center
    :bg (if (= instrument-panel-tab idx) :dark-gray :transparent)
    :on-click |x y r| (set! instrument-panel-tab idx)
    (label text :font-size 15
      :color (if (= instrument-panel-tab idx) :white :gray)
      :bg :transparent)))

(def inst-param (inst name)
  (nth (filter |p| (= (get p :name) name) (get inst :synth)) 0))

(def inst-base-note-param (inst)
  (nth (filter |p| (= (get p :control) "base-note") (get inst :synth)) 0))

(def inst-param-row (inst name key)
  (let ((p (inst-param inst name)))
    (if p
      (fx-param-row p false key)
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def ui-param-control (name)
  (inst-param-row synth-ui-current-inst name (str "custom-ui-" synth-ui-current-name "-" name)))

(def base-note ()
  (let ((p (inst-base-note-param synth-ui-current-inst)))
    (if p
      (fx-param-row p false (str "custom-ui-" synth-ui-current-name "-base-note"))
      (label "missing: base_note" :font-size 10 :color :red :bg :transparent))))

(def custom-instrument-synth-ui (inst)
  false)

(def instrument-synth-panel-body (inst)
  (let ((custom (custom-instrument-synth-ui inst)))
    (if custom
      (box :debug-name "custom-synth-wrapper" :padding 0 :h-align :start :v-align :start custom)
      (box :debug-name "fallback-synth-wrapper"
        (fx-param-grid (get inst :synth) false)))))

(defstate sampler-view-start 0.0)
(defstate sampler-view-duration 0)
(defstate sampler-cursor-time 0.0)

(def sampler-set-start-end (start-seconds end-seconds duration)
  (if (> duration 0)
    (do
      (fx-set-instrument-value (dict :idx 2 :control "param") (* 100 (/ start-seconds duration)))
      (fx-set-instrument-value (dict :idx 3 :control "param") (* 100 (/ end-seconds duration))))))

(def sampler-clamp-start (next-start duration)
  (max 0 (min next-start (max 0 (- duration sampler-view-duration)))))

(def sampler-clamp-duration (next-duration duration)
  (max 0.001 (min next-duration (max 0.001 duration))))

(def handle-sampler-waveform-action (event duration)
  (match event.type
    :set-cursor
    (set! sampler-cursor-time event.time)
    :set-selection
    (sampler-set-start-end event.start event.end duration)
    :clear-selection
    (sampler-set-start-end 0 duration duration)
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

(def sampler-panel (inst)
  (box :background "fx-panel-bg" :padding 0
    (v-stack :gap 0
      (box :height 1 :padding 0 :v-align :center :h-align :start
        (h-stack :gap 0 :align :center
          (box :width 0.75 :height 0)
          (label "Sampler" :font-size 13 :color :white :bg :transparent)))
      (box :padding 1.5
        (v-stack :gap 0.8
          (if (get inst :buffer)
            (subtree :key (str "sampler-waveform-" (get inst :buffer))
              (box :width 25 :height 2.5
                (waveform
                  :height 2.5
                  :header-height 0.3
                  :ruler-font-size 8
                  :ruler-color :gray
                  :ruler-bg :black
                  :grid-major-color (rgba 0.15 0.15 0.15 1)
                  :grid-minor-color (rgba 0.10 0.10 0.10 1)
                  :bg :black
                  :focusable true
                  :buffer (get inst :buffer)
                  :view-start sampler-view-start
                  :view-duration (if (= sampler-view-duration 0) (get inst :duration) sampler-view-duration)
                  :cursor-time sampler-cursor-time
                  :playhead-time SEQ.sampler-playhead
                  :selection-start (get inst :start-time)
                  :selection-end (get inst :end-time)
                  :time-ruler (dict :mode :seconds)
                  :on-action |event| (handle-sampler-waveform-action event (get inst :duration)))))
            (label "No sample" :font-size 12 :color :gray :bg :transparent))
          (fx-param-grid (get inst :params) false))))))

(def instrument-panel (inst)
  (if (= (get inst :type) "sampler")
    (sampler-panel inst)
    (box :debug-name "instrument-panel" :background "fx-panel-bg" :padding 0
      (v-stack :debug-name "instrument-panel-vstack" :gap 0
        (box :debug-name "instrument-header-box" :height 1 :padding 0 :v-align :start :h-align :start
          (h-stack :debug-name "instrument-header-row" :gap 0.6 :align :center
            (box :width 0.75 :height 0)
            (box :debug-name "instrument-name-box" :width 12 :height 1 :v-align :center :h-align :start
              (label (substring (get inst :display-name) 0 12)
                :font-size 13 :width 12 :color :white :bg :transparent))
            (box :debug-name "instrument-edit-button" :bg :dark-gray :width 3.2 :height 0.9 :align :center
              :on-click |x y r|
                (host-command "enter-edit-instrument"
                  (dict :name SEQ.sidebar-instrument-name))
              (label "edit" :font-size 8 :color :gray :bg :transparent))
            (box :debug-name "instrument-preset-button" :width 2 :height 1 :align :center
              (fx-mini-save-icon
                :on-click |x y r| (sbrowser-enter-preset-save)
                :active 0))))
        (box :debug-name "instrument-content-box" :padding 0.35
          (v-stack :debug-name "instrument-content-vstack" :gap 0.0
            (h-stack :debug-name "instrument-tabs-row" :gap 0.85 :align :center
              (instrument-tab-button "synth" 0 4.5)
              (instrument-tab-button "mods" 1 4.0)
              (instrument-tab-button "sources" 2 5.8))
            (if (= instrument-panel-tab 0)
              (instrument-synth-panel-body inst)
              (if (= instrument-panel-tab 1)
                (box :debug-name "mods-wrapper" (fx-param-grid (get inst :mod) false))
                (box :debug-name "sources-wrapper" (instrument-source-tabs inst))))))))))

(effect-buffer "*fx*"
  (v-stack :padding 0.5 :gap 1
    (h-stack :gap 1
      (each SEQ.instrument-panel |inst inst-idx|
        (instrument-panel inst))
      (each (filter |fx| (> (len (get fx :params)) 0) SEQ.effects) |fx slot-idx|
        (fx-panel (get fx :name) (get fx :params) fx))
      ;; Add effect
      (box :background "fx-panel-bg" :padding 1.5
        (v-stack :gap 0.5 :align :center
          (label "+" :font-size 15 :color :gray :bg :transparent)
          (dropdown :value ""
            :options SEQ.available-effects
            :placeholder "Add Effect"
            :on-change (lambda (v)
              (if (= v "+ New Effect")
                (do
                  (set! sbrowser-editor-name "")
                  (host-command "enter-new-effect-editor" (dict)))
                (host-command "add-effect" (dict :name v))))
            :width 12 :height 1.5 :font-size 14)
          (compile-progress
            :active (if SEQ.compiling 1 0)
            :width 12 :height 0.3))))))
