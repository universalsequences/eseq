;; Instrument, MIDI FX, and audio FX panel body selection.
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
        (if instrument-mods-open
          (h-stack :debug-name "instrument-mods-inline-body" :height :fill :gap 0.45 :align :stretch
            (instrument-mod-control-panel inst)
            body)
          body)))))

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
    (if (get fx :midi-fx)
      (seq-delete-target? :fx-effect (dict :chain "midi" :slot (get fx :slot-idx)))
      (if (get fx :bus-fx)
        (seq-delete-target? :fx-effect
          (dict :chain "bus" :bus (get fx :bus-idx) :slot (get fx :slot-idx)))
        (seq-delete-target? :fx-effect (dict :chain "audio" :slot (get fx :slot-idx)))))))

(def fx-panel-header-bg (selected)
  (if selected :fx-panel-header-selected-bg :fx-panel-header-bg))
