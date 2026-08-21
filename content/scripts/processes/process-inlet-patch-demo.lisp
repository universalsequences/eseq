;; Process-inlet patching demo: one process drives another process.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "content/scripts/processes/process-inlet-patch-demo.lisp")
;;
;; Put active steps on track 0 and start the transport. The first process rolls
;; a deterministic integer; its `out` target port is mapped to the `times`
;; inlet on the second process, which uses that value to ratchet the current
;; step. The second process still has its own lane, `enabled`, so you can gate
;; where the random repeat count matters.
;;
;; Useful live calls:
;;   (lane! process-inlet-demo-dice-h :roll 1 0 1 1 0 1 0 1)
;;   (lane! process-inlet-demo-repeater-h :enabled 1 1 0 1 0 1 1 0)
;;   (lane! process-inlet-demo-repeater-h :mode 0 0 0 1 0 1 0 1)
;;   (lane! process-inlet-demo-repeater-h :span 1 1 0.5 0.5 1 0.25 1 0.5)
;;   (process-inlet-demo-attach-track 0)
;;   (process-inlet-demo-reverse-track 0) ; writer after target: value lands next fire
;;   (processes :track 0)                 ; clear the process chain
;;
;; Equivalent inline connection shape:
;;   (processes :track 0
;;     (process-inlet-demo-dice
;;       :connect '((out (process-inlet :process-inlet-demo-repeater :times))))
;;     process-inlet-demo-repeater-h)

(eseq.seq-script-picker/seq-register-script-source-tab "Process Inlet Patch Demo")

(def-process process-inlet-demo-dice
  :doc "Roll a deterministic integer and write it to a connected process inlet."
  :targets ((out :process-inlet))
  :in ((lo :int 0 8 :default 1 :lane true)
       (hi :int 0 8 :default 4 :lane true)
       (roll :gate :default 1 :lane true))
  :seed :locked
  :state ((held 1))
  :run (do
         (if (> (in :roll) 0.5)
           (set! held
             (+ (min (in :lo) (in :hi))
                (floor (* (rand)
                          (+ 1 (- (max (in :lo) (in :hi))
                                  (min (in :lo) (in :hi))))))))
           nil)
         (target-set! :out held)))

(def-process process-inlet-demo-repeater
  :doc "Receive a patched repeat count on :times, then ratchet when :enabled is high."
  :in ((enabled :gate :default 1 :lane true)
       (times :int 0 8 :default 0 :lane true)
       (mode :int 0 1 :default 0 :lane true)
       (span :float 0.25 4 :default 1 :lane true)
       (decay :float 0 1 :default 0.75 :lane true)
       (pitch-step :float -12 12 :default 0 :lane true))
  :seed :locked
  :run (if (> (in :enabled) 0.5)
         (if (> (in :times) 0)
           (ratchet! :times (floor (in :times))
                     :mode (if (> (in :mode) 0.5) :repeat :subdivide)
                     :span (* (step-length) (in :span))
                     :shape (lambda (i ev)
                              (do
                                (vel! ev (clip (* (vel ev) (pow (in :decay) i)) 0 1))
                                (note! ev (+ (note ev) (* (in :pitch-step) i)))
                                ev)))
           nil)
         nil))

(def process-inlet-demo-dice-h
  (process-inlet-demo-dice
    :lo   (lane 1 1 2 1 2 3 1 2)
    :hi   (lane 2 4 5 3 6 4 7 5)
    :roll (lane 1 1 0 1 1 0 1 1)))

(def process-inlet-demo-repeater-h
  (process-inlet-demo-repeater
    :enabled    (lane 1 1 0 1 0 1 1 0)
    :times      (lane 0 0 0 0 0 0 0 0)
    :mode       (lane 0 0 0 1 0 1 0 1)
    :span       (lane 1 1 0.5 0.5 1 0.25 1 0.5)
    :decay      (lane 0.9 0.8 0.7 0.85 0.75 0.65 0.9 0.8)
    :pitch-step (lane 0 0 0 0 7 0 -12 0)))

(def process-inlet-demo-attach-track (track)
  (do
    (processes :track track
      process-inlet-demo-dice-h
      process-inlet-demo-repeater-h)
    (connect! process-inlet-demo-dice-h :out
      (inlet process-inlet-demo-repeater-h :times))))

(def process-inlet-demo-attach-current-track ()
  (process-inlet-demo-attach-track (seq-current-track)))

(def process-inlet-demo-reverse-track (track)
  (do
    (processes :track track
      process-inlet-demo-repeater-h
      process-inlet-demo-dice-h)
    (connect! process-inlet-demo-dice-h :out
      (inlet process-inlet-demo-repeater-h :times))))

(def process-inlet-demo-chain
  (process-inlet-demo-attach-track 0))
