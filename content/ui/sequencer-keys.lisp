;; ui/sequencer-keys.lisp — the shared sequencer keymap as a parent mode.
;;
;; Every sequencer-facing view (step grid, mixer, fx panel, arrangement,
;; piano roll, browser, transport, …) used to get its bare navigation keys
;; from a Rust table in src/ui/input.rs that ran before the editor keymap and
;; admitted any widget-only buffer. That table was invisible to Lisp: a
;; package could not see it, rebind it, or keep its own view out of it.
;;
;; It now lives here as an ordinary mode. Views that want the sequencer keys
;; inherit it — `(define-mode "my-mode" :inherit "eseq.sequencer-keys/sequencer-keys")`
;; or `(set-buffer-mode-for "*my-buffer*" "eseq.sequencer-keys/sequencer-keys")` —
;; and a view that declares its own mode without inheriting owns its keys.
;; Key precedence is the editor's: mode on-key, mode keymap (ancestors
;; included), global bind-key, chords, builtins. Rebind any of these with
;; mode-bind-key from user lisp; `M-x describe-key` shows the winner.
(module eseq.sequencer-keys)

(export mode-name
        cursor-left
        cursor-right
        cursor-select-left
        cursor-select-right
        cursor-toggle
        delete-selected-steps
        track-up
        track-down
        track-relative)

(def mode-name "eseq.sequencer-keys/sequencer-keys")

;; The step-grid verbs are late-bound qualified calls: eseq.step-grid-interactions
;; is a library module loaded by main.lisp, and dispatch happens at key time.
(def cursor-left () (do (eseq.step-grid-interactions/cursor-left) true))
(def cursor-right () (do (eseq.step-grid-interactions/cursor-right) true))
(def cursor-select-left () (do (eseq.step-grid-interactions/cursor-select-left) true))
(def cursor-select-right () (do (eseq.step-grid-interactions/cursor-select-right) true))
(def cursor-toggle () (do (eseq.step-grid-interactions/cursor-toggle) true))

;; BS / Delete: delete the selected steps when there are any and no p-lock
;; row owns the selection; otherwise leave the key to the next binding
;; (a child mode's own delete, then the global map).
(def delete-selected-steps ()
  (if (and (seq-has-selection?) (not (eseq.effects.track-panels/plock-row-selected?)))
    (do (eseq.step-grid-interactions/delete-selected-steps) true)
    false))

;; UP / DOWN: select the previous / next track in visual order. The drum
;; rack owns the visual order (group members are not contiguous); a nil or
;; out-of-range answer falls back to plain wrap-around.
(def track-relative (current delta count)
  (let ((next (eseq.drum-rack-v2/track-relative current delta)))
    (if (and (number? next) (>= next 0) (< next count))
      next
      (if (< delta 0)
        (if (= current 0) (- count 1) (- current 1))
        (mod (+ current delta) count)))))

(def select-track-delta (delta)
  (let ((count SEQ.num-tracks))
    (if (> count 0)
      (eseq.sequencer/select-track-for-edit
        (track-relative (min SEQ.current-track (- count 1)) delta count))
      nil)))

(def track-up () (do (select-track-delta -1) true))
(def track-down () (do (select-track-delta 1) true))

(define-mode "eseq.sequencer-keys/sequencer-keys" :read-only true :live-keys true)
(mode-bind-key mode-name "LEFT" "cursor-left")
(mode-bind-key mode-name "RIGHT" "cursor-right")
(mode-bind-key mode-name "S-LEFT" "cursor-select-left")
(mode-bind-key mode-name "S-RIGHT" "cursor-select-right")
(mode-bind-key mode-name "RET" "cursor-toggle")
(mode-bind-key mode-name "UP" "track-up")
(mode-bind-key mode-name "DOWN" "track-down")
(mode-bind-key mode-name "BS" "delete-selected-steps")
(mode-bind-key mode-name "Delete" "delete-selected-steps")
