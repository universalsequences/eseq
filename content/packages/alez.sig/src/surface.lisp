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
;; Mechanics mirror alez.jaki.surface: the macro parses its option forms and
;; rewrites the pipeline on the authoring VM, then returns ordinary process
;; syntax. The def-process :run body auto-quotes and ships to the scheduler VM.
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

(def rewrite-all (forms phase)
  (if (empty? forms)
      (list)
      (cons (rewrite (first forms) phase) (rewrite-all (rest forms) phase))))

(def rewrite (form phase)
  (if (= form 'phase) phase
  (if (= form 'tau) tau-value
  (if (= form 'pi) pi-value
    (let ((head (nth form 0)))
      (if (= head nil)
          form
      (if (= head '->)
          (rewrite (thread-form (nth form 1) (rest (rest form)) false) phase)
      (if (= head '->>)
          (rewrite (thread-form (nth form 1) (rest (rest form)) true) phase)
      (if (= head 'scale)
          (let ((x (rewrite (nth form 1) phase))
                (a (rewrite (nth form 2) phase))
                (b (rewrite (nth form 3) phase))
                (c (rewrite (nth form 4) phase))
                (d (rewrite (nth form 5) phase)))
            (list '+ c (list '* (list '- d c)
                             (list '/ (list '- x a) (list '- b a)))))
      (if (= head 'unipolar)
          (list '+ 0.5 (list '* 0.5 (rewrite (nth form 1) phase)))
      (if (= head 'bipolar)
          (list '- (list '* 2 (rewrite (nth form 1) phase)) 1)
      (if (= head 'sine)
          (list '+ 0.5
                (list '* 0.5 (list 'sin (list '* tau-value
                                              (rewrite (nth form 1) phase)))))
      (if (= head 'tri)
          (list '- 1 (list 'abs (list '- (list '* 2 (list 'mod (rewrite (nth form 1) phase) 1)) 1)))
      (if (= head 'saw)
          (list 'mod (rewrite (nth form 1) phase) 1)
      (if (= head 'sqr)
          (let ((duty (if (= (nth form 2) nil) 0.5 (rewrite (nth form 2) phase))))
            (list 'if (list '< (list 'mod (rewrite (nth form 1) phase) 1) duty) 1 0))
          (rewrite-all form phase))))))))))))))))

;; ── expansion ───────────────────────────────────────────────────────────────
;; The generated process class and local bindings use deterministic expansion-
;; site symbols. This keeps repeated evaluations stable while preventing
;; authored pipeline names from capturing implementation bindings.

(defmacro sig (name &rest spec)
  (let ((rate (time-beats (opt-value spec :rate nil) 0.125))
        (over (time-beats (opt-value spec :over nil) 4))
        (from (opt-value spec :from 0))
        (proc (gensym (str "__sig-" name)))
        (phase (gensym "phase"))
        (handle (gensym "handle")))
    (let ((pipe (rewrite (pipeline-form (strip-opts spec)) phase)))
      `(if (__jaki-declare-value-channels (list (list ,name 0)))
           (do
             (def-process ,proc
               :every (beats ,rate)
               :run (let ((,phase (mod (+ (/ (now-beats) ,over) ,from) 1)))
                      (send ,name ,pipe)))
             (let ((,handle (,proc)))
               (do (start ,handle) ,handle)))
           false))))
