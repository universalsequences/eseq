;; metal-seq-fx.lisp — Effect chain UI for Metal Sequencer.
;;
;; Public entrypoint loaded by metal-seq-grid.lisp. Keep this file as a
;; dependency-ordered manifest; implementation lives under metal-seq-fx/.

(load "metal-seq-macro-state.lisp")
(load "metal-seq-fx/state.lisp")
(load "metal-seq-fx/panel-frame.lisp")
(load "metal-seq-fx/drag-drop.lisp")
(load "metal-seq-fx/track-panels.lisp")
(load "metal-seq-fx/panel-widgets.lisp")
(load "metal-seq-fx/param-controls.lisp")
(load "metal-seq-fx/process-panel.lisp")
(load "metal-seq-fx/param-grid.lisp")
(load "metal-seq-fx/instrument-modulation.lisp")
(load "metal-seq-fx/effect-modulation.lisp")
(load "metal-seq-fx/instrument-sources.lisp")
(load "metal-seq-builtin-fx-ui.lisp")
(load "metal-seq-fx/effect-panels.lisp")
(load "metal-seq-fx/custom-ui-runtime.lisp")
(load "metal-seq-fx/custom-ui-sections.lisp")
(load "metal-seq-fx/custom-ui-controls.lisp")
(load "metal-seq-fx/custom-ui-lego.lisp")
(load "metal-seq-fx/custom-effect-ui.lisp")
(load "metal-seq-fx/panel-bodies.lisp")
(load "metal-seq-fx/sampler-panel.lisp")
(load "metal-seq-fx/modulator-panel.lisp")
(load "metal-seq-fx/instrument-panel.lisp")
(load "metal-seq-fx/buffers.lisp")
