;; Shared state and sizing constants for the Metal Sequencer effect strip.
;; metal-seq-fx.lisp — Effect chain UI for Metal Sequencer
;; Renders to *fx* buffer. Loaded by metal-seq-grid.lisp.

(defstate instrument-panel-tab 0)
(defstate instrument-source-tab 0)
(defstate instrument-mods-open false)
(defstate instrument-selected-mod-slot 1)
(defstate effect-mods-open false)
(defstate effect-mods-chain "audio")
(defstate effect-mods-slot -1)
(defstate effect-mods-bus -1)
(defstate effect-selected-mod-slot 1)
;; These are temporary render-context globals used by generated custom synth UI.
;; They must NOT be defstate: custom UI functions set them while rendering, and
;; writing reactive state during measurement/layout can perturb the layout.
(def synth-ui-current-inst false)
(def synth-ui-current-name "")
(def midi-fx-ui-current-fx false)
(def midi-fx-ui-current-name "")
(def audio-fx-ui-current-fx false)
(def audio-fx-ui-current-name "")
(def custom-ui-current-kind "instrument")

;; Matches a standard built-in FX panel with four parameter rows.
(def fx-fixed-panel-height 9.10)
(def fx-panel-header-height 1.2)
(def fx-panel-body-padding 0.15)
(def fx-panel-body-top-spacer-height 0.16)
(def fx-panel-body-content-height 
  (- fx-fixed-panel-height fx-panel-header-height (* 2 fx-panel-body-padding) fx-panel-body-top-spacer-height))
