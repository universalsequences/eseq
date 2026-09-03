;; SEQV per-track accessors: track lists, process lanes, param min/max/color/origin.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; This is the per-track param accessor hub: ui/sequencer.lisp, ui/seq-grid-mode.lisp,
;; ui/step-grid-interactions.lisp, ui/effects/track-panels.lisp and several Rust test
;; call sites reach its names by their flat `seqv-` spellings, so it converts with NO
;; renames and a full set of *identity* compat aliases (the seq-core-state /
;; step-grid-interactions precedent, spec §10 wave-7 addendum): an unconverted vanilla
;; caller matches the alias key flat, and a converted module's bare reference qualifies
;; against itself, misses, and lands on the same alias by base name. Every aliased name
;; is a function (or one write-once constant, `seqv-process-lane-mode-offset`), both
;; immune to hazard (m): function slots are written once, by their `def`.
;;
;; The `seqv-` prefix is NOT stripped. It is the flat spelling ui/sequencer.lisp
;; (eseq.sequencer, a UI root that must never be imported) and src/ui/input.rs use, and
;; several names here are deliberate wrappers around vanilla names that stripping would
;; collapse into unbounded recursion — `seqv-step-param-value` vs `step-param-value`,
;; `seqv-step-slider-param-value` vs `step-slider-param-value`, `seqv-param-decimals`
;; vs `param-decimals` (all three vanilla halves live in ui/step-grid-interactions.lisp).
;;
;; `duration-slider-position` / `duration-slider-value` come from
;; eseq.step-grid-interactions, which main.lisp loads immediately before this file
;; (:46 then :47), so the import is load-order safe and load-once. The reverse edge is
;; deliberately NOT requalified: step-grid-interactions.lisp calls
;; `seqv-track-process-lane` and `seqv-param-decimals` bare and must keep doing so —
;; making it import this module would create a mutual import cycle with the file that
;; loads first. Its bare calls resolve through the identity aliases below.
;;
;; No `set!`s, no widgets, no `:key`s, no modes, no macros, no hooks: hazards
;; (a)/(d)/(e)/(g)/(h)/(j)/(l)/(m) do not fire here. Hazard (n) does not fire either —
;; no Rust harness reads or slices this file's source.
(module eseq.seqv-track-params)

(import eseq.step-grid-interactions :as sgi)

(export seqv-process-lane-mode-offset
        seqv-process-lane-mode?
        seqv-process-lane-index
        seqv-track-process-lanes
        seqv-track-process-lane
        seqv-current-param-values
        seqv-track-param-values
        seqv-param-value-at
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


(def seqv-track-list (lists track)
  (if (< track (len lists))
    (nth lists track)
    '()))

;; Modes 0..8 are the built-in step params; process lanes start after them.
;; MUST match PROCESS_LANE_MODE_OFFSET in
;; crates/sequencer/src/ui/state_values/process_and_macros.rs.
(def seqv-process-lane-mode-offset 9)

(def seqv-process-lane-mode? (mode)
  (>= mode seqv-process-lane-mode-offset))

(def seqv-process-lane-index (mode)
  (- mode seqv-process-lane-mode-offset))

(def seqv-empty-process-lane ()
  (dict
    :values '()
    :min 0
    :max 1
    :default 0
    :decimals 2
    :label "Process"
    :short-label "proc"
    :instance-id 0
    :inlet ""))

(def seqv-list-ref (items idx fallback)
  (if (and (>= idx 0) (< idx (len items)))
    (nth items idx)
    fallback))

(def seqv-track-process-lanes (track)
  (seqv-list-ref SEQ.track-process-lanes track '()))

(def seqv-track-process-slots (track)
  (seqv-list-ref SEQ.track-process-slots track '()))

(def seqv-current-process-lane (mode)
  (seqv-list-ref
    SEQ.process-lanes
    (seqv-process-lane-index mode)
    (seqv-empty-process-lane)))

(def seqv-track-process-lane (track mode)
  (seqv-list-ref
    (seqv-track-process-lanes track)
    (seqv-process-lane-index mode)
    (seqv-empty-process-lane)))

(def seqv-current-param-values (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :values)
    (if (= mode 0) SEQ.velocities
      (if (= mode 1) SEQ.durations
        (if (= mode 2) SEQ.auxas
          (if (= mode 3) SEQ.transposes
            (if (= mode 4) SEQ.pans
              (if (= mode 5) SEQ.syncs
                (if (= mode 6) SEQ.delays
                  (if (= mode 7) SEQ.retrigs
                    SEQ.retrig-rates))))))))))

(def seqv-track-param-values (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :values)
    (if (= mode 0) (seqv-track-list SEQ.track-velocities track)
      (if (= mode 1) (seqv-track-list SEQ.track-durations track)
        (if (= mode 2) (seqv-track-list SEQ.track-auxas track)
          (if (= mode 3) (seqv-track-list SEQ.track-transposes track)
            (if (= mode 4) (seqv-track-list SEQ.track-pans track)
              (if (= mode 5) (seqv-track-list SEQ.track-syncs track)
                (if (= mode 6) (seqv-track-list SEQ.track-delays track)
                  (if (= mode 7) (seqv-track-list SEQ.track-retrigs track)
                    (seqv-track-list SEQ.track-retrig-rates track)))))))))))

(def seqv-param-values (track mode)
  (if (= track SEQ.current-track)
    (seqv-current-param-values mode)
    (seqv-track-param-values track mode)))

(def seqv-param-value-at (track mode step)
  (let ((values (seqv-param-values track mode)))
    (if (< step (len values))
      (nth values step)
      0)))

(def seqv-param-min (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :min)
    (if (= mode 0) 0
      (if (= mode 1) 0
        (if (= mode 2) 0
          (if (= mode 3) -12
            (if (= mode 4) -1
              (if (= mode 8) 1
                0))))))))

(def seqv-param-max (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :max)
    (if (= mode 0) 1
      (if (= mode 1) 32
        (if (= mode 2) 16
          (if (= mode 3) 12
            (if (= mode 4) 1
              (if (= mode 5) (- (len SEQ.sync-labels) 1)
                (if (= mode 7) 127
                  (if (= mode 8) 1024
                    1))))))))))

;; Modes 1 (duration), 7 (retrig) and 8 (retrig rate) ride bespoke slider curves:
;; the lane slider travels 0..1 and the value is mapped through the curve, so
;; equal travel is equal musical interval instead of equal number.
(def seqv-param-slider-min (mode)
  (if (or (= mode 1) (= mode 7) (= mode 8)) 0 (seqv-param-min mode)))

(def seqv-param-slider-max (mode)
  (if (or (= mode 1) (= mode 7) (= mode 8)) 1 (seqv-param-max mode)))

(def seqv-param-slider-value (track mode step)
  (if (= mode 1)
    (sgi/duration-slider-position (seqv-param-value-at track mode step))
    (if (= mode 8)
      (sgi/retrig-rate-slider-position (seqv-param-value-at track mode step))
      (if (= mode 7)
        (sgi/retrig-slider-position (seqv-param-value-at track mode step))
        (seqv-param-value-at track mode step)))))

(def seqv-param-haptic-pivot-position (mode)
  (if (= mode 1) 0.5 1))

(def seqv-param-haptic-pivot-value (mode)
  (if (= mode 1) 2 (seqv-param-max mode)))

(def seqv-param-haptic-exponent (mode)
  (if (= mode 1) 4 1))

(def seqv-param-keyword (mode)
  (if (seqv-process-lane-mode? mode) :process-lane
    (if (= mode 0) :velocity
      (if (= mode 1) :duration
        (if (= mode 2) :aux-a
          (if (= mode 3) :transpose
            (if (= mode 4) :pan
              (if (= mode 5) :sync
                (if (= mode 6) :delay
                  (if (= mode 7) :retrig
                    :retrig-rate))))))))))

(def seqv-param-color (mode)
  (if (seqv-process-lane-mode? mode) :orange
    (if (= mode 0) :blue
      (if (= mode 1) :green
        (if (= mode 2) :magenta
          (if (= mode 3) :yellow
            (if (= mode 4) :red
              (if (= mode 5) :green
                (if (= mode 6) :cyan
                  (if (= mode 7) :orange
                    :magenta))))))))))

(def seqv-param-name (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :label)
    (if (= mode 0) "Velocity"
      (if (= mode 1) "Duration"
        (if (= mode 2) "Aux A"
          (if (= mode 3) "Transpose"
            (if (= mode 4) "Pan"
              (if (= mode 5) "Sync"
                (if (= mode 6) "Delay"
                  (if (= mode 7) "Retrig"
                    "Rate"))))))))))

(def seqv-range-origin (min-value max-value)
  (if (and (< min-value 0) (= (abs min-value) max-value))
    0
    min-value))

(def seqv-param-origin (mode)
  (if (seqv-process-lane-mode? mode)
    (seqv-range-origin (seqv-param-min mode) (seqv-param-max mode))
    (if (= mode 3) 0
      (if (= mode 4) 0
        (if (= mode 5) 0
          ;; Rate rides a 0..1 slider curve (see seqv-param-slider-min), so
          ;; its fill grows from the bottom, not from param-min (= 1 = top).
          (if (= mode 8) 0
            (seqv-param-min mode)))))))

(def seqv-param-decimals (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :decimals)
    ;; Transpose, Retrig and Rate are whole numbers.
    (if (or (= mode 3) (= mode 7) (= mode 8)) 0 2)))

(def seqv-track-param-min (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :min)
    (seqv-param-min mode)))

(def seqv-track-param-max (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :max)
    (seqv-param-max mode)))

(def seqv-track-param-name (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :label)
    (seqv-param-name mode)))

(def seqv-track-param-origin (track mode)
  (if (seqv-process-lane-mode? mode)
    (seqv-range-origin (seqv-track-param-min track mode) (seqv-track-param-max track mode))
    (seqv-param-origin mode)))

(def seqv-track-param-decimals (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :decimals)
    (seqv-param-decimals mode)))

(def seqv-track-param-slider-min (track mode)
  (if (or (= mode 1) (= mode 7) (= mode 8)) 0 (seqv-track-param-min track mode)))

(def seqv-track-param-slider-max (track mode)
  (if (or (= mode 1) (= mode 7) (= mode 8)) 1 (seqv-track-param-max track mode)))

(def seqv-track-param-haptic-pivot-value (track mode)
  (if (= mode 1) 2 (seqv-track-param-max track mode)))

;; NB: wrapper around eseq.step-grid-interactions' `step-param-value` — the `seqv-`
;; prefix must stay (hazard k: stripping it would make this call itself).
(def seqv-step-param-value (mode value)
  (if (or (= mode 3) (= (seqv-param-decimals mode) 0))
    (round value)
    value))

(def seqv-step-slider-param-value (mode value)
  (if (= mode 1)
    (sgi/duration-slider-value value)
    (if (= mode 8)
      (sgi/retrig-rate-slider-value value)
      (if (= mode 7)
        (round (sgi/retrig-slider-value value))
        (seqv-step-param-value mode value)))))

(def seqv-track-step-param-value (track mode value)
  (if (or (= mode 3) (= (seqv-track-param-decimals track mode) 0))
    (round value)
    value))

(def seqv-track-step-slider-param-value (track mode value)
  (if (= mode 1)
    (sgi/duration-slider-value value)
    (if (= mode 8)
      (sgi/retrig-rate-slider-value value)
      (if (= mode 7)
        (round (sgi/retrig-slider-value value))
        (seqv-track-step-param-value track mode value)))))
