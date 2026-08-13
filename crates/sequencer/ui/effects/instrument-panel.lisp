;; Instrument panel composition for sampler, rack, modulator, and synth tracks.
(module eseq.effects.instrument-panel)

(import eseq.macro-state :as ms)
(import eseq.effects.state :as st)
(import eseq.effects.param-controls :as pc)
(import eseq.effects.drag-drop :as dd)
(import eseq.effects.effect-panels :as ep)
(import eseq.effects.panel-frame :as pf)
(import eseq.effects.panel-bodies :as pb)
;; Mutual imports with the sampler/modulator panels (this file dispatches to
;; them; sampler-panel routes rack drops back through
;; rack-selected-instrument-drop). Load-once terminates the cycle.
(import eseq.effects.sampler-panel :as sp)
(import eseq.effects.modulator-panel :as mp)

;; Migration aliases (module spec §10), all identity: every name below keeps
;; its spelling and is reached flat by an unconverted caller —
;; ui/capture-fixtures/rack-macro-mapping-sidebar.lisp (rack-macro-arm) — or
;; by Rust test evals in src/ui/state_values/tests.rs and src/ui/tests.rs
;; (the rack-panel-toggle-*/rack-slot-*/rack-pad-*/drop entry points).
;; (The buffers.lisp aliases retired with eseq.effects.buffers, which now
;; imports this module.)
(module-compat-alias rack-macro-arm rack-macro-arm)
(module-compat-alias rack-panel-toggle-slot-list rack-panel-toggle-slot-list)
(module-compat-alias rack-panel-toggle-selected-chain rack-panel-toggle-selected-chain)
(module-compat-alias rack-panel-toggle-macros rack-panel-toggle-macros)
(module-compat-alias rack-slot-select rack-slot-select)
(module-compat-alias rack-slot-select-delete-target rack-slot-select-delete-target)
(module-compat-alias rack-slot-set-gain rack-slot-set-gain)
(module-compat-alias rack-slot-set-choke-group-label rack-slot-set-choke-group-label)
(module-compat-alias rack-panel-drop-on-rack rack-panel-drop-on-rack)
(module-compat-alias rack-selected-instrument-drop rack-selected-instrument-drop)
(module-compat-alias rack-panel-drop-on-drum-pad rack-panel-drop-on-drum-pad)
(module-compat-alias rack-pad-select rack-pad-select)
(module-compat-alias rack-pad-bank-select rack-pad-bank-select)

;; `sbrowser-drop-sound-on-track` / `sbrowser-enter-preset-save` stay bare:
;; owned by eseq.browser (a UI-root module that must not be imported from
;; library code); reached through its compat aliases.

(def rack-panel-toggle-slot-list ()
  (set! st/rack-panel-slot-list-open (not st/rack-panel-slot-list-open)))

(def rack-panel-toggle-selected-chain ()
  (set! st/rack-panel-selected-chain-open (not st/rack-panel-selected-chain-open)))

(def rack-panel-toggle-macros ()
  (do
    (set! st/rack-panel-macros-open (not st/rack-panel-macros-open))
    (if (not st/rack-panel-macros-open) (ms/rack-clear-mapping-arm) false)))

(defwidget rack-macro-view-icon
  :width 2.25 :height 1.05 :paint-margin 0.15 :state (active)
  :shader
  (let (
      (disc-border (if (= active 1)
          (rgba 1.0 0.58 0.25 1.0)
          :white
          ))
      (disc-color (if (= active 1) (rgba 1.0 0.58 0.25 1.0) (rgba 0.24 0.25 0.26 1.0)))
      (glyph-color (if (= active 1) (rgba 0.10 0.10 0.11 1.0) (rgba 0.72 0.73 0.74 1.0))))
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
          (rgba 1.0 0.58 0.25 1.0)
          (rgba 0.24 0.25 0.26 1.0)
          ))
      (disc-border (if (= active 1)
          (rgba 1.0 0.58 0.25 1.0)
          :white
          ))
      (glyph-color (if (= active 1)
          (rgba 0.10 0.10 0.11 1.0)
          (rgba 0.58 0.59 0.60 1.0))))
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
          (rgba 1.0 0.58 0.25 1.0)
          :white
          ))
      (disc-color (if (= active 1)
          (rgba 1.0 0.58 0.25 1.0)
          (rgba 0.24 0.25 0.26 1.0)))
      (glyph-color (if (= active 1)
          (rgba 0.10 0.10 0.11 1.0)
          (rgba 0.58 0.59 0.60 1.0))))
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

(def %rack-panel-view-toolbar ()
  (box :debug-name "rack-view-toolbar"
    :width 2.85 :height 9.7
    :padding 0.2 :h-align :center :v-align :start
    (v-stack :width 2.85 :height :fill :gap 0.18 :align :center
      (box :width 2.35 :height 0.18)
      (rack-chain-view-icon
        :key "rack-chain-view-toggle"
        :debug-name "rack-chain-view-toggle"
        :active (if st/rack-panel-selected-chain-open 1 0)
        :on-click |x y r| (rack-panel-toggle-selected-chain))
      (rack-slot-list-view-icon
        :key "rack-slot-list-view-toggle"
        :debug-name "rack-slot-list-view-toggle"
        :active (if st/rack-panel-slot-list-open 1 0)
        :on-click |x y r| (rack-panel-toggle-slot-list))
      (rack-macro-view-icon
        :key "rack-macro-view-toggle"
        :debug-name "rack-macro-view-toggle"
        :active (if st/rack-panel-macros-open 1 0)
        :on-click |x y r| (rack-panel-toggle-macros)))
    ))

(def %rack-macro-set (track macro value)
  (host-command (if (seq-has-selection?) "set-rack-macro-plock" "set-rack-macro-value")
    (dict :track track :id (get macro :id) :value value)))

(def %rack-macro-plock-row (macro)
  (nth (filter |row|
    (and (= (get row :target) "rack-macro")
         (= (get row :param-idx) (get macro :id)))
    SEQ.track-plocks) 0))

(def %rack-macro-display-value (macro)
  (if (get macro :value-field)
    (bind-seq (get macro :value-field))
    (get macro :value)))

(def %rack-macro-plock-active (macro)
  (if (get macro :plock-active-field)
    (bind-seq (get macro :plock-active-field))
    0))

(def %rack-macro-plock-default (macro)
  (if (get macro :plock-default-field)
    (bind-seq (get macro :plock-default-field))
    (get macro :value)))

(def rack-macro-arm (macro)
  (let ((next (if (= ms/rack-mapping-selected (get macro :id)) -1 (get macro :id))))
    (if (< next 0)
      (ms/rack-clear-mapping-arm)
      (do
        (ms/clear-mapping-arm)
        (pc/process-map-clear)
        (set! st/instrument-mods-open false)
        (set! st/effect-mods-open false)
        (set! ms/rack-mapping-selected next)
        ;; Hook natives register at runtime under flat names; inside a module,
        ;; reach hooks through the data-addressed flat keyspace (spec §10 e).
        (run-hook "macro-mapping-sidebar-open-hook")
        (run-hook "macro-mapping-sidebar-refresh-hook")))))

(def %rack-macro-control (track macro)
  (let ((plock-row (%rack-macro-plock-row macro)))
    (box :key (str "rack-macro-" (get macro :id)) :width 5.7 :height 4.35 :padding 0.18
      :corner-radius 9
      :background-color :mixer-strip-bg :border-color
      (if (= ms/rack-mapping-selected (get macro :id)) (rgba 0.18 0.85 0.42 0.9) :mixer-strip-border)
      (v-stack :gap 0.08 :align :center
        (text-input :key (str "rack-macro-name-" (get macro :id))
          :width 5.2 :height 0.9 :font-size 8.5 :value (get macro :name)
          :on-change (lambda (name) (host-command "rename-rack-macro"
              (dict :track track :id (get macro :id) :name name))))
        (knob-number :debug-name (str "rack-macro-knob-" (get macro :id))
          :value (%rack-macro-display-value macro) :min 0 :max 1 :decimals 2
          :width 4.8 :height 2.45 :knob-size 1.8 :font-size 8 :label-font-size 8
          :track-color '(rgba 0.4, 0.4, 0.4, 1)
          :plock-active (%rack-macro-plock-active macro)
          :plock-default (%rack-macro-plock-default macro)
          :plock-color-r (pc/param-plock-color-r)
          :plock-color-g (pc/param-plock-color-g)
          :plock-color-b (pc/param-plock-color-b)
          :on-change (lambda (value) (%rack-macro-set track macro value)))
        (button (str "map " (get macro :mapping-count)) :width 4.6 :height 0.7 :font-size 7.5
          :active (if (= ms/rack-mapping-selected (get macro :id)) 1 0)
          :background-color :mixer-control-bg
          :active-background-color (rgba 0.18 0.85 0.42 1.0)
          :border-color :transparent
          :color :dim :active-color :black
          :on-click (lambda (event) (rack-macro-arm macro)))))))

(def %rack-macro-bank (inst)
  (let ((track (get inst :track)) (macros (get inst :macros)))
    (box :debug-name "rack-macro-bank" :width 24 :height 9.7 :padding 0.2
      :background-color :bg :border-color :buffer-bg :corner-radius 10
      (v-stack :gap 0.15
        (h-stack :gap 0.15
          (%rack-macro-control track (nth macros 0)) (%rack-macro-control track (nth macros 1))
          (%rack-macro-control track (nth macros 2)) (%rack-macro-control track (nth macros 3)))
        (h-stack :gap 0.15
          (%rack-macro-control track (nth macros 4)) (%rack-macro-control track (nth macros 5))
          (%rack-macro-control track (nth macros 6)) (%rack-macro-control track (nth macros 7)))))))

(def rack-panel-drop-on-rack (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((track (get target :track))
          (routing (get target :routing))
          (pad-note (get target :pad-note))
          (path (get payload :path))
          (name (get payload :name))
          (drag-type (get event :drag-type)))
      (if (= routing "by-pitch")
        (if (= drag-type "sample")
          (if path
            (host-command "add-rack-sample-pad"
              (dict :track track :pad-note pad-note :path path :preserve-browser-context true))
            (status "Drop a sample file, not a folder"))
          (if (= drag-type "instrument")
            (if name
              (host-command "add-rack-instrument-pad"
                (dict :track track :pad-note pad-note :name name))
              (status "Drop an instrument, not a folder"))
            (status "Drop a sample or instrument")))
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
            (status "Drop a sample or instrument")))))))

(def %rack-panel-drop-on-container (event)
  (if (= (get event :drag-type) "sound")
    (sbrowser-drop-sound-on-track event)
    (rack-panel-drop-on-rack event)))

(def rack-selected-instrument-drop (event)
  (let ((payload (get event :payload))
        (target (get event :target))
        (drag-type (get event :drag-type)))
    (if (= drag-type "sound")
      (sbrowser-drop-sound-on-track event)
      (if (= drag-type "instrument")
        (let ((name (get payload :name)))
          (if name
            (host-command "replace-rack-slot-instrument"
              (dict :track (get target :track)
                    :slot (get target :slot)
                    :name name))
            (status "Drop an instrument, not a folder")))
        (if (= drag-type "sample")
          (let ((path (get payload :path)))
            (if path
              (host-command "replace-rack-slot-sample"
                (dict :track (get target :track)
                      :slot (get target :slot)
                      :path path
                      :preserve-browser-context true))
              (status "Drop a sample file, not a folder")))
          (status "Drop a sample or instrument"))))))

(def rack-panel-drop-on-drum-pad (event)
  (let ((target (get event :target)))
    (rack-panel-drop-on-rack
      (dict :drag-type (get event :drag-type)
            :payload (get event :payload)
            :target (dict :track (get target :track)
                          :routing "by-pitch"
                          :pad-note (get target :pad-note))))))

(def rack-pad-select (pad)
  (host-command "select-rack-pad"
    (dict :track (get pad :track) :pad-note (get pad :pad-note))))

(def rack-pad-bank-select (inst bank)
  (host-command "select-rack-pad-bank"
    (dict :track (get inst :track) :bank-start (get bank :bank-start))))

(def %drum-rack-pad-bank-cell (inst bank)
  (let ((selected (get bank :selected)))
    (box :key (str "drum-rack-bank-" (get bank :bank-start))
         :debug-name "drum-rack-pad-bank"
         :width 3.65
         :height 0.78
         :padding 0.05
         :selected selected
         :background-color (if selected '(rgba 0.34 0.36 0.38 1.0) '(rgba 0.12 0.13 0.14 1.0))
         :border-width 1
         :border-color (if selected '(rgba 0.58 0.60 0.62 1.0) '(rgba 0.20 0.21 0.22 1.0))
         :corner-radius 1
         :on-click |x y r| (rack-pad-bank-select inst bank)
      (label (get bank :label)
        :font-size 5.6
        :color (if selected :white :dim)
        :bg :transparent
        :width :fill
        :text-align :center))))

(def %drum-rack-pad-bank-selector (inst)
  (v-stack :debug-name "drum-rack-pad-bank-selector"
           :width 3.8
           :height st/fx-panel-body-content-height
           :gap 0.055
           :align :center
    (each (get inst :pad-banks) |bank idx|
      (%drum-rack-pad-bank-cell inst bank))))

(def %rack-drum-pad-cell (pad)
  (let ((occupied (get pad :occupied))
      (selected (get pad :selected)))
    (box :key (str "drum-rack-pad-" (get pad :pad-note))
      :debug-name "drum-rack-pad"
      :width 6.55
      :height 2.35
      :padding 0.18
      :selected selected
      :background-color (if selected
        '(rgba 0.30 0.37 0.39 1.0)
        (if occupied '(rgba 0.18 0.22 0.23 1.0) '(rgba 0.12 0.13 0.14 1.0)))
      :border-width 1
      :border-color (if selected '(rgba 0.40 0.82 0.90 1.0) '(rgba 0.30 0.31 0.32 1.0))
      :corner-radius 10
      :drop-types (list "sample" "instrument")
      :drop-meta (dict :kind "drum-rack-pad"
        :track (get pad :track)
        :pad-note (get pad :pad-note))
      :drop-hover-border-color :mixer-strip-selected-border
      :on-drop (lambda (event) (rack-panel-drop-on-drum-pad event))
      :on-click |x y r| (rack-pad-select pad)
      (v-stack :width :fill :height :fill :gap 0.05
        (label (get pad :label)
          :font-size 8.1
          :color (if selected :white :gray)
          :bg :transparent
          :width :fill
          :text-align :center)
        (box :width :fill :flex 1 :h-align :center :v-align :center
          (label (if occupied (substring (get pad :display-name) 0 12) "")
            :font-size 6.8
            :color (if occupied :white :dim)
            :bg :transparent
            :width :fill
            :text-align :center))
        (if occupied
          (h-stack :width :fill :height 0.62 :gap 0.12 :align :center
            (button "M"
              :width 1.0 :height 0.56 :padding 0 :font-size 6.5
              :background-color (if (get pad :mute) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
              :color (if (get pad :mute) :black :dim)
              :on-click |x y r| (%rack-slot-set-mute pad (not (get pad :mute))))
            (button "S"
              :width 1.0 :height 0.56 :padding 0 :font-size 6.5
              :background-color (if (get pad :solo) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
              :color (if (get pad :solo) :black :dim)
              :on-click |x y r| (%rack-slot-set-solo pad (not (get pad :solo))))
            (dropdown :value-index (get pad :choke-group)
              :options (%rack-choke-group-options)
              :width 3.85
              :height 0.56
              :font-size 6.2
              :on-change (lambda (v) (rack-slot-set-choke-group-label pad v))))
          (box :width :fill :height 0.62))))))

(def %drum-rack-pad-row (pads row-start)
  (h-stack :width :fill :height 2.43 :gap 0.18 :align :center
    (%rack-drum-pad-cell (nth pads row-start))
    (%rack-drum-pad-cell (nth pads (+ row-start 1)))
    (%rack-drum-pad-cell (nth pads (+ row-start 2)))
    (%rack-drum-pad-cell (nth pads (+ row-start 3)))))

(def %drum-rack-pad-grid (inst)
  (h-stack :debug-name "drum-rack-pad-grid"
           :width :fill
           :height st/fx-panel-body-content-height
           :gap 0.24
           :align :center
    (%drum-rack-pad-bank-selector inst)
    (v-stack :debug-name "drum-rack-pad-grid-cells"
             :width :fill
             :height st/fx-panel-body-content-height
             :gap 0.08
             :align :center
      (%drum-rack-pad-row (get inst :pads) 0)
      (%drum-rack-pad-row (get inst :pads) 4)
      (%drum-rack-pad-row (get inst :pads) 8)
      (%drum-rack-pad-row (get inst :pads) 12))))

(def rack-slot-select (slot)
  (host-command "select-rack-slot"
    (dict :track (get slot :track) :slot (get slot :idx))))

(def %rack-slot-delete-target-payload (slot)
  (dict :track (get slot :track) :slot (get slot :idx)))

(def rack-slot-select-delete-target (slot)
  (do
    (rack-slot-select slot)
    (seq-set-delete-target :rack-slot (%rack-slot-delete-target-payload slot))))

(def %rack-slot-delete-target-binding (slot)
  (bind-seq (str "rack-slot-delete-target-" (get slot :track) "-" (get slot :idx))))

(def %rack-slot-delete-target? (slot)
  (%rack-slot-delete-target-binding slot))

(def %rack-slot-set-param-or-plock (slot param default-command v)
  (host-command (if (seq-has-selection?) "set-rack-slot-param-plock" default-command)
    (dict :track (get slot :track)
          :slot (get slot :idx)
          :param param
          :value v)))

(def rack-slot-set-gain (slot v)
  (%rack-slot-set-param-or-plock slot "gain" "set-rack-slot-gain" v))

(def %rack-slot-set-pan (slot v)
  (%rack-slot-set-param-or-plock slot "pan" "set-rack-slot-pan" v))

(def %rack-slot-set-base-note (slot v)
  (%rack-slot-set-param-or-plock slot "base-note" "set-rack-slot-base-note" v))

(def %rack-slot-set-max-polyphony (slot v)
  (%rack-slot-set-param-or-plock slot "max-polyphony" "set-rack-slot-max-polyphony" v))

(def %rack-slot-set-mute (slot v)
  (%rack-slot-set-param-or-plock slot "mute" "set-rack-slot-mute" v))

(def %rack-slot-set-solo (slot v)
  (%rack-slot-set-param-or-plock slot "solo" "set-rack-slot-solo" v))

(def %rack-choke-group-options ()
  (list "Off" "1" "2" "3" "4" "5" "6" "7" "8" "9" "10" "11" "12" "13" "14" "15" "16"))

(def %rack-choke-group-label-value (label)
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
          :value (%rack-choke-group-label-value label))))

(def %rack-slot-drop-fx (slot event)
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

(def %rack-slot-display-value (slot prop field-prop)
  (if (get slot field-prop)
    (bind-seq (get slot field-prop))
    (get slot prop)))

;; Only used for mute/solo flags. bind-seq is a float binding (bools arrive as
;; 1.0/0.0) and `not` doesn't negate numbers, so normalize to a real boolean.
(def %rack-slot-display-scalar (slot prop field-prop)
  (let ((field (get slot field-prop)))
    (if field
      (> (reactive-value (bind-seq field)) 0.5)
      (get slot prop))))

(def %rack-slot-row (slot)
  (let ((delete-target (%rack-slot-delete-target? slot))
      (selected (get slot :selected)))
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
        '(rgba 0.48 0.50 0.52 1.0)
        '(rgba 0.16 0.17 0.19 1.0))
      :selected-border-color :mixer-strip-selected-border
      :corner-radius 10
      :drop-types (list "audio-effect")
      :drop-meta (dict :kind "rack-slot-fx"
        :track (get slot :track)
        :rack-slot (get slot :idx))
      :drop-hover-border-color :mixer-strip-selected-border
      :on-drop (lambda (event) (%rack-slot-drop-fx slot event))
      :on-click |x y r| (rack-slot-select slot)
      (h-stack :width :fill :height :fill :gap 0.15 :align :baseline
        (box :width 1)
        (label (str (+ (get slot :idx) 1))
          :font-size 10
          :color (if selected :white :gray)
          :width 1.0
          :bg :transparent)
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
              :color :white
              :active delete-target
              :active-color :white
              :bg :transparent)))

        (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
          (label "T" :font-size 8.2 :color :dim :bg :transparent)
          (number-picker :value (%rack-slot-display-value slot :base-note :base-note-field)
            :min (get slot :base-note-min) :max (get slot :base-note-max) :decimals 0
            :noui true :font-size 9.4
            :text-align :center :text-color :dim :edit-color :yellow
            :width 3.55 :height 0.84
            :on-change (lambda (v) (%rack-slot-set-base-note slot v))))
        (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
          (label "G" :font-size 8.2 :color :dim :bg :transparent)
          (number-picker :value (%rack-slot-display-value slot :gain :gain-field)
            :min (get slot :gain-min) :max (get slot :gain-max) :decimals 2
            :noui true :font-size 9.4
            :text-align :center :text-color :dim :edit-color :yellow
            :width 3.55 :height 0.84
            :on-change (lambda (v) (rack-slot-set-gain slot v))))
        (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
          (label "P" :font-size 8.2 :color :dim :bg :transparent)
          (number-picker :value (%rack-slot-display-value slot :pan :pan-field)
            :min (get slot :pan-min) :max (get slot :pan-max) :decimals 2
            :noui true :font-size 9.4
            :text-align :center :text-color :dim :edit-color :yellow
            :width 3.55 :height 0.84
            :on-change (lambda (v) (%rack-slot-set-pan slot v))))
        (v-stack :width 3.75 :height 1.9 :gap 0.05 :align :center
          (label "V" :font-size 8.2 :color :dim :bg :transparent)
          (number-picker :value (%rack-slot-display-value slot :max-polyphony :max-polyphony-field)
            :min (get slot :max-polyphony-min) :max (get slot :max-polyphony-max) :decimals 0
            :noui true :font-size 9.4
            :text-align :center :text-color :dim :edit-color :yellow
            :width 3.55 :height 0.84
            :on-change (lambda (v) (%rack-slot-set-max-polyphony slot v))))
        (button "M"
          :width 2.0 :height 1.02 :padding 0 :font-size 9
          :border-color :transparent
          :background-color (if (%rack-slot-display-scalar slot :mute :mute-field) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
          :color (if (%rack-slot-display-scalar slot :mute :mute-field) :black :dim)
          :on-click |x y r| (%rack-slot-set-mute slot (not (%rack-slot-display-scalar slot :mute :mute-field))))
        (button "S"
          :width 2.0 :height 1.02 :padding 0 :font-size 9
          :border-color :transparent
          :background-color (if (%rack-slot-display-scalar slot :solo :solo-field) (rgba 0.95 0.48 0.18 1.0) :mixer-control-bg)
          :color (if (%rack-slot-display-scalar slot :solo :solo-field) :black :dim)
          :on-click |x y r| (%rack-slot-set-solo slot (not (%rack-slot-display-scalar slot :solo :solo-field))))))))

(def %rack-empty-selected-panel (inst)
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
       :drop-types (list "sample" "instrument" "sound")
       :drop-meta (dict :kind "rack-empty-selected"
                        :track (get inst :track)
                        :routing (get inst :routing)
                        :pad-note (get inst :selected-pad-note))
       :drop-hover-border-color :mixer-strip-selected-border
       :on-drop (lambda (event) (%rack-panel-drop-on-container event))
    (label "Drop an Instrument or Sample"
      :font-size 11 :color :dim :bg :transparent)))

(def %rack-selected-instrument-panel (inst)
  (let ((selected (get inst :selected-instrument)))
    (if selected
      (instrument-panel selected)
      (%rack-empty-selected-panel inst))))

(def rack-selected-fx-panel (inst)
  (let ((slot-idx (get inst :selected-slot)))
    (if (< slot-idx 0)
      (box :width 0 :height 0)
      (let ((slot (nth (get inst :slots) slot-idx)))
        (if (= (len (get slot :effects)) 0)
          (box :width 0 :height 0)
          (h-stack :debug-name "rack-slot-fx-panel"
                   :height st/fx-fixed-panel-height :gap 1 :align :stretch
            (each (get slot :effects) |fx fx-idx|
              (subtree :key (str "rack-slot-fx-" (get fx :slot-idx) "-" (get fx :name))
                (ep/fx-panel (get fx :name) (get fx :params) fx)))))))))

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
             :on-drop (lambda (event) (%rack-slot-drop-fx slot event))
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

(def %rack-panel (inst)
  (box
    (v-stack :debug-name "rack-panel-vstack" :gap 0 :height :fill
      (box :debug-name "rack-header-box" :height 1 :padding 0 :v-align :center :h-align :start :width :fill
        (h-stack :debug-name "rack-header-row" :gap 0.6 :align :center :width :fill
          (pf/fx-panel-header-leading-spacer)
          (if st/rack-panel-slot-list-open
            (h-stack :debug-name "rack-expanded-header-content" :gap 0.6 :align :center :flex 1
              (label (substring (get inst :display-name) 0 16)
                :font-size 11 :color :white :bg :transparent)
              (box :flex 1 :height 0.15))
            (box :debug-name "rack-compact-header-content"
              :flex 1 :height 0.8 :padding 0 :h-align :center :v-align :center
              (label "R" :width :fill :font-size 8 :text-align :center
                :color :dim :bg :transparent)))
          (if st/rack-panel-slot-list-open
            (box :debug-name "rack-preset-button" :padding 0 :width 2 :align :center
              (v-stack
                (box :width 1 :height 0.1)
                (fx-mini-save-icon
                  :on-click |x y r| (sbrowser-enter-preset-save)
                  :active 0)))
            (box :width 0 :height 0))
          (box :width 0.5)))
      (pf/fx-panel-body "rack-content-box"
        (h-stack :debug-name "rack-content-row" :gap 0.20
          :width :fill :align :stretch
          (%rack-panel-view-toolbar)
          (if st/rack-panel-macros-open (%rack-macro-bank inst) (box :width 0 :height 0))
          (if st/rack-panel-slot-list-open
            (if (= (get inst :routing) "by-pitch")
              (%drum-rack-pad-grid inst)
              (box
                :background-color :bg
                :border-color :buffer-bg
                :corner-radius 10
                (v-stack :debug-name "rack-chain-list" :gap 0.025 :height 5 :width :fill
                  (if (> (len (get inst :slots)) 0)
                    (each (get inst :slots) |slot idx|
                      (%rack-slot-row slot))
                    (box :width :fill :height 9 :h-align :center :v-align :center
                      (label "Drop an Instrument or Sample"
                        :font-size 11 :color :dim :bg :transparent))))))
            (box :width 0 :height 0)))))
    :debug-name "rack-panel"
    :drop-types (list "sample" "instrument" "sound")
    :drop-meta (dict :kind "rack-panel"
      :track (get inst :track)
      :routing (get inst :routing)
      :pad-note (get inst :selected-pad-note))
    :drop-hover-border-color :mixer-strip-selected-border
    :on-drop (lambda (event) (%rack-panel-drop-on-container event))
    :background "fx-panel-bg"
    :color :instrument-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :padding 0
    :width (+ 3.35 (if st/rack-panel-slot-list-open 34.7 0) (if st/rack-panel-macros-open 24.2 0))
    :height st/fx-fixed-panel-height
    :selected 0))

(def %rack-instrument-panel-row (inst)
  (h-stack :debug-name "rack-instrument-panel-row"
           :gap 0.2
           :height st/fx-fixed-panel-height
           :align :stretch
    (%rack-panel inst)
    (if st/rack-panel-selected-chain-open
      (%rack-selected-instrument-panel inst)
      (box :width 0 :height 0))))

(def instrument-panel (inst)
  (if (= (get inst :type) "sampler")
    (sp/sampler-panel inst)
    (if (= (get inst :type) "rack")
      (%rack-instrument-panel-row inst)
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
                    :font-size 11  :color :white :bg :transparent)
                  (ep/instrument-synth-button)
                  (ep/instrument-mods-toggle-button)
                  (ep/instrument-keys-button)
                  (ep/instrument-sound-binding-badge inst))
                (box :flex 1 :height 0.15)
                (pf/instrument-header-actions-menu inst)
                (box :debug-name "instrument-preset-button" :padding 0.0 :width 2 :align :center
                  (v-stack
                    (box :width 1 :height 0.1)
                    (fx-mini-save-icon
                      :on-click |x y r| (sbrowser-enter-preset-save)
                      :active 0))
                  )
                (box :width 0.5)
                ))
            (pf/fx-panel-body "instrument-content-box"
              (pb/instrument-synth-panel-body inst)))
          :debug-name "instrument-panel"
          :background "fx-panel-bg"
          :color :instrument-panel-bg
          :header :fx-panel-header-bg
          :selected-header :fx-panel-header-selected-bg
          ;; Rack-slot instruments reuse this panel renderer, but the rack owns
          ;; their drop semantics.
          :drop-types (if (= (get inst :rack-slot) nil)
            (list "sample" "instrument" "sound")
            (list "sample" "instrument" "sound"))
          :drop-meta (if (= (get inst :rack-slot) nil)
            (dict :kind "instrument-panel" :track (get inst :track))
            (dict :kind "rack-selected-instrument"
                  :track (get inst :rack-track)
                  :slot (get inst :rack-slot)))
          :drop-hover-border-color :mixer-strip-selected-border
          :on-drop (lambda (event)
            (if (= (get inst :rack-slot) nil)
              (sbrowser-drop-sound-on-track event)
              (rack-selected-instrument-drop event)))
          :padding 0
          :height st/fx-fixed-panel-height
          :selected 0)))))
