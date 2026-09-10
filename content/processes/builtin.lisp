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

(def-process follow-harmony
  :doc "Move the current note toward the previous-tick pitch field. Missing publishers are inert; amount is sequenceable obedience."
  :target (step-param :transpose)
  :in ((listen :field :default :harmony)
       (amount :float 0 1 :default 1 :lane true)
       (grace :int 0 3 :default 0))
  :run (let ((field (hear (in :listen))))
         (if field
           (target-add!
             (* (in :amount)
                (field-weight field)
                (field-nearest-delta field (current-note) (in :grace))))
           nil)))

;; ---------------------------------------------------------------------------
;; Default project lanes (docs/default-process-lanes-spec.md).
;;
;; These classes back the always-on project layer that every scene carries:
;; prob, tacc, reset, acc A, acc B, grab, rand, count. The Rust side installs
;; the instances (`crate::process::ensure_default_project_layer`); the classes
;; live here so they survive a project switch with the rest of the package
;; layer. Class names carry a `lane-` prefix because every def-process name is
;; also a constructor native, and `rand` / `count` are already taken.
;;
;; Every generator exposes two output ports: `out` (parameter-mappable) and
;; `wire` (connectable to another lane's inlet). Both receive the same value
;; each fire, so one lane can drive a synth param and feed another lane at
;; once. Mappable ports never bind to process inlets and vice versa, which is
;; why the pair exists.

(def-process lane-prob
  :doc "Step probability: veto the step when the seeded roll exceeds the lane."
  :in ((prob :float 0 1 :default 1 :lane true))
  :seed :per-cycle
  :run (if (> (rand) (in :prob))
         (veto!)
         nil))

(def-process lane-acc
  :doc "Accumulator lane. Mode 0 folds lane deltas into a running value that wraps inside lo..hi; mode 1 passes each fire's input straight through. Reset clears the running value before the step."
  :targets ((out :mappable)
            (wire :process-inlet))
  :in ((amount :float -24 24 :default 0 :lane true)
       (reset :gate :default 0)
       (mode :int 0 1 :default 0)
       (lo :float -128 128 :default -48)
       (hi :float -128 128 :default 48))
  :state ((value 0))
  ;; State writes stay at the top level of the body: `set!` inside a nested
  ;; `let` does not reach the process state cell.
  :run (do
         (if (> (in :reset) 0.5) (set! value 0) nil)
         (set! value (+ value (in :amount)))
         (if (> (- (in :hi) (in :lo)) 0)
           (set! value
             (+ (in :lo)
                (- (- value (in :lo))
                   (* (- (in :hi) (in :lo))
                      (floor (/ (- value (in :lo)) (- (in :hi) (in :lo))))))))
           nil)
         (target-add! :out (if (> (in :mode) 0.5) (in :amount) value))
         (target-set! :wire (if (> (in :mode) 0.5) (in :amount) value))))

(def-process lane-reset
  :doc "Shared reset lane: a high step resets every accumulator wired to its ports before that step plays."
  :targets ((a :process-inlet)
            (b :process-inlet)
            (c :process-inlet))
  :in ((reset :gate :default 0 :lane true))
  :run (if (> (in :reset) 0.5)
         (do
           (target-set! :a 1)
           (target-set! :b 1)
           (target-set! :c 1))
         nil))

(def-process lane-grab
  :doc "Transpose grab: add another track's previous-tick resolved transpose, lagged by source-track grid steps and scaled by the lane."
  :target (step-param :transpose)
  :in ((amount :float 0 1 :default 0 :lane true)
       (source :track :default 0)
       (lag :int 0 255 :default 0))
  :run (target-add!
         (* (in :amount)
            (read (track (in :source)
                         :transpose
                         :steps-ago (in :lag))))))

(def-process lane-rand
  :doc "Random generator: on a high roll step, draw a new value between lo and hi; hold it otherwise. Writes the held value to its outputs every fire."
  :targets ((out :mappable)
            (wire :process-inlet))
  :in ((roll :gate :default 1 :lane true)
       (lo :float -128 128 :default 0)
       (hi :float -128 128 :default 12)
       (whole :int 0 1 :default 1))
  :seed :per-cycle
  :state ((held 0))
  :run (do
         (if (> (in :roll) 0.5)
           (set! held
             (+ (min (in :lo) (in :hi))
                (* (rand) (- (max (in :lo) (in :hi)) (min (in :lo) (in :hi))))))
           nil)
         (if (and (> (in :roll) 0.5) (> (in :whole) 0.5))
           (set! held (floor (+ held 0.5)))
           nil)
         (target-add! :out held)
         (target-set! :wire held)))

(def-process lane-count
  :doc "Counter generator: each high step advances the count by step and wraps from hi back to lo. Writes the count to its outputs every fire."
  :targets ((out :mappable)
            (wire :process-inlet))
  :in ((step :float -24 24 :default 0 :lane true)
       (lo :float -128 128 :default 0)
       (hi :float -128 128 :default 8))
  :state ((count 0))
  :run (do
         (set! count (+ count (in :step)))
         (if (> count (in :hi)) (set! count (in :lo)) nil)
         (if (< count (in :lo)) (set! count (in :hi)) nil)
         (target-add! :out count)
         (target-set! :wire count)))
