;; Jaki tier-2 package surface. Callers explicitly refer the exported macro:
;;
;;   (import alez.jaki.surface :refer (jak))
;;
;; The evaluator core lives in alez.jaki.core and is loaded by this module.
;;
;; One form defines a sequencer:
;;
;;   (jak "kit" :16
;;     . . - . (every 2 swap)
;;     -> 0
;;     -> 1 left
;;     -> 2 (shift 1) stac
;;     -> 3 accent (vel 0.7))
;;
;; Everything before the first `->` is the pattern (alez.jaki.core/pat
;; grammar); each `->` starts a route: a track number followed by route words
;; (see alez.jaki.core/run).
;; With no routes the pattern plays on track 0. Multi-voice stacks one
;; parenthesized line per voice:
;;
;;   (jak "kit" :16
;;     (. . - . (every 2 swap) -> 0)
;;     (- . . .                -> 1 stac))
;;
;; The expansion is the plain def-sequencer skeleton, so the body still ships
;; as quoted source and runs on the scheduler VM.

(module alez.jaki.surface)

(import alez.jaki.core)
(export jak)

(defmacro jak (name res &rest body)
  `(def-sequencer ,name
     :resolution ,res
     :tick (do
       (alez.jaki.core/init ,res)
       (alez.jaki.core/run '(,@body)))))
