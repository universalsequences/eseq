;; Instrument panel composition for sampler, modulator, and synth tracks.
(def instrument-panel (inst)
  (if (= (get inst :type) "sampler")
    (sampler-panel inst)
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
                  :background-color :black
		  :height 0.75
                  :debug-name "instrument-edit-button" 
                  :font-size 10  
		  :border-color :transparent
                  :on-click |x y r|
                  (host-command "enter-edit-instrument"
                    (dict :name SEQ.sidebar-instrument-name))
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
        :selected 0))))
