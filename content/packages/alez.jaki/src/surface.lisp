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
;; The macro walks the body on the authoring VM and returns ordinary
;; def-sequencer syntax; its rewritten tick body is captured after macro
;; expansion and runs on the scheduler VM.

(module alez.jaki.surface)

(import alez.jaki.core)
(export jak register)

;; `jak` expands on the authoring VM while its quoted body executes on the
;; scheduler VM. Channel widgets therefore have to become plain `(chan ...)`
;; data here; the scheduler intentionally has no widget layer.
(def channel-widget? (form)
  (let ((head (nth form 0)))
    (or (= head '~slider) (= head '~knob) (= head '~toggle))))

(def channel-tail2 (items)
  (if (empty? items) (list) (rest (rest items))))

(def channel-property (args key)
  (if (empty? args)
      (dict :found false :value nil)
      (if (= (first args) key)
          (dict :found true :value (nth args 1))
          (channel-property (channel-tail2 args) key))))

(def channel-binding (form name)
  (let ((args (channel-tail2 form)))
    (let ((revision (get (channel-property args :__source-revision) :value))
          (start-byte (get (channel-property args :__source-start-byte) :value))
          (end-byte (get (channel-property args :__source-end-byte) :value)))
      (if (and revision (and start-byte end-byte))
          (list name revision start-byte end-byte)
          nil))))

(def bind-channel-widgets (bindings)
  (if (empty? bindings)
      true
      (let ((binding (first bindings)))
        (if (__bind-inline-widget-target
              (nth binding 1) (nth binding 2) (nth binding 3) "set"
              (channel-handle (nth binding 0)))
            (bind-channel-widgets (rest bindings))
            false))))

;; The counter is threaded through the recursive result so naming is stable and
;; independent of evaluator-global state. It advances for every channel widget,
;; named or anonymous, making the suffix its pre-order channel index.
(def channel-walk-list (forms seq-name index)
  (if (empty? forms)
      (dict :forms (list) :decls (list) :bindings (list) :next index)
      (let ((first-result (channel-walk (first forms) seq-name index)))
        (let ((rest-result
                (channel-walk-list (rest forms) seq-name (get first-result :next))))
          (dict :forms (cons (get first-result :form) (get rest-result :forms))
                :decls (append (get first-result :decls) (get rest-result :decls))
                :bindings (append (get first-result :bindings)
                                  (get rest-result :bindings))
                :next (get rest-result :next))))))

(def channel-walk (form seq-name index)
  (let ((head (nth form 0)))
    (if (= head nil)
        (dict :form form :decls (list) :bindings (list) :next index)
        (let ((property
                (if (channel-widget? form)
                    (channel-property (channel-tail2 form) :chan)
                    (dict :found false :value nil))))
          (if (get property :found)
              (let ((marker (get property :value))
                    (initial (nth form 1)))
                (let ((channel-name
                        (if (or (= marker true) (= marker 'true))
                            (str seq-name "#" index)
                            marker)))
                  (let ((binding (channel-binding form channel-name)))
                    (dict :form (list 'chan channel-name initial)
                          :decls (list (list channel-name initial))
                          :bindings (if binding (list binding) (list))
                          :next (+ index 1)))))
              (let ((walked (channel-walk-list form seq-name index)))
                (dict :form (get walked :forms)
                      :decls (get walked :decls)
                      :bindings (get walked :bindings)
                      :next (get walked :next))))))))

;; Publish a Jaki sequencer whose body is already a data value. Unlike `jak`,
;; this entry does not quote or rewrite the body expression: a defscene-backed
;; builder can publish once and let the shipped read resolve the active
;; pattern's value at each scheduler boundary.
(defmacro register (name res body)
  `(def-sequencer ,name
     :resolution ,res
     ;; The shipped tick calls package functions; the scheduler VM imports
     ;; them through its module load path before compiling the tick.
     :requires (alez.jaki.core)
     :tick (do
       (alez.jaki.core/init ,res)
       (alez.jaki.core/run ,body))))

;; A body of exactly one bare non-event symbol is never pattern data — it is
;; an expression body (typically a defscene slot): (jak "yo" :16 pat)
;; delegates to `register`, resolving `pat` at each scheduler boundary.
(def expression-body? (body)
  (if (= (len body) 1)
      (let ((x (nth body 0)))
        (and (= (nth x 0) nil)
             (and (not (= x '.)) (not (= x '-)))))
      false))

;; Quasiquote unquotes only resolve inside defmacro bodies here, so both
;; expansions stay in the macro; only the shape test is a helper.
(defmacro jak (name res &rest body)
  (if (alez.jaki.surface/expression-body? body)
      `(alez.jaki.surface/register ,name ,res ,(nth body 0))
      (let ((walked (channel-walk-list body name 0)))
        (let ((decls (list 'quote (get walked :decls)))
              (bindings (list 'quote (get walked :bindings)))
              (forms (list 'quote (get walked :forms))))
          ;; Declarations and editor bindings remain runtime authoring
          ;; operations; the pure walk that computes their data runs during
          ;; macro expansion. A failed declaration must not bind widgets or
          ;; publish the sequencer.
          `(if (__jaki-declare-value-channels ,decls)
               (if (alez.jaki.surface/bind-channel-widgets ,bindings)
                   (def-sequencer ,name
                     :resolution ,res
                     :requires (alez.jaki.core)
                     :tick (do
                       (alez.jaki.core/init ,res)
                       (alez.jaki.core/run ,forms)))
                   false)
               false)))))
