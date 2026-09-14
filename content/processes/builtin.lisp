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
  :doc "Shared reset lane: a high step resets every accumulator wired to its port before that step plays."
  ;; One out port, three cables: the primary binding reaches tacc and two
  ;; fan-out entries reach acc A and acc B (rev 4; the a/b/c trio predates
  ;; fan-out on connectable ports).
  :targets ((wire :process-inlet))
  :in ((reset :gate :default 0 :lane true))
  :run (if (> (in :reset) 0.5)
         (target-set! :wire 1)
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
  :doc "Random generator: on a high roll step, draw a new value between lo and hi and send it. Quiet steps send nothing, so a wired accumulator only moves on a roll; hold 1 keeps sending the last draw every fire (sample-and-hold)."
  :targets ((out :mappable)
            (wire :process-inlet))
  :in ((roll :gate :default 1 :lane true)
       (lo :float -128 128 :default 0)
       (hi :float -128 128 :default 12)
       (whole :int 0 1 :default 1)
       (hold :int 0 1 :default 0))
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
         (if (or (> (in :roll) 0.5) (> (in :hold) 0.5))
           (do
             (target-add! :out held)
             (target-set! :wire held))
           nil)))

(def-process lane-count
  :doc "Counter generator: each nonzero step advances the count by step and wraps from hi back to lo, then sends it. Zero steps send nothing, so a wired accumulator only moves when the count does; hold 1 sends the count every fire."
  :targets ((out :mappable)
            (wire :process-inlet))
  :in ((step :float -24 24 :default 0 :lane true)
       (lo :float -128 128 :default 0)
       (hi :float -128 128 :default 8)
       (hold :int 0 1 :default 0))
  :state ((count 0))
  :run (do
         (set! count (+ count (in :step)))
         (if (> count (in :hi)) (set! count (in :lo)) nil)
         (if (< count (in :lo)) (set! count (in :hi)) nil)
         (if (or (> (in :step) 0) (< (in :step) 0) (> (in :hold) 0.5))
           (do
             (target-add! :out count)
             (target-set! :wire count))
           nil)))

(def-process lane-cmp
  :doc "Comparator: sends 1 when the input satisfies op against value, else 0. The input is the painted lane, or whatever another lane wires in (the wire replaces the painted value on the fires it sends). hold 1 keeps comparing the last wired input on fires that bring none, instead of the lane."
  :targets ((out :mappable)
            (wire :process-inlet))
  :in ((a :float -128 128 :default 0 :lane true)
       (op :enum ("<" ">" ">=" "<=" "==" "!=") :default 1)
       (value :float -128 128 :default 0)
       (hold :int 0 1 :default 0))
  ;; `hit` first so the strip's scope draws the 1/0 output; `last` is the
  ;; most recent wired input, for hold. `in?` tells a quiet fire from an
  ;; input of 0: inlet writes are per fire. == / != use a small tolerance
  ;; since inputs are floats.
  :state ((hit 0) (last 0))
  :run (do
         (if (in? :a) (set! last (in :a)) nil)
         (set! hit
           (let ((op (in :op))
                 (d (- (if (and (> (in :hold) 0.5) (not (in? :a))) last (in :a))
                       (in :value))))
             (let ((near (and (< d 0.0001) (> d -0.0001))))
               (if (or (and (= op 0) (< d 0))
                       (and (= op 1) (> d 0))
                       (and (= op 2) (or (> d 0) near))
                       (and (= op 3) (or (< d 0) near))
                       (and (= op 4) near)
                       (and (= op 5) (not near)))
                 1
                 0))))
         (target-add! :out hit)
         (target-set! :wire hit)))

(def-process lane-veto
  :doc "Veto lane: a high step, painted or wired from a comparator, silences the step's note. Later lanes still run and advance."
  :in ((gate :gate :default 0 :lane true))
  :run (if (> (in :gate) 0.5)
         (veto!)
         nil))

(def-process lane-roll
  :doc "Roll lane: a high step, painted or wired from a comparator, rolls the whole project from that step for the step's duration, looping a window at rate. A roll already running is never restarted, so the re-fired step inside the loop cannot chain it."
  :in ((gate :gate :default 0 :lane true)
       (rate :enum ("1/4" "1/4T" "1/8" "1/8T" "1/16" "1/16T" "1/32" "1/32T") :default 4))
  :run (if (> (in :gate) 0.5)
         (roll! (in :rate))
         nil))
