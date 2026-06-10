;; DJ Mixer built-in FX panel.
(def builtin-fx-dj-mixer-knob (fx label-text p decimals)
  (param-mod-wrapper fx p (str "dj-mixer-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "dj-mixer-param-" (get p :idx) (param-control-key-mode fx p))
      (knob-number :label label-text
        :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals decimals
        :base-value (param-base-value-prop fx p)
        :base-min (param-base-min-prop fx p) :base-max (param-base-max-prop fx p)
        :mod-range-0-slot (param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (param-selected-mod-slot-prop fx p)
        :font-size 10.0 :label-font-size 10.0
        :text-color :fg :label-color :dim
        :width 5.6 :height 2.65 :knob-size 1.9
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def builtin-fx-dj-mixer-toggle-active? (fx p)
  (> (reactive-value (fx-param-value-for fx p)) 0.5))

(def builtin-fx-dj-mixer-toggle-mod-mode? (fx p)
  (and (param-mods-open? fx) (get p :modulatable)))

(def builtin-fx-dj-mixer-toggle-button (fx p label-text)
  (param-mod-wrapper fx p (str "dj-mixer-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "dj-mixer-param-" (get p :idx) (param-control-key-mode fx p))
      (let ((mod-mode (builtin-fx-dj-mixer-toggle-mod-mode? fx p)))
        (button label-text
          :width 5.6 :height 1.2 :padding 0 :font-size 11.0
          :active (fx-param-value-for fx p)
          :background-color :mixer-control-bg
          :active-background-color (if mod-mode (rgba 0.00 0.48 0.95 1.0) (rgba 1.0 0.62 0.25 1.0))
          :color :dim
          :active-color (if mod-mode :white :black)
          :on-click |x y r|
            (let ((active (builtin-fx-dj-mixer-toggle-active? fx p)))
              (if mod-mode
                (param-set-control-value fx p (if active 0 1))
                (fx-toggle-effect-value fx p))))))))

(def builtin-fx-dj-mixer-sync-button (fx p)
  (button "Sync"
    :width 5.6 :height 1.2 :padding 0 :font-size 11.0
    :background-color (if (fx-param-on? p) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
    :color (if (fx-param-on? p) :black :dim)
    :on-click |x y r| (fx-toggle-effect-value fx p)))

(def builtin-fx-dj-mixer-div-button (fx p label-text)
  (button label-text
    :width 2.72 :height 0.92 :padding 0 :font-size 8.0
    :background-color (if (= (get p :text-value) label-text) (rgba 1.0 0.62 0.25 1.0) :mixer-control-bg)
    :color (if (= (get p :text-value) label-text) :black :dim)
    :on-click |x y r| (builtin-fx-set-effect-option fx p label-text)))

(def builtin-fx-dj-mixer-div-grid (fx p)
  (v-stack :gap 0.11
    (h-stack :gap 0.12
      (builtin-fx-dj-mixer-div-button fx p "1/16")
      (builtin-fx-dj-mixer-div-button fx p "1/8"))
    (h-stack :gap 0.12
      (builtin-fx-dj-mixer-div-button fx p "1/4")
      (builtin-fx-dj-mixer-div-button fx p "1/2"))
    (h-stack :gap 0.12
      (builtin-fx-dj-mixer-div-button fx p "1 bar")
      (builtin-fx-dj-mixer-div-button fx p "2 bars"))))

(def builtin-fx-dj-mixer-length-section (fx sync-p div-p length-p)
  (box :width 6.4 :height 7.45 :padding 0.28
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.22 :align :center
      (builtin-fx-dj-mixer-sync-button fx sync-p)
      (if (fx-param-on? sync-p)
        (builtin-fx-dj-mixer-div-grid fx div-p)
        (builtin-fx-dj-mixer-knob fx "length" length-p 3)))))

(def builtin-fx-dj-mixer-ui (fx)
  (let ((params (get fx :params)))
    (let ((enabled-p (builtin-fx-param params "enabled"))
          (speed-p (builtin-fx-param params "speed"))
          (length-p (builtin-fx-param params "length"))
          (loop-p (builtin-fx-param params "loop"))
          (sync-p (builtin-fx-param params "sync"))
          (div-p (builtin-fx-param params "div"))
          (warp-p (builtin-fx-param params "warp")))
      (if (and speed-p length-p loop-p sync-p div-p warp-p)
        (h-stack :gap 0.35 :align :start
          (builtin-fx-dj-mixer-length-section fx sync-p div-p length-p)
          (box :width 6.4 :height 7.45 :padding 0.28
               :background-color :fx-inner-panel-bg :corner-radius 7
            (v-stack :gap 0.26 :align :center
              (builtin-fx-dj-mixer-knob fx "speed" speed-p 2)
              (builtin-fx-dj-mixer-knob fx "warp" warp-p 2)))
          (box :width 6.4 :height 7.45 :padding 0.28
               :background-color :fx-inner-panel-bg :corner-radius 7
            (v-stack :gap 0.26 :align :center
              (builtin-fx-dj-mixer-toggle-button fx loop-p "Loop")
              (box :height 0.32 :width 5.6)
              (if enabled-p
                (builtin-fx-dj-mixer-toggle-button fx enabled-p "On")
                (box :width 0 :height 0)))))
        (fx-param-grid params fx)))))
