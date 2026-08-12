;; Bus sequencer panel: bus-seq-* accessors and bus-step-* grid interactions.
;; Extracted from ui/main.lisp (module-system spec slice S2). Headerless on
;; purpose: implicit eseq.vanilla until per-file (module …) headers land in S3.

(def bus-seq-list (lists)
  (if (seq-has-selected-bus?)
    (nth lists selected-bus)
    '()))

(def bus-seq-playhead ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-playheads selected-bus)
    0))

(def bus-seq-num-steps ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-num-steps selected-bus)
    16))

(def bus-seq-timebase ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-timebases selected-bus)
    "16"))

(def bus-seq-swing ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-swings selected-bus)
    50))

(def bus-seq-swing-resolution ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-swing-resolutions selected-bus)
    "1/16"))

(def bus-seq-param-values ()
  (if (= param-mode 1) (bus-seq-list SEQ.bus-durations)
    (if (= param-mode 2) (bus-seq-list SEQ.bus-syncs)
      (bus-seq-list SEQ.bus-velocities))))

(def bus-seq-param-name ()
  (if (= param-mode 1) "Duration"
    (if (= param-mode 2) "Sync"
      "Gate Amount")))

(def bus-seq-param-key ()
  (if (= param-mode 1) "duration"
    (if (= param-mode 2) "sync"
      "velocity")))

(def bus-seq-param-min ()
  (if (= param-mode 1) 0.1 0))

(def bus-seq-param-max ()
  (if (= param-mode 1) 2
    (if (= param-mode 2) (- (len SEQ.sync-labels) 1)
      1)))

(def bus-page-count ()
  (max 1 (floor (/ (+ (bus-seq-num-steps) (- page-size 1)) page-size))))

(def bus-current-step ()
  (min cursor-step (- (max 1 (bus-seq-num-steps)) 1)))

(def bus-current-page ()
  (min (floor (/ (bus-current-step) page-size)) (- (bus-page-count) 1)))

(def bus-page-offset ()
  (* (bus-current-page) page-size))

(def bus-step-index (i)
  (+ (bus-page-offset) i))

(def bus-step-visible? (i)
  (< (bus-step-index i) (bus-seq-num-steps)))

(def bus-page-panel-width ()
  (+ 0.4 (* (bus-page-count) (page-slot-width))))

(def bus-goto-page (page)
  (do
    (cool-off-follow)
    (set-cursor-step-value (min (* page page-size) (- (max 1 (bus-seq-num-steps)) 1)))))

(def bus-set-step-param (step value)
  (host-command "set-bus-step-param"
    (dict :bus selected-bus :step step :param (bus-seq-param-key) :value value)))

(def bus-set-selected-step-param (value)
  (host-command "set-selected-bus-step-param"
    (dict :bus selected-bus :param (bus-seq-param-key) :value value)))

(def bus-toggle-step (step)
  (do
    (cool-off-follow)
    (set-cursor-step-value step)
    (host-command "toggle-bus-step" (dict :bus selected-bus :step step))))

(def bus-set-step-active (step active)
  (do
    (cool-off-follow)
    (set-cursor-step-value step)
    (host-command "set-bus-step-active"
      (dict :bus selected-bus :step step :active active))))

(def bus-step-active? (step)
  (nth (bus-seq-list SEQ.bus-steps) step))

(def bus-select-step-range (start end)
  (host-command "select-bus-step-range"
    (dict :bus selected-bus :start start :end end)))

(def bus-select-step (step)
  (host-command "select-bus-step"
    (dict :bus selected-bus :step step)))

(def bus-select-all-steps ()
  (host-command "select-all-bus-steps" (dict :bus selected-bus)))

(def bus-delete-selected-steps ()
  (host-command "delete-selected-bus-steps" (dict :bus selected-bus)))

(def bus-move-step-drag (start target)
  (host-command "move-bus-step-drag"
    (dict :bus selected-bus :start start :target target)))

(def bus-shift-selected-steps (direction)
  (host-command "shift-selected-bus-steps"
    (dict :bus selected-bus :direction direction)))

(def bus-step-select-drag-start (step evt)
  (do
    (cool-off-follow)
    (set! step-click-pending nil)
    (set! step-press-ms nil)
    (set! step-press-step nil)
    (set! step-drag-progressed nil)
    (set! step-hold-select nil)
    (if (cmd-click? evt)
      (do
        (set! step-key-select-anchor nil)
        (set-cursor-step-value step)
        (set! step-drag-anchor nil)
        (set! step-cmd-drag-last step)
        (bus-select-step step))
      (let ((anchor (step-shift-anchor step)))
        (do
          (set! step-key-select-anchor anchor)
          (set-cursor-step-value step)
          (set! step-drag-anchor anchor)
          (set! step-cmd-drag-last nil)
          (bus-select-step-range anchor step))))))

(def bus-step-select-drag-over (step evt)
  (do
    (step-hold-select-maybe-engage step evt)
    (if (or (selection-click? evt) step-hold-select)
      (do
        (set! step-click-pending nil)
        (set! step-move-last nil)
        (set! step-toggle-drag-value nil)
        (cool-off-follow)
        (set-cursor-step-value step)
        (if (and (cmd-click? evt) (not step-hold-select))
          (if (= step step-cmd-drag-last)
            nil
            (do
              (set! step-cmd-drag-last step)
              (if (step-selected? step) nil (bus-select-step step))))
          (do
            (if (= step-drag-anchor nil) (set! step-drag-anchor step) nil)
            (bus-select-step-range step-drag-anchor step))))
      (do
        (if (= step step-press-step) nil (set! step-drag-progressed true))
        (if (not (= step-toggle-drag-value nil))
          (do
            (set! step-click-pending nil)
            (cool-off-follow)
            (set-cursor-step-value step)
            (if (= (bus-step-active? step) step-toggle-drag-value)
              nil
              (bus-set-step-active step step-toggle-drag-value)))
          (if (= step-move-last nil)
            nil
            (if (= step step-move-last)
              nil
              (do
                (set! step-click-pending nil)
                (cool-off-follow)
                (bus-move-step-drag step-move-last step)
                (set! step-move-last step)
                (set-cursor-step-value step)))))))))

(def bus-step-pointer-down (step evt)
  (if (selection-click? evt)
    (bus-step-select-drag-start step evt)
    (do
      (cool-off-follow)
      (set-cursor-step-value step)
      (set! step-drag-anchor nil)
      (set! step-press-ms (now-ms))
      (set! step-press-step step)
      (set! step-drag-progressed nil)
      (set! step-hold-select nil)
      (if (or (bus-step-active? step) (step-selected? step))
        (do
          (set! step-move-last step)
          (set! step-click-pending step)
          (set! step-click-was-active (bus-step-active? step))
          (set! step-toggle-drag-value nil))
        (do
          (set! step-move-last nil)
          (set! step-click-pending nil)
          (set! step-toggle-drag-value true)
          (bus-step-select-drag-over step evt))))))

(def bus-step-pointer-up (step evt)
  (do
    (if (and (= step-click-pending step) (not (selection-click? evt)))
      (if step-click-was-active
        (bus-select-step-range step step)
        (bus-toggle-step step))
      nil)
    (set! step-click-was-active nil)
    (set! step-click-pending nil)
    (set! step-drag-anchor nil)
    (set! step-move-last nil)
    (set! step-toggle-drag-value nil)
    (set! step-press-ms nil)
    (set! step-press-step nil)
    (set! step-drag-progressed nil)
    (set! step-hold-select nil)
    (set! step-cmd-drag-last nil)))

(def bus-set-sequencer-param (param value)
  (host-command "set-bus-sequencer-param"
    (dict :bus selected-bus :param param :value value)))

(def bus-set-sequencer-label (param label)
  (host-command "set-bus-sequencer-param"
    (dict :bus selected-bus :param param :label label)))
