;; Project-wide performance lanes.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "content/scripts/processes/process-project-performance-lanes-demo.lisp")
;;
;; This installs one shared project process layer. Its lane values are shared
;; across all tracks, while the accumulator state and probability RNG remain
;; independent per track. The layer runs before every track's local process
;; chain, including tracks created after this file is evaluated.
;;
;; Lanes exposed on every track:
;;   repeats   integer 0..16: extra ratchet hits
;;   span      float 0.125..8: ratchet duration in step-length multiples
;;   gate      float 0..1: probability that a step survives
;;   transpose float -24..24: delta added to the running transpose
;;   reset     gate 0 or 1: resets the running transpose before this step
;;
;; Useful live calls:
;;   (lane! project-ratchets-h :repeats 0 2 0 4)
;;   (lane! project-ratchets-h :span 1 0.5 1 0.25)
;;   (lane! project-gate-h :gate 1 0.5 1 0.25)
;;   (lane! project-transpose-h :transpose 0 7 0 -7)
;;   (lane! project-transpose-h :reset 1 0 0 0)
;;   (processes :project) ; clear the project-wide layer
;;
;; Re-evaluation reconciles the named process instances, so lane edits remain
;; pattern-owned and the whole project layer is replaced atomically.

(eseq.seq-script-picker/seq-register-script-source-tab "Project Performance Lanes")

(def-process project-ratchets
  :doc "Add a lane-controlled subdivided ratchet burst to every project track."
  :in ((repeats :int 0 16 :default 0 :lane true)
       (span :float 0.125 8 :default 1 :lane true))
  :run (if (> (in :repeats) 0)
         (ratchet! :times (in :repeats)
                   :mode :subdivide
                   :span (* (step-length) (in :span)))
         nil))

(def-process project-probability-gate
  :doc "Veto every project step whose deterministic probability roll fails."
  :in ((gate :float 0 1 :default 1 :lane true))
  :seed :per-cycle
  :run (if (> (rand) (in :gate))
         (veto!)
         nil))

(def-accumulator project-transpose-accumulator
  :doc "Accumulate project-wide transpose deltas independently on each track."
  :target (step-param :transpose)
  :amount (transpose :float -24 24 :default 0 :lane true)
  :reset :lane
  :range (-48 48)
  :mode :wrap)

(def project-ratchets-h
  (project-ratchets
    :repeats (lane 0 0 2 0 4 0 1 0)
    :span (lane 1 1 0.5 1 1 0.25 0.5 1)))

(def project-gate-h
  (project-probability-gate :gate (lane 1 1 0.75 1 1 0.5 1 0.25)))

(def project-transpose-h
  (project-transpose-accumulator
    :transpose (lane 0 2 0 -2 0 5 0 -5)
    :reset (lane 1 0 0 0 0 0 0 0)))

;; Ratchets are created first; a failed gate vetoes both the original and its
;; generated hits. The accumulator then shapes each surviving event.
(processes :project project-ratchets-h project-gate-h project-transpose-h)
