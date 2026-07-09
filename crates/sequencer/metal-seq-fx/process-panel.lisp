;; Ordered, step-time process chain shown before the instrument/MIDI-FX strip.
;; This panel deliberately has no empty state: a track without an attached
;; process should not imply that a process stage exists in its signal path.

(defwidget process-panel-enabled-dot
  :width 1.35 :height 0.9
  :paint-margin 0.1
  :state (active)
  :bindable (active)
  :shader
  (sdf/fill (sdf/circle 0.78)
    (material :color
      (if (> active 0.5)
        (rgba 1.0 0.8 0.12 1.0)
        (rgba 0 0 0 1.0)))))

(def process-panel-clear-selection ()
  (do
    (process-map-clear)
    (set! process-panel-selected-track -1)
    (set! process-panel-selected-instance-id 0)))

(def process-panel-slot-selected? (slot)
  (and (= process-panel-selected-track SEQ.current-track)
       (= process-panel-selected-instance-id (get slot :instance-id))))

(def process-panel-select-slot (slot)
  (do
    (if (not (process-panel-slot-selected? slot))
      (process-map-clear)
      nil)
    (set! process-panel-selected-track SEQ.current-track)
    (set! process-panel-selected-instance-id (get slot :instance-id))
    (fx-clear-delete-selection)))

(def process-panel-selected-slot ()
  (if (and (not (fx-has-selected-bus?))
           (= process-panel-selected-track SEQ.current-track))
    (let ((matches
            (filter
              (lambda (slot)
                (= (get slot :instance-id) process-panel-selected-instance-id))
              SEQ.process-slots)))
      (if (> (len matches) 0) (nth matches 0) nil))
    nil))

(def process-panel-source-read-only? (path)
  (string-ends-with? path "processes/builtin.lisp"))

(def process-panel-open-slot-source (slot)
  (let ((path (get slot :source-path)))
    (if (and path (not (= path "")))
      (host-command "open-script-source-tab"
        (dict :path path
              :label (str (get slot :process) " source")
              :read-only (process-panel-source-read-only? path)))
      (status (str "No source file is registered for " (get slot :process))))))

(def process-panel-open-selected-source ()
  (let ((slot (process-panel-selected-slot)))
    (if slot
      (do
        (process-panel-open-slot-source slot)
        true)
      false)))

(def process-panel-delete-selected ()
  (let ((slot (process-panel-selected-slot)))
    (if slot
      (do
        (seq-remove-process-slot SEQ.current-track (get slot :instance-id))
        (process-panel-clear-selection)
        true)
      false)))

(def process-panel-toggle-enabled (slot)
  (seq-set-process-slot-enabled
    SEQ.current-track
    (get slot :instance-id)
    (not (get slot :enabled))))

(def process-panel-port-status-color (slot port)
  (if (process-map-port-active? SEQ.current-track slot port)
    (rgba 0.95 0.48 0.18 0.22)
    (if (= (get port :status) "bound")
      (rgba 0.34 0.36 0.38 0.42)
      (if (= (get port :status) "hint")
        (rgba 0.25 0.27 0.31 0.42)
        (rgba 0.18 0.19 0.21 0.42)))))

(def process-panel-port-action-label (slot port)
  (if (process-map-port-active? SEQ.current-track slot port)
    "armed"
    (if (get port :bindable) "map" "unavailable")))

(def process-panel-port-row (slot port)
  (box
    :key (str "process-panel-port-" (get slot :instance-id) "-" (get port :name))
    :width :fill :padding 0.10 :corner-radius 5
    :background-color (process-panel-port-status-color slot port)
    (v-stack :gap 0.08
        (h-stack :width :fill :gap 0.25 :align :baseline
        (label (get port :label)
          :width 5.0 :font-size 8.5 :color :white :bg :transparent)
        (label (get port :status)
          :width 4.0 :font-size 8.0 :color :dim :bg :transparent)
        (button (process-panel-port-action-label slot port)
          :key (str "process-panel-map-" (get slot :instance-id) "-" (get port :name))
          :width 5.0 :height 0.92 :padding 0 :font-size 8.4
          :background-color :transparent :border-color :transparent :color :white
          :on-click (lambda (event)
            (if (get port :bindable)
              (process-map-arm-port SEQ.current-track slot port)
              nil)))
        (if (get port :clearable)
          (button "clear"
            :key (str "process-panel-clear-" (get slot :instance-id) "-" (get port :name))
            :width 3.5 :height 0.92 :padding 0 :font-size 8.2
            :background-color :transparent :border-color :transparent :color :dim
            :on-click (lambda (event)
              (do
                (seq-clear-process-port-binding
                  SEQ.current-track
                  (get slot :instance-id)
                  (get port :name))
                (process-map-clear))))
          (box :width 3.5 :height 0.92)))
      (label (get port :target)
        :width :fill :font-size 8.2 :color :dim :bg :transparent))))

(def process-panel-inlet-row (slot inlet)
  (h-stack
    :key (str "process-panel-inlet-" (get slot :instance-id) "-" (get inlet :name))
    :width :fill :gap 0.35 :align :center
    (v-stack :width 8.0 :gap 0
      (label (get inlet :label)
        :font-size 8.8 :color :white :bg :transparent)
      (if (not (= (get inlet :doc) ""))
        (label (substring (get inlet :doc) 0 18)
          :font-size 7.5 :color :dim :bg :transparent)
        (box :height 0)))
    (number-picker
      :key (str "process-panel-inlet-control-" (get slot :instance-id) "-" (get inlet :name))
      :value (get inlet :value)
      :min (get inlet :min)
      :max (get inlet :max)
      :decimals (get inlet :decimals)
      :noui true :font-size 9 :text-color :white :text-align :right
      :on-change (lambda (value)
        (seq-set-process-inlet
          SEQ.current-track
          (get slot :instance-id)
          (get inlet :name)
          value))
      :width 6.2 :height 1.0)))

(def process-panel-mappable-ports (slot)
  (filter (lambda (port) (get port :mappable))
    (if (get slot :ports) (get slot :ports) '())))

(def process-panel-slot-editor (slot)
  (let ((inlets (if (get slot :inlets) (get slot :inlets) '()))
        (ports (process-panel-mappable-ports slot)))
    (box :width :fill :padding 0.28
      :debug-name (str "process-panel-slot-editor-" (get slot :instance-id))
      :background-color :instrument-control-bg
      (v-stack :width :fill :gap 0.14
        (if (not (= (get slot :doc) ""))
          (label (substring (get slot :doc) 0 48)
            :width :fill :font-size 8.0 :color :dim :bg :transparent)
          (box :height 0))
        (each inlets |inlet|
          (process-panel-inlet-row slot inlet))
        (each ports |port|
          (process-panel-port-row slot port))
        (if (and (= (len inlets) 0) (= (len ports) 0))
          (label "No inline controls"
            :font-size 8.2 :color :dim :bg :transparent)
          (box :height 0))))))

(def process-panel-drag-payload (slot)
  (dict :kind "process-instance"
        :track SEQ.current-track
        :instance-id (get slot :instance-id)))

(def process-panel-drop-meta (slot)
  (dict :kind "process-slot"
        :track SEQ.current-track
        :before-instance-id (get slot :instance-id)))

(def process-panel-drop-at-end-meta ()
  (dict :kind "process-slot-end"
        :track SEQ.current-track))

(def process-panel-drop (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (if (and (= (get payload :kind) "process-instance")
             (= (get payload :track) (get target :track)))
      (seq-move-process-slot-before
        (get target :track)
        (get payload :instance-id)
        (if (= (get target :kind) "process-slot-end")
          nil
          (get target :before-instance-id)))
      (status "Move processes within the same track chain"))))

(def process-panel-slot-header (slot index)
  (box :key (str "process-panel-header-" (get slot :instance-id))
    :width :fill :height 1.34 :padding 0.10
    :background-color
      (if (process-panel-slot-selected? slot)
        (rgba 0.30 0.32 0.34 1.0)
        (rgba 1 1 1 0.025))
    :on-click (lambda (event) (process-panel-select-slot slot))
    :on-double-click (lambda (event) (process-panel-open-slot-source slot))
    (h-stack :debug-name (str "process-panel-slot-header-row-" (get slot :instance-id))
      :width :fill :gap 0.30 :align :baseline
      (label (str (+ index 1))
        :width 1.25 :font-size 8.5 :color :dim :bg :transparent)
      (box :key (str "process-panel-enabled-" (get slot :instance-id))
        :width 1.35 :height 1.05 :padding 0
        :on-click (lambda (event) (process-panel-toggle-enabled slot))
        (process-panel-enabled-dot
          :active (if (get slot :enabled) 1 0)))
      (label (substring (get slot :process) 0 (min 24 (len (get slot :process))))
        :flex 1 :font-size 9.4
        :color (if (get slot :enabled) :white :dim) :bg :transparent)
      (if (not (get slot :enabled))
        (label "BYPASS" :width 4.2 :font-size 7.5 :color :dim :bg :transparent)
        (box :width 0))
      (button "edit"
        :key (str "process-panel-edit-" (get slot :instance-id))
        :width 3.2 :height 0.9 :padding 0 :font-size 8
        :background-color :transparent :border-color :transparent :color :dim
        :on-click (lambda (event) (process-panel-open-slot-source slot))))))

(def process-panel-slot (slot index)
  (subtree :key (str "process-panel-slot-" (get slot :instance-id))
    (box :key (str "process-panel-slot-drop-" (get slot :instance-id))
      :width :fill :padding 0 :corner-radius 5
      :debug-name (str "process-panel-slot-" index)
      :border-width (if (process-panel-slot-selected? slot) 0.8 0.35)
      :border-color
        (if (process-panel-slot-selected? slot)
          (rgba 0.48 0.50 0.52 1.0)
          (rgba 1 1 1 0.10))
      :drag-type "process-instance"
      :drag-payload (process-panel-drag-payload slot)
      :drop-types (list "process-instance")
      :drop-meta (process-panel-drop-meta slot)
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-hover-background-color (rgba 0.22 0.23 0.25 0.42)
      :on-drop (lambda (event) (process-panel-drop event))
      (v-stack :width :fill :gap 0
        (process-panel-slot-header slot index)
        (if (process-panel-slot-selected? slot)
          (process-panel-slot-editor slot)
          (box :height 0))))))

(def process-panel-end-drop-zone ()
  (box :key "process-panel-end-drop-zone"
    :width :fill :height 0.55 :padding 0 :corner-radius 3
    :border-width 0.35 :border-color (rgba 1 1 1 0.08)
    :drop-types (list "process-instance")
    :drop-meta (process-panel-drop-at-end-meta)
    :drop-hover-border-color :mixer-strip-selected-border
    :drop-hover-background-color (rgba 0.22 0.23 0.25 0.42)
    :on-click (lambda (event) (process-panel-clear-selection))
    :on-drop (lambda (event) (process-panel-drop event))))

(def process-chain-panel ()
  (box :width 26 :height fx-fixed-panel-height :padding 0
    :debug-name "process-chain-panel"
    :background "fx-panel-bg"
    :color :instrument-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :selected 0
    (v-stack :width :fill :height :fill :gap 0
      (box :debug-name "process-panel-header-box"
        :width :fill :height 1 :padding 0 :v-align :center :h-align :start
        :on-click (lambda (event) (process-panel-clear-selection))
        (h-stack :width :fill :gap 0.5 :align :center
          (fx-panel-header-leading-spacer)
          (label "PROCESS"
            :font-size 11 :color :white :bg :transparent)
          (box :flex 1 :height 0.1)
          (label "PRE MIDI"
            :font-size 8.5 :color :dim :bg :transparent)
          (box :width 0.4 :height 0.1)))
      (scroll :key (str "process-chain-scroll-" SEQ.current-track)
        :width :fill :flex 1
        :on-click (lambda (event) (process-panel-clear-selection))
        (v-stack :width :fill :padding 0.24 :gap 0.16
          (each SEQ.process-slots |slot index|
            (process-panel-slot slot index))
          (process-panel-end-drop-zone))))))
