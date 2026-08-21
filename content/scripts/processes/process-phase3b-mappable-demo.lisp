;; Phase 3B mappable process target demo.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "content/scripts/processes/process-phase3b-mappable-demo.lisp")
;;
;; Put active steps on track 0 and start the transport. By default this attaches
;; one process to Lisp track 0, which is the first track in the UI. To use the
;; currently selected track instead, evaluate:
;;   (phase3b-attach-current-track)
;;
;; In the expanded step sequencer, select either process lane from the process
;; lane dropdown. The mapper should show the same mappable targets for this
;; process slot:
;;
;;   shape -> mappable instrument param only
;;   color -> mappable instrument/effect/MIDI-FX device param
;;
;; The pitch port is fixed to step transpose and should not show a map control.
;;
;; Good quick checks:
;;   1. On a sampler track, map "shape" to speed or release.
;;   2. Add a Filter effect, then map "color" to cutoff.
;;   3. Switch between the amount and pitch lanes; both should show the same
;;      shape/color mapping widgets because they belong to the same process slot.
;;   4. The fixed transpose target should never appear as a mapper.
;;
;; Useful live calls:
;;   (seq-current-track)
;;   (phase3b-attach-current-track)
;;   (phase3b-attach-track 0)
;;   (phase3b-writer-h :amount 0.9)
;;   (phase3b-writer-h :pitch 7)
;;   (processes :track 0) ; clear the process chain
;;
;; Re-evaluating this buffer is idempotent: it replaces track 0's process chain
;; for the current pattern.

(eseq.seq-script-picker/seq-register-script-source-tab "Phase 3B Mappable Targets")

(def-process phase3b-mappable-writer
  :doc "Phase 3B mappable target UI demo"
  :in ((amount :float 0 1 :default 0.5 :lane true)
       (pitch :float -12 12 :default 0 :lane true))
  :targets '((pitch (step-param :transpose))
             (shape :mappable :instrument-param)
             (color :mappable :device-param))
  :run (do
         (target-add! :pitch (in :pitch))
         (target-set! :shape (in :amount))
         (target-set! :color (in :amount))))

(def phase3b-attach-track (track)
  (processes :track track phase3b-writer-h))

(def phase3b-attach-current-track ()
  (phase3b-attach-track (seq-current-track)))

(def phase3b-writer-h
  (phase3b-mappable-writer
    :amount (lane 0.0 0.25 0.5 0.75 1.0 0.75 0.5 0.25)
    :pitch  (lane 0 0 7 0 12 0 7 0)))

(def phase3b-mappable-demo
  (processes :track 0 phase3b-writer-h))
