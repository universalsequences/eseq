;; Phase 4 verdict and ratchet process demo.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "crates/sequencer/scripts/process-phase4-verdict-ratchet-demo.lisp")
;;
;; Put active steps on track 0 and start the transport. This attaches one
;; process to Lisp track 0, the first track in the UI. The process exposes one
;; lane for each Phase 4 playback control:
;;
;;   veto        -> calls (veto!) when > 0.5
;;   times       -> ratchet count
;;   mode        -> 0 subdivides, 1 repeats
;;   span        -> ratchet span as a multiple of (step-length)
;;   note-delta  -> shape (note! ...)
;;   vel-scale   -> shape (vel! ...)
;;   dur-scale   -> shape (dur! ...)
;;   speed-scale -> shape (speed! ...)
;;   pan-delta   -> shape (pan! ...)
;;   chop-scale  -> shape (chop! ...)
;;   nudge       -> shape (nudge! ...) as a multiple of (step-length)
;;
;; Useful live calls:
;;   (phase4-attach-track 0)
;;   (phase4-attach-current-track)
;;   (lane! phase4-verdict-ratchet-h :veto 0 1 0 0 1 0 0 0)
;;   (lane! phase4-verdict-ratchet-h :times 0 2 3 4 0 2 3 4)
;;   (processes :track 0) ; clear the process chain
;;
;; Re-evaluating this buffer is idempotent: it replaces track 0's process chain
;; for the current pattern.

(seq-register-script-source-tab "Phase 4 Verdicts/Ratchets")

(def-process phase4-verdict-ratchet
  :doc "Phase 4 demo: veto base steps and spawn shaped ratchets from lanes."
  :in ((veto :float 0 1 :default 0 :lane true)
    (times :int 0 8 :default 0 :lane true)
    (mode :int 0 1 :default 0 :lane true)
    (span :float 0 4 :default 1 :lane true)
    (note-delta :float -24 24 :default 0 :lane true)
    (vel-scale :float 0 2 :default 1 :lane true)
    (dur-scale :float 0.125 4 :default 1 :lane true)
    (speed-scale :float 0.125 4 :default 1 :lane true)
    (pan-delta :float -1 1 :default 0 :lane true)
    (chop-scale :float 0.05 1 :default 1 :lane true)
    (nudge :float -1 1 :default 0 :lane true))
  :run (do
    (if (< (rand-int 100) (* (in :veto) 100))
      (veto!)
      nil)
    (if (> (in :times) 0)
      (ratchet! :times (in :times)
        :mode (if (> (in :mode) 0.5) :repeat :subdivide)
        :span (* (step-length) (in :span))
        :shape (lambda (i ev)
          (do
            (note! ev (+ (note ev) (in :note-delta)))
            (vel! ev (clip (* (vel ev) (in :vel-scale)) 0 1))
            (dur! ev (clip (* (dur ev) (in :dur-scale)) 0.01 8))
            (speed! ev (clip (* (speed ev) (in :speed-scale)) 0.01 8))
            (pan! ev (clip (+ (pan ev) (in :pan-delta)) -1 1))
            (chop! ev (clip (* (chop ev) (in :chop-scale)) 0.01 1))
            (nudge! ev (* (step-length) (in :nudge)))
            ev)))
      nil)))

(def phase4-attach-track (track)
  (processes :track track phase4-verdict-ratchet-h))

(def phase4-attach-current-track ()
  (phase4-attach-track (seq-current-track)))

(def phase4-verdict-ratchet-h
  (phase4-verdict-ratchet
    :veto        (lane 0 0 1 0 0 1 0 0)
    :times       (lane 0 2 3 4 0 2 3 4)
    :mode        (lane 0 0 0 1 0 1 0 1)
    :span        (lane 1 1 1 0.5 1 0.25 1 0.5)
    :note-delta  (lane 0 0 7 0 -12 0 12 0)
    :vel-scale   (lane 1 0.9 0.7 0.8 1 0.6 0.85 0.75)
    :dur-scale   (lane 1 0.5 0.25 1 1 0.5 0.75 1.5)
    :speed-scale (lane 1 1 1.5 0.75 1 2 0.5 1)
    :pan-delta   (lane 0 -0.5 0.5 0 0.25 -0.25 0.75 -0.75)
    :chop-scale  (lane 1 0.75 0.5 1 1 0.5 0.25 1)
    :nudge       (lane 0 0 0.125 0 0 -0.125 0.25 0)))

(def phase4-verdict-ratchet-demo
  (processes :track 0 phase4-verdict-ratchet-h))
