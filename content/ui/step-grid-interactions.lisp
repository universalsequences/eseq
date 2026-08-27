;; Step-grid pointer/cursor/selection interactions: paging, drag gestures, step param helpers.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; This is the step-gesture hub: ui/step-grid.lisp,
;; ui/sequencer.lisp, ui/seq-grid-mode.lisp, ui/seqv-track-params.lisp and several
;; Rust call sites reach its names by their flat spellings, so it converts with NO
;; renames and a full set of *identity* compat aliases (the seq-core-state /
;; custom-ui-lego precedent, spec §10 wave-7 addendum): an unconverted vanilla
;; caller matches the alias key flat, and a converted module's bare reference
;; qualifies against itself, misses, and lands on the same alias by base name.
;; Every aliased name here is a function or a `defstate`, both immune to hazard (m).
;;
;; TWO exceptions, both hazard (m)/(i):
;;
;;   1. The eleven drag-state globals below are *mutable plain defs* shared with
;;      vanilla callers that read AND `set!` them by flat spelling. A compat alias
;;      cannot rescue that: the late-binding heal repairs an *empty* slot and the
;;      next `set!` unlinks it. They are pinned into `eseq.vanilla` with the §3
;;      escape hatch, get no alias, and EVERY in-file reference must use the
;;      `eseq.vanilla/` spelling — a bare one here would intern this module's own
;;      slot, a different cell, and the divergence is silent.
;;   2. `sequencer-cursor-step-changed` is a stub-then-override protocol:
;;      ui/sequencer.lisp pins its own `(def eseq.vanilla/sequencer-cursor-step-changed …)`
;;      override (see its comment at :304) which must land on the same slot the
;;      stub and the call site below use.
;;
;; Reads of ui/seq-core-state.lisp (a converted module) are written with its FULL
;; dotted module spelling rather than the `core/` import alias, deliberately:
;; `crates/sequencer/src/ui/state_values/tests.rs` evals two *slices* of this file
;; (`load_step_gesture_source`, `load_keyboard_step_selection_source`) with no
;; import in scope, and the compiler treats an undotted unknown alias as a hard
;; error while a dotted module namespace only warns — the qualified slot then
;; heals onto those harnesses' flat natives (function slots, so heal-safe).
;; `param-mode` stays bare: it is a `defstate`, which resolves through
;; `state_bindings` and never touches the global ladder.
(module eseq.step-grid-interactions)

(import eseq.seq-core-state :as core)

(export page-button-width
        page-slot-width
        step-index
        step-visible?
        cursor-left
        cursor-right
        cursor-select-left
        cursor-select-right
        cursor-toggle
        selection-click?
        cmd-click?
        set-track-cursor-step
        step-clear-drag-state
        step-shift-anchor
        step-hold-select-maybe-engage
        step-selected?
        step-select-drag-start
        step-select-drag-over-for-track
        step-select-drag-over-for-track-no-cursor
        step-select-drag-over
        step-pointer-down-for-track
        step-pointer-down
        step-pointer-up
        step-double-click-for-track
        step-double-click
        seq-set-step-param-from-step
        seq-set-process-lane-from-step
        select-all-steps
        seq-global-select-all-steps
        seq-global-toggle-record
        delete-selected-steps
        duration-slider-position
        duration-slider-value
        step-param-value
        step-slider-param-value
        param-decimals)


(def page-button-width 2.8)

(def page-button-gap 0.4)

(def page-slot-width ()
  (+ page-button-width page-button-gap))

;; Private: no caller anywhere in the app; kept as the track-grid page-width
;; helper.
(def page-panel-width ()
  (+ 0.4 (* (eseq.seq-core-state/page-count) (page-slot-width))))

(def step-index (i)
  (+ (eseq.seq-core-state/page-offset) i))

(def step-visible? (i)
  (< (step-index i) SEQ.tp-num-steps))

(def cursor-left ()
  (if (seq-has-selection?)
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set! eseq.vanilla/step-key-select-anchor nil)
      (seq-shift-selected-steps -1))
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set! eseq.vanilla/step-key-select-anchor nil)
      (let ((num-steps (max 1 (eseq.seq-core-state/cursor-num-steps))))
        (set-track-cursor-step
          (if (= (eseq.seq-core-state/current-step) 0)
            (- num-steps 1)
            (- (eseq.seq-core-state/current-step) 1)))))))

(def cursor-right ()
  (if (seq-has-selection?)
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set! eseq.vanilla/step-key-select-anchor nil)
      (seq-shift-selected-steps 1))
    (do
      (eseq.seq-core-state/cool-off-follow)
      (set! eseq.vanilla/step-key-select-anchor nil)
      (let ((num-steps (max 1 (eseq.seq-core-state/cursor-num-steps))))
        (set-track-cursor-step
          (if (>= (eseq.seq-core-state/current-step) (- num-steps 1))
            0
            (+ (eseq.seq-core-state/current-step) 1)))))))

;; PINNED (hazard m): vanilla callers `set!` this flat. See the file header.
(def eseq.vanilla/step-key-select-anchor nil)

(def cursor-select-step-range (start end)
  (seq-select-step-range start end))

(def cursor-select-move (direction)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (let ((num-steps (max 1 (eseq.seq-core-state/cursor-num-steps)))
          (start (eseq.seq-core-state/current-step)))
      (let ((anchor (if (= eseq.vanilla/step-key-select-anchor nil) start eseq.vanilla/step-key-select-anchor))
            (next (if (< direction 0)
                    (if (= start 0) 0 (- start 1))
                    (if (>= start (- num-steps 1)) (- num-steps 1) (+ start 1)))))
        (do
          (set! eseq.vanilla/step-key-select-anchor anchor)
          (set-track-cursor-step next)
          (cursor-select-step-range anchor next))))))

(def cursor-select-left ()
  (cursor-select-move -1))

(def cursor-select-right ()
  (cursor-select-move 1))

(def cursor-toggle ()
  (do
    (eseq.seq-core-state/cool-off-follow)
    (set! eseq.vanilla/step-key-select-anchor nil)
    (seq-toggle-step (eseq.seq-core-state/current-step))))

(def selection-click? (evt)
  (or (get evt :shift)
    (get evt :additive-selection)
    (get evt :ctrl)))

;; Historical name retained because this function is part of the module's
;; command-facing contract. The event field is semantic: Command on macOS and
;; Alt on Linux, so the window manager never has to relinquish Super there.
(def cmd-click? (evt)
  (or (get evt :additive-selection)
    (get evt :ctrl)))

;; PINNED (hazard i, lisp→lisp stub-then-override): ui/sequencer.lisp installs
;; the real implementation over this slot with its own
;; `(def eseq.vanilla/sequencer-cursor-step-changed …)` at :313, and the call
;; below must reach that override — a module-local slot never would.
(def eseq.vanilla/sequencer-cursor-step-changed (track step)
  nil)

(def set-track-cursor-step (step)
  (do
    (eseq.seq-core-state/set-cursor-step-value step)
    (eseq.vanilla/sequencer-cursor-step-changed SEQ.current-track step)))

;; PINNED (hazard m): the shared drag-gesture state, read and `set!` flat by
;; vanilla callers. See the file header.
(def eseq.vanilla/step-drag-anchor nil)
(def eseq.vanilla/step-click-pending nil)
(def eseq.vanilla/step-move-last nil)

;; Owner-side setter for the three drag-state globals above, so a *module* can
;; reset them (module spec §10 hazard m).  A converted module's bare
;; `(set! step-click-pending nil)` interns its own `<module>/step-click-pending`
;; slot and never reaches these, silently leaving a stale click pending; routing
;; the write through this function keeps it on the pinned vanilla slot.
;; ui/sequencer.lisp's grid pointer-down handlers are the callers.
(def step-clear-drag-state ()
  (do
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-drag-anchor nil)
    (set! eseq.vanilla/step-move-last nil)))
(def eseq.vanilla/step-toggle-drag-value nil)
(def eseq.vanilla/step-click-was-active nil)
(def eseq.vanilla/step-press-ms nil)
(def eseq.vanilla/step-press-step nil)
(def eseq.vanilla/step-drag-progressed nil)
(def eseq.vanilla/step-hold-select nil)
(def eseq.vanilla/step-cmd-drag-last nil)

; Finder-style shift extension: anchor at the prior keyboard anchor if any,
; else the cursor step when a selection exists, else the clicked step.
; `cursor-step` is pinned to eseq.vanilla by ui/seq-core-state.lisp.
(def step-shift-anchor (step)
  (if (not (= eseq.vanilla/step-key-select-anchor nil))
    eseq.vanilla/step-key-select-anchor
    (if (seq-has-selection?) eseq.vanilla/cursor-step step)))

(def step-hold-select-ms 300)

; Press-and-hold (~300ms) before dragging turns the drag into a selection
; sweep instead of a move/paint. Once a move or paint has already advanced
; past the pressed step, the hold can no longer engage.
(def step-hold-select-maybe-engage (step evt)
  (if (and (not eseq.vanilla/step-hold-select)
        (not (selection-click? evt))
        (not eseq.vanilla/step-drag-progressed)
        (not (= eseq.vanilla/step-press-ms nil))
        (>= (- (now-ms) eseq.vanilla/step-press-ms) step-hold-select-ms))
    (do
      (set! eseq.vanilla/step-hold-select true)
      (set! eseq.vanilla/step-click-pending nil)
      (set! eseq.vanilla/step-move-last nil)
      (set! eseq.vanilla/step-toggle-drag-value nil)
      (set! eseq.vanilla/step-drag-anchor (if (= eseq.vanilla/step-press-step nil) step eseq.vanilla/step-press-step)))
    nil))

(def step-selected? (step)
  (seq-step-selected? step))

(def step-select-drag-start (step evt)
  (do
    (eseq.seq-core-state/cool-off-follow)
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-press-ms nil)
    (set! eseq.vanilla/step-press-step nil)
    (set! eseq.vanilla/step-drag-progressed nil)
    (set! eseq.vanilla/step-hold-select nil)
    (if (cmd-click? evt)
      (do
        (set! eseq.vanilla/step-key-select-anchor nil)
        (set-track-cursor-step step)
        (set! eseq.vanilla/step-drag-anchor nil)
        (set! eseq.vanilla/step-cmd-drag-last step)
        (seq-select-step step))
      (let ((anchor (step-shift-anchor step)))
        (do
          (set! eseq.vanilla/step-key-select-anchor anchor)
          (set-track-cursor-step step)
          (set! eseq.vanilla/step-drag-anchor anchor)
          (set! eseq.vanilla/step-cmd-drag-last nil)
          (seq-select-step-range anchor step))))))

(def step-set-cursor-if (update-cursor step)
  (if update-cursor
    (set-track-cursor-step step)
    nil))

(def step-select-drag-over-for-track-with-cursor (track step evt update-cursor)
  (do
    (step-hold-select-maybe-engage step evt)
    (if (or (selection-click? evt) eseq.vanilla/step-hold-select)
      (do
        (set! eseq.vanilla/step-click-pending nil)
        (set! eseq.vanilla/step-move-last nil)
        (set! eseq.vanilla/step-toggle-drag-value nil)
        (eseq.seq-core-state/cool-off-follow)
        (step-set-cursor-if update-cursor step)
        (if (and (cmd-click? evt) (not eseq.vanilla/step-hold-select))
          (if (= step eseq.vanilla/step-cmd-drag-last)
            nil
            (do
              (set! eseq.vanilla/step-cmd-drag-last step)
              (if (step-selected? step) nil (seq-select-step step))))
          (do
            (if (= eseq.vanilla/step-drag-anchor nil) (set! eseq.vanilla/step-drag-anchor step) nil)
            (seq-select-step-range eseq.vanilla/step-drag-anchor step))))
      (do
        (if (= step eseq.vanilla/step-press-step) nil (set! eseq.vanilla/step-drag-progressed true))
        (if (not (= eseq.vanilla/step-toggle-drag-value nil))
          (do
            (set! eseq.vanilla/step-click-pending nil)
            (eseq.seq-core-state/cool-off-follow)
            (step-set-cursor-if update-cursor step)
            (if (= (seq-track-step-active? track step) eseq.vanilla/step-toggle-drag-value)
              nil
              (seq-toggle-step step)))
          (if (= eseq.vanilla/step-move-last nil)
            nil
            (if (= step eseq.vanilla/step-move-last)
              nil
              (do
                (set! eseq.vanilla/step-click-pending nil)
                (eseq.seq-core-state/cool-off-follow)
                (seq-move-step-drag eseq.vanilla/step-move-last step)
                (set! eseq.vanilla/step-move-last step)
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
      (eseq.seq-core-state/cool-off-follow)
      (set-track-cursor-step step)
      (set! eseq.vanilla/step-drag-anchor nil)
      (set! eseq.vanilla/step-press-ms (now-ms))
      (set! eseq.vanilla/step-press-step step)
      (set! eseq.vanilla/step-drag-progressed nil)
      (set! eseq.vanilla/step-hold-select nil)
      (if (or (seq-track-step-active? track step) (and use-selection (step-selected? step)))
        (do
          (set! eseq.vanilla/step-move-last step)
          (set! eseq.vanilla/step-click-pending step)
          (set! eseq.vanilla/step-click-was-active (seq-track-step-active? track step))
          (set! eseq.vanilla/step-toggle-drag-value nil))
        (do
          (set! eseq.vanilla/step-move-last nil)
          (set! eseq.vanilla/step-click-pending nil)
          (set! eseq.vanilla/step-toggle-drag-value true)
          (step-select-drag-over-for-track track step evt))))))

(def step-pointer-down (step evt)
  (step-pointer-down-for-track SEQ.current-track step evt true))

(def step-pointer-up (step evt)
  (do
    (if (and (= eseq.vanilla/step-click-pending step) (not (selection-click? evt)))
      (if eseq.vanilla/step-click-was-active
        (seq-select-step-range step step)
        (seq-toggle-step step))
      nil)
    (set! eseq.vanilla/step-click-was-active nil)
    (set! eseq.vanilla/step-click-pending nil)
    (set! eseq.vanilla/step-drag-anchor nil)
    (set! eseq.vanilla/step-move-last nil)
    (set! eseq.vanilla/step-toggle-drag-value nil)
    (set! eseq.vanilla/step-press-ms nil)
    (set! eseq.vanilla/step-press-step nil)
    (set! eseq.vanilla/step-drag-progressed nil)
    (set! eseq.vanilla/step-hold-select nil)
    (set! eseq.vanilla/step-cmd-drag-last nil)))

(def step-double-click-for-track (track step evt)
  (if (and (not (selection-click? evt)) (seq-track-step-active? track step))
    (seq-toggle-step step)
    nil))

(def step-double-click (step evt)
  (step-double-click-for-track SEQ.current-track step evt))

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
  (let ((lane (eseq.seqv-track-params/seqv-track-process-lane track mode)))
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
    (eseq.seq-core-state/cool-off-follow)
    (seq-select-all-steps)))

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
    (eseq.seq-core-state/cool-off-follow)
    (seq-delete-selected-steps)))

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

;; `param-mode` is a `defstate` owned by eseq.seq-core-state: bare is the
;; documented path (state_bindings, not the global ladder).
(def step-param-value (v)
  (if (= eseq.seq-core-state/param-mode 3)
    (round v)
    v))

(def step-slider-param-value (v)
  (if (= eseq.seq-core-state/param-mode 1)
    (duration-slider-value v)
    (step-param-value v)))

(def param-decimals ()
  (eseq.seqv-track-params/seqv-param-decimals eseq.seq-core-state/param-mode))
