;; eseq.effects — one import for every module factory instrument/effect UIs
;; call by qualified name (eseq.effects.custom-ui-lego/…, drum-surface/…, …).
;;
;; `(import eseq.effects)` resolves here: a module name whose path is a
;; directory loads that directory's index.lisp (kind-bindings spec §11). A
;; bare `-noui` view needs only this import to render factory device panels.
;; The app's own ui/effects.lisp manifest still `load`s these files by path;
;; import is load-once per pass, so a module already `load`ed is not re-run.

(module eseq.effects)

(import eseq.effects.state)
(import eseq.effects.param-controls)
(import eseq.effects.custom-ui-runtime)
(import eseq.effects.custom-ui-sections)
(import eseq.effects.custom-ui-controls)
(import eseq.effects.custom-ui-lego)
(import eseq.effects.custom-effect-ui)
(import eseq.effects.mnm-surface)
(import eseq.effects.drum-surface)
(import eseq.effects.physical-model-surface)
(import eseq.effects.identified-drum)
(import eseq.effects.panel-bodies)
