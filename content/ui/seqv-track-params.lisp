;; The step editors' param modes: what each mode edits on a track and how its
;; sliders scale. Modes 0..8 are the built-in step params (step instances'
;; fields); a mode from `seqv-process-lane-mode-offset` on picks a process
;; lane by its place in t.lanes. Tracks, steps and lanes are eseq.kinds
;; instances; the `seqv-current-…` / `seqv-param-…` forms read the current
;; track (selection.track), for the *step* panel (ui/effects/track-panels.lisp)
;; and the grid mode (ui/seq-grid-mode.lisp).
;;
;; The `seqv-` prefix stays: several names here wrap ui/step-grid-interactions'
;; unprefixed ones (`seqv-step-param-value` around `step-param-value`, …), and
;; the other views spell them so.
(module eseq.seqv-track-params)

(import eseq.step-grid-interactions :as sgi)
(import eseq.kinds :refer (selection project))

(export seqv-process-lane-mode-offset
        seqv-process-lane-mode?
        seqv-process-lane-index
        seqv-track-process-lane
        seqv-step-value
        seqv-step-ref
        seqv-set-step-value!
        seqv-current-param-values
        seqv-current-param-value
        seqv-param-min
        seqv-param-max
        seqv-param-haptic-pivot-position
        seqv-param-haptic-exponent
        seqv-param-keyword
        seqv-param-color
        seqv-param-name
        seqv-range-origin
        seqv-param-origin
        seqv-param-decimals
        seqv-param-curved?
        seqv-param-slider-position
        seqv-track-param-min
        seqv-track-param-max
        seqv-track-param-name
        seqv-track-param-origin
        seqv-track-param-decimals
        seqv-track-param-slider-min
        seqv-track-param-slider-max
        seqv-track-param-haptic-pivot-value
        seqv-step-param-value
        seqv-step-slider-param-value
        seqv-track-step-param-value
        seqv-track-step-slider-param-value)

;; Modes 0..8 are the built-in step params; process lanes start after them.
;; MUST match PROCESS_LANE_MODE_OFFSET in
;; crates/sequencer/src/ui/state_values/process_and_macros.rs.
(def seqv-process-lane-mode-offset 9)

(def seqv-process-lane-mode? (mode)
  (>= mode seqv-process-lane-mode-offset))

(def seqv-process-lane-index (mode)
  (- mode seqv-process-lane-mode-offset))

;; The lane a lane mode picks on track t (one of t.lanes), or nil (a built-in
;; mode, no track, or a lane the track no longer has).
(def seqv-track-process-lane (t mode)
  (when (and t (seqv-process-lane-mode? mode))
    (nth t.lanes (seqv-process-lane-index mode))))

;; The step fields of the built-in param modes, in one place: step s's value
;; of mode `mode`, a binding to it (`#'`), and its setter.
(def seqv-step-value (s mode)
  (match mode
    0 s.velocity
    1 s.duration
    2 s.aux-a
    3 s.transpose
    4 s.pan
    5 s.sync
    6 s.delay
    7 s.retrig
    _ s.retrig-rate))

(def seqv-step-ref (s mode)
  (match mode
    0 #'s.velocity
    1 #'s.duration
    2 #'s.aux-a
    3 #'s.transpose
    4 #'s.pan
    5 #'s.sync
    6 #'s.delay
    7 #'s.retrig
    _ #'s.retrig-rate))

(def seqv-set-step-value! (s mode v)
  (match mode
    0 (set! s.velocity v)
    1 (set! s.duration v)
    2 (set! s.aux-a v)
    3 (set! s.transpose v)
    4 (set! s.pan v)
    5 (set! s.sync v)
    6 (set! s.delay v)
    7 (set! s.retrig v)
    _ (set! s.retrig-rate v)))

;; The current track's values of mode `mode`, by step (the grid mode's
;; slider list).
(def seqv-current-param-values (mode)
  (let ((t selection.track))
    (if (seqv-process-lane-mode? mode)
      (let ((lane (seqv-track-process-lane t mode))) (if lane lane.values '()))
      (if t (map (lambda (s) (seqv-step-value s mode)) t.steps) '()))))

;; The current track's value of mode `mode` at step `step` (0 past its end):
;; one step's field, where the list form reads every step.
(def seqv-current-param-value (mode step)
  (let ((t selection.track))
    (if (seqv-process-lane-mode? mode)
      (let ((lane (seqv-track-process-lane t mode)))
        (if lane (or (nth lane.values step) 0) 0))
      (let ((s (when t (nth t.steps step))))
        (if s (seqv-step-value s mode) 0)))))

(def builtin-param-min (mode)
  (match mode
    3 -12
    4 -1
    8 1
    _ 0))

(def builtin-param-max (mode)
  (match mode
    0 1
    1 32
    2 16
    3 12
    4 1
    5 (- (len project.sync-options) 1)
    7 127
    8 1024
    _ 1))

(def builtin-param-name (mode)
  (match mode
    0 "Velocity"
    1 "Duration"
    2 "Aux A"
    3 "Transpose"
    4 "Pan"
    5 "Sync"
    6 "Delay"
    7 "Retrig"
    _ "Rate"))

;; The current track's ranges, names and precision for mode `mode`.
(def seqv-param-min (mode)
  (seqv-track-param-min selection.track mode))

(def seqv-param-max (mode)
  (seqv-track-param-max selection.track mode))

(def seqv-param-name (mode)
  (seqv-track-param-name selection.track mode))

(def seqv-param-origin (mode)
  (seqv-track-param-origin selection.track mode))

(def seqv-param-decimals (mode)
  (seqv-track-param-decimals selection.track mode))

;; Modes 1 (duration), 7 (retrig) and 8 (retrig rate) ride bespoke slider
;; curves: the slider travels 0..1 and the value is mapped through the curve,
;; so equal travel is equal musical interval instead of equal number.
(def seqv-param-curved? (mode)
  (or (= mode 1) (= mode 7) (= mode 8)))

;; Where value v of mode `mode` sits on its slider.
(def seqv-param-slider-position (mode v)
  (match mode
    1 (sgi/duration-slider-position v)
    7 (sgi/retrig-slider-position v)
    8 (sgi/retrig-rate-slider-position v)
    _ v))

(def seqv-param-haptic-pivot-position (mode)
  (if (= mode 1) 0.5 1))

(def seqv-param-haptic-exponent (mode)
  (if (= mode 1) 4 1))

(def seqv-param-keyword (mode)
  (if (seqv-process-lane-mode? mode)
    :process-lane
    (match mode
      0 :velocity
      1 :duration
      2 :aux-a
      3 :transpose
      4 :pan
      5 :sync
      6 :delay
      7 :retrig
      _ :retrig-rate)))

(def seqv-param-color (mode)
  (if (seqv-process-lane-mode? mode)
    :process-lane-accent
    (match mode
      0 :blue
      1 :green
      2 :magenta
      3 :yellow
      4 :red
      5 :green
      6 :cyan
      7 :orange
      _ :magenta)))

(def seqv-range-origin (min-value max-value)
  (if (and (< min-value 0) (= (abs min-value) max-value))
    0
    min-value))

;; Track t's ranges, names and precision for mode `mode`: a lane mode reads
;; its lane (one that is gone reads as an empty 0..1 lane).
(def seqv-track-param-min (t mode)
  (if (seqv-process-lane-mode? mode)
    (let ((lane (seqv-track-process-lane t mode))) (if lane lane.min 0))
    (builtin-param-min mode)))

(def seqv-track-param-max (t mode)
  (if (seqv-process-lane-mode? mode)
    (let ((lane (seqv-track-process-lane t mode))) (if lane lane.max 1))
    (builtin-param-max mode)))

(def seqv-track-param-name (t mode)
  (if (seqv-process-lane-mode? mode)
    (let ((lane (seqv-track-process-lane t mode))) (if lane lane.label "Process"))
    (builtin-param-name mode)))

(def seqv-track-param-origin (t mode)
  (if (seqv-process-lane-mode? mode)
    (seqv-range-origin (seqv-track-param-min t mode) (seqv-track-param-max t mode))
    ;; Rate rides a 0..1 slider curve, so its fill grows from the bottom,
    ;; not from its minimum.
    (match mode
      3 0
      4 0
      5 0
      8 0
      _ (builtin-param-min mode))))

(def seqv-track-param-decimals (t mode)
  (if (seqv-process-lane-mode? mode)
    (let ((lane (seqv-track-process-lane t mode))) (if lane lane.decimals 2))
    ;; Transpose, Retrig and Rate are whole numbers.
    (if (or (= mode 3) (= mode 7) (= mode 8)) 0 2)))

(def seqv-track-param-slider-min (t mode)
  (if (seqv-param-curved? mode) 0 (seqv-track-param-min t mode)))

(def seqv-track-param-slider-max (t mode)
  (if (seqv-param-curved? mode) 1 (seqv-track-param-max t mode)))

(def seqv-track-param-haptic-pivot-value (t mode)
  (if (= mode 1) 2 (seqv-track-param-max t mode)))

;; NB: wrapper around eseq.step-grid-interactions' `step-param-value` — the
;; `seqv-` prefix must stay (stripping it would make this call itself).
(def seqv-step-param-value (mode value)
  (seqv-track-step-param-value selection.track mode value))

(def seqv-step-slider-param-value (mode value)
  (seqv-track-step-slider-param-value selection.track mode value))

;; A typed (picker) value for mode `mode` on track t: whole numbers where the
;; param takes them.
(def seqv-track-step-param-value (t mode value)
  (if (or (= mode 3) (= (seqv-track-param-decimals t mode) 0))
    (round value)
    value))

;; The value a slider position sets for mode `mode` on track t.
(def seqv-track-step-slider-param-value (t mode value)
  (match mode
    1 (sgi/duration-slider-value value)
    8 (sgi/retrig-rate-slider-value value)
    7 (round (sgi/retrig-slider-value value))
    _ (seqv-track-step-param-value t mode value)))
