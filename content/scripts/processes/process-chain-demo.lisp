;; Step-process chain demo: sparse-lane accumulator attached to a track.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "content/scripts/processes/process-chain-demo.lisp")
;;
;; Put steps on track 0 and start the transport: the sparse delta lane folds
;; into a running transpose (Cirklon accumulator), replay-safe and previewable.
;;
;; Useful live calls:
;;   (lane! climb :amount 0 2 0 0 2 0 0 0)   ; re-sequence the delta lane
;;   (processes :track 0)                    ; clear track 0's chain
;;   (processes :track :all (sparse-transpose :amount (lane 0 1)))
;;
;; Re-evaluating this buffer is idempotent: `processes` replaces the whole
;; chain for the current pattern.

(def-accumulator sparse-transpose
  :target (step-param :transpose)
  :amount (amount :lane true :default 0)
  :range (-24 24)
  :mode :clip)

(def climb
  (processes :track 0
    (sparse-transpose :amount (lane 0 1 0 0 1 0 0 0))))
