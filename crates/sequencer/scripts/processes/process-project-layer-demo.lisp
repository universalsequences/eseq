;; Project process layer demo (Phase 5B).
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "crates/sequencer/scripts/processes/process-project-layer-demo.lisp")
;;
;; (processes :project ...) declares a project-level default chain: every
;; track — including tracks added later — runs these slots ahead of its own
;; per-track chain. The layer is a policy composed at fire time, never a
;; stamped snapshot: there is one shared configuration (knobs + lanes), but
;; runtime state and RNG streams are keyed per (instance, track), so a
;; project prob-mask thins each track independently instead of dropping the
;; whole mix on the same steps.
;;
;; Put active steps on a few tracks and start the transport:
;;   - every track climbs by the shared `delta` lane (its own accumulator)
;;   - every track rolls its own dice against the shared `prob` lane
;;
;; Useful live calls:
;;   (lane! ext-mask :prob 1 0.5 1 0.25)     ; edit the one shared lane
;;   (lane! ext-climb :delta 0 2 0 0)
;;   (processes :track 0 ...)                ; per-track chains compose on top
;;   (processes :project)                    ; clear the whole layer
;;
;; Re-evaluating this buffer is idempotent: named instances keep their
;; pattern-owned lane edits across re-evals (whole-layer replace with
;; reconciliation, same rules as track chains).

(eseq.seq-script-picker/seq-register-script-source-tab "Project Process Layer")

(def-process project-prob-mask
  :doc "Roll against a per-step probability lane; veto the trig on failure."
  :in ((prob :float 0 1 :default 1 :lane true))
  :seed :locked
  :run (when (> (rand) (in :prob))
         (veto!)))

(def-accumulator project-climb
  :doc "Sparse lane deltas accumulate into transpose on every track."
  :target (step-param :transpose)
  :amount (delta :float -12 12 :lane true :default 0)
  :range (-24 24)
  :mode :wrap)

(def ext-mask (project-prob-mask :prob (lane 1 1 0.75 1 1 0.5 1 0.25)))
(def ext-climb (project-climb :delta (lane 0 1 0 0 1 0 0 0)))

;; Whole-layer replace: project slots run before every track's own chain.
(processes :project ext-mask ext-climb)
