;; Multi-accumulator UI stress test.
;;
;; Load this from the Scripts sidebar, then open track 0 in the expanded
;; sequencer. The process-lane tabs should include four dynamic lanes:
;;   octave-rise / amount
;;   fifth-fall / amount
;;   phrase-reset / amount
;;   phrase-reset / reset
;;
;; Put steps on track 0 and start the transport. All three accumulators write
;; transient step transpose and add together in chain order.
;;
;; Useful live calls:
;;   (lane! octave-rise-h :amount 0 0 0 0 12 0 0 0 0 0 0 0 -12 0 0 0)
;;   (lane! fifth-fall-h :amount 0 -7 0 0 0 0 0 7 0 0 0 -7 0 0 0 7)
;;   (lane! phrase-reset-h :reset 1 0 0 0 0 0 0 0 1 0 0 0 0 0 0 0)
;;   (processes :track 0) ; clear track 0's chain
;;
;; Re-evaluating this buffer is idempotent: `processes` replaces the whole
;; chain for the current pattern.

(def-accumulator octave-rise
  :doc "Octave rise accumulator"
  :target (step-param :transpose)
  :amount (amount :lane true :default 0)
  :range (-24 24)
  :mode :wrap)

(def-accumulator fifth-fall
  :doc "Fifth fall accumulator"
  :target (step-param :transpose)
  :amount (amount :lane true :default 0)
  :range (-14 14)
  :mode :wrap)

(def-accumulator phrase-reset
  :doc "Phrase reset accumulator"
  :target (step-param :transpose)
  :amount (amount :lane true :default 0)
  :reset :lane
  :range (-12 12)
  :mode :wrap)

(def octave-rise-h
  (octave-rise :amount (lane 0 0 0 0 12 0 0 0 0 0 0 0 -12 0 0 0)))

(def fifth-fall-h
  (fifth-fall :amount (lane 0 -7 0 0 0 0 0 7 0 0 0 -7 0 0 0 7)))

(def phrase-reset-h
  (phrase-reset
    :amount (lane 0 1 0 0 1 0 0 0 -2 0 1 0 0 0 -1 0)
    :reset (lane 1 0 0 0 0 0 0 0 1 0 0 0 0 0 0 0)))

(def stacked-accumulators
  (processes :track 0
    octave-rise-h
    fifth-fall-h
    phrase-reset-h))
