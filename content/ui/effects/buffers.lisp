;; *track* and *fx* buffer definitions and keybindings.
;;
;; NOTE: this module is UI-root-ish — it registers the "*track*" and "*fx*"
;; effect-buffers and the seq-fx-mode / seq-plock-panel-mode keymaps at top
;; level. It is loaded from the effects manifest (ui/effects.lisp); do NOT
;; (import eseq.effects.buffers) from library code.
(module eseq.effects.buffers)

(import eseq.kinds :refer (tracks buses engine selection editor))
(import eseq.drum-rack-v2)
(import eseq.effects.devices :as dv)
(import eseq.effects.drag-drop :as dd)
(import eseq.effects.effect-panels :as ep)
(import eseq.effects.instrument-panel :as ip)
(import eseq.effects.panel-data :as pd)
(import eseq.effects.panel-frame :as pf)
(import eseq.effects.panel-widgets :as pw)
(import eseq.effects.param-controls :as pc)
(import eseq.effects.process-panel :as pp)
(import eseq.effects.state :as st)
(import eseq.effects.track-panels :as tp)
(import eseq.sound-palette)

(export empty-track-fallback
        no-tracks?
        step-buffer-body
        track-buffer-body
        fx-buffer-body
        drop-placeholder-panel
        track-drop-placeholder-panel
        delete-selected-plock-row-key)

;; Flat callers: step-buffer.lisp calls fx-empty-track-fallback and binds
;; "*step*" to seq-plock-panel-mode; state_values/tests.rs evals
;; (fx-delete-selected-plock-row-key) by name.

(defwidget black
  :width 2 :height 2
  :shader
  (rgba 0.0 0.0 0 1))

(def no-tracks? ()
  (= (len (tracks)) 0))

(def empty-track-fallback ()
  (box :width :fill :height :fill :padding 1 :h-align :center :v-align :center
    (v-stack :gap 0.4 :align :center
      (label "Instrument and effects appear here"
        :font-size 12 :color :dim :bg :transparent)
      (compile-progress
        :active (if engine.compiling 1 0)
        :width 12 :height 0.3))))

;; The effects the selected bus shows (none while no bus is selected), as
;; devices: each panel's dict (eseq.effects.panel-data) is built inside its
;; own subtree, so an option param's label it shows re-renders that panel
;; alone.
(def selected-bus-effects ()
  (if (pw/has-selected-bus?)
    (pd/bus-effects (nth (buses) eseq.seq-core-state/selected-bus))
    '()))

;; Bus effect `d`'s panel and its output meter.
(def bus-effect-panel (d)
  (h-stack :gap 1 :height st/fx-fixed-panel-height :align :stretch
    (subtree :key (str "bus-fx-panel-" d.bus.index "-" d.slot "-" d.name)
      (let ((fx (pd/fx-panel-of d)))
        (ep/fx-panel (get fx :name) (get fx :params) fx)))
    (pf/device-output-meter d.meter)))

(def drop-placeholder-panel ()
  (box :debug-name "fx-drop-placeholder-panel" :part "fx/drop-zone"
       :background-color :buffer-bg
       :corner-radius 10
       :border-color :mixer-strip-border
       :border-width 2
       :drop-types (if (pw/has-selected-bus?)
         (list "audio-effect" "effect-instance")
         (list "audio-effect" "midi-effect" "effect-instance"))
       :drop-meta (dict :kind "fx-append"
                    :chain "append"
                    :track (dv/current-track-index)
                    :bus (if (pw/has-selected-bus?) eseq.seq-core-state/selected-bus -1)
                    :slot -1)
       :drop-hover-border-color :mixer-strip-selected-border
       :drop-hover-background-color :mixer-control-bg
       :on-drop (lambda (event) (dd/drop-on-effect event))
       :height st/fx-fixed-panel-height
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

(def track-drop-placeholder-panel ()
  (box :debug-name "fx-track-drop-placeholder-panel" :part "fx/drop-zone"
       :background-color :buffer-bg
       :corner-radius 10
       :border-color :mixer-strip-border
       :border-width 2
       :drop-types (list "audio-effect" "midi-effect" "effect-instance")
       :drop-meta (dict :kind "fx-append"
                    :chain "append"
                    :track (dv/current-track-index)
                    :bus -1
                    :slot -1)
       :drop-hover-border-color :mixer-strip-selected-border
       :drop-hover-background-color :mixer-control-bg
       :on-drop (lambda (event) (dd/drop-on-effect event))
       :height st/fx-fixed-panel-height
       :width 34
       :padding 0
       :h-align :center
       :v-align :center
    (v-stack :gap 0.35 :align :center
      (label "Track FX" :font-size 9 :color :blue :bg :transparent)
      (label "Drop Audio or MIDI Effect Here"
        :width 30 :font-size 12 :h-align :center
        :color :dim :bg :transparent))))

;; ── Drum rack selection panel (docs/drum-rack-v2-spec.md) ──────────────
;; Selecting a rack selects its backing bus (ui/sequencer.lisp, select-rack),
;; so without this branch a kit shows up here as a bare — usually empty — bus
;; chain. A rack has to feel like a track in *fx*: its kit first, then the
;; rack-level effects that bus chain really is.
;;
;; The pads come from ui/sequencer.lisp — one pad-cell component, so drops,
;; badges and audition cannot drift between this panel and any other surface
;; that draws them. The eseq.sequencer names are spelled qualified rather than
;; imported ON PURPOSE: this module is loaded from the effects manifest, which
;; several Rust harnesses load on its own, and an import edge would drag the
;; whole sequencer view (and its top-level registrations) into every one of
;; them.
;; They only run inside the rack branch, which cannot be reached without the
;; sequencer loaded. eseq.drum-rack-v2 IS imported (at the top of this file):
;; the buffer asks it about every bus selection (`selected-bus-rack`), and it
;; is a pure lookups module over the group kinds — no view, no top-level
;; registrations — that answers nil when no rack is selected.

(defwidget rack-kit-mixed-dot
  :width 0.5 :height 0.5
  :paint-margin 0.1
  :shader
  (sdf/fill (sdf/circle 0.8) (material :color :fg)))

;; The kit's power toggle (docs/rig-spec.md §6): the instrument/effect enable
;; dot, scoped to the viewed bank. A mixed bank (some scenes off) shows on
;; with a small dot beside it; clicking turns the whole bank off. The hint
;; after the name stands in for a tooltip ("Kit on in bank C").
(def rack-kit-power-toggle (g kit-state)
  (do
    (h-stack :debug-name "rack-kit-power" :gap 0 :align :center
      (box :width 1.55 :height 0.99 :v-align :center :h-align :center :padding 0
        (fx-enabled-dot
          :key (str "rack-kit-power-" g.gid)
          :active (if (= kit-state :off) 0 1)
          :on-click |x y r| (eseq.drum-rack-v2/toggle-kit-in-viewed-bank! g)))
      (if (= kit-state :mixed)
        (box :debug-name "rack-kit-power-mixed" :width 0.5 :height 0.99
          :v-align :center :h-align :center :padding 0
          (rack-kit-mixed-dot))
        (box :width 0 :height 0)))))

(def rack-selection-panel (g)
  (let ((kit (eseq.drum-rack-v2/kit-viewed-header g)))
  (v-stack :padding 0.05 :gap 1
    (h-stack :gap 1 :align :start
      (box :debug-name "rack-fx-pads-panel"
        :key (str "rack-fx-pads-panel-" g.gid)
        :background (pf/panel-surface)
        :color :instrument-panel-bg
        :header :fx-panel-header-bg
        :selected-header :fx-panel-header-selected-bg
        :height st/fx-fixed-panel-height
        :padding 0
        :v-align :start :h-align :start
        (v-stack :gap 0 :height :fill
          (box :debug-name "rack-fx-header-box"
            :width :fill :height 1 :padding 0 :v-align :center :h-align :start
            (h-stack :debug-name "rack-fx-header-row" :gap 0.6 :align :center :width :fill
              (pf/fx-panel-header-leading-spacer)
              (rack-kit-power-toggle g (get kit :state))
              (label (substring g.name 0 12)
                :v-align :center
                :font-size 11 :color :white :bg :transparent)
              (label (get kit :hint)
                :debug-name "rack-kit-power-hint"
                :v-align :center
                :font-size 9 :color :dim :bg :transparent)
              (box :flex 1 :height 0.15)
              (box :debug-name "rack-kit-save-button" :padding 0 :width 2 :align :center
                (v-stack
                  (box :width 1.65 :height 0.85
                    (fx-mini-save-icon
                      :key (str "rack-fx-save-kit-" g.gid)
                      :on-click |x y r| (eseq.browser/enter-kit-save g)
                      :active 0))))
              (box :width 0.5)))
          (pf/fx-panel-body "rack-fx-pads-body"
            ;; Mini-map on the LEFT of the enlarged grid, the way a drum rack
            ;; reads: the whole note range at a glance, the enlarged window
            ;; highlighted inside it (eseq-4b5.15).
            (h-stack :gap 0.2 :align :start
              (eseq.sequencer/pad-map g)
              (eseq.sequencer/pad-grid g)
              ))))
      ;; The pad grid's right-click menu (Role ▸ …), overlaying the grid.
      (eseq.sequencer/rack-pad-context-menu)
      ;; Rack-level fx still matter: the bus chain stays right here, edited the
      ;; same way an ordinary bus selection edits it.
      (each (selected-bus-effects) |d slot-idx|
        (bus-effect-panel d))
      (drop-placeholder-panel)))))

(def bus-selection-panel ()
  (v-stack :padding 0.05 :gap 1
    (h-stack :gap 1
      (each (selected-bus-effects) |d slot-idx|
        (bus-effect-panel d))
      (drop-placeholder-panel))))

;; Keep this as a macro rather than a normal function. Custom instrument/effect
;; UI establishes render-local scope while this tree is evaluated, so the FX
;; strip must remain inline at the effect-buffer callsite.
(defmacro track-selection-panel ()
  `(v-stack :padding 0.05 :gap 1
    (h-stack :gap 0.5
      ;; An effect's panel dict (eseq.effects.panel-data) is built inside
      ;; its own subtree, so a structural change re-renders that panel
      ;; alone. The instrument's is built inline: a custom instrument UI
      ;; sets its render scope (`synth-ui-current-inst`, the selected
      ;; section) as the tree evaluates, which a subtree re-render of the
      ;; panel alone loses; its dict reads no value or text (only which
      ;; source settings show), so it moves on a structural change only.
      (each (pd/track-instrument-devices selection.track) |d inst-idx|
        (h-stack :gap 0.5 :height st/fx-fixed-panel-height :align :stretch
          (do
            (let ((inst (pd/instrument-panel-of d.track d)))
              (if (= (get inst :type) "rack")
                (h-stack :gap 0.2 :height st/fx-fixed-panel-height :align :stretch
                  (ip/instrument-panel inst)
                  (if (st/rack-panel-selected-chain-open inst)
                    (h-stack :debug-name "rack-selected-chain-fx"
                      :gap 1 :height st/fx-fixed-panel-height :align :stretch
                      (ip/rack-selected-fx-panel inst)
                      (ip/rack-slot-fx-drop-panel inst)
                      (ip/rack-slot-track-fx-divider))
                    (box :width 0 :height 0)))
                (ip/instrument-panel inst))))
          (pf/device-output-meter d.meter)))
      (each (pd/track-midi-effects selection.track) |d slot-idx|
        (subtree :key (str "midi-fx-panel-" d.slot "-" d.name)
          (let ((fx (pd/fx-panel-of d)))
            (ep/midi-fx-panel (get fx :name) (get fx :params) fx))))
      (each (pd/track-effects selection.track) |d slot-idx|
        (h-stack :gap 0.5 :height st/fx-fixed-panel-height :align :stretch
          (subtree :key (str "audio-fx-panel-" d.slot "-" d.name)
            (let ((fx (pd/fx-panel-of d)))
              (ep/fx-panel (get fx :name) (get fx :params) fx)))
          (pf/device-output-meter d.meter)))
      ;; The patch editor previews its instrument dry; Track FX are the
      ;; sequencer's (docs/patcher-workspace-spec.md §2).
      (if (= editor.mode "")
        (track-drop-placeholder-panel)
        (box :width 0 :height 0)))))

;; Each side buffer's whole content is a named function, so a package can
;; restyle the tile edge to edge with an :around (a buffer body runs on every
;; redraw, unlike a subtree's).

;; *step*: registered by step-buffer.lisp.
(def step-buffer-body ()
  (if (no-tracks?)
    (empty-track-fallback)
    (box :padding 0.5
      (v-stack :gap 0.1
        ;; Sound palette overlay (takes spec 17.6), opened from the
        ;; instrument panel's binding badge. Collapses to nothing while
        ;; closed.
        (subtree :key "step-sound-palette"
          (eseq.sound-palette/panel))
        ;; Each panel is its own subtree so selection/p-lock updates rerun
        ;; only the panel that reads them instead of the whole buffer.
        (subtree :key "step-parameters-panel"
          (tp/step-parameters-panel))
        (subtree :key "step-track-plocks-panel"
          (tp/track-plocks-panel))))))

(def track-buffer-body ()
  (if (no-tracks?)
    (empty-track-fallback)
    (box :padding 1.0
      (v-stack :gap 0.2
        ;; Own subtree so p-lock highlight updates rerun only this panel,
        ;; not the whole buffer.
        (subtree :key "track-parameters-panel"
          (tp/track-parameters-panel))
        ;(fx-track-accumulator-panel)
        ))))

(effect-buffer "*track*"
  (track-buffer-body))

;; The knob right-click menu (bead eseq-1gy6) is an overlay, so it has to be
;; rendered by the tile that owns the knobs — this buffer. Zero footprint when
;; closed; the wrapper boxes only set the shared menu state.
(def fx-buffer-body ()
  (h-stack :debug-name "fx-buffer-root" :gap 0 :width :fill :height :fill
  (if (pw/has-selected-bus?)
    (let ((g (eseq.drum-rack-v2/selected-bus-rack)))
      (if g
        (rack-selection-panel g)
        (bus-selection-panel)))
    (if (no-tracks?)
    (empty-track-fallback)
    ;; Mapping changes the wrapper structure of every compatible parameter.
    ;; A distinct root forces those cached parameter subtrees to be rebuilt
    ;; immediately when mapping is armed from this same panel.
    (if (pc/param-macro-mapping-active?)
      (box :debug-name "fx-param-map-active-root" :padding 0
        (subtree :key (pc/param-macro-structure-key)
          (track-selection-panel)))
    (if (pc/process-map-active?)
      (box :debug-name "fx-param-map-active-root" :padding 0
        (track-selection-panel))
      (track-selection-panel)))))
  (subtree :key "fx-param-plock-menu" (pc/param-plock-context-menu "fx"))
  (subtree :key "fx-polyphony-menu" (tp/polyphony-context-menu))))

(effect-buffer "*fx*"
  (fx-buffer-body))

(def copy-selected-effect ()
  (seq-copy-selected-effect))

(def paste-effect ()
  (seq-paste-effect))

;; BS/Delete: selected steps win over the selected effect, as the host table
;; used to order them; with no step selection the parent binding declines and
;; the effect delete runs.
(def delete-key ()
  (if (eseq.sequencer-keys/delete-selected-steps)
    true
    (eseq.effects.panel-widgets/delete-selected-effect)))

(define-mode "seq-fx-mode" :read-only true :live-keys true
  :inherit "eseq.sequencer-keys/sequencer-keys")
;; Handler strings qualify against THIS module; handlers that live in other
;; converted modules are written pre-qualified (dispatch resolves an
;; already-qualified name directly, resolve_handler_name).
(mode-bind-key "seq-fx-mode" "BS" "delete-key")
(mode-bind-key "seq-fx-mode" "Delete" "delete-key")
(mode-bind-key "seq-fx-mode" "s-c" "copy-selected-effect")
(mode-bind-key "seq-fx-mode" "s-v" "paste-effect")
(mode-bind-key "seq-fx-mode" "C-S-c" "copy-selected-effect")
(mode-bind-key "seq-fx-mode" "C-S-v" "paste-effect")
(mode-bind-key "seq-fx-mode" "RET" "eseq.effects.process-panel/open-selected-source")
(set-buffer-mode-for "*fx*" "seq-fx-mode")

(def delete-selected-plock-row-key ()
  (if (tp/plock-row-selected?)
    (do
      (tp/delete-selected-plock-row)
      true)
    false))

(define-mode "eseq.effects.buffers/seq-plock-panel-mode" :read-only true :live-keys true
  :inherit "eseq.sequencer-keys/sequencer-keys")
(mode-bind-key "eseq.effects.buffers/seq-plock-panel-mode" "BS" "delete-selected-plock-row-key")
(mode-bind-key "eseq.effects.buffers/seq-plock-panel-mode" "Delete" "delete-selected-plock-row-key")
(set-buffer-mode-for "*track*" "eseq.effects.buffers/seq-plock-panel-mode")
