;; Common panel framing, headers, and effect selection helpers.
(def fx-panel-body (debug-name children)
  (box
    (v-stack :gap 0 
      children)
    :debug-name debug-name
    :on-click (lambda (info) (fx-clear-selected-effect))
    :padding 0 
    :flex 1
    :v-align :start
    :h-align :start))

(def fx-panel-header-leading-spacer ()
  (box :width 0.4 :height 0))

(def fx-effect-chain-kind (fx)
  (if (get fx :rack-fx)
    "rack"
    (if (get fx :midi-fx)
    "midi"
    (if (get fx :bus-fx) "bus" "audio"))))

(def fx-copy-values-to-all-scenes (fx)
  (host-command "copy-effect-values-to-all-scenes"
    (dict :chain (fx-effect-chain-kind fx)
          :track (get fx :track-idx)
          :bus (if (get fx :bus-fx) (get fx :bus-idx) nil)
          :slot (get fx :slot-idx))))

(def instrument-copy-values-to-all-scenes (inst)
  (host-command "copy-instrument-values-to-all-scenes"
    (dict :track (get inst :track)
          :rack-slot (get inst :rack-slot))))

(def header-actions-menu (debug-name options action)
  (menu-button
    :key debug-name
    :debug-name debug-name
    :icon "•••"
    :options options
    :width 2.25 :height 0.70 :font-size 10
    :bg-color :mixer-control-bg
    :text-color :dim
    :menu-bg :dropdown-menu-bg
    :menu-border-color :dropdown-menu-border
    :hover-bg :dropdown-hover-bg
    :on-change (lambda (item) (action item))))

(def fx-header-actions-menu (fx)
  (header-actions-menu
    (str "effect-header-actions-" (fx-effect-chain-kind fx) "-"
         (if (get fx :bus-fx) (get fx :bus-idx) (get fx :track-idx)) "-"
         (get fx :slot-idx))
    (list "Copy current values to all scenes")
    (lambda (item) (fx-copy-values-to-all-scenes fx))))

(def instrument-group-rack (inst)
  (host-command "group-track-to-instrument-rack"
    (dict :track (get inst :track))))

(def instrument-edit-source (inst)
  (host-command "enter-edit-instrument"
    (dict :name (if (get inst :name) (get inst :name) SEQ.sidebar-instrument-name))))

(def instrument-header-action-options (inst)
  (append
    (append
      (list "Copy current values to all scenes")
      (if (and (= (get inst :rack-slot) nil)
               (not (= (get inst :type) "modulator")))
        (list "Group Rack")
        (list)))
    (if (and (not (= (get inst :type) "sampler"))
             (not (= (get inst :type) "modulator"))
             (not (= (get inst :type) "rack")))
      (list "Edit")
      (list))))

(def instrument-run-header-action (inst item)
  (if (= item "Group Rack")
    (instrument-group-rack inst)
    (if (= item "Edit")
      (instrument-edit-source inst)
      (instrument-copy-values-to-all-scenes inst))))

(def instrument-header-actions-menu (inst)
  (header-actions-menu
    (str "instrument-header-actions-"
         (get inst :track) "-"
         (if (= (get inst :rack-slot) nil) "main" (get inst :rack-slot)))
    (instrument-header-action-options inst)
    (lambda (item) (instrument-run-header-action inst item))))

(def fx-effect-drag-kind (fx)
  (if (get fx :rack-fx)
    "rack-effect-instance"
    (if (get fx :midi-fx)
    "midi-effect-instance"
    (if (get fx :bus-fx) "bus-effect-instance" "audio-effect-instance"))))

(def fx-effect-drag-payload (fx title)
  (dict :kind (fx-effect-drag-kind fx)
        :chain (fx-effect-chain-kind fx)
        :track (if (get fx :rack-fx) (get fx :track-idx) SEQ.current-track)
        :rack-slot (if (get fx :rack-fx) (get fx :rack-slot) -1)
        :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
        :slot (get fx :slot-idx)
        :name title
        :builtin (get fx :builtin)))

(def fx-effect-drop-meta (fx)
  (dict :kind "fx-slot"
        :chain (fx-effect-chain-kind fx)
        :track (if (get fx :rack-fx) (get fx :track-idx) SEQ.current-track)
        :rack-slot (if (get fx :rack-fx) (get fx :rack-slot) -1)
        :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
        :slot (get fx :slot-idx)))

(def fx-effect-drop-types (fx)
    (if (get fx :midi-fx)
    (list "midi-effect" "effect-instance")
    (if (get fx :bus-fx)
      (list "audio-effect" "effect-instance")
      (if (get fx :rack-fx)
        (list "audio-effect" "effect-instance")
        (list "audio-effect" "effect-instance")))))

(def fx-panel-header (title params fx)
  (box :width :fill :height 1 :padding 0 :v-align :center :h-align :start
    :debug-name (if (get fx :midi-fx) "midi-fx-panel-header" "audio-fx-panel-header")
    :drag-type "effect-instance"
    :drag-payload (fx-effect-drag-payload fx title)
    :on-click (lambda (info)
      (if (get fx :midi-fx)
        (fx-select-midi-effect (get fx :slot-idx))
        (if (get fx :rack-fx)
          (fx-select-rack-effect
            (get fx :track-idx)
            (get fx :rack-slot)
            (get fx :slot-idx))
          (if (get fx :bus-fx)
          (fx-select-bus-effect (get fx :bus-idx) (get fx :slot-idx))
          (fx-select-effect (get fx :slot-idx))))))
    (h-stack :gap 0.5 :align :center :width :fill
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
      (box :flex 1 :height 0.15)
      (if (get fx :rack-fx) (box) (fx-header-actions-menu fx))
      (if (and (not (get fx :rack-fx)) (not (get fx :midi-fx)) (not (get fx :builtin)))
        (button "edit" :background-color :black :width 4 :height 0.75 :align :center :font-size 10
          :on-click (lambda (info)
            (do
              (fx-clear-selected-effect)
              (host-command "enter-edit-effect"
                (if (get fx :bus-fx)
                  (dict :name title :slot (get fx :slot-idx) :bus (get fx :bus-idx))
                  (dict :name title :slot (get fx :slot-idx))))))
	  )
        (box)))))

(def fx-clear-delete-selection ()
  (do
    (if (not (= fx-selected-plock-row -1))
      (set! fx-selected-plock-row -1)
      false)
    (seq-clear-delete-target)))

(def fx-clear-selected-effect ()
  (do
    (process-panel-clear-selection)
    (fx-clear-delete-selection)))
