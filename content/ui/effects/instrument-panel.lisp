;; Instrument panel composition for sampler, rack, modulator, and synth tracks.
(module eseq.effects.instrument-panel)

(import eseq.macro-state :as ms :refer (macro-arm))
(import eseq.effects.devices :as dv)
(import eseq.effects.panel-data :as pd)
(import eseq.view-kit :refer (listed?))
(import eseq.effects.state :as st :refer (effect-mods instrument-view))
(import eseq.effects.param-controls :as pc)
(import eseq.effects.drag-drop :as dd)
(import eseq.effects.effect-panels :as ep)
(import eseq.effects.panel-frame :as pf)
(import eseq.effects.panel-bodies :as pb)
(import eseq.effects.track-panels :as tp)
;; Mutual imports with the sampler/modulator panels (this file dispatches to
;; them; sampler-panel routes rack drops back through
;; rack-selected-instrument-drop). Load-once terminates the cycle.
(import eseq.effects.sampler-panel :as sp)
(import eseq.effects.modulator-panel :as mp)

(export rack-panel-toggle-slot-list
        rack-panel-toggle-selected-chain
        rack-panel-toggle-macros
        rack-macro-arm
        rack-panel-drop-on-rack
        rack-selected-instrument-drop
        rack-slot-select
        rack-slot-select-delete-target
        rack-slot-set-gain
        rack-slot-set-choke-group-label
        rack-selected-fx-panel
        rack-slot-fx-drop-panel
        rack-slot-track-fx-divider
        instrument-polyphony-control
        instrument-panel)

;; Migration aliases (module spec §10), all identity: every name below keeps
;; its spelling and is reached flat by an unconverted caller —
;; ui/capture-fixtures/rack-macro-mapping-sidebar.lisp (rack-macro-arm) — or
;; by Rust test evals in src/ui/state_values/tests.rs and src/ui/tests.rs
;; (the rack-panel-toggle-*/rack-slot-*/drop entry points).
;; (The buffers.lisp aliases retired with eseq.effects.buffers, which now
;; imports this module.)

;; `sbrowser-drop-sound-on-track` / `sbrowser-enter-preset-save` stay bare:
;; owned by eseq.browser (a UI-root module that must not be imported from
;; library code); reached through its compat aliases.

(def rack-panel-toggle-slot-list (inst)
  (st/rack-panel-set-view (get inst :track-id)
    (not (st/rack-panel-slot-list-open inst))
    (st/rack-panel-macros-open inst) (st/rack-panel-selected-chain-open inst)))

(def rack-panel-toggle-selected-chain (inst)
  (st/rack-panel-set-view (get inst :track-id)
    (st/rack-panel-slot-list-open inst) (st/rack-panel-macros-open inst)
    (not (st/rack-panel-selected-chain-open inst))))

(def rack-panel-toggle-macros (inst)
  (let ((open (not (st/rack-panel-macros-open inst))))
    (st/rack-panel-set-view (get inst :track-id)
      (st/rack-panel-slot-list-open inst) open (st/rack-panel-selected-chain-open inst))
    (if (not open) (ms/rack-clear-mapping-arm) false)))

(defwidget rack-macro-view-icon
  :width 2.25 :height 1.05 :paint-margin 0.15 :state (active)
  :shader
  (let (
      (disc-border (if (= active 1)
          :rack-view-on
          :white
          ))
      (disc-color (if (= active 1) :rack-view-on :rack-view-off))
      (glyph-color (if (= active 1) :rack-view-on-fg :rack-macro-off-fg)))
    (sdf/layer
      (sdf/fill (sdf/circle 0.72) (material :color disc-border))
      (sdf/fill (sdf/circle 0.68) (material :color disc-color))
      (sdf/fill (sdf/circle 0.31) (material :color glyph-color))
      (sdf/fill (sdf/circle 0.23) (material :color disc-color))
      (sdf/fill (sdf/translate 0.18 -0.22 (sdf/rounded-rect 0.045 0.25 0.025))
        (material :color glyph-color)))))

(defwidget rack-chain-view-icon
  :width 2.25 :height 1.05
  :paint-margin 0.15
  :state (active)
  :shader
  (let ((disc-color (if (= active 1)
          :rack-view-on
          :rack-view-off
          ))
      (disc-border (if (= active 1)
          :rack-view-on
          :white
          ))
      (glyph-color (if (= active 1)
          :rack-view-on-fg
          :rack-view-off-fg)))
    (sdf/layer
      (sdf/fill (sdf/circle 0.72)
        (material :color disc-border))
      (sdf/fill (sdf/circle 0.68)
        (material :color disc-color))
      (sdf/fill (sdf/rounded-rect 0.50 0.12 0.05)
        (material :color glyph-color)))))

(defwidget rack-slot-list-view-icon
  :width 2.25 :height 1.05
  :paint-margin 0.15
  :state (active)
  :shader
  (let (
      (disc-border (if (= active 1)
          :rack-view-on
          :white
          ))
      (disc-color (if (= active 1)
          :rack-view-on
          :rack-view-off))
      (glyph-color (if (= active 1)
          :rack-view-on-fg
          :rack-view-off-fg)))
    (sdf/layer
      (sdf/fill (sdf/circle 0.72)
        (material :color disc-border))
      (sdf/fill (sdf/circle 0.68)
        (material :color disc-color))
      (sdf/fill (sdf/translate -0.39 -0.32 (sdf/circle 0.055))
        (material :color glyph-color))
      (sdf/fill (sdf/translate 0.12 -0.32 (sdf/rounded-rect 0.36 0.09 0.035))
        (material :color glyph-color))
      (sdf/fill (sdf/translate -0.39 0.0 (sdf/circle 0.055))
        (material :color glyph-color))
      (sdf/fill (sdf/translate 0.12 0.0 (sdf/rounded-rect 0.36 0.09 0.035))
        (material :color glyph-color))
      (sdf/fill (sdf/translate -0.39 0.32 (sdf/circle 0.055))
        (material :color glyph-color))
      (sdf/fill (sdf/translate 0.12 0.32 (sdf/rounded-rect 0.36 0.09 0.035))
        (material :color glyph-color)))))

(def rack-panel-view-toolbar (inst)
  (box :debug-name "rack-view-toolbar"
    :width 2.85 :height 9.7
    :padding 0.2 :h-align :center :v-align :start
    (v-stack :width 2.85 :height :fill :gap 0.18 :align :center
      (box :width 2.35 :height 0.18)
      (rack-chain-view-icon
        :key "rack-chain-view-toggle"
        :debug-name "rack-chain-view-toggle"
        :active (if (st/rack-panel-selected-chain-open inst) 1 0)
        :on-click |x y r| (rack-panel-toggle-selected-chain inst))
      (rack-slot-list-view-icon
        :key "rack-slot-list-view-toggle"
        :debug-name "rack-slot-list-view-toggle"
        :active (if (st/rack-panel-slot-list-open inst) 1 0)
        :on-click |x y r| (rack-panel-toggle-slot-list inst))
      (rack-macro-view-icon
        :key "rack-macro-view-toggle"
        :debug-name "rack-macro-view-toggle"
        :active (if (st/rack-panel-macros-open inst) 1 0)
        :on-click |x y r| (rack-panel-toggle-macros inst)))
    ))

(def rack-macro-set (track rm value)
  (host-command (if (seq-has-selection?) "set-rack-macro-plock" "set-rack-macro-value")
    (dict :track track :id rm.index :value value)))

;; Arm rack macro `index` for mapping (its map button), or disarm it when
;; it is the armed one.
(def rack-macro-arm (index)
  (if (= macro-arm.rack-index index)
    (ms/rack-clear-mapping-arm)
    (do
      (ms/clear-mapping-arm)
      (pc/process-map-clear)
      (set! instrument-view.mods-open false)
      (set! effect-mods.open false)
      (set! macro-arm.rack-index index)
      ;; Hook natives register at runtime under flat names; inside a module,
      ;; reach hooks through the data-addressed flat keyspace (spec §10 e).
      (run-hook "macro-mapping-sidebar-open-hook")
      (run-hook "macro-mapping-sidebar-refresh-hook"))))

(def rack-macro-control (track rm)
  (let ((id rm.index)
      (target (dict :track track :target "rack-macro" :param-idx rm.index)))
    (box :key (str "rack-macro-" id) :width 5.7 :height 4.35 :padding 0.18
      :corner-radius 9
      :background-color :mixer-strip-bg :border-color
      (if (= macro-arm.rack-index id) :rack-mapping-border :mixer-strip-border)
      (v-stack :gap 0.08 :align :center
        (subtree :key (str "rack-macro-name-" id)
          (text-input :debug-name (str "rack-macro-name-" id)
            :width 5.2 :height 0.9 :font-size 8.5 :value rm.name
            :on-change (lambda (name) (set! rm.name name))))
        ;; The instrument panel has no subtree of its own, so a by-value
        ;; read here would sit in the *fx* root: the dot and the knob's lock
        ;; state are bound (the menu reads has-locks on the click).
        (box :debug-name (str "rack-macro-control-" id)
          :plock-any #'rm.has-locks
          :on-right-click (lambda (event) (pc/open-target-plock-menu event target rm.has-locks))
          (knob-number :debug-name (str "rack-macro-knob-" id)
            :value #'rm.value :min 0 :max 1 :decimals 2
            :width 4.8 :height 2.45 :knob-size 1.8 :font-size 8 :label-font-size 8
            :plock-active #'rm.locked
            :text-color :fg
            :plock-default #'rm.base
              :plock-color-r (pc/param-plock-color-r)
              :plock-color-g (pc/param-plock-color-g)
              :plock-color-b (pc/param-plock-color-b)
              :on-change (lambda (value) (rack-macro-set track rm value))))
          (button (str "map " (len rm.mappings)) :width 4.6 :height 0.7 :font-size 7.5
            :active (if (= macro-arm.rack-index id) 1 0)
            :background-color :mixer-control-bg
            :active-background-color :rack-mapping-bg
            :border-color :transparent
            :color :dim :active-color :black
            :on-click (lambda (event) (rack-macro-arm id)))))))

;; The rack's macros are its instrument device's (rack-macro instances).
(def rack-macro-bank (inst)
  (let ((track (get inst :track))
        (rack (dv/inst-device inst))
        (macros (if rack rack.macros '())))
    (box :debug-name "rack-macro-bank" :width 24 :height 9.7 :padding 0.2
      :background-color :mixer-control-bg :border-color :buffer-bg :corner-radius 10
      (v-stack :gap 0.15
        (h-stack :gap 0.15
          (rack-macro-cell track macros 0) (rack-macro-cell track macros 1)
          (rack-macro-cell track macros 2) (rack-macro-cell track macros 3))
        (h-stack :gap 0.15
          (rack-macro-cell track macros 4) (rack-macro-cell track macros 5)
          (rack-macro-cell track macros 6) (rack-macro-cell track macros 7))))))

;; Macro i of the bank, or an empty cell while the rack device is not
;; published.
(def rack-macro-cell (track macros i)
  (let ((rm (nth macros i)))
    (if rm (rack-macro-control track rm) (box :width 5.7 :height 4.35))))

(def rack-panel-drop-on-rack (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((track (get target :track))
          (path (get payload :path))
          (name (get payload :name))
          (drag-type (get event :drag-type)))
      (if (= drag-type "sample")
        (if path
          (host-command "add-rack-sample-slot"
            (dict :track track :path path :preserve-browser-context true))
          (status "Drop a sample file, not a folder"))
        (if (= drag-type "instrument")
          (if name
            (host-command "add-rack-instrument-slot"
              (dict :track track :name name))
            (status "Drop an instrument, not a folder"))
          (if (= drag-type "instrument-preset")
            (let ((instrument (eseq.browser/preset-payload-instrument payload)))
              (if instrument
                (host-command "add-rack-instrument-slot"
                  (dict :track track :name instrument :preset (get payload :preset)))
                (status "Drop an instrument preset")))
            (status "Drop a sample or instrument")))))))

(def rack-panel-drop-on-container (event)
  (if (= (get event :drag-type) "sound")
    (eseq.browser/drop-sound-on-track event)
    (rack-panel-drop-on-rack event)))

(def rack-selected-instrument-drop (event)
  (let ((payload (get event :payload))
        (target (get event :target))
        (drag-type (get event :drag-type)))
    (if (= drag-type "sound")
      (eseq.browser/drop-sound-on-track event)
      (if (= drag-type "instrument")
        (let ((name (get payload :name)))
          (if name
            (host-command "replace-rack-slot-instrument"
              (dict :track (get target :track)
                    :slot (get target :slot)
                    :name name))
            (status "Drop an instrument, not a folder")))
        ;; A preset of the layer's own instrument only switches its preset;
        ;; the host decides, like it does for a track swap.
        (if (= drag-type "instrument-preset")
          (let ((instrument (eseq.browser/preset-payload-instrument payload)))
            (if instrument
              (host-command "replace-rack-slot-instrument"
                (dict :track (get target :track)
                      :slot (get target :slot)
                      :name instrument
                      :preset (get payload :preset)))
              (status "Drop an instrument preset")))
        (if (= drag-type "sample")
          (let ((path (get payload :path)))
            (if path
              (host-command "replace-rack-slot-sample"
                (dict :track (get target :track)
                      :slot (get target :slot)
                      :path path
                      :preserve-browser-context true))
              (status "Drop a sample file, not a folder")))
          (status "Drop a sample or instrument")))))))

(def rack-slot-select (slot)
  (host-command "select-rack-slot"
    (dict :track (get slot :track) :slot (get slot :idx))))

(def rack-slot-delete-target-payload (slot)
  (dict :track (get slot :track) :slot (get slot :idx)))

(def rack-slot-select-delete-target (slot)
  (do
    (rack-slot-select slot)
    (seq-set-delete-target :rack-slot (rack-slot-delete-target-payload slot))))

;; The rack slot device a rack panel's slot dict stands for, or nil.
(def slot-device (slot)
  (dv/rack-slot-device (get slot :track) (get slot :idx)))

(def rack-slot-set-param-or-plock (slot param default-command v)
  (host-command (if (seq-has-selection?) "set-rack-slot-param-plock" default-command)
    (dict :track (get slot :track)
          :slot (get slot :idx)
          :param param
          :value v)))

(def rack-slot-set-gain (slot v)
  (rack-slot-set-param-or-plock slot "gain" "set-rack-slot-gain" v))

(def rack-slot-set-pan (slot v)
  (rack-slot-set-param-or-plock slot "pan" "set-rack-slot-pan" v))

(def rack-slot-set-base-note (slot v)
  (rack-slot-set-param-or-plock slot "base-note" "set-rack-slot-base-note" v))

(def rack-slot-set-max-polyphony (slot v)
  (rack-slot-set-param-or-plock slot "max-polyphony" "set-rack-slot-max-polyphony" v))

(def rack-slot-set-mute (slot v)
  (rack-slot-set-param-or-plock slot "mute" "set-rack-slot-mute" v))

(def rack-slot-set-enabled (slot v)
  (host-command "set-rack-slot-enabled"
    (dict :track (get slot :track)
          :slot (get slot :idx)
          :value v)))

(def rack-slot-set-solo (slot v)
  (rack-slot-set-param-or-plock slot "solo" "set-rack-slot-solo" v))

(def rack-choke-group-options ()
  (list "Off" "1" "2" "3" "4" "5" "6" "7" "8" "9" "10" "11" "12" "13" "14" "15" "16"))

(def rack-choke-group-label-value (label)
  (if (= label "Off") 0
    (if (= label "1") 1
      (if (= label "2") 2
        (if (= label "3") 3
          (if (= label "4") 4
            (if (= label "5") 5
              (if (= label "6") 6
                (if (= label "7") 7
                  (if (= label "8") 8
                    (if (= label "9") 9
                      (if (= label "10") 10
                        (if (= label "11") 11
                          (if (= label "12") 12
                            (if (= label "13") 13
                              (if (= label "14") 14
                                (if (= label "15") 15 16)))))))))))))))))

(def rack-slot-set-choke-group-label (slot label)
  (host-command "set-rack-slot-choke-group"
    (dict :track (get slot :track)
          :slot (get slot :idx)
          :value (rack-choke-group-label-value label))))

(def rack-slot-drop-fx (slot event)
  (let ((payload (get event :payload)))
    (if (= (get payload :kind) "rack-effect-instance")
      (dd/drop-existing-effect payload
        (dict :chain "append"
              :track (get slot :track)
              :rack-slot (get slot :idx)))
      (host-command "add-rack-slot-effect"
        (dict :track (get slot :track)
              :rack-slot (get slot :idx)
              :name (get payload :name)
              :builtin (get payload :builtin))))))

;; Slot controls are separate from instrument params, but use the same macro
;; dot: a macro of the rack mapped onto the control (the slot device's
;; strip-macros), as their p-lock dot reads its strip-locks.
(def rack-slot-param-wrapper (inst slot sd param body)
  (let ((target (nth (filter |target| (= (get target :name) param)
                      (get slot :param-targets)) 0))
        (has-locks (listed? param sd.strip-locks)))
    (box :key (str "rack-slot-control-" (get slot :track) "-" (get slot :idx) "-" param)
      :debug-name (str "rack-slot-control-" (get slot :idx) "-" param)
      :background-color :transparent
      :macro-owned (if (listed? param sd.strip-macros) 1 0)
      :plock-any (if has-locks 1 0)
      :on-right-click (lambda (event) (pc/open-target-plock-menu event target has-locks))
      body)))

;; A rack slot's row: its strip, read from the slot device (an empty row
;; while the device is not published).
(def rack-slot-row (inst slot)
  (let ((sd (slot-device slot)))
    (if sd
      (rack-slot-strip inst slot sd)
      (box :key (str "rack-slot-row-" (get slot :idx)) :width 34.6 :height 1.65))))

;; The delete-target highlight is bound (a box's :selected, a label's
;; :active).
(def rack-slot-strip (inst slot sd)
  (let ((selected (get slot :selected))
        (delete-target #'sd.delete-target)
        (muted sd.muted-display)
        (soloed sd.soloed-display))
    (box :key (str "rack-slot-row-" (get slot :idx))
      :width 34.6
      :height 1.65
      :padding 0.18
      :selected delete-target
      :background-color (if selected
        :mixer-strip-selected-bg
        :mixer-strip-bg)
      :selected-background-color :fx-panel-header-selected-bg
      :border-width 1
      :border-color (if selected
        :rack-row-selected-border
        :rack-row-border)
      :selected-border-color :mixer-strip-selected-border
      :corner-radius 10
      :drop-types (list "audio-effect")
      :drop-meta (dict :kind "rack-slot-fx"
        :track (get slot :track)
        :rack-slot (get slot :idx))
      :drop-hover-border-color :mixer-strip-selected-border
      :on-drop (lambda (event) (rack-slot-drop-fx slot event))
      :on-click |x y r| (rack-slot-select slot)
      (h-stack :width :fill :height :fill :gap 0.15 :align :center
        (box :width 1)
        ;; The slot number doubles as the live-set enable toggle (eseq-bw9v):
        ;; a disabled slot gets no triggers and runs no DSP, so a set can park
        ;; one instrument per bank without paying for the parked ones.
        (button (str (+ (get slot :idx) 1))
          :width 1.5 :height 1.02 :padding 0 :font-size 10
          :border-color :transparent
          :background-color (if (get slot :enabled) :transparent :mixer-control-bg)
          :color (if (get slot :enabled) (if selected :fg :gray) :dim)
          :on-click |x y r| (rack-slot-set-enabled slot (not (get slot :enabled))))
        (box :width 1)
        (box :key (str "rack-slot-label-" (get slot :idx))
          :width 9.5 :height :fill  :padding 0
          :selected delete-target
          :background-color :transparent
          :selected-background-color :fx-panel-header-selected-bg
          :corner-radius 3
          :on-click |x y r| (rack-slot-select-delete-target slot)
          (v-stack
            (box :height 0.2)
            (label (substring (get slot :display-name) 0 14)
              :font-size 10.5
              :color (if (get slot :enabled) :fg :dim)
              :active delete-target
              :active-color :white
              :bg :transparent)))
        
        (rack-slot-param-wrapper inst slot sd "base-note"
          (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
            (label "T" :font-size 8.2 :color :dim :bg :transparent)
            (number-picker :value #'sd.base-note-display
              :min (get slot :base-note-min) :max (get slot :base-note-max) :decimals 0
              :noui true :font-size 9.4
              :text-align :center :text-color :dim :edit-color :yellow
              :width 3.55 :height 0.84
              :on-change (lambda (v) (rack-slot-set-base-note slot v)))))
        (rack-slot-param-wrapper inst slot sd "gain"
          (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
            (label "G" :font-size 8.2 :color :dim :bg :transparent)
            (number-picker :value #'sd.gain-display
              :min (get slot :gain-min) :max (get slot :gain-max) :decimals 2
              :noui true :font-size 9.4
              :text-align :center :text-color :dim :edit-color :yellow
              :width 3.55 :height 0.84
              :on-change (lambda (v) (rack-slot-set-gain slot v)))))
        (rack-slot-param-wrapper inst slot sd "pan"
          (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
            (label "P" :font-size 8.2 :color :dim :bg :transparent)
            (number-picker :value #'sd.pan-display
              :min (get slot :pan-min) :max (get slot :pan-max) :decimals 2
              :noui true :font-size 9.4
              :text-align :center :text-color :dim :edit-color :yellow
              :width 3.55 :height 0.84
              :on-change (lambda (v) (rack-slot-set-pan slot v)))))
        (rack-slot-param-wrapper inst slot sd "max-polyphony"
          (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
            (label "V" :font-size 8.2 :color :dim :bg :transparent)
            (number-picker :value #'sd.voices-display
              :min (get slot :max-polyphony-min) :max (get slot :max-polyphony-max) :decimals 0
              :noui true :font-size 9.4
              :text-align :center :text-color :dim :edit-color :yellow
              :width 3.55 :height 0.84
              :on-change (lambda (v) (rack-slot-set-max-polyphony slot v)))))
        (rack-slot-param-wrapper inst slot sd "mute"
          (button "M"
            :width 2.0 :height 1.02 :padding 0 :font-size 9
            :border-color :transparent
            :background-color (if muted :control-on-bg :mixer-control-bg)
            :color (if muted :control-on-fg :dim)
            :on-click |x y r| (rack-slot-set-mute slot (not muted))))
        (rack-slot-param-wrapper inst slot sd "solo"
          (button "S"
            :width 2.0 :height 1.02 :padding 0 :font-size 9
            :border-color :transparent
            :background-color (if soloed :control-on-bg :mixer-control-bg)
            :color (if soloed :control-on-fg :dim)
            :on-click |x y r| (rack-slot-set-solo slot (not soloed))))))))

(def rack-empty-selected-panel (inst)
  (box :debug-name "rack-empty-selected-panel"
       :width 34
       :height st/fx-fixed-panel-height
       :background "fx-panel-bg"
       :color :instrument-panel-bg
       :header :fx-panel-header-bg
       :selected-header :fx-panel-header-selected-bg
       :padding 0
       :selected 0
       :h-align :center
       :v-align :center
       :drop-types (list "sample" "instrument" "instrument-preset" "sound")
       :drop-meta (dict :track (get inst :track))
       :drop-hover-border-color :mixer-strip-selected-border
       :on-drop (lambda (event) (rack-panel-drop-on-container event))
    (label "Drop an Instrument or Sample"
      :font-size 11 :color :dim :bg :transparent)))

(def rack-selected-instrument-panel (inst)
  (let ((selected (get inst :selected-instrument)))
    (if selected
      (h-stack :gap 1 :height st/fx-fixed-panel-height :align :stretch
        (instrument-panel selected)
        (pf/device-output-meter (get selected :meter)))
      (rack-empty-selected-panel inst))))

(def rack-selected-fx-panel (inst)
  (let ((slot-idx (get inst :selected-slot)))
    (if (< slot-idx 0)
      (box :width 0 :height 0)
      ;; The selected slot's running effects, each panel's dict built in its
      ;; own subtree (eseq.effects.panel-data), as a bus effect's.
      (subtree :key (str "rack-selected-fx-" (get inst :track) "-" slot-idx)
        (let ((effects (pd/rack-slot-effect-devices
                         (dv/rack-slot-device (get inst :track) slot-idx))))
          (if (= (len effects) 0)
            (box :width 0 :height 0)
            (h-stack :debug-name "rack-slot-fx-panel"
                     :height st/fx-fixed-panel-height :gap 1 :align :stretch
              (each effects |d fx-idx|
                (h-stack :gap 1 :height st/fx-fixed-panel-height :align :stretch
                  (subtree :key (str "rack-slot-fx-" d.slot "-" d.name)
                    (let ((fx (pd/fx-panel-of d)))
                      (ep/fx-panel (get fx :name) (get fx :params) fx)))
                  (pf/device-output-meter d.meter))))))))))

(def rack-slot-fx-drop-panel (inst)
  (let ((slot-idx (get inst :selected-slot)))
    (if (< slot-idx 0)
      (box :width 0 :height 0)
      (let ((slot (nth (get inst :slots) slot-idx)))
        (box :debug-name "rack-slot-fx-drop-panel"
             :background-color :buffer-bg
             :corner-radius 10
             :border-color :mixer-strip-border
             :border-width 2
             :drop-types (list "audio-effect" "effect-instance")
             :drop-meta (dict :kind "rack-slot-fx"
                              :chain "append"
                              :track (get inst :track)
                              :rack-slot (get slot :idx))
             :drop-hover-border-color :mixer-strip-selected-border
             :drop-hover-background-color :mixer-control-bg
             :on-drop (lambda (event) (rack-slot-drop-fx slot event))
             :height st/fx-fixed-panel-height
             :width 34
             :padding 0
             :h-align :center
             :v-align :center
          (v-stack :gap 0.35 :align :center
            (label "Slot FX" :font-size 9 :color :blue :bg :transparent)
            (label "Drop Audio Effect Here"
              :width 30 :font-size 12 :h-align :center
              :color :dim :bg :transparent)))))))

(def rack-slot-track-fx-divider ()
  (v-stack :debug-name "rack-slot-track-fx-divider"
           :width 1.2 :height st/fx-fixed-panel-height :gap 0 :align :center
    (box :width 0.08 :flex 1 :background-color :mixer-strip-border)))

(def rack-panel-expanded? (inst)
  (or (st/rack-panel-macros-open inst) (st/rack-panel-slot-list-open inst) (st/rack-panel-selected-chain-open inst)))

(def rack-panel (inst)
  (box
    (v-stack :debug-name "rack-panel-vstack" :gap 0 :height :fill
      (box :debug-name "rack-header-box" :height 1 :padding 0 :v-align :center :h-align :start :width :fill
        (h-stack :debug-name "rack-header-row" :gap 0.6 :align :center :width :fill
          (pf/fx-panel-header-leading-spacer)
          (if (rack-panel-expanded? inst)
            (h-stack :debug-name "rack-expanded-header-content" :gap 0.6 :align :start :flex 1
              (label (substring (get inst :display-name) 0 16)
                :v-align :center
                :font-size 11 :color :fg :bg :transparent)
              (box :flex 1 :height 0.15))
            (box :debug-name "rack-compact-header-content"
              :flex 1 :height 0.8 :padding 0 :h-align :center :v-align :center
              (label "R" :width :fill :font-size 8 :text-align :center
                :v-align :center
                :color :dim :bg :transparent))
            )
          (if (rack-panel-expanded? inst)
            (pf/rack-header-actions-menu inst)
            (box :width 0 :height 0))
          (if (rack-panel-expanded? inst)
            (box :debug-name "rack-preset-button" :padding 0 :width 2 :align :center
              (v-stack
                (box :width 1 :height 0.1)
                (fx-mini-save-icon
                  :on-click |x y r| (eseq.browser/enter-preset-save)
                  :active 0)))
            (box :width 0 :height 0))
          (box :width 0.5)))
      (pf/fx-panel-body "rack-content-box"
        (h-stack :debug-name "rack-content-row" :gap 0.20
          :width :fill :align :stretch
          (rack-panel-view-toolbar inst)
          (if (st/rack-panel-macros-open inst) (rack-macro-bank inst) (box :width 0 :height 0))
          (if (st/rack-panel-slot-list-open inst)
            (box
              :background-color :mixer-control-bg
              :border-color :buffer-bg
              :corner-radius 10
              (v-stack :debug-name "rack-chain-list" :gap 0.025 :height 5 :width :fill
                (if (> (len (get inst :slots)) 0)
                  (each (get inst :slots) |slot idx|
                    (rack-slot-row inst slot))
                  (box :width :fill :height 9 :h-align :center :v-align :center
                    (label "Drop an Instrument or Sample"
                      :font-size 11 :color :dim :bg :transparent)))))
            (box :width 0 :height 0)))))
    :key (str "rack-panel-" (get inst :track)) :debug-name "rack-panel"
    :drop-types (list "sample" "instrument" "instrument-preset" "sound")
    :drop-meta (dict :track (get inst :track))
    :drop-hover-border-color :mixer-strip-selected-border
    :on-drop (lambda (event) (rack-panel-drop-on-container event))
    :background "fx-panel-bg"
    :color :instrument-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :padding 0
    :width (max (if (rack-panel-expanded? inst) 18 3.35)
                (+ 3.35 (if (st/rack-panel-slot-list-open inst) 34.7 0) (if (st/rack-panel-macros-open inst) 24.2 0)))
    :height st/fx-fixed-panel-height
    :selected 0))

(def rack-instrument-panel-row (inst)
  (h-stack :debug-name "rack-instrument-panel-row"
           :gap 0.2
           :height st/fx-fixed-panel-height
           :align :stretch
    (rack-panel inst)
    (if (st/rack-panel-selected-chain-open inst)
      (rack-selected-instrument-panel inst)
      (box :width 0 :height 0))))

(def instrument-polyphony-control ()
  (button (if (tp/poly?) "poly" "mono")
    :debug-name "instrument-polyphony" :width 4 :height 0.8 :padding 0
    :font-size 9 :color :fg :background-color :transparent :border-color :transparent
    :on-click |x y r| (tp/toggle-polyphony)
    :on-right-click (lambda (event) (tp/open-polyphony-menu event))))

;; Host-owned pitch reference is available on every custom instrument page,
;; including selected rack-slot instruments, independently of the authored UI.
(def instrument-base-note-control (inst)
  (let ((p (find-by-key (get inst :synth) :control "base-note")))
    (if p
      (pc/instrument-param-mod-wrapper p
        (str "instrument-header-base-note-" (get inst :track) "-" (get inst :rack-slot))
        (h-stack :gap 0.3 :height 0.7 :align :center
          (label "Base note" :height 0.7 :v-align :center :font-size 8 :color :dim :bg :transparent)
          (number-picker :debug-name "instrument-base-note" :width 3.5 :height 0.7
            :noui true :font-size 9 :decimals 0 :step 1
            :value (pc/fx-param-value p) :min (get p :min) :max (get p :max)
            :text-color :dim
            :plock-active (pc/param-plock-active-prop false p)
            :plock-color-r (pc/param-plock-color-r)
            :plock-color-g (pc/param-plock-color-g)
            :plock-color-b (pc/param-plock-color-b)
            :on-change (lambda (v) (pc/fx-set-instrument-value p v)))))
      (box :width 0 :height 0))))

(def instrument-panel (inst)
  (if (= (get inst :type) "sampler")
    (sp/sampler-panel inst)
    (if (= (get inst :type) "rack")
      (rack-instrument-panel-row inst)
      (if (= (get inst :type) "modulator")
        (mp/modulator-panel inst)
        (box
          (v-stack :debug-name "instrument-panel-vstack" :gap 0 :height :fill
            (box :debug-name "instrument-header-box" :height 1 :padding 0 :v-align :center :h-align :start :width :fill
              (h-stack :debug-name "instrument-header-row" :gap 0.6 :align :center :width :fill
                (pf/fx-panel-header-leading-spacer)
                (ep/enabled-toggle (ep/enabled-param (get inst :synth)) false "instrument-enabled")
                (h-stack :v-align :center :height st/fx-panel-header-height :gap 1 :padding 0.1
                  (label (substring (get inst :display-name) 0 12)
                    :font-size 11  :color :fg :bg :transparent)
                  (ep/instrument-synth-button)
                  (ep/instrument-mods-toggle-button)
                  (ep/instrument-keys-button)
                  ;(ep/instrument-sound-binding-badge inst)
                  )
                (box :flex 1 :height 0.15)
                (instrument-base-note-control inst)
                (instrument-polyphony-control)
                (pf/instrument-header-actions-menu inst)
                (box :debug-name "instrument-preset-button" :padding 0.0 :width 2 :align :center
                  (v-stack
                    (box :width 1 :height 0.1)
                    (fx-mini-save-icon
                      :on-click |x y r| (eseq.browser/enter-preset-save)
                      :active 0))
                  )
                (box :width 0.5)
                ))
            (pf/fx-panel-body "instrument-content-box"
              (pb/instrument-synth-panel-body inst)))
          :key (if (= (get inst :rack-slot) nil)
            (str "instrument-panel-" (get inst :track))
            (str "rack-instrument-panel-" (get inst :track) "-" (get inst :rack-slot)))
          :debug-name "instrument-panel"
          :background "fx-panel-bg"
          :color :instrument-panel-bg
          :header :fx-panel-header-bg
          :selected-header :fx-panel-header-selected-bg
          ;; Rack-slot instruments reuse this panel renderer, but the rack owns
          ;; their drop semantics.
          :drop-types (if (= (get inst :rack-slot) nil)
            (list "sample" "instrument" "instrument-preset" "sound")
            (list "sample" "instrument" "instrument-preset" "sound"))
          :drop-meta (if (= (get inst :rack-slot) nil)
            (dict :kind "instrument-panel" :track (get inst :track))
            (dict :kind "rack-selected-instrument"
              :track (get inst :rack-track)
              :slot (get inst :rack-slot)))
          :drop-hover-border-color :mixer-strip-selected-border
          :on-drop (lambda (event)
            (if (= (get inst :rack-slot) nil)
              (eseq.browser/drop-sound-on-track event)
              (rack-selected-instrument-drop event)))
          :padding 0
          :height st/fx-fixed-panel-height
          :selected 0)))))
