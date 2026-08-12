;; Step-grid pointer/cursor/selection interactions: paging, drag gestures, drum-step gestures, step param helpers.
;; Extracted from ui/main.lisp (module-system spec slice S2). Headerless on
;; purpose: implicit eseq.vanilla until per-file (module …) headers land in S3.

(def page-button-width 2.8)

(def page-button-gap 0.4)

(def page-slot-width ()
  (+ page-button-width page-button-gap))

(def page-panel-width ()
  (+ 0.4 (* (page-count) (page-slot-width))))

(def step-index (i)
  (+ (page-offset) i))

(def step-visible? (i)
  (< (step-index i) SEQ.tp-num-steps))

(def cursor-left ()
  (if (seq-has-selection?)
    (do
      (cool-off-follow)
      (set! step-key-select-anchor nil)
      (if (seq-has-selected-bus?)
        (bus-shift-selected-steps -1)
        (seq-shift-selected-steps -1)))
    (do
      (cool-off-follow)
      (set! step-key-select-anchor nil)
      (let ((num-steps (max 1 (cursor-num-steps))))
        (set-track-cursor-step
          (if (= (current-step) 0)
            (- num-steps 1)
            (- (current-step) 1)))))))

(def cursor-right ()
  (if (seq-has-selection?)
    (do
      (cool-off-follow)
      (set! step-key-select-anchor nil)
      (if (seq-has-selected-bus?)
        (bus-shift-selected-steps 1)
        (seq-shift-selected-steps 1)))
    (do
      (cool-off-follow)
      (set! step-key-select-anchor nil)
      (let ((num-steps (max 1 (cursor-num-steps))))
        (set-track-cursor-step
          (if (>= (current-step) (- num-steps 1))
            0
            (+ (current-step) 1)))))))

(def step-key-select-anchor nil)

(def cursor-select-step-range (start end)
  (if (seq-has-selected-bus?)
    (bus-select-step-range start end)
    (seq-select-step-range start end)))

(def cursor-select-move (direction)
  (do
    (cool-off-follow)
    (let ((num-steps (max 1 (cursor-num-steps)))
          (start (current-step)))
      (let ((anchor (if (= step-key-select-anchor nil) start step-key-select-anchor))
            (next (if (< direction 0)
                    (if (= start 0) 0 (- start 1))
                    (if (>= start (- num-steps 1)) (- num-steps 1) (+ start 1)))))
        (do
          (set! step-key-select-anchor anchor)
          (set-track-cursor-step next)
          (cursor-select-step-range anchor next))))))

(def cursor-select-left ()
  (cursor-select-move -1))

(def cursor-select-right ()
  (cursor-select-move 1))

(def cursor-toggle ()
  (do
    (cool-off-follow)
    (set! step-key-select-anchor nil)
    (if (seq-has-selected-bus?)
      (bus-toggle-step (bus-current-step))
      (seq-toggle-step (current-step)))))

(def selection-click? (evt)
  (or (get evt :shift)
    (get evt :cmd)
    (get evt :super)
    (get evt :meta)
    (get evt :ctrl)))

(def cmd-click? (evt)
  (or (get evt :cmd)
    (get evt :super)
    (get evt :meta)
    (get evt :ctrl)))

(def sequencer-cursor-step-changed (track step)
  nil)

(def set-track-cursor-step (step)
  (do
    (set-cursor-step-value step)
    (sequencer-cursor-step-changed SEQ.current-track step)))

(def step-drag-anchor nil)
(def step-click-pending nil)
(def step-move-last nil)
(def step-toggle-drag-value nil)
(def step-click-was-active nil)
(def step-press-ms nil)
(def step-press-step nil)
(def step-drag-progressed nil)
(def step-hold-select nil)
(def step-cmd-drag-last nil)

; Finder-style shift extension: anchor at the prior keyboard anchor if any,
; else the cursor step when a selection exists, else the clicked step.
(def step-shift-anchor (step)
  (if (not (= step-key-select-anchor nil))
    step-key-select-anchor
    (if (seq-has-selection?) cursor-step step)))

(def step-hold-select-ms 300)

; Press-and-hold (~300ms) before dragging turns the drag into a selection
; sweep instead of a move/paint. Once a move or paint has already advanced
; past the pressed step, the hold can no longer engage.
(def step-hold-select-maybe-engage (step evt)
  (if (and (not step-hold-select)
        (not (selection-click? evt))
        (not step-drag-progressed)
        (not (= step-press-ms nil))
        (>= (- (now-ms) step-press-ms) step-hold-select-ms))
    (do
      (set! step-hold-select true)
      (set! step-click-pending nil)
      (set! step-move-last nil)
      (set! step-toggle-drag-value nil)
      (set! step-drag-anchor (if (= step-press-step nil) step step-press-step)))
    nil))

(def step-selected? (step)
  (seq-step-selected? step))

(def step-select-drag-start (step evt)
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
        (set-track-cursor-step step)
        (set! step-drag-anchor nil)
        (set! step-cmd-drag-last step)
        (seq-select-step step))
      (let ((anchor (step-shift-anchor step)))
        (do
          (set! step-key-select-anchor anchor)
          (set-track-cursor-step step)
          (set! step-drag-anchor anchor)
          (set! step-cmd-drag-last nil)
          (seq-select-step-range anchor step))))))

(def step-set-cursor-if (update-cursor step)
  (if update-cursor
    (set-track-cursor-step step)
    nil))

(def step-select-drag-over-for-track-with-cursor (track step evt update-cursor)
  (do
    (step-hold-select-maybe-engage step evt)
    (if (or (selection-click? evt) step-hold-select)
      (do
        (set! step-click-pending nil)
        (set! step-move-last nil)
        (set! step-toggle-drag-value nil)
        (cool-off-follow)
        (step-set-cursor-if update-cursor step)
        (if (and (cmd-click? evt) (not step-hold-select))
          (if (= step step-cmd-drag-last)
            nil
            (do
              (set! step-cmd-drag-last step)
              (if (step-selected? step) nil (seq-select-step step))))
          (do
            (if (= step-drag-anchor nil) (set! step-drag-anchor step) nil)
            (seq-select-step-range step-drag-anchor step))))
      (do
        (if (= step step-press-step) nil (set! step-drag-progressed true))
        (if (not (= step-toggle-drag-value nil))
          (do
            (set! step-click-pending nil)
            (cool-off-follow)
            (step-set-cursor-if update-cursor step)
            (if (= (seq-track-step-active? track step) step-toggle-drag-value)
              nil
              (seq-toggle-step step)))
          (if (= step-move-last nil)
            nil
            (if (= step step-move-last)
              nil
              (do
                (set! step-click-pending nil)
                (cool-off-follow)
                (seq-move-step-drag step-move-last step)
                (set! step-move-last step)
                (step-set-cursor-if update-cursor step)))))))))

(def step-select-drag-over-for-track (track step evt)
  (step-select-drag-over-for-track-with-cursor track step evt true))

(def step-select-drag-over-for-track-no-cursor (track step evt)
  (step-select-drag-over-for-track-with-cursor track step evt false))

(def step-select-drag-over (step evt)
  (step-select-drag-over-for-track SEQ.current-track step evt))

(def step-pointer-down-for-track (track step evt use-selection)
  (if (selection-click? evt)
    (step-select-drag-start step evt)
    (do
      (cool-off-follow)
      (set-track-cursor-step step)
      (set! step-drag-anchor nil)
      (set! step-press-ms (now-ms))
      (set! step-press-step step)
      (set! step-drag-progressed nil)
      (set! step-hold-select nil)
      (if (or (seq-track-step-active? track step) (and use-selection (step-selected? step)))
        (do
          (set! step-move-last step)
          (set! step-click-pending step)
          (set! step-click-was-active (seq-track-step-active? track step))
          (set! step-toggle-drag-value nil))
        (do
          (set! step-move-last nil)
          (set! step-click-pending nil)
          (set! step-toggle-drag-value true)
          (step-select-drag-over-for-track track step evt))))))

(def step-pointer-down (step evt)
  (step-pointer-down-for-track SEQ.current-track step evt true))

(def step-pointer-up (step evt)
  (do
    (if (and (= step-click-pending step) (not (selection-click? evt)))
      (if step-click-was-active
        (seq-select-step-range step step)
        (seq-toggle-step step))
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

(def step-double-click-for-track (track step evt)
  (if (and (not (selection-click? evt)) (seq-track-step-active? track step))
    (seq-toggle-step step)
    nil))

(def step-double-click (step evt)
  (step-double-click-for-track SEQ.current-track step evt))

(defstate drum-step-gesture-track nil)
(defstate drum-step-gesture-pad nil)
(defstate drum-step-cursor-track nil)
(defstate drum-step-cursor-pad nil)

(def drum-step-gesture-lane? (track pad-note)
  (and (= drum-step-gesture-track track)
    (= drum-step-gesture-pad pad-note)))

(def drum-step-set-cursor (track pad-note step)
  (do
    (seq-set-track track)
    (set! drum-step-cursor-track track)
    (set! drum-step-cursor-pad pad-note)
    (set-track-cursor-step step)))

(def drum-step-selected? (track pad-note step)
  (seq-drum-lane-step-selected? track pad-note step))

(def drum-step-shift-anchor (track pad-note step)
  (if (and (not (= step-key-select-anchor nil))
        (seq-drum-lane-has-selection? track pad-note))
    step-key-select-anchor
    (if (seq-drum-lane-has-selection? track pad-note) cursor-step step)))

(def drum-step-select-drag-start (track pad-note step evt)
  (do
    (cool-off-follow)
    (drum-step-set-cursor track pad-note step)
    (set! drum-step-gesture-track track)
    (set! drum-step-gesture-pad pad-note)
    (set! step-click-pending nil)
    (set! step-press-ms nil)
    (set! step-press-step nil)
    (set! step-drag-progressed nil)
    (set! step-hold-select nil)
    (if (cmd-click? evt)
      (do
        (set! step-key-select-anchor nil)
        (set! step-drag-anchor nil)
        (set! step-cmd-drag-last step)
        (seq-select-drum-lane-step track pad-note step))
      (let ((anchor (drum-step-shift-anchor track pad-note step)))
        (do
          (set! step-key-select-anchor anchor)
          (set! step-drag-anchor anchor)
          (set! step-cmd-drag-last nil)
          (seq-select-drum-lane-step-range track pad-note anchor step))))))

(def drum-step-select-drag-over (track pad-note step evt)
  (if (drum-step-gesture-lane? track pad-note)
    (do
      (step-hold-select-maybe-engage step evt)
      (if (or (selection-click? evt) step-hold-select)
        (do
          (set! step-click-pending nil)
          (set! step-move-last nil)
          (set! step-toggle-drag-value nil)
          (cool-off-follow)
          (drum-step-set-cursor track pad-note step)
          (if (and (cmd-click? evt) (not step-hold-select))
            (if (= step step-cmd-drag-last)
              nil
              (do
                (set! step-cmd-drag-last step)
                (if (drum-step-selected? track pad-note step)
                  nil
                  (seq-select-drum-lane-step track pad-note step))))
            (do
              (if (= step-drag-anchor nil) (set! step-drag-anchor step) nil)
              (seq-select-drum-lane-step-range
                track pad-note step-drag-anchor step))))
        (do
          (if (= step step-press-step) nil (set! step-drag-progressed true))
          (if (not (= step-toggle-drag-value nil))
            (do
              (set! step-click-pending nil)
              (cool-off-follow)
              (drum-step-set-cursor track pad-note step)
              (if (= (seq-drum-lane-step-active? track pad-note step)
                    step-toggle-drag-value)
                nil
                (seq-toggle-drum-lane-step track pad-note step)))
            (if (= step-move-last nil)
              nil
              (if (= step step-move-last)
                nil
                (do
                  (set! step-click-pending nil)
                  (cool-off-follow)
                  (seq-move-drum-lane-step-drag
                    track pad-note step-move-last step)
                  (set! step-move-last step)
                  (drum-step-set-cursor track pad-note step))))))))
    nil))

(def drum-step-pointer-down (track pad-note step evt)
  (do
    (set! drum-step-gesture-track track)
    (set! drum-step-gesture-pad pad-note)
    (if (selection-click? evt)
      (drum-step-select-drag-start track pad-note step evt)
      (do
        (cool-off-follow)
        (drum-step-set-cursor track pad-note step)
        (set! step-drag-anchor nil)
        (set! step-press-ms (now-ms))
        (set! step-press-step step)
        (set! step-drag-progressed nil)
        (set! step-hold-select nil)
        (if (or (seq-drum-lane-step-active? track pad-note step)
              (drum-step-selected? track pad-note step))
          (do
            (set! step-move-last step)
            (set! step-click-pending step)
            (set! step-click-was-active
              (seq-drum-lane-step-active? track pad-note step))
            (set! step-toggle-drag-value nil))
          (do
            (set! step-move-last nil)
            (set! step-click-pending nil)
            (set! step-toggle-drag-value true)
            (drum-step-select-drag-over track pad-note step evt)))))))

(def drum-step-pointer-up (track pad-note step evt)
  (do
    (if (and (drum-step-gesture-lane? track pad-note)
          (= step-click-pending step)
          (not (selection-click? evt)))
      (if step-click-was-active
        (seq-select-drum-lane-step-range track pad-note step step)
        (seq-toggle-drum-lane-step track pad-note step))
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
    (set! step-cmd-drag-last nil)
    (set! drum-step-gesture-track nil)
    (set! drum-step-gesture-pad nil)))

(def drum-step-double-click (track pad-note step evt)
  (if (and (not (selection-click? evt))
        (seq-drum-lane-step-active? track pad-note step))
    (seq-toggle-drum-lane-step track pad-note step)
    nil))

(def bus-step-double-click (step evt)
  (if (and (not (selection-click? evt)) (bus-step-active? step))
    (bus-toggle-step step)
    nil))

(def seq-set-step-param-from-step (step param value)
  (if (step-selected? step)
    (seq-set-step-param-plock param value)
    (do
      (if (seq-has-selection?) (seq-clear-selection) nil)
      (seq-set-step-param step param value))))

(def seq-selected-step-indexes ()
  (seq-selected-step-indexes-native))

(def seq-set-process-lane-step-value (track lane step value)
  (seq-set-process-lane-step
    track
    (get lane :instance-id)
    (get lane :inlet)
    step
    value))

(def seq-set-process-lane-from-step (track mode step value)
  (let ((lane (seqv-track-process-lane track mode)))
    (if (step-selected? step)
      (for-each
        (lambda (selected-step)
          (seq-set-process-lane-step-value track lane selected-step value))
        (seq-selected-step-indexes))
      (do
        (if (seq-has-selection?) (seq-clear-selection) nil)
        (seq-set-process-lane-step-value track lane step value)))))

(def select-all-steps ()
  (do
    (cool-off-follow)
    (if (seq-has-selected-bus?)
      (bus-select-all-steps)
      (seq-select-all-steps))))

(def seq-global-select-all-steps ()
  (if (and
        (or (buffer-read-only?) (= (current-buffer-name) "*transport*"))
        (not (= (current-buffer-name) "*piano-roll*")))
    (select-all-steps)
    false))

(bind-key "C-a" "seq-global-select-all-steps")

(def seq-global-toggle-record ()
  (if (or (buffer-read-only?) (= (view-mode) "ui"))
    (seq-toggle-record)
    false))

(bind-key "." "seq-global-toggle-record")

(def delete-selected-steps ()
  (do
    (cool-off-follow)
    (if (seq-has-selected-bus?)
      (bus-delete-selected-steps)
      (seq-delete-selected-steps))))

(def duration-slider-position (duration)
  (let ((d (max 0 (min duration 32))))
    (if (<= d 2)
      (/ d 4)
      (+ 0.5 (* 0.5 (pow (/ (- d 2) 30) 0.25))))))

(def duration-slider-value (position)
  (let ((p (max 0 (min position 1))))
    (if (<= p 0.5)
      (* p 4)
      (+ 2 (* 30 (pow (* 2 (- p 0.5)) 4))))))

(def step-param-value (v)
  (if (= param-mode 3)
    (round v)
    v))

(def step-slider-param-value (v)
  (if (= param-mode 1)
    (duration-slider-value v)
    (step-param-value v)))

(def param-decimals ()
  (seqv-param-decimals param-mode))
