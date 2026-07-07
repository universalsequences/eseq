;; Built-in effect panel shell, enable controls, and instrument header controls.
(defwidget header
  :shader
  (rgba 1 1 1 1))

(def enabled-param (params)
  (nth (filter |p| (= (get p :name) "enabled") params) 0))

(def visible-params (params)
  (filter |p| (not (= (get p :name) "enabled")) params))

(defwidget fx-enabled-dot
  :width 1.55 :height 1.0
  :paint-margin 0.1
  :state (active)
  :bindable (active)
  :shader
  (sdf/fill (sdf/circle 0.86)
    (material :color (if (> active 0.5) (rgba 1.0 0.8 0.12 1.0) (rgba 0 0 0 1)))))

(def fx-enabled-toggle (p fx subtree-key)
  (subtree :key subtree-key
    (box :width 1.55 :height 1.35 :v-align :start :h-align :center :padding 0
      (v-stack :gap 0 :align :center
        (box :width 1.55 :height 0.04)
        (if p
          (fx-enabled-dot
            :active (fx-param-value p)
            :on-click |x y r|
              (if fx
                (fx-toggle-effect-value fx p)
                (fx-toggle-instrument-value p)))
          (box :width 1.55 :height 1.0))))))

(defwidget fx-mini-save-icon
  :width 1.8 :height 0.8
  :paint-margin 0.2
  :state (active)
  :shader
  (let ((fg-col (rgba 0.92 0.92 0.96 1.0))
        (bg-col (if (= active 1)
          (rgba 0.00 0.35 0.82 1.0)
          (rgba 0.08 0.08 0.01 1.0))))
    (sdf/layer
      (sdf/fill
        (sdf/rounded-rect width height 0.5)
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 -0.42
          (sdf/rounded-rect 0.50 0.20 0.08))
        (material :color fg-col))
      (sdf/fill
        (sdf/translate 0.16 -0.42
          (sdf/rounded-rect 0.10 0.16 0.06))
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 0.27
          (sdf/rounded-rect 0.54 0.22 0.08))
        (material :color fg-col)))))

(def fx-panel (title params fx)
  (let ((selected (fx-panel-selected? fx)))
  (box
    (v-stack :gap 0 :height :fill
      (fx-panel-header title params fx)
      (fx-panel-body (if (get fx :midi-fx) "midi-fx-panel-content" "audio-fx-panel-content")
        (if (get fx :midi-fx)
          (midi-fx-panel-body fx)
          (audio-fx-panel-body fx params))))
    :background "fx-panel-bg"
    :color :fx-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :height fx-fixed-panel-height
    :debug-name (if (get fx :midi-fx)
      (str "midi-fx-panel-root-" (get fx :slot-idx) "-" (get fx :name))
      (if (get fx :bus-fx)
        (str "bus-fx-panel-root-" (get fx :bus-idx) "-" (get fx :slot-idx) "-" title)
        (str "audio-fx-panel-root-" (get fx :slot-idx) "-" title)))
    :drop-types (fx-effect-drop-types fx)
    :drop-meta (fx-effect-drop-meta fx)
    :drop-hover-border-color :blue
    :on-drop (lambda (event) (fx-drop-on-effect event))
    :selected (if selected 1 0)
    :padding 0)))

(def midi-fx-panel (title params fx)
  (let ((selected (fx-panel-selected? fx)))
  (box
    (v-stack :gap 0 :height :fill
      (fx-panel-header title params fx)
      (fx-panel-body "midi-fx-panel-content"
        (subtree :key (str "midi-fx-panel-body-" (get fx :slot-idx) "-" (get fx :name))
          (midi-fx-panel-body fx))))
    :background "fx-panel-bg"
    :color :fx-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :height fx-fixed-panel-height
    :debug-name (str "midi-fx-panel-bg-" (get fx :slot-idx) "-" (get fx :name))
    :drop-types (fx-effect-drop-types fx)
    :drop-meta (fx-effect-drop-meta fx)
    :drop-hover-border-color :blue
    :on-drop (lambda (event) (fx-drop-on-effect event))
    :selected (if selected 1 0)
    :padding 0)))

(def instrument-tab-button (text idx width)
  (box :width width :height 1.2 :align :center
    :bg (if (= instrument-panel-tab idx) :dark-gray :transparent)
    :on-click |x y r| (set! instrument-panel-tab idx)
    (label text :font-size 11
      :color (if (= instrument-panel-tab idx) :white :dim)
      :bg :transparent)))

(def instrument-header-button (text active width click)
  (box :width width :height 1.2 :align :center
    :bg (if active :dark-gray :transparent)
    :on-click click
    (label text :font-size 11
      :color (if active :white :dim)
      :bg :transparent)))

(def instrument-header-tab-button (text active width click)
     (v-stack (box :height 0.1)
  (button text
	  :width (* 1.5 width)
    :height 3.2
    :padding 0.05
    :font-size 11
    :shape :tab
    :active (if active 1 0)
    :background-color :transparent
    :active-background-color :black
    :color :dim
    :active-color :white
    :border-color :transparent
    :on-click click)))

(def instrument-synth-button ()
  (instrument-header-tab-button "synth" (and (= instrument-panel-tab 0) (not instrument-mods-open)) 4.5
    (lambda (info) (do (set! instrument-panel-tab 0) (set! instrument-mods-open false)))))

(def instrument-toggle-mods-view ()
  (do
    (set! instrument-panel-tab 0)
    (set! instrument-mods-open (not instrument-mods-open))))

(def instrument-mods-toggle-button ()
  (instrument-header-tab-button "mods" (and (= instrument-panel-tab 0) instrument-mods-open) 4.0
    (lambda (info) (instrument-toggle-mods-view))))

(def instrument-keys-button ()
  (instrument-header-tab-button "keys" (= instrument-panel-tab 1) 4.0
    (lambda (info) (do (set! instrument-panel-tab 1) (set! instrument-mods-open false)))))

(def effect-toggle-mods-view (fx)
  (let ((chain (fx-effect-chain-kind fx))
        (slot (get fx :slot-idx))
        (bus (if (get fx :bus-fx) (get fx :bus-idx) -1)))
    (if (and effect-mods-open
             (= effect-mods-chain chain)
             (= effect-mods-slot slot)
             (= effect-mods-bus bus))
      (set! effect-mods-open false)
      (do
        (set! effect-mods-open true)
        (set! effect-mods-chain chain)
        (set! effect-mods-slot slot)
        (set! effect-mods-bus bus)))))

(def effect-mods-toggle-button (fx)
  (instrument-header-tab-button "mods" (effect-mods-active? fx) 4.0
    (lambda (info) (effect-toggle-mods-view fx))))
