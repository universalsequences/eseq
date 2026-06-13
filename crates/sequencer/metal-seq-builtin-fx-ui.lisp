;; metal-seq-builtin-fx-ui.lisp — custom UI bodies for built-in audio effects.
;;
;; Public entrypoint used by metal-seq-fx.lisp and tests. Keep load order
;; explicit because later panels depend on shared helpers defined first.

(load "metal-seq-fx/builtin/filter-core.lisp")
(load "metal-seq-fx/builtin/str8-delay.lisp")
(load "metal-seq-fx/builtin/space-echo.lisp")
(load "metal-seq-fx/builtin/dimension.lisp")
(load "metal-seq-fx/builtin/filter-panel.lisp")
(load "metal-seq-fx/builtin/dynamics.lisp")
(load "metal-seq-fx/builtin/tape.lisp")
(load "metal-seq-fx/builtin/dj-mixer.lisp")
(load "metal-seq-fx/builtin/convolution-reverb.lisp")
(load "metal-seq-fx/builtin/audio-fx.lisp")
