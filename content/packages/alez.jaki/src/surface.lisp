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
;; The macro sends the quoted body through the authoring-side expansion walk;
;; the rewritten body then ships as source and runs on the scheduler VM.

(module alez.jaki.surface)

(import alez.jaki.core)
(export jak)

;; `jak` expands on the authoring VM while its quoted body executes on the
;; scheduler VM. Channel widgets therefore have to become plain `(chan ...)`
;; data here; the scheduler intentionally has no widget layer.
(def channel-widget? (form)
  (let ((head (nth form 0)))
    (or (= head '~slider) (= head '~knob) (= head '~toggle))))

(def channel-tail2 (items)
  (if (empty? items) (list) (rest (rest items))))

(def channel-property (args)
  (if (empty? args)
      (dict :found false :value nil)
      (if (= (first args) :chan)
          (dict :found true :value (nth args 1))
          (channel-property (channel-tail2 args)))))

;; The counter is threaded through the recursive result so naming is stable and
;; independent of evaluator-global state. It advances for every channel widget,
;; named or anonymous, making the suffix its pre-order channel index.
(def channel-walk-list (forms seq-name index)
  (if (empty? forms)
      (dict :forms (list) :decls (list) :next index)
      (let ((first-result (channel-walk (first forms) seq-name index)))
        (let ((rest-result
                (channel-walk-list (rest forms) seq-name (get first-result :next))))
          (dict :forms (cons (get first-result :form) (get rest-result :forms))
                :decls (append (get first-result :decls) (get rest-result :decls))
                :next (get rest-result :next))))))

(def channel-walk (form seq-name index)
  (let ((head (nth form 0)))
    (if (= head nil)
        (dict :form form :decls (list) :next index)
        (let ((property
                (if (channel-widget? form)
                    (channel-property (channel-tail2 form))
                    (dict :found false :value nil))))
          (if (get property :found)
              (let ((marker (get property :value))
                    (initial (nth form 1)))
                (let ((channel-name
                        (if (or (= marker true) (= marker 'true))
                            (str seq-name "#" index)
                            marker)))
                  (dict :form (list 'chan channel-name initial)
                        :decls (list (list channel-name initial))
                        :next (+ index 1))))
              (let ((walked (channel-walk-list form seq-name index)))
                (dict :form (get walked :forms)
                      :decls (get walked :decls)
                      :next (get walked :next))))))))

(def channel-register (name res body)
  (let ((walked (channel-walk-list body name 0)))
    ;; Runtime natives report a hard authoring error as false plus status. Do
    ;; not let a later registration overwrite that status and publish a body
    ;; whose channel declarations failed.
    (if (__jaki-declare-value-channels (get walked :decls))
        ;; Build scheduler source after the walk, rather than letting the compiler
        ;; auto-quote the unrewritten source as `def-sequencer` would. `source`
        ;; provides canonical escaping for every authored literal in the body.
        (__register-sequencer name
          :resolution res
          :tick-source
            (str "(do (alez.jaki.core/init " (source res)
                 ") (alez.jaki.core/run '" (source (get walked :forms)) "))"))
        false)))

(defmacro jak (name res &rest body)
  `(alez.jaki.surface/channel-register ,name ,res '(,@body)))
