;; *track* and *fx* buffer definitions and keybindings.
;;
;; NOTE: this module is UI-root-ish — it registers the "*track*" and "*fx*"
;; effect-buffers and the seq-fx-mode / seq-plock-panel-mode keymaps at top
;; level. It is loaded from the effects manifest (ui/effects.lisp); do NOT
;; (import eseq.effects.buffers) from library code.
(module eseq.effects.buffers)

(import eseq.effects.drag-drop :as dd)
(import eseq.effects.effect-panels :as ep)
(import eseq.effects.instrument-panel :as ip)
(import eseq.effects.panel-widgets :as pw)
(import eseq.effects.param-controls :as pc)
(import eseq.effects.process-panel :as pp)
(import eseq.effects.state :as st :refer (eseq.effects.state/rack-panel-selected-chain-open))
(import eseq.effects.track-panels :as tp)

;; Flat callers: step-buffer.lisp calls fx-empty-track-fallback and binds
;; "*step*" to seq-plock-panel-mode; state_values/tests.rs evals
;; (fx-delete-selected-plock-row-key) by name.

(defwidget black
  :width 2 :height 2
  :shader
  (rgba 0.0 0.0 0 1))

(def empty-track-fallback ()
  (box :width :fill :height :fill :padding 1 :h-align :center :v-align :center
    (v-stack :gap 0.4 :align :center
      (label "Instrument and effects appear here"
        :font-size 12 :color :dim :bg :transparent)
      (compile-progress
        :active (if SEQ.compiling 1 0)
        :width 12 :height 0.3))))

(def %selected-bus-effects ()
  (if (pw/has-selected-bus?)
    (nth SEQ.bus-effects eseq.seq-core-state/selected-bus)
    '()))

(def %drop-placeholder-panel ()
  (box :debug-name "fx-drop-placeholder-panel"
       :background-color :buffer-bg
       :corner-radius 10
       :border-color :mixer-strip-border
       :border-width 2
       :drop-types (if (pw/has-selected-bus?)
         (list "audio-effect" "effect-instance")
         (list "audio-effect" "midi-effect" "effect-instance"))
       :drop-meta (dict :kind "fx-append"
                    :chain "append"
                    :track SEQ.current-track
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

(def %track-drop-placeholder-panel ()
  (box :debug-name "fx-track-drop-placeholder-panel"
       :background-color :buffer-bg
       :corner-radius 10
       :border-color :mixer-strip-border
       :border-width 2
       :drop-types (list "audio-effect" "midi-effect" "effect-instance")
       :drop-meta (dict :kind "fx-append"
                    :chain "append"
                    :track SEQ.current-track
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

(def %bus-selection-panel ()
  (v-stack :padding 0.05 :gap 1
    (h-stack :gap 1
      (each (filter |fx| (> (len (get fx :params)) 0) (%selected-bus-effects)) |fx slot-idx|
        (subtree :key (str "bus-fx-panel-" (get fx :bus-idx) "-" (get fx :slot-idx) "-" (get fx :name))
          (ep/fx-panel (get fx :name) (get fx :params) fx)))
      (%drop-placeholder-panel))))

;; Keep this as a macro rather than a normal function. Custom instrument/effect
;; UI establishes render-local scope while this tree is evaluated, so the FX
;; strip must remain inline at the effect-buffer callsite.
(defmacro %track-selection-panel ()
  `(v-stack :padding 0.05 :gap 1
    (h-stack :gap 1
      (if (> (len SEQ.process-slots) 0)
        (pp/process-chain-panel))
      (each SEQ.instrument-panel |inst inst-idx|
        (if (= (get inst :type) "rack")
          (h-stack :gap 0.2 :height st/fx-fixed-panel-height :align :stretch
            (ip/instrument-panel inst)
            (if eseq.effects.state/rack-panel-selected-chain-open
              (h-stack :debug-name "rack-selected-chain-fx"
                :gap 1 :height st/fx-fixed-panel-height :align :stretch
                (ip/rack-selected-fx-panel inst)
                (ip/rack-slot-fx-drop-panel inst)
                (ip/rack-slot-track-fx-divider))
              (box :width 0 :height 0)))
          (ip/instrument-panel inst)))
      (each (filter |fx| (> (len (get fx :params)) 0) SEQ.midi-effects) |fx slot-idx|
        (ep/midi-fx-panel (get fx :name) (get fx :params) fx))
      (each (filter |fx| (> (len (get fx :params)) 0) SEQ.effects) |fx slot-idx|
        (subtree :key (str "audio-fx-panel-" (get fx :slot-idx) "-" (get fx :name))
          (ep/fx-panel (get fx :name) (get fx :params) fx)))
      (%track-drop-placeholder-panel))))

(effect-buffer "*track*"
  (if (= SEQ.num-tracks 0)
    (empty-track-fallback)
    (box :padding 1.0
      (v-stack :gap 0.2
        ;; Own subtree so p-lock highlight updates rerun only this panel,
        ;; not the whole buffer.
        (subtree :key "track-parameters-panel"
          (tp/track-parameters-panel))
        ;(fx-track-accumulator-panel)
        ))))

(effect-buffer "*fx*"
  (if (pw/has-selected-bus?)
    (%bus-selection-panel)
    (if (= SEQ.num-tracks 0)
    (empty-track-fallback)
    ;; Mapping changes the wrapper structure of every compatible parameter.
    ;; A distinct root forces those cached parameter subtrees to be rebuilt
    ;; immediately when mapping is armed from this same panel.
    (if (pc/param-macro-mapping-active?)
      (box :debug-name "fx-param-map-active-root" :padding 0
        (subtree :key (pc/param-macro-structure-key)
          (%track-selection-panel)))
    (if (pc/process-map-active?)
      (box :debug-name "fx-param-map-active-root" :padding 0
        (%track-selection-panel))
      (%track-selection-panel))))))

(define-mode "seq-fx-mode" :read-only true)
;; Handler strings qualify against THIS module; the two below live in other
;; converted modules, so they are written pre-qualified (dispatch resolves an
;; already-qualified name directly, resolve_handler_name).
(mode-bind-key "seq-fx-mode" "BS" "eseq.effects.panel-widgets/delete-selected-effect")
(mode-bind-key "seq-fx-mode" "Delete" "eseq.effects.panel-widgets/delete-selected-effect")
(mode-bind-key "seq-fx-mode" "RET" "eseq.effects.process-panel/open-selected-source")
(set-buffer-mode-for "*fx*" "seq-fx-mode")

(def delete-selected-plock-row-key ()
  (if (tp/plock-row-selected?)
    (do
      (tp/delete-selected-plock-row)
      true)
    false))

(define-mode "eseq.effects.buffers/seq-plock-panel-mode" :read-only true)
(mode-bind-key "eseq.effects.buffers/seq-plock-panel-mode" "BS" "delete-selected-plock-row-key")
(mode-bind-key "eseq.effects.buffers/seq-plock-panel-mode" "Delete" "delete-selected-plock-row-key")
(set-buffer-mode-for "*track*" "eseq.effects.buffers/seq-plock-panel-mode")
