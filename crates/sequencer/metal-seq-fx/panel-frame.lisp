;; Common panel framing, headers, and effect selection helpers.
(def fx-panel-body (debug-name children)
  (box
    (v-stack :gap 0 :height :fill
      (box :width 1 :height fx-panel-body-top-spacer-height)
      children)
    :debug-name debug-name
    :on-click (lambda (info) (fx-clear-selected-effect))
    :padding fx-panel-body-padding
    :height :fill
    :v-align :start
    :h-align :start))

(def fx-panel-header-leading-spacer ()
  (box :width 0.4 :height 0))

(def fx-effect-chain-kind (fx)
  (if (get fx :midi-fx)
    "midi"
    (if (get fx :bus-fx) "bus" "audio")))

(def fx-effect-drag-kind (fx)
  (if (get fx :midi-fx)
    "midi-effect-instance"
    (if (get fx :bus-fx) "bus-effect-instance" "audio-effect-instance")))

(def fx-effect-drag-payload (fx title)
  (dict :kind (fx-effect-drag-kind fx)
        :chain (fx-effect-chain-kind fx)
        :track SEQ.current-track
        :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
        :slot (get fx :slot-idx)
        :name title
        :builtin (get fx :builtin)))

(def fx-effect-drop-meta (fx)
  (dict :kind "fx-slot"
        :chain (fx-effect-chain-kind fx)
        :track SEQ.current-track
        :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
        :slot (get fx :slot-idx)))

(def fx-effect-drop-types (fx)
  (if (get fx :midi-fx)
    (list "midi-effect" "effect-instance")
    (if (get fx :bus-fx)
      (list "audio-effect" "effect-instance")
      (list "audio-effect" "effect-instance"))))

(def fx-panel-header (title params fx)
  (box :width :fill :height fx-panel-header-height :padding 0 :v-align :center :h-align :start
    :debug-name (if (get fx :midi-fx) "midi-fx-panel-header" "audio-fx-panel-header")
    :drag-type "effect-instance"
    :drag-payload (fx-effect-drag-payload fx title)
    :on-click (lambda (info)
      (if (get fx :midi-fx)
        (fx-select-midi-effect (get fx :slot-idx))
        (if (get fx :bus-fx)
          (fx-select-bus-effect (get fx :bus-idx) (get fx :slot-idx))
          (fx-select-effect (get fx :slot-idx)))))
    (h-stack :gap 0.5 :align :center
      (fx-panel-header-leading-spacer)
      (fx-enabled-toggle (enabled-param params) fx
        (if (get fx :midi-fx)
          (str "midi-fx-enabled-" (get fx :slot-idx))
          (if (get fx :bus-fx)
            (str "bus-fx-enabled-" (get fx :bus-idx) "-" (get fx :slot-idx))
            (str "audio-fx-enabled-" (get fx :slot-idx)))))
      (label title :font-size 11 :color :white :bg :transparent)
      (if (fx-has-modulators? fx)
        (effect-mods-toggle-button fx)
        (box))
      (if (and (not (get fx :midi-fx)) (not (get fx :builtin)))
        (box :width 4 :height 1.0 :align :center
          :on-click (lambda (info)
            (do
              (fx-clear-selected-effect)
              (host-command "enter-edit-effect"
                (if (get fx :bus-fx)
                  (dict :name title :slot (get fx :slot-idx) :bus (get fx :bus-idx))
                  (dict :name title :slot (get fx :slot-idx))))))
          (label "edit" :font-size 8 :color :dim :bg :transparent))
        (box)))))

(def fx-clear-selected-effect ()
  (seq-clear-delete-target))
