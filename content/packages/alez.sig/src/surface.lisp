;; alez.sig — signal-pipeline sugar over def-process + channels.
;;
;;   (import alez.sig.surface :refer (sig))
;;
;;   (sig "hello" :over (bars 4)
;;     (-> phase (* tau) sin (scale -1 1 0 1)))
;;
;;   (sig "dash" :over (bars 1) :rate :32 (sine phase))
;;
;; One form = one named value channel driven by one stateful phasor process.
;; Options (all optional, must precede the pipeline):
;;   :over (bars n)|(beats n)|:16|number   full cycle length   (default (bars 1))
;;   :rate (bars n)|(beats n)|:16|number   tick interval       (default :32)
;;   :from n                               phase offset 0..1   (default 0)
;; The pipeline is one expression over `phase` (the 0..1 ramp); with no
;; pipeline the raw ramp is published. Consumers read the channel as usual:
;;   (dotdecay (~slider 0 :chan "hello"))
;;
;; Pipeline vocabulary rewritten at authoring time into plain arithmetic, so
;; the shipped body only touches scheduler natives:
;;   tau pi                                numeric constants
;;   (sine x) (tri x) (saw x)              0..1 shapers of a 0..1 phase
;;   (sqr x) (sqr x duty)                  0/1 pulse
;;   (unipolar x) (bipolar x)              -1..1 <-> 0..1
;;   (scale x in-lo in-hi out-lo out-hi)   linear remap
;; Everything else passes through untouched (sin, cos, abs, mod, ->, ...).
;;
;; Mechanics mirror alez.jaki.surface: the macro quotes its spec on the
;; authoring VM; sig-register parses options as data, rewrites the pipeline,
;; declares the value channel idempotently, then evals a generated string-named
;; def-process whose :run body auto-quotes and ships to the scheduler VM.
;; Phase is derived from the transport each tick — (now-beats) is the tick's
;; quantized boundary beat, so phase = fract(now-beats / over + from). No
;; accumulator state: signals are seek-safe, restart-safe, drift-free, and
;; every sig of the same :over is automatically phase-locked.

(module alez.sig.surface)

(export sig)

(def tau-value 6.283185307179586)
(def pi-value 3.141592653589793)

;; ── musical-time forms as data (numbers must be literals) ───────────────────

(def time-beats (form default)
  (if (= form nil) default
  (if (= form :1) 4
  (if (= form :2) 2
  (if (= form :4) 1
  (if (= form :8) 0.5
  (if (= form :16) 0.25
  (if (= form :32) 0.125
  (if (= form :64) 0.0625
    (let ((head (nth form 0)))
      (if (= head 'beats)
          (nth form 1)
          (if (= head 'bars)
              (* 4 (nth form 1))
              form))))))))))))

;; ── leading keyword options ─────────────────────────────────────────────────

(def known-opt? (key)
  (or (= key :rate) (= key :over) (= key :from)))

(def opt-value (spec key default)
  (if (empty? spec)
      default
      (if (known-opt? (first spec))
          (if (= (first spec) key)
              (nth spec 1)
              (opt-value (rest (rest spec)) key default))
          default)))

(def strip-opts (spec)
  (if (empty? spec)
      spec
      (if (known-opt? (first spec))
          (strip-opts (rest (rest spec)))
          spec)))

(def pipeline-form (body)
  (if (empty? body)
      'phase
      (if (empty? (rest body))
          (first body)
          (cons 'do body))))

;; ── authoring-side vocabulary rewrite ───────────────────────────────────────
;; Each shaper uses its argument exactly once, so no temporaries are needed.
;; `->`/`->>` are desugared here first (same semantics as the compiler's
;; threading desugar: acc into slot 1 / last slot, bare symbol = call) so
;; shapers inside a pipeline see their threaded argument.

(def thread-stage (acc stage last?)
  (if (= (nth stage 0) nil)
      (list stage acc)
      (if last?
          (append stage (list acc))
          (cons (nth stage 0) (cons acc (rest stage))))))

(def thread-form (acc stages last?)
  (if (empty? stages)
      acc
      (thread-form (thread-stage acc (first stages) last?) (rest stages) last?)))

(def rewrite-all (forms)
  (if (empty? forms)
      (list)
      (cons (rewrite (first forms)) (rewrite-all (rest forms)))))

(def rewrite (form)
  (if (= form 'tau) tau-value
  (if (= form 'pi) pi-value
    (let ((head (nth form 0)))
      (if (= head nil)
          form
      (if (= head '->)
          (rewrite (thread-form (nth form 1) (rest (rest form)) false))
      (if (= head '->>)
          (rewrite (thread-form (nth form 1) (rest (rest form)) true))
      (if (= head 'scale)
          (let ((x (rewrite (nth form 1)))
                (a (rewrite (nth form 2)))
                (b (rewrite (nth form 3)))
                (c (rewrite (nth form 4)))
                (d (rewrite (nth form 5))))
            (list '+ c (list '* (list '- d c)
                             (list '/ (list '- x a) (list '- b a)))))
      (if (= head 'unipolar)
          (list '+ 0.5 (list '* 0.5 (rewrite (nth form 1))))
      (if (= head 'bipolar)
          (list '- (list '* 2 (rewrite (nth form 1))) 1)
      (if (= head 'sine)
          (list '+ 0.5
                (list '* 0.5 (list 'sin (list '* tau-value
                                              (rewrite (nth form 1))))))
      (if (= head 'tri)
          (list '- 1 (list 'abs (list '- (list '* 2 (list 'mod (rewrite (nth form 1)) 1)) 1)))
      (if (= head 'saw)
          (list 'mod (rewrite (nth form 1)) 1)
      (if (= head 'sqr)
          (let ((duty (if (= (nth form 2) nil) 0.5 (rewrite (nth form 2)))))
            (list 'if (list '< (list 'mod (rewrite (nth form 1)) 1) duty) 1 0))
          (rewrite-all form)))))))))))))))

;; ── registration ────────────────────────────────────────────────────────────
;; Build the def-process source explicitly (jak-style) rather than letting the
;; compiler auto-quote the unrewritten spec; `source` provides canonical
;; escaping for every literal. The channel name doubles as the process class
;; suffix, so keep names symbol-safe (letters, digits, - _ .).

(def sig-register (name spec)
  (let ((rate (time-beats (opt-value spec :rate nil) 0.125))
        (over (time-beats (opt-value spec :over nil) 4))
        (from (opt-value spec :from 0)))
    (let ((pipe (rewrite (pipeline-form (strip-opts spec))))
          (proc (str "__sig-" name)))
      (if (__jaki-declare-value-channels (list (list name 0)))
          (eval
            (str "(do (def-process " (source proc)
                 " :every (beats " (source rate) ")"
                 " :run (let ((phase (mod (+ (/ (now-beats) " (source over) ") "
                 (source from) ") 1)))"
                 " (send " (source name) " " (source pipe) ")))"
                 " (let ((h (" proc "))) (start h) h))"))
          false))))

(defmacro sig (name &rest spec)
  `(alez.sig.surface/sig-register ,name '(,@spec)))
