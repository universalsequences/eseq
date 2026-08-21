;; ui/builtin-effects.lisp — custom UI bodies for built-in audio effects.
;;
;; Public entrypoint used by ui/effects.lisp and tests. Keep load order
;; explicit because later panels depend on shared helpers defined first.

(load "@/ui/effects/builtin/filter-core.lisp")
(load "@/ui/effects/builtin/eq8.lisp")
(load "@/ui/effects/builtin/str8-delay.lisp")
(load "@/ui/effects/builtin/space-echo.lisp")
(load "@/ui/effects/builtin/multiverb.lisp")
(load "@/ui/effects/builtin/dimension.lisp")
(load "@/ui/effects/builtin/phaser-flanger.lisp")
(load "@/ui/effects/builtin/roar.lisp")
(load "@/ui/effects/builtin/filter-panel.lisp")
(load "@/ui/effects/builtin/dynamics.lisp")
(load "@/ui/effects/builtin/compressor.lisp")
(load "@/ui/effects/builtin/multiband.lisp")
(load "@/ui/effects/builtin/tape.lisp")
(load "@/ui/effects/builtin/dj-mixer.lisp")
(load "@/ui/effects/builtin/convolution-reverb.lisp")
(load "@/ui/effects/builtin/filter-table.lisp")
(load "@/ui/effects/builtin/filterbank.lisp")
(load "@/ui/effects/builtin/audio-fx.lisp")
