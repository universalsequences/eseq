; Per-voice modulation oscillator. Physical units: Hz, cycles, milliseconds.
; Runs at audio rate: reproducing Analog's internal control grid is outside
; Heat's scope. Shapes: sine, skew triangle, pulse, random step, random ramp.
(defmacro heat-lfo (rate_hz width shape note_on retrigger phase_offset delay_ms fade_ms)
  (make-history initialized_hist)
  (make-history phase_hist)
  (make-history phase_fraction_hist)
  (make-history random_hist)
  (make-history random_start_hist)
  (make-history elapsed_hist)
  (def first (eq (read-history initialized_hist) 0))
  (def note_start (max first (gt note_on 0.5)))
  (def restart (max first (gt (* note_on retrigger) 0.5)))
  ; Keep the accumulator's fractional remainder small. Adding tiny LFO
  ; increments directly to a single float phase drifts at high sample rates.
  ; Coarse phase steps are exact binary fractions; this also supports changes
  ; of rate without deriving phase from an ever-growing sample counter.
  (def previous_coarse (read-history phase_hist))
  (def fraction (+ (read-history phase_fraction_hist) (/ (max 0 rate_hz) samplerate)))
  (def carry (/ (floor (* fraction 1024)) 1024))
  (def offset (wrap phase_offset 0 1))
  (def offset_coarse (/ (floor (* offset 1024)) 1024))
  (def coarse (gswitch restart offset_coarse (wrap (+ previous_coarse carry) 0 1)))
  (def remainder (gswitch restart (- offset offset_coarse) (- fraction carry)))
  (def phase (+ coarse remainder))
  (def cycle (max restart (gte (+ previous_coarse carry) 1)))
  (def previous_random (read-history random_hist))
  (def random_value (gswitch cycle (noise) previous_random))
  (def random_start (gswitch cycle previous_random (read-history random_start_hist)))
  (def duty (clip width 0 1))
  (def skew (+ 0.05 (* 0.9 duty)))
  (def triangle (min 1 (gswitch (lt phase duty)
    (- 1 (/ (* 2 phase) skew))
    (- 1 (/ (* 2 (- 1 phase)) (- 1 skew))))))
  (def pulse (selector (+ (lt phase duty) 1) -1 1))
  (def random_ramp (+ random_start
    (* (- random_value random_start) (clip (/ phase 0.4) 0 1))))
  (def raw (selector (+ 1 (clip (round shape) 0 4))
    (sin (* 6.28318530718 phase)) triangle pulse random_value random_ramp))
  ; Delay/fade restart for each note even when oscillator retrigger is off.
  ; Count samples exactly over the bounded delay/fade interval instead of
  ; accumulating rounded millisecond increments on every sample.
  (def elapsed_frames (gswitch note_start 0 (read-history elapsed_hist)))
  (def elapsed (* elapsed_frames (/ 1000 samplerate)))
  (def delay (max 0 delay_ms))
  (def fade (max 0 fade_ms))
  (def gain (gswitch (gt fade 0) (clip (/ (- elapsed delay) (max 0.001 fade)) 0 1)
    (gte elapsed delay)))
  (write-history phase_hist coarse)
  (write-history phase_fraction_hist remainder)
  (write-history initialized_hist 1)
  (write-history random_hist random_value)
  (write-history random_start_hist random_start)
  (write-history elapsed_hist (min (ceil (* (+ delay fade) (/ samplerate 1000))) (+ elapsed_frames 1)))
  (* raw gain))
