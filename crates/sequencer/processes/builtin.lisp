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
  :doc "Roll a deterministic integer and write it to a mappable process-inlet target."
  :targets ((out :mappable :process-inlet))
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
