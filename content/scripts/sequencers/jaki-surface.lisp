;; Jaki tier-2 surface — the bare `jaki` macro. This file is deliberately
;; headerless (implicit module) so the macro keeps its unqualified name; the
;; evaluator core lives in jaki.lisp under (module jaki).
;;
;; One form defines a sequencer:
;;
;;   (jaki "kit" :16
;;     . . - . (every 2 swap)
;;     -> 0
;;     -> 1 left
;;     -> 2 (shift 1) stac
;;     -> 3 accent (vel 0.7))
;;
;; Everything before the first `->` is the pattern (jaki/pat grammar); each
;; `->` starts a route: a track number followed by route words (see jaki/run).
;; With no routes the pattern plays on track 0. Multi-voice stacks one
;; parenthesized line per voice:
;;
;;   (jaki "kit" :16
;;     (. . - . (every 2 swap) -> 0)
;;     (- . . .                -> 1 stac))
;;
;; The expansion is the plain def-sequencer skeleton, so the body still ships
;; as quoted source and runs on the scheduler VM.

(defmacro jaki (name res &rest body)
  `(def-sequencer ,name
     :resolution ,res
     :tick (do
       (jaki/init ,res)
       (jaki/run '(,@body)))))
