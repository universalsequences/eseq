;; Phase 7 resolved reads and history demo.
;;
;; Evaluate this file directly as an ESeqLisp buffer, or load it from project
;; scratch:
;;   (load "crates/sequencer/scripts/process-phase7-reads-demo.lisp")
;;
;; Prepare two melodic tracks, then start the transport:
;;
;;   track 0 (source): use a sparse pattern, for example active steps 0, 2, 5
;;                     with transpose values 0, 7, and 12.
;;   track 1 (reader): activate all eight steps with transpose 0.
;;
;; Track 1 rotates through one Phase 7 source per step, so the results do not
;; collapse into one ambiguous sum:
;;
;;   step 0  current resolved transpose from track 0
;;   step 1  track 0's held transpose two grid steps ago
;;   step 2  track 0's previous fired-note transpose (`:trigs-ago 1`)
;;   step 3  standalone brain state (`phase`)
;;   step 4  standalone brain outlet (`value`)
;;   step 5  named channel value (`phase7-demo-density`)
;;   step 6  current resolved transpose again
;;   step 7  track 0's held transpose four grid steps ago
;;
;; Track reads obey the previous-tick rule. When both tracks fire on the same
;; boundary, track 1 hears track 0's value from the completed prior step, never
;; a scheduler-order-dependent same-tick value. Grid gaps remain sample-and-
;; hold values; `:trigs-ago` counts only actual source fires.
;;
;; Useful live calls:
;;   (ps)
;;   (phase7-demo-attach 0 1)
;;   (lane! phase7-demo-current-h :amount 1 0 0 0 0 0 1 0)
;;   (processes :track 1) ; clear the reader chain
;;   (stop phase7-demo-brain)
;;   (start phase7-demo-brain)

(seq-register-script-source-tab "Phase 7 Reads")

(def phase7-demo-density
  (defchan phase7-demo-density 0))

(def-process phase7-demo-clock
  :doc "Publish a small deterministic phrase through state, an outlet, and a channel for Phase 7 reads."
  :out ((value :float))
  :state ((phase 0)
          (direction 1))
  :every (beats 1)
  :run (do
         (set! phase (+ phase direction))
         (if (>= phase 4)
           (set! direction -1)
           nil)
         (if (<= phase 0)
           (set! direction 1)
           nil)
         (out :value phase)
         (send :phase7-demo-density (* phase 0.5))))

(def phase7-demo-brain (phase7-demo-clock))
(start phase7-demo-brain)

(def-process phase7-demo-current
  :doc "Read another track's most recent resolved transpose under the previous-tick rule."
  :target (step-param :transpose)
  :in ((source :track :default 0)
       (amount :float 0 1 :default 0 :lane true))
  :run (target-add!
         (* (in :amount)
            (read (track (in :source) :transpose)))))

(def-process phase7-demo-steps
  :doc "Read sample-and-hold transpose history measured in source-track grid steps."
  :target (step-param :transpose)
  :in ((source :track :default 0)
       (lag :int 0 255 :default 2)
       (amount :float 0 1 :default 0 :lane true))
  :run (target-add!
         (* (in :amount)
            (read (track (in :source)
                         :transpose
                         :steps-ago (in :lag))))))

(def-process phase7-demo-trigs
  :doc "Read event-locked transpose history measured only in fired source notes."
  :target (step-param :transpose)
  :in ((source :track :default 0)
       (lag :int 0 255 :default 1)
       (amount :float 0 1 :default 0 :lane true))
  :run (target-add!
         (* (in :amount)
            (read (track (in :source)
                         :transpose
                         :trigs-ago (in :lag))))))

(def-process phase7-demo-state
  :doc "Read the named standalone brain's persistent phase state."
  :target (step-param :transpose)
  :in ((amount :float 0 1 :default 0 :lane true))
  :run (target-add!
         (* (in :amount)
            (read (process phase7-demo-brain :phase)))))

(def-process phase7-demo-outlet
  :doc "Read the named standalone brain's published value outlet."
  :target (step-param :transpose)
  :in ((amount :float 0 1 :default 0 :lane true))
  :run (target-add!
         (* (in :amount)
            (if (read (process phase7-demo-brain :value))
              (read (process phase7-demo-brain :value))
              0))))

(def-process phase7-demo-channel
  :doc "Read the brain's named channel value."
  :target (step-param :transpose)
  :in ((amount :float 0 1 :default 0 :lane true))
  :run (target-add!
         (* (in :amount)
            (read :channel :phase7-demo-density))))

(def phase7-demo-current-h
  (phase7-demo-current :source 0 :amount (lane 1 0 0 0 0 0 1 0)))

(def phase7-demo-steps-h
  (phase7-demo-steps :source 0 :lag 2 :amount (lane 0 1 0 0 0 0 0 0)))

(def phase7-demo-trigs-h
  (phase7-demo-trigs :source 0 :lag 1 :amount (lane 0 0 1 0 0 0 0 0)))

(def phase7-demo-state-h
  (phase7-demo-state :amount (lane 0 0 0 1 0 0 0 0)))

(def phase7-demo-outlet-h
  (phase7-demo-outlet :amount (lane 0 0 0 0 1 0 0 0)))

(def phase7-demo-channel-h
  (phase7-demo-channel :amount (lane 0 0 0 0 0 1 0 0)))

(def phase7-demo-steps-four-h
  (phase7-demo-steps :source 0 :lag 4 :amount (lane 0 0 0 0 0 0 0 1)))

(def phase7-demo-attach (source reader)
  (do
    (phase7-demo-current-h :source source)
    (phase7-demo-steps-h :source source)
    (phase7-demo-trigs-h :source source)
    (phase7-demo-steps-four-h :source source)
    (processes :track reader
      phase7-demo-current-h
      phase7-demo-steps-h
      phase7-demo-trigs-h
      phase7-demo-state-h
      phase7-demo-outlet-h
      phase7-demo-channel-h
      phase7-demo-steps-four-h)))

(phase7-demo-attach 0 1)
(ps)
