;; Built-in effect panel shell, enable controls, and instrument header controls.
(module eseq.effects.effect-panels)

(import eseq.effects.state :as st)
(import eseq.effects.param-controls :as pc)
(import eseq.effects.drag-drop :as dd)
(import eseq.macro-state :as ms)
(import eseq.effects.panel-frame :as pf)
(import eseq.effects.panel-bodies :as pb)

(export enabled-param
        visible-params
        enabled-toggle
        fx-panel
        midi-fx-panel
        instrument-synth-button
        instrument-toggle-mods-view
        instrument-mods-toggle-button
        instrument-sound-binding-badge
        instrument-keys-button
        effect-mods-toggle-button)

;; Migration aliases (module spec §10). Callers are still-unconverted lisp
;; files — effects/panel-frame.lisp,
;; effects/instrument-panel.lisp, effects/sampler-panel.lisp,
;; effects/modulator-panel.lisp, effects/buffers.lisp, ui/seq-panels.lisp —
;; plus Rust test harnesses that eval the flat spellings
;; (src/ui/state_values/tests.rs). Bare callers cannot see qualified names,
;; so unchanged spellings still need identity aliases. Converted callers
;; (eseq.effects.param-grid for visible-params) import this module instead —
;; no alias for visible-params. The aliases are deleted as callers convert.
;; defwidget names (header, fx-enabled-dot, fx-mini-save-icon) are a flat
;; keyspace that never qualifies (hazard e) — no aliases, no renames.

(defwidget header
  :shader
  (rgba 1 1 1 1))

(def enabled-param (params)
  (find-by-key params :name "enabled"))

(def visible-params (params)
  (filter |p| (not (= (get p :name) "enabled")) params))

(defwidget fx-enabled-dot
  :width 1.55 :height 1.0
  :paint-margin 0.1
  :state (active)
  :bindable (active)
  :shader
  (sdf/fill (sdf/circle 0.86)
    (material :color (if (> active 0.5) (rgba 1.0 0.8 0.12 1.0) (rgba 0 0 0 1)))))

(def enabled-toggle (p fx subtree-key)
  (subtree :key subtree-key
    (box :width 1.55 :height 1.35 :v-align :start :h-align :center :padding 0
      (v-stack :gap 0 :align :center
        (box :width 1.55 :height 0.04)
        (if p
          (fx-enabled-dot
            :active (pc/fx-param-value p)
            :on-click |x y r|
              (if fx
                (pc/fx-toggle-effect-value fx p)
                (pc/fx-toggle-instrument-value p)))
          (box :width 1.55 :height 1.0))))))

(defwidget fx-mini-save-icon
  :width 1.8 :height 0.8
  :paint-margin 0.2
  :state (active)
  :shader
  (let ((fg-col (rgba 0.92 0.92 0.96 1.0))
        (bg-col (if (= active 1)
          (rgba 0.00 0.35 0.82 1.0)
          (rgba 0.08 0.08 0.01 1.0))))
    (sdf/layer
      (sdf/fill
        (sdf/rounded-rect width height 0.5)
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 -0.42
          (sdf/rounded-rect 0.50 0.20 0.08))
        (material :color fg-col))
      (sdf/fill
        (sdf/translate 0.16 -0.42
          (sdf/rounded-rect 0.10 0.16 0.06))
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 0.27
          (sdf/rounded-rect 0.54 0.22 0.08))
        (material :color fg-col)))))

(def fx-panel (title params fx)
  (let ((selected (pb/fx-panel-selected? fx)))
  (box
    (v-stack :gap 0 :height :fill
      (pf/fx-panel-header title params fx)
      (pf/fx-panel-body (if (get fx :midi-fx) "midi-fx-panel-content" "audio-fx-panel-content")
        (if (get fx :midi-fx)
          (pb/midi-fx-panel-body fx)
          (pb/audio-fx-panel-body fx params))))
    :background "fx-panel-bg"
    :color :fx-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :height st/fx-fixed-panel-height
    :debug-name (if (get fx :midi-fx)
      (str "midi-fx-panel-root-" (get fx :slot-idx) "-" (get fx :name))
      (if (get fx :bus-fx)
        (str "bus-fx-panel-root-" (get fx :bus-idx) "-" (get fx :slot-idx) "-" title)
        (str "audio-fx-panel-root-" (get fx :slot-idx) "-" title)))
    :drop-types (pf/fx-effect-drop-types fx)
    :drop-meta (pf/fx-effect-drop-meta fx)
    :drop-hover-border-color :blue
    :on-drop (lambda (event) (dd/drop-on-effect event))
    :selected (if selected 1 0)
    :padding 0)))

(def midi-fx-panel (title params fx)
  (let ((selected (pb/fx-panel-selected? fx)))
  (box
    (v-stack :gap 0 :height :fill
      (pf/fx-panel-header title params fx)
      (pf/fx-panel-body "midi-fx-panel-content"
        (subtree :key (str "midi-fx-panel-body-" (get fx :slot-idx) "-" (get fx :name))
          (pb/midi-fx-panel-body fx))))
    :background "fx-panel-bg"
    :color :fx-panel-bg
    :header :fx-panel-header-bg
    :selected-header :fx-panel-header-selected-bg
    :height st/fx-fixed-panel-height
    :debug-name (str "midi-fx-panel-bg-" (get fx :slot-idx) "-" (get fx :name))
    :drop-types (pf/fx-effect-drop-types fx)
    :drop-meta (pf/fx-effect-drop-meta fx)
    :drop-hover-border-color :blue
    :on-drop (lambda (event) (dd/drop-on-effect event))
    :selected (if selected 1 0)
    :padding 0)))

(def instrument-tab-button (text idx width)
  (box :width width :height 1.2 :align :center
    :bg (if (= st/instrument-panel-tab idx) :dark-gray :transparent)
    :on-click |x y r| (set! st/instrument-panel-tab idx)
    (label text :font-size 11
      :color (if (= st/instrument-panel-tab idx) :white :dim)
      :bg :transparent)))

(def instrument-header-button (text active width click)
  (box :width width :height 1.2 :align :center
    :bg (if active :dark-gray :transparent)
    :on-click click
    (label text :font-size 11
      :color (if active :white :dim)
      :bg :transparent)))

(def instrument-header-tab-button (text active width click)
  (v-stack :height 0.9 :gap 0
    (box :height 0.15)
    (button text
      :width (* 1.5 width)
      :height 2.0
      :padding 0.55
      :font-size 11
      :shape :tab
      :active (if active 1 0)
      :background-color :transparent
      :active-background-color :bg
      :color :dim
      :active-color :white
      :border-color :transparent
      :on-click click)))

(def instrument-synth-button ()
  (instrument-header-tab-button "synth" (and (= st/instrument-panel-tab 0) (not st/instrument-mods-open)) 4.5
    (lambda (info) (do (set! st/instrument-panel-tab 0) (set! st/instrument-mods-open false)))))

;; src/ui/state_values/tests.rs slices the next def out of this file by its
;; exact flat def-header text (up to the following def's header text) and
;; evals it headerless, so the def keeps its flat name and its body keeps
;; the flat alias-mediated spellings (instrument-panel-tab /
;; instrument-mods-open are eseq.effects.state defstates,
;; macro-clear-mapping-arm / rack-macro-clear-mapping-arm are
;; eseq.macro-state aliases, process-map-clear is an
;; eseq.effects.param-controls alias) — they must resolve both inside this
;; module and in a vanilla eval. Do not rename either def below.
(def instrument-toggle-mods-view ()
  (do
    (set! eseq.effects.state/instrument-panel-tab 0)
    (if (not eseq.effects.state/instrument-mods-open)
      (do
        (eseq.macro-state/clear-mapping-arm)
        (eseq.effects.param-controls/process-map-clear)
        (eseq.macro-state/rack-clear-mapping-arm)))
    (set! eseq.effects.state/instrument-mods-open (not eseq.effects.state/instrument-mods-open))))

(def instrument-mods-toggle-button ()
  (instrument-header-tab-button "mods" (and (= st/instrument-panel-tab 0) st/instrument-mods-open) 4.0
    (lambda (info) (instrument-toggle-mods-view))))

;; Sound-binding badge (takes spec 16.6): which source the panel below is
;; actually showing and editing — `Take 2 - bars 1-3` vs `Pattern 2 (scene)`.
;; Rides inside the existing header row so the panel's tuned height is
;; unchanged, and collapses to nothing when the track has no binding.
(def instrument-sound-binding-badge (inst)
  (let ((binding (get inst :sound-binding)))
    (if (= binding nil)
      (box :width 0 :height 0 :bg :transparent)
      ;; Clicking the badge opens the sound palette on this track's binding
      ;; (takes spec 17.6). The badge text itself rides the `inst` map -- a
      ;; panel-scope SEQ.* read here would break the *fx* buffer.
      ;; seq-sound-palette-open is a Rust native (src/ui/natives.rs).
      (box :key (str "sound-binding-badge-" (get inst :track))
        :bg :transparent
        :on-click |x y r| (seq-sound-palette-open (get inst :track))
        (label (str "> " binding)
          :font-size 9 :color :dim :bg :transparent)))))

(def instrument-keys-button ()
  (instrument-header-tab-button "keys" (= st/instrument-panel-tab 1) 4.0
    (lambda (info) (do (set! st/instrument-panel-tab 1) (set! st/instrument-mods-open false)))))

(def effect-toggle-mods-view (fx)
  (let ((chain (pf/fx-effect-chain-kind fx))
        (track (if (get fx :bus-fx) -1 (get fx :track-idx)))
        (slot (get fx :slot-idx))
        (rack-slot (if (get fx :rack-fx) (get fx :rack-slot) -1))
        (bus (if (get fx :bus-fx) (get fx :bus-idx) -1)))
    (if (and st/effect-mods-open
             (= st/effect-mods-chain chain)
             (= st/effect-mods-track track)
             (= st/effect-mods-slot slot)
             (= st/effect-mods-rack-slot rack-slot)
             (= st/effect-mods-bus bus))
      (set! st/effect-mods-open false)
      (do
        (ms/clear-mapping-arm)
        (pc/process-map-clear)
        (ms/rack-clear-mapping-arm)
        (set! st/effect-mods-open true)
        (set! st/effect-mods-chain chain)
        (set! st/effect-mods-track track)
        (set! st/effect-mods-slot slot)
        (set! st/effect-mods-rack-slot rack-slot)
        (set! st/effect-mods-bus bus)))))

(def effect-mods-toggle-button (fx)
  (instrument-header-tab-button "mods" (pc/effect-mods-active? fx) 4.0
    (lambda (info) (effect-toggle-mods-view fx))))
