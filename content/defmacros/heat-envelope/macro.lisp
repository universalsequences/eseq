; Independent finite-stage contour used by Heat's filter and amp envelopes.
; Times are milliseconds except sustain_seconds; negative sustain_seconds
; selects Analog's displayed infinite-sustain setting: linear holds, while
; exponential still traverses its 1000-second contour (measured in the
; envelope-times corpus). restart is an event pulse already filtered by the
; caller's per-envelope legato policy. gate remains the physical held gate.
; Modes: 0 ADSR, 1 AD-R, 2 ADR-R, 3 ADS-AR. Free ignores note-off:
; mode 0 runs ADR once, modes 1/2 loop, mode 3 runs ADAR once.
(defmacro heat-envelope
  (gate restart attack_ms decay_ms sustain sustain_seconds release_ms exponential loop_mode free_run)
  (make-history stage_hist)
  (make-history phase_hist)
  (make-history start_hist)
  (make-history value_hist)
  (make-history gate_hist)
  (def previous_stage (read-history stage_hist))
  (def previous_phase (read-history phase_hist))
  (def previous_start (read-history start_hist))
  (def previous_value (read-history value_hist))
  (def previous_gate (read-history gate_hist))
  (def held (gt gate 0.5))
  (def free (gt free_run 0.5))
  (def mode (clip (round loop_mode) 0 3))
  (def level (clip sustain 0 1))
  ; States: idle 0, attack 1, decay 2, sustain 3, release 4,
  ; note-off/Free return attack 5. All transitions use the same history set.
  (def completed (gt (* (gt previous_stage 0) (gte previous_phase 1)) 0.5))
  ; selector accepts both static settings and signal-rate controls. Keep the
  ; state policy numeric so callers can use constants or live parameters.
  (def free_end (selector (+ (eq mode 3) 1) 4 5))
  (def sustain_or_free (selector (+ free 1) 3 free_end))
  (def after_decay (selector (+ mode 1) sustain_or_free 1 4 sustain_or_free))
  (def after_release (gswitch (gt (* (eq mode 2) (max held free)) 0.5) 1 0))
  (def next_stage
    (selector (+ previous_stage 1) 0 2 after_decay 0 after_release 4))
  (def note_off (gt (* previous_gate (- 1 held) (- 1 free) (gt previous_stage 0)) 0.5))
  (def begin (gt restart 0.5))
  (def transition (gt (max begin note_off completed) 0.5))
  (def stage
    (gswitch begin 1
      (gswitch note_off free_end
        (gswitch completed next_stage previous_stage))))
  (def endpoint (selector (+ previous_stage 1) 0 1 level 0 0 1))
  (def start
    (gswitch (gt (max begin note_off) 0.5) previous_value
      (gswitch completed endpoint previous_start)))
  (def phase (gswitch transition 0 previous_phase))
  (def infinite_setting (lt sustain_seconds 0))
  (def infinite_hold (gt (* (eq stage 3) infinite_setting (lte exponential 0.5)) 0.5))
  (def sustain_duration (selector (+ infinite_setting 1) (max 0 sustain_seconds) 1000))
  (def duration_ms
    (selector (+ stage 1) 1 attack_ms decay_ms
      (* 1000 sustain_duration) release_ms attack_ms))
  (def increment (gswitch infinite_hold 0 (/ 1000 (* samplerate (max 0.001 duration_ms)))))
  (def progress (clip phase 0 1))
  (def curve (selector (+ (gt exponential 0.5) 1) progress
    (/ (- 1 (exp (* -3.5 progress))) 0.9698026166)))
  (def target (selector (+ stage 1) 0 1 level 0 0 1))
  (def value (gswitch (eq stage 0) 0 (+ start (* (- target start) curve))))
  (write-history stage_hist stage)
  (write-history phase_hist (gswitch (eq stage 0) 0 (+ phase increment)))
  (write-history start_hist start)
  (write-history value_hist value)
  (write-history gate_hist held)
  value)
