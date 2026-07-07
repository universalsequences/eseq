;; *track* and *fx* buffer definitions and keybindings.
(defwidget black
  :width 2 :height 2
  :shader
  (rgba 0.0 0.0 0 1))

(def fx-empty-track-fallback ()
  (box :width :fill :height :fill :padding 1 :h-align :center :v-align :center
    (v-stack :gap 0.4 :align :center
      (label "Instrument and effects appear here"
        :font-size 12 :color :dim :bg :transparent)
      (compile-progress
        :active (if SEQ.compiling 1 0)
        :width 12 :height 0.3))))

(def selected-bus-effects ()
  (if (fx-has-selected-bus?)
    (nth SEQ.bus-effects selected-bus)
    '()))

(def fx-drop-placeholder-panel ()
  (box :debug-name "fx-drop-placeholder-panel"
       :background-color :buffer-bg
       :corner-radius 10
       :border-color :mixer-strip-border
       :border-width 2
       :drop-types (if (fx-has-selected-bus?)
         (list "audio-effect" "effect-instance")
         (list "audio-effect" "midi-effect" "effect-instance"))
       :drop-meta (dict :kind "fx-append"
                    :chain "append"
                    :track SEQ.current-track
                    :bus (if (fx-has-selected-bus?) selected-bus -1)
                    :slot -1)
       :drop-hover-border-color :mixer-strip-selected-border
       :drop-hover-background-color :mixer-control-bg
       :on-drop (lambda (event) (fx-drop-on-effect event))
       :height fx-fixed-panel-height
       :width 34
       :padding 0
       :h-align :center
       :v-align :center
    (label "Drop Audio or Midi Effect Here"
      :width 30
      :font-size 12
      :h-align :center
      :color :dim
      :bg :transparent)))

(def fx-bus-selection-panel ()
  (v-stack :padding 0.05 :gap 1
    (h-stack :gap 1
      (each (filter |fx| (> (len (get fx :params)) 0) (selected-bus-effects)) |fx slot-idx|
        (subtree :key (str "bus-fx-panel-" (get fx :bus-idx) "-" (get fx :slot-idx) "-" (get fx :name))
          (fx-panel (get fx :name) (get fx :params) fx)))
      (fx-drop-placeholder-panel))))

(effect-buffer "*track*"
  (if (= SEQ.num-tracks 0)
    (fx-empty-track-fallback)
    (box :padding 1.0
      (v-stack :gap 0.2
        (fx-track-parameters-panel)
        (fx-track-accumulator-panel)))))

(effect-buffer "*fx*"
  (if (fx-has-selected-bus?)
    (fx-bus-selection-panel)
    (if (= SEQ.num-tracks 0)
    (fx-empty-track-fallback)
    (v-stack :padding 0.05 :gap 1 
      (h-stack :gap 1
        (each SEQ.instrument-panel |inst inst-idx|
          (instrument-panel inst))
        (each (filter |fx| (> (len (get fx :params)) 0) SEQ.midi-effects) |fx slot-idx|
          (midi-fx-panel (get fx :name) (get fx :params) fx))
        (each (filter |fx| (> (len (get fx :params)) 0) SEQ.effects) |fx slot-idx|
          (subtree :key (str "audio-fx-panel-" (get fx :slot-idx) "-" (get fx :name))
            (fx-panel (get fx :name) (get fx :params) fx)))
        (fx-drop-placeholder-panel))))))

(define-mode "seq-fx-mode" :read-only true)
(mode-bind-key "seq-fx-mode" "BS" "fx-delete-selected-effect")
(mode-bind-key "seq-fx-mode" "Delete" "fx-delete-selected-effect")
(set-buffer-mode-for "*fx*" "seq-fx-mode")

(def fx-delete-selected-plock-row-key ()
  (if (fx-plock-row-selected?)
    (do
      (fx-delete-selected-plock-row)
      true)
    false))

(define-mode "seq-plock-panel-mode" :read-only true)
(mode-bind-key "seq-plock-panel-mode" "BS" "fx-delete-selected-plock-row-key")
(mode-bind-key "seq-plock-panel-mode" "Delete" "fx-delete-selected-plock-row-key")
(set-buffer-mode-for "*track*" "seq-plock-panel-mode")
