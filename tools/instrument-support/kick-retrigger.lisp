;; Two independent hit slots, using the 808 Kick's exponential retrigger
;; crossfade. At 5 ms the old slot is -60 dB; the first hit bypasses the
;; gain ramp to preserve its attack. Return weights before updating them
;; so the outgoing hit continues at the trigger sample.
(defmacro kick-retrigger-gain (target first_hit)
  (make-history gain_h)
  (def previous_gain (read-history gain_h))
  (def coefficient (exp (/ -6.9077553 (* samplerate 0.005))))
  (def next_gain (gswitch first_hit target
    (+ target (* coefficient (- previous_gain target)))))
  (write-history gain_h next_gain)
  (gswitch first_hit target previous_gain))

(defmacro kick-retrigger (trigger gate)
  (make-history previous_trigger)
  (make-history previous_gate)
  (def trigger_high (gt trigger 0.5))
  (def gate_high (gt gate 0.5))
  (def onset (max (* trigger_high (lte (read-history previous_trigger) 0.5))
    (* gate_high (lte (read-history previous_gate) 0.5))))
  (write-history previous_trigger trigger_high)
  (write-history previous_gate gate_high)
  (make-history selector_h)
  (def previous_selector (read-history selector_h))
  (def slot (gswitch onset (- 1 previous_selector) previous_selector))
  (write-history selector_h slot)
  (make-history ever_triggered)
  (def first_hit (* onset (lte (read-history ever_triggered) 0.5)))
  (write-history ever_triggered (max onset (read-history ever_triggered)))
  (def target_a (lt slot 0.5))
  (def target_b (- 1 target_a))
  (def gain_a (kick-retrigger-gain target_a first_hit))
  (def gain_b (kick-retrigger-gain target_b first_hit))
  (tuple (* onset target_a) (* onset target_b) gain_a gain_b))
