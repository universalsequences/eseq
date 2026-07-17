;; Phase 3A process ports demo.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "crates/sequencer/scripts/processes/process-phase3a-ports-demo.lisp")
;;
;; Put a few active steps on track 0 and start the transport. By default this
;; attaches one process to Lisp track 0, which is the first track in the UI. To
;; use the currently selected track instead, evaluate:
;;   (phase3a-attach-current-track)
;;
;; The process has three named output ports:
;;
;;   pitch -> transient step transpose
;;   gate  -> transient sampler release param by default, remappable in the UI
;;   speed -> transient sampler instrument param, using normalized values
;;
;; Nothing below should write back into step data, p-locks, key-locks, MIDI-FX
;; slot defaults, or instrument defaults.
;;
;; Useful live calls:
;;   (seq-current-track)
;;   (phase3a-attach-current-track)
;;   (phase3a-attach-track 0)
;;   (phase3a-port-writer-h :pitch 7)
;;   (phase3a-port-writer-h :gate 1)
;;   (phase3a-port-writer-h :gate 0)
;;   (phase3a-port-writer-h :speed 0.625) ; sampler speed normalizes to raw 1.0
;;   (phase3a-port-writer-h :speed 1.0)   ; sampler speed normalizes to raw max
;;   (processes :track 0)                ; clear the process chain
;;
;; Re-evaluating this buffer is idempotent: it replaces track 0's process chain
;; for the current pattern.

(seq-register-script-source-tab "Phase 3A Ports")

(def-process phase3a-port-writer
  :doc "Phase 3A named process port target demo"
  :in ((pitch :float -24 24 :default 12)
       (gate :float 0 1 :default 0 :lane true)
       (speed :float 0 1 :default 0.625 :lane true))
  :targets '((pitch (step-param :transpose))
             (gate :mappable (instrument-param :release))
             (speed :mappable (instrument-param :speed)))
  :run (do
         (target-add! :pitch (in :pitch))
         (target-set! :gate (in :gate))
         (target-set! :speed (in :speed))))

(def phase3a-attach-track (track)
  (processes :track track phase3a-port-writer-h))

(def phase3a-attach-current-track ()
  (phase3a-attach-track (seq-current-track)))

(def phase3a-port-writer-h
  (phase3a-port-writer :pitch 12 :gate 0 :speed 0.625))

(def phase3a-ports-demo
  (processes :track 0 phase3a-port-writer-h))
