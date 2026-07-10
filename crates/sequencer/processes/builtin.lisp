; Builtin scheduler process definitions.

(def-process prob-mask
  :doc "Veto the current step when a deterministic process RNG roll exceeds the probability inlet."
  :in ((prob :float 0 1 :default 1 :lane true))
  :seed :locked
  :run (if (> (rand) (in :prob))
         (veto!)
         nil))

(def-process repeater
  :doc "Clone the current step into a ratchet burst. Mode 0 subdivides the span; mode 1 repeats at the span interval."
  :in ((times :int 0 8 :default 0 :lane true)
       (mode :int 0 1 :default 0)
       (decay :float 0 1 :default 0.7)
       (spread :float 0.5 2 :default 1))
  :seed :locked
  :run (if (> (in :times) 0)
         (ratchet! :times (in :times)
                   :mode (if (> (in :mode) 0.5) :repeat :subdivide)
                   :span (* (step-length) (in :spread))
                   :shape (lambda (i ev)
                            (vel! ev (* (vel ev) (pow (in :decay) i)))))
         nil))

(def-process dice
  :doc "Roll a deterministic integer and write it to a connected process inlet."
  :targets ((out :process-inlet))
  :in ((lo :int 0 16 :default 1 :lane true)
       (hi :int 0 16 :default 4 :lane true)
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

(def-process echo-track
  :doc "Add a previous-tick resolved transpose from another track; lag is measured in source-track grid steps."
  :target (step-param :transpose)
  :in ((source :track :default 0)
       (lag :int 0 255 :default 8)
       (amount :float 0 1 :default 1 :lane true))
  :run (target-add!
         (* (in :amount)
            (read (track (in :source)
                         :transpose
                         :steps-ago (in :lag))))))

(def-process wrap-crash
  :doc "Accumulate a lane delta, emit a crash to the selected track on each octave wrap, and add the held phase to transpose."
  :target (step-param :transpose)
  :in ((delta :float 0 4 :default 0 :lane true)
       (track :track :default 7))
  :state ((acc 0))
  :run (do
         (set! acc (+ acc (in :delta)))
         (if (>= acc 12)
           (do
             (set! acc (- acc 12))
             (emit :track (in :track) :note 0 :vel 0.9 :duration 0.5))
           nil)
         (target-add! acc)))
