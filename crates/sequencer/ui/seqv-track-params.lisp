;; SEQV per-track accessors: track lists, drum sounds, process lanes, param min/max/color/origin.
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


(def %seqv-track-list (lists track)
  (if (< track (len lists))
    (nth lists track)
    '()))

;; A drum rack's sequencer pitch is its pad note.  The host publishes the
;; occupied pads as {transpose, label} pairs so every sequencer surface can
;; present the same names that appear on the drum-pad grid.
(def seqv-track-drum-rack? (track)
  (%seqv-list-ref SEQ.track-drum-racks track false))

(def seqv-track-drum-sounds (track)
  (%seqv-list-ref SEQ.track-drum-sounds track '()))

(def seqv-drum-sound-mode? (track mode)
  (and (= mode 3) (seqv-track-drum-rack? track)))

(def seqv-drum-sound-count (track)
  (len (seqv-track-drum-sounds track)))

(def seqv-drum-sound-labels (track)
  (map (lambda (sound) (get sound :label)) (seqv-track-drum-sounds track)))

(def seqv-drum-sound-short-labels (track)
  (map (lambda (sound) (get sound :short-label)) (seqv-track-drum-sounds track)))

(def %seqv-drum-sound-indices-for-transpose (track transpose)
  (let ((sounds (seqv-track-drum-sounds track)))
    (filter
      (lambda (idx)
        (= (round (get (nth sounds idx) :transpose)) (round transpose)))
      (range 0 (len sounds)))))

(def seqv-drum-sound-index-for-transpose (track transpose)
  (let ((indices (%seqv-drum-sound-indices-for-transpose track transpose)))
    (if (> (len indices) 0) (nth indices 0) 0)))

(def seqv-drum-sound-label-for-transpose (track transpose)
  (let ((sounds (seqv-track-drum-sounds track))
        (indices (%seqv-drum-sound-indices-for-transpose track transpose)))
    (if (> (len indices) 0)
      (get (nth sounds (nth indices 0)) :label)
      (str "Unassigned " (round transpose)))))

(def seqv-drum-sound-transpose-at-index (track index)
  (let ((sounds (seqv-track-drum-sounds track)))
    (if (> (len sounds) 0)
      (get (nth sounds (max 0 (min (round index) (- (len sounds) 1)))) :transpose)
      0)))

(def seqv-drum-sound-transpose-for-label (track label)
  (let ((sounds (seqv-track-drum-sounds track))
        (matches
          (filter (lambda (sound) (= (get sound :label) label))
            (seqv-track-drum-sounds track))))
    (if (> (len matches) 0)
      (get (nth matches 0) :transpose)
      (if (> (len sounds) 0) (get (nth sounds 0) :transpose) 0))))

(def seqv-process-lane-mode-offset 7)

(def seqv-process-lane-mode? (mode)
  (>= mode seqv-process-lane-mode-offset))

(def seqv-process-lane-index (mode)
  (- mode seqv-process-lane-mode-offset))

(def %seqv-empty-process-lane ()
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

(def %seqv-list-ref (items idx fallback)
  (if (and (>= idx 0) (< idx (len items)))
    (nth items idx)
    fallback))

(def seqv-track-process-lanes (track)
  (%seqv-list-ref SEQ.track-process-lanes track '()))

(def %seqv-track-process-slots (track)
  (%seqv-list-ref SEQ.track-process-slots track '()))

(def %seqv-current-process-lane (mode)
  (%seqv-list-ref
    SEQ.process-lanes
    (seqv-process-lane-index mode)
    (%seqv-empty-process-lane)))

(def seqv-track-process-lane (track mode)
  (%seqv-list-ref
    (seqv-track-process-lanes track)
    (seqv-process-lane-index mode)
    (%seqv-empty-process-lane)))

(def seqv-current-param-values (mode)
  (if (seqv-process-lane-mode? mode)
    (get (%seqv-current-process-lane mode) :values)
    (if (= mode 0) SEQ.velocities
      (if (= mode 1) SEQ.durations
        (if (= mode 2) SEQ.auxas
          (if (= mode 3) SEQ.transposes
            (if (= mode 4) SEQ.pans
              (if (= mode 5) SEQ.syncs
                SEQ.delays))))))))

(def seqv-track-param-values (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :values)
    (if (= mode 0) (%seqv-track-list SEQ.track-velocities track)
      (if (= mode 1) (%seqv-track-list SEQ.track-durations track)
        (if (= mode 2) (%seqv-track-list SEQ.track-auxas track)
          (if (= mode 3) (%seqv-track-list SEQ.track-transposes track)
            (if (= mode 4) (%seqv-track-list SEQ.track-pans track)
              (if (= mode 5) (%seqv-track-list SEQ.track-syncs track)
                (%seqv-track-list SEQ.track-delays track)))))))))

(def %seqv-param-values (track mode)
  (if (= track SEQ.current-track)
    (seqv-current-param-values mode)
    (seqv-track-param-values track mode)))

(def seqv-param-value-at (track mode step)
  (let ((values (%seqv-param-values track mode)))
    (if (< step (len values))
      (nth values step)
      0)))

(def seqv-param-min (mode)
  (if (seqv-process-lane-mode? mode)
    (get (%seqv-current-process-lane mode) :min)
    (if (= mode 0) 0
      (if (= mode 1) 0
        (if (= mode 2) 0
          (if (= mode 3) -12
            (if (= mode 4) -1
              0)))))))

(def seqv-param-max (mode)
  (if (seqv-process-lane-mode? mode)
    (get (%seqv-current-process-lane mode) :max)
    (if (= mode 0) 1
      (if (= mode 1) 32
        (if (= mode 2) 16
          (if (= mode 3) 12
            (if (= mode 4) 1
              (if (= mode 5) (- (len SEQ.sync-labels) 1)
                1))))))))

(def %seqv-param-slider-min (mode)
  (if (= mode 1) 0 (seqv-param-min mode)))

(def %seqv-param-slider-max (mode)
  (if (= mode 1) 1 (seqv-param-max mode)))

(def %seqv-param-slider-value (track mode step)
  (if (= mode 1)
    (sgi/duration-slider-position (seqv-param-value-at track mode step))
    (seqv-param-value-at track mode step)))

(def seqv-param-haptic-pivot-position (mode)
  (if (= mode 1) 0.5 1))

(def %seqv-param-haptic-pivot-value (mode)
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
                :delay))))))))

(def seqv-param-color (mode)
  (if (seqv-process-lane-mode? mode) :orange
    (if (= mode 0) :blue
      (if (= mode 1) :green
        (if (= mode 2) :magenta
          (if (= mode 3) :yellow
            (if (= mode 4) :red
              (if (= mode 5) :green
                :cyan))))))))

(def seqv-param-name (mode)
  (if (seqv-process-lane-mode? mode)
    (get (%seqv-current-process-lane mode) :label)
    (if (= mode 0) "Velocity"
      (if (= mode 1) "Duration"
        (if (= mode 2) "Aux A"
          (if (= mode 3) "Transpose"
            (if (= mode 4) "Pan"
              (if (= mode 5) "Sync"
                "Delay"))))))))

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
          (seqv-param-min mode))))))

(def seqv-param-decimals (mode)
  (if (seqv-process-lane-mode? mode)
    (get (%seqv-current-process-lane mode) :decimals)
    (if (= mode 3) 0 2)))

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
  (if (= mode 1) 0 (seqv-track-param-min track mode)))

(def seqv-track-param-slider-max (track mode)
  (if (= mode 1) 1 (seqv-track-param-max track mode)))

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
    (seqv-step-param-value mode value)))

(def seqv-track-step-param-value (track mode value)
  (if (or (= mode 3) (= (seqv-track-param-decimals track mode) 0))
    (round value)
    value))

(def seqv-track-step-slider-param-value (track mode value)
  (if (= mode 1)
    (sgi/duration-slider-value value)
    (seqv-track-step-param-value track mode value)))
