;; Shared UI state + cursor/page primitives. Loads before the render-root files below so their defstates and defs exist when those files compile.
;; Extracted from ui/main.lisp (module-system spec slice S2). Headerless on
;; purpose: implicit eseq.vanilla until per-file (module …) headers land in S3.

(defstate selected-bus -1)

(def selected-bus-name ()
  (if (and (>= selected-bus 0) (< selected-bus (len SEQ.bus-names)))
    (nth SEQ.bus-names selected-bus)
    "Bus"))

(def seq-has-selected-bus? ()
  (and (>= selected-bus 0) (< selected-bus (len SEQ.bus-names))))

(defstate samples-sidebar-visible true)
(defstate mixer-panel-visible true)
(defstate lower-panel-visible true)
(defstate patch-macros-panel-visible true)

; 0=vel 1=dur 2=aux_a 3=transpose 4=pan 5=sync 6=delay
(defstate param-mode 0)

(def page-size 16)

;; Step cursor helpers are used by the FX step buffer root, so define them
;; before loading render roots.
(def cursor-step 0)

(def set-cursor-step-value (step)
  (let ((parameter-step
          (if (> (or SEQ.fx-step-selection-count 0) 0)
            (or SEQ.fx-step-parameter-step step)
            step)))
    (do
      (set! cursor-step step)
      (reactive-set "SEQ" "fx-step-cursor-number" (+ step 1))
      (reactive-set "SEQ" "fx-step-parameter-step" parameter-step)
      (reactive-set "SEQ" "fx-step-value-transpose" (nth SEQ.transposes parameter-step))
      (reactive-set "SEQ" "fx-step-value-velocity" (nth SEQ.velocities parameter-step))
      (reactive-set "SEQ" "fx-step-value-duration" (nth SEQ.durations parameter-step)))))

(def cursor-num-steps ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-num-steps selected-bus)
    SEQ.tp-num-steps))

(def current-step ()
  (mod cursor-step (max 1 (cursor-num-steps))))

(def page-count ()
  (max 1 (floor (/ (+ SEQ.tp-num-steps (- page-size 1)) page-size))))

(def current-page ()
  (min (floor (/ (current-step) page-size)) (- (page-count) 1)))

(def visible-page ()
  (if (and SEQ.playing SEQ.auto-follow (not (seq-has-selection?)))
    (playhead-page)
    (current-page)))

(def playhead-page ()
  (min SEQ.playhead-page
    (- (page-count) 1)))

(def page-offset ()
  (* (visible-page) page-size))

(def cool-off-follow ()
  (seq-pause-auto-follow))
