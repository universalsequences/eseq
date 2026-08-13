;; Shared state and sizing constants for the Metal Sequencer effect strip.
;; ui/effects.lisp — Effect chain UI for Metal Sequencer
;; Renders to *fx* buffer. Loaded by ui/main.lisp after shared macro state.

(module eseq.effects.state)

;; Migration aliases (module spec §10). Every name below keeps its spelling —
;; this file is a shared state hub for six unrelated prefix families
;; (`instrument-`, `rack-panel-`, `effect-mods-`, `process-panel-`, the
;; `*-ui-current-*` render-context globals and the `fx-panel-` sizing
;; constants), and stripping any of them to the module would collide
;; (`instrument-mods-open` and `effect-mods-open` both want `mods-open`).
;; The aliases exist for the bare-name reason from the recipe's step 2: a
;; bare `instrument-mods-open` in an unconverted caller does not find
;; `eseq.effects.state/instrument-mods-open`. They are deleted as each
;; consumer family converts. Not aliased: the two %-private sizing helpers,
;; which have no caller outside this file.

(defstate instrument-panel-tab 0)
(defstate instrument-source-tab 0)
(defstate instrument-mods-open false)
(defstate instrument-selected-mod-slot 1)
(defstate instrument-key-lock-octave 4)
(defstate instrument-key-lock-selected-notes '())
(defstate instrument-key-lock-audition true)
(defstate rack-panel-slot-list-open true)
(defstate rack-panel-selected-chain-open true)
(defstate rack-panel-macros-open false)
(defstate effect-mods-open false)
(defstate effect-mods-chain "audio")
(defstate effect-mods-track -1)
(defstate effect-mods-slot -1)
(defstate effect-mods-rack-slot -1)
(defstate effect-mods-bus -1)
(defstate effect-selected-mod-slot 1)
(defstate process-panel-selected-track -1)
(defstate process-panel-selected-instance-id 0)
;; These are temporary render-context globals used by generated custom synth UI.
;; They must NOT be defstate: custom UI functions set them while rendering, and
;; writing reactive state during measurement/layout can perturb the layout.
;;
;; They also stay in `eseq.vanilla` explicitly (module spec §3's cross-module
;; def escape hatch) instead of joining this module behind an alias, because
;; they are a host->script protocol, not this module's API: `custom_ui.rs`
;; GENERATES lisp that writes them by bare name (`(set! synth-ui-current-inst
;; inst)`), and that generated unit's compile time is not ordered against this
;; file. An alias would only rescue readers — the stage-3 late-binding heal is
;; read-side — so a generated writer compiled first would keep storing into the
;; stale vanilla slot while later-compiled readers followed the alias to this
;; module's, and the two would silently diverge. Whoever teaches custom_ui.rs
;; to emit qualified names can fold these in.
(def eseq.vanilla/synth-ui-current-inst false)
(def eseq.vanilla/synth-ui-current-name "")
(def eseq.vanilla/midi-fx-ui-current-fx false)
(def eseq.vanilla/midi-fx-ui-current-name "")
(def eseq.vanilla/audio-fx-ui-current-fx false)
(def eseq.vanilla/audio-fx-ui-current-name "")
(def eseq.vanilla/custom-ui-current-kind "instrument")

(def seq-timebase-options
  '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh"))

;; Matches a standard built-in FX panel with four parameter rows.
(def fx-fixed-panel-height 10.8)
(def fx-panel-header-height 1.0)
(def %fx-panel-body-padding 0.25)
(def %fx-panel-body-top-spacer-height 0.16)
(def fx-panel-body-content-height 
  (- fx-fixed-panel-height fx-panel-header-height (* 2 %fx-panel-body-padding) %fx-panel-body-top-spacer-height))
