;; Ordered, step-time process chain shown before the instrument/MIDI-FX strip.
;; This panel deliberately has no empty state: a track without an attached
;; process should not imply that a process stage exists in its signal path.

(module eseq.effects.process-panel)

(import eseq.kinds :refer (selection set-process-enabled! remove-process! move-process!
                           clear-port!))
(import eseq.effects.state :as st :refer (process-panel-view))
(import eseq.effects.devices :as dv)
(import eseq.effects.param-controls :as pc)
(import eseq.effects.panel-widgets :as pw)
(import eseq.effects.panel-frame :as pf)

(export clear-selection
        track-process-rows
        select-slot
        selected-slot
        open-selected-source
        delete-selected
        process-chain-panel)

;; The panel lists the current track's own processes as eseq.kinds
;; `process` instances: `selection.track`'s `processes` but the project
;; layer's (`p.project`, edited from the lane strip). Its selection is the
;; process-panel-view singleton (the track's position and the process's
;; stable id).

(defwidget process-panel-enabled-dot
  :width 1.35 :height 0.9
  :paint-margin 0.1
  :state (active)
  :shader
  (sdf/fill (sdf/circle 0.78)
    (material :color
      (if (> active 0.5)
        (rgba 1.0 0.8 0.12 1.0)
        (rgba 0 0 0 1.0)))))

(def clear-selection ()
  (pc/process-map-clear)
  (if (or (not (= process-panel-view.track -1))
          (not (= process-panel-view.instance-id 0)))
    (do
      (set! process-panel-view.track -1)
      (set! process-panel-view.instance-id 0))
    false))

(def slot-selected? (slot)
  (and (= process-panel-view.track (dv/current-track-index))
       (= process-panel-view.instance-id slot.proc-id)))

(def select-slot (slot)
  (if (not (slot-selected? slot))
    (pc/process-map-clear)
    nil)
  (set! process-panel-view.track (dv/current-track-index))
  (set! process-panel-view.instance-id slot.proc-id)
  (pf/fx-clear-delete-selection))

;; The current track's process with stable id `id`, or nil.
(def process-of (id)
  (let ((t selection.track))
    (if t (first (filter (lambda (p) (= p.proc-id id)) t.processes)) nil)))

(def selected-slot ()
  (if (and (not (pw/has-selected-bus?))
           (= process-panel-view.track (dv/current-track-index)))
    (process-of process-panel-view.instance-id)
    nil))

(def source-read-only? (path)
  (string-ends-with? path "processes/builtin.lisp"))

(def open-slot-source (slot)
  (let ((path slot.source-path))
    (if path
      (host-command "open-script-source-tab"
        (dict :path path
              :label (str slot.class-name " source")
              :read-only (source-read-only? path)))
      (status (str "No source file is registered for " slot.class-name)))))

(def open-selected-source ()
  (let ((slot (selected-slot)))
    (if slot
      (do
        (open-slot-source slot)
        true)
      false)))

(def delete-selected ()
  (let ((slot (selected-slot)))
    (if slot
      (do
        (remove-process! slot)
        (clear-selection)
        true)
      false)))

(def toggle-enabled (slot)
  (set-process-enabled! slot (not slot.enabled)))

;; The process-map arm addresses a process and its port by dicts (pc's
;; shape: sequencer.lisp arms it too).
(def map-slot (slot) (dict :instance-id slot.proc-id))
(def map-port (port) (dict :name port.name :target-kind port.target-kind))

(def port-armed? (slot port)
  (pc/process-map-port-active? (dv/current-track-index) (map-slot slot) (map-port port)))

(def port-status-color (port armed)
  (if armed
    (rgba 0.95 0.48 0.18 0.22)
    (if (= port.status "bound")
      (rgba 0.34 0.36 0.38 0.42)
      (if (= port.status "hint")
        (rgba 0.25 0.27 0.31 0.42)
        (rgba 0.18 0.19 0.21 0.42)))))

(def port-action-label (port armed)
  (if armed
    "armed"
    (if port.bindable "map" "unavailable")))

(def port-row (slot port)
  (let ((armed (port-armed? slot port)))
    (box
      :key (str "port-" slot.proc-id "-" port.name)
      :width :fill :padding 0.10 :corner-radius 5
      :background-color (port-status-color port armed)
      (v-stack :gap 0.08
        (h-stack :width :fill :gap 0.25 :align :baseline
          (label port.label
            :width 5.0 :font-size 8.5 :color :white :bg :transparent)
          (label port.status
            :width 4.0 :font-size 8.0 :color :dim :bg :transparent)
          (button (port-action-label port armed)
            :key (str "map-" slot.proc-id "-" port.name)
            :width 5.0 :height 0.92 :padding 0 :font-size 8.4
            :background-color :transparent :border-color :transparent :color :white
            :on-click (lambda (event)
              (if port.bindable
                (pc/process-map-arm-port (dv/current-track-index) (map-slot slot) (map-port port))
                nil)))
          ;; Clearable: a binding of its own, or disconnected outright.
          (if (or port.manual port.disconnected)
            (button "clear"
              :key (str "clear-" slot.proc-id "-" port.name)
              :width 3.5 :height 0.92 :padding 0 :font-size 8.2
              :background-color :transparent :border-color :transparent :color :dim
              :on-click (lambda (event)
                (clear-port! port)
                (pc/process-map-clear)))
            (box :width 3.5 :height 0.92)))
        (label port.target
          :width :fill :font-size 8.2 :color :dim :bg :transparent)))))

;; Enum inlets carry their option labels; the value is the option index.
(def inlet-enum-option (inlet index)
  (let ((options inlet.options))
    (if (and (< -1 index) (< index (len options))) (nth options index) "")))

(def inlet-enum-option-index (inlet label)
  (let ((options inlet.options))
    (reduce |acc index| (if (= label (nth options index)) index acc)
      0
      (range 0 (len options)))))

(def inlet-control (slot inlet)
  (if (= inlet.type "enum")
    (dropdown
      :key (str "inlet-control-" slot.proc-id "-" inlet.name)
      :value (inlet-enum-option inlet (floor inlet.value))
      :options inlet.options
      :on-change (lambda (label)
        (set! inlet.value (inlet-enum-option-index inlet label)))
      :width 6.2 :height 1.1 :font-size 8.5)
    (number-picker
      :key (str "inlet-control-" slot.proc-id "-" inlet.name)
      :value #'inlet.value
      :min inlet.min
      :max inlet.max
      :decimals inlet.decimals
      :noui true :font-size 9 :text-color :white :text-align :right
      :on-change (lambda (value) (set! inlet.value value))
      :width 6.2 :height 1.0)))

(def inlet-row (slot inlet)
  (h-stack
    :key (str "inlet-" slot.proc-id "-" inlet.name)
    :width :fill :gap 0.35 :align :center
    (v-stack :width 8.0 :gap 0
      (label inlet.name
        :font-size 8.8 :color :white :bg :transparent)
      (if inlet.doc
        (label (substring inlet.doc 0 18)
          :font-size 7.5 :color :dim :bg :transparent)
        (box :height 0)))
    (inlet-control slot inlet)))

(def mappable-ports (slot)
  (filter (lambda (port) port.mappable) slot.ports))

(def slot-editor (slot)
  (let ((inlets slot.inlets)
        (ports (mappable-ports slot)))
    (box :width :fill :padding 0.28
      :debug-name (str "process-panel-slot-editor-" slot.proc-id)
      :background-color :instrument-control-bg
      (v-stack :width :fill :gap 0.14
        (if slot.doc
          (label (substring slot.doc 0 48)
            :width :fill :font-size 8.0 :color :dim :bg :transparent)
          (box :height 0))
        (each inlets |inlet|
          (inlet-row slot inlet))
        (each ports |port|
          (port-row slot port))
        (if (and (= (len inlets) 0) (= (len ports) 0))
          (label "No inline controls"
            :font-size 8.2 :color :dim :bg :transparent)
          (box :height 0))))))

(def drag-payload (slot)
  (dict :kind "process-instance"
        :track (dv/current-track-index)
        :instance-id slot.proc-id))

(def drop-meta (slot)
  (dict :kind "process-slot"
        :track (dv/current-track-index)
        :before-instance-id slot.proc-id))

(def drop-at-end-meta ()
  (dict :kind "process-slot-end"
        :track (dv/current-track-index)))

(def drop (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (if (and (= (get payload :kind) "process-instance")
             (= (get payload :track) (get target :track))
             (= (get target :track) (dv/current-track-index)))
      (let ((moved (process-of (get payload :instance-id))))
        (if moved
          (move-process! moved
            (if (= (get target :kind) "process-slot-end")
              nil
              (process-of (get target :before-instance-id))))
          nil))
      (status "Move processes within the same track chain"))))

(def slot-header (slot index)
  (box :key (str "header-" slot.proc-id)
    :width :fill :height 1.34 :padding 0.10
    :background-color
      (if (slot-selected? slot)
        (rgba 0.30 0.32 0.34 1.0)
        (rgba 1 1 1 0.025))
    :on-click (lambda (event) (select-slot slot))
    :on-double-click (lambda (event) (open-slot-source slot))
    (h-stack :debug-name (str "process-panel-slot-header-row-" slot.proc-id)
      :width :fill :gap 0.30 :align :baseline
      (label (str (+ index 1))
        :width 1.25 :font-size 8.5 :color :dim :bg :transparent)
      (box :key (str "enabled-" slot.proc-id)
        :width 1.35 :height 1.05 :padding 0
        :on-click (lambda (event) (toggle-enabled slot))
        (process-panel-enabled-dot
          :active (if slot.enabled 1 0)))
      (label (substring slot.class-name 0 (min 24 (len slot.class-name)))
        :flex 1 :font-size 9.4
        :color (if slot.enabled :white :dim) :bg :transparent)
      ;; Project-layer slots are one shared object shown atop every track's
      ;; column; badge them so edits read as project-wide.
      (if slot.project
        (label "PROJECT" :width 4.6 :font-size 7.5
          :color (rgba 0.55 0.72 0.95 1.0) :bg :transparent)
        (box :width 0))
      (if (not slot.enabled)
        (label "BYPASS" :width 4.2 :font-size 7.5 :color :dim :bg :transparent)
        (box :width 0))
      (button "edit"
        :key (str "edit-" slot.proc-id)
        :width 3.2 :height 0.9 :padding 0 :font-size 8
        :background-color :transparent :border-color :transparent :color :dim
        :on-click (lambda (event) (open-slot-source slot))))))

(def slot-row (slot index)
  (subtree :key (str "process-panel-slot-" slot.proc-id)
    (box :key (str "slot-drop-" slot.proc-id)
      :width :fill :padding 0 :corner-radius 5
      :debug-name (str "process-panel-slot-" index)
      :border-width (if (slot-selected? slot) 0.8 0.35)
      :border-color
        (if (slot-selected? slot)
          (rgba 0.48 0.50 0.52 1.0)
          (rgba 1 1 1 0.10))
      :drag-type "process-instance"
      :drag-payload (drag-payload slot)
      :drop-types (list "process-instance")
      :drop-meta (drop-meta slot)
      :drop-hover-border-color :mixer-strip-selected-border
      :drop-hover-background-color (rgba 0.22 0.23 0.25 0.42)
      :on-drop (lambda (event) (drop event))
      (v-stack :width :fill :gap 0
        (slot-header slot index)
        (if (slot-selected? slot)
          (slot-editor slot)
          (box :height 0))))))

(def end-drop-zone ()
  (box :key "end-drop-zone"
    :width :fill :height 0.55 :padding 0 :corner-radius 3
    :border-width 0.35 :border-color (rgba 1 1 1 0.08)
    :drop-types (list "process-instance")
    :drop-meta (drop-at-end-meta)
    :drop-hover-border-color :mixer-strip-selected-border
    :drop-hover-background-color (rgba 0.22 0.23 0.25 0.42)
    :on-click (lambda (event) (clear-selection))
    :on-drop (lambda (event) (drop event))))

(def track-process-rows ()
  (let ((t selection.track))
    (if t (filter (lambda (slot) (not slot.project)) t.processes) '())))

(def process-chain-panel ()
  (box :width 26 :height st/fx-fixed-panel-height :padding 0
    :debug-name "process-chain-panel"
    :background "fx-panel-bg"
    :color :instrument-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :selected 0
    (v-stack :width :fill :height :fill :gap 0
      (box :debug-name "process-panel-header-box"
        :width :fill :height 1 :padding 0 :v-align :center :h-align :start
        :on-click (lambda (event) (clear-selection))
        (h-stack :width :fill :gap 0.5 :align :center
          (pf/fx-panel-header-leading-spacer)
          (label "PROCESS"
            :font-size 11 :color :white :bg :transparent)
          (box :flex 1 :height 0.1)
          (label "PRE MIDI"
            :font-size 8.5 :color :dim :bg :transparent)
          (box :width 0.4 :height 0.1)))
      (scroll :key (str "process-chain-scroll-" (dv/current-track-index))
        :width :fill :flex 1
        :on-click (lambda (event) (clear-selection))
        (v-stack :width :fill :padding 0.24 :gap 0.16
          ;; Project-layer slots (the default lanes and any script-authored
          ;; project layer) are edited from the lane strip in the step editor,
          ;; not here: they are lanes, not effects.
          (each (track-process-rows) |slot index|
            (slot-row slot index))
          (end-drop-zone))))))
