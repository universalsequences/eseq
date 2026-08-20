;; seq-grid-mode key handling, page/pattern commands, param-mode setters and current-param accessors.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; This file is the mode keyspace's end-to-end exerciser (spec §10 hazard d):
;; it is the only *defining* file for a mode whose keymap binds handlers it
;; does not own. Three distinct rungs fire here, all pre-built infra:
;;
;;   1. `define-mode` qualifies the registry key to
;;      `eseq.seq-grid-mode/seq-grid-mode`, so the two flat
;;      `(set-buffer-mode-for … "eseq.seq-grid-mode/seq-grid-mode")` callers — ui/step-grid.lisp
;;      and ui/sequencer.lisp (both converted modules as of S3b wave 8, whose
;;      bare references qualify against *themselves*, miss, and fall to the
;;      same base-name rung) — both reach it through the identity alias below.
;;   2. `mode-bind-key` qualifies its *handler* string against this module
;;      unconditionally, so the seven handlers bound below that are defined
;;      OUTSIDE this file (cursor-left/-right, select-all-steps,
;;      delete-selected-steps, cursor-toggle, seqv-collapse-all-tracks) become
;;      `eseq.seq-grid-mode/<name>` and land on `resolve_handler_name`'s
;;      qualified→flat fallback. Pinned by
;;      `module_mode_binding_dispatches_a_vanilla_handler` (eseqlisp editor
;;      tests), which was written for exactly this file.
;;   3. The eight `set-*-mode` handlers ARE defined here, so their bound
;;      strings qualify to a slot this module owns — an exact hit.
;;
;; `seqv-*` names belong to eseq.sequencer, a UI-root module: referenced bare
;; through its compat aliases, never imported (importing a UI root drags a
;; whole render tree into every test VM — the wave-2 lesson).
(module eseq.seq-grid-mode)

(import eseq.seq-core-state :as core)

(export seq-grid-handle-key
        goto-page
        double-track-pattern
        halve-track-pattern
        sequence-roll-hold
        roll-mode-toggle
        set-vel-mode
        set-dur-mode
        set-aux-mode
        set-transpose-mode
        set-pan-mode
        set-sync-mode
        set-delay-mode
        set-process-lane-mode
        param-values
        param-min
        param-max
        param-slider-min
        param-slider-max
        param-slider-value
        param-haptic-pivot-position
        param-haptic-pivot-value
        param-haptic-exponent
        param-keyword
        param-color
        param-name
        param-origin
        sync-current-label)

;; The mode name itself. Both callers are listed in rung 1 above.

;; src/ui/input.rs:1232 evals "(double-track-pattern)" / "(halve-track-pattern)"
;; by their flat names for the pattern-length keyboard shortcut, so these two
;; aliases are load-bearing production surface and must stay.
;;
;; The 14 `param-*` / `sync-current-label` aliases and `goto-page` that this
;; block carried in wave 7 were minted for exactly one caller, the then-
;; headerless ui/step-grid.lisp. That file became `eseq.step-grid` in wave 8
;; and now reaches these names through `(import eseq.seq-grid-mode :as gm)`,
;; so the aliases were retired — a whole-repo bounded grep confirms no other
;; lisp or Rust caller spells any of them flat. (`param-color` had no caller
;; at all; the only `param-name` hit is the `"param-name"` *map field* string
;; in src/ui/natives.rs:272, not a global reference.)

(def seq-grid-handle-key (key text)
  (if (= (current-buffer-name) "*sequencer*")
    (eseq.sequencer/handle-key key text)
    (if (= key "LEFT")
      (do (eseq.step-grid-interactions/cursor-left) true)
      (if (= key "RIGHT")
        (do (eseq.step-grid-interactions/cursor-right) true)
        (if (= key "C-a")
          (do (eseq.step-grid-interactions/select-all-steps) true)
          (if (or (= key "BS") (= key "Delete"))
            (do (eseq.step-grid-interactions/delete-selected-steps) true)
            (if (= key "RET")
              (do (eseq.step-grid-interactions/cursor-toggle) true)
              false)))))))

(def goto-page (page)
  (do
    (core/cool-off-follow)
    (eseq.step-grid-interactions/set-track-cursor-step (min (* page core/page-size) (- (max 1 SEQ.tp-num-steps) 1)))))

(def double-track-pattern ()
  (do
    (core/cool-off-follow)
    (seq-double-track-pattern)
    (eseq.step-grid-interactions/set-track-cursor-step (min (core/current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

(def halve-track-pattern ()
  (do
    (core/cool-off-follow)
    (seq-halve-track-pattern)
    (eseq.step-grid-interactions/set-track-cursor-step (min (core/current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

;; Cursor keys scoped to *metal* buffer via mode
(define-mode "eseq.seq-grid-mode/seq-grid-mode" :read-only true :live-keys true :on-key "seq-grid-handle-key")
;; Named hold command: Rust recognizes this semantic binding on both key-down
;; and key-up, while the ordinary mode keymap remains the customization seam.
;; Rebind this command (and replace the old binding) in user lisp to move the
;; sequence-roll gesture without changing host code.
(def sequence-roll-hold () true)
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "`" "sequence-roll-hold")
(def roll-mode-toggle () (host-command "toggle-roll-mode"))
;; Global performance shortcut: editor widget focus runs before direct Lisp
;; bindings, so active text/value input still takes precedence.
(bind-key ";" "roll-mode-toggle")
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "LEFT" "cursor-left")
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "RIGHT" "cursor-right")
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "C-a" "select-all-steps")
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "BS" "delete-selected-steps")
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "Delete" "delete-selected-steps")
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "RET" "cursor-toggle")
;; Qualified since S4: `seqv-collapse-all-tracks` was a compat alias, not the
;; def's base name, so the qualified→flat dispatch fallback cannot reach it.
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "C-h" "eseq.sequencer/collapse-all-tracks")

;; `param-mode` stays BARE, deliberately, even though eseq.seq-core-state now
;; owns it and is imported above: it is a `defstate`, which resolves through
;; `state_bindings` on the flat key at compile time and never touches the
;; global ladder (hazard m's immunity clause). Its owner's identity alias
;; covers that keyspace too. A qualified write to another module's `defstate`
;; is not a path any test pins, so the documented one is the one used.
(def set-vel-mode () (set! eseq.seq-core-state/param-mode 0))
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "v" "set-vel-mode")
(def set-dur-mode () (set! eseq.seq-core-state/param-mode 1))
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "d" "set-dur-mode")
(def set-aux-mode () (set! eseq.seq-core-state/param-mode 2))
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "a" "set-aux-mode")
(def set-transpose-mode () (set! eseq.seq-core-state/param-mode 3))
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "t" "set-transpose-mode")
(def set-pan-mode () (set! eseq.seq-core-state/param-mode 4))
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "p" "set-pan-mode")
(def set-sync-mode () (set! eseq.seq-core-state/param-mode 5))
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "s" "set-sync-mode")
(def set-delay-mode () (set! eseq.seq-core-state/param-mode 6))
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "l" "set-delay-mode")
(def set-process-lane-mode ()
  (if (> (len SEQ.process-lanes) 0)
    (set! eseq.seq-core-state/param-mode eseq.seqv-track-params/seqv-process-lane-mode-offset)
    nil))
(mode-bind-key "eseq.seq-grid-mode/seq-grid-mode" "x" "set-process-lane-mode")


(def param-values ()
  (eseq.seqv-track-params/seqv-current-param-values eseq.seq-core-state/param-mode))

(def param-min ()
  (eseq.seqv-track-params/seqv-param-min eseq.seq-core-state/param-mode))

(def param-max ()
  (eseq.seqv-track-params/seqv-param-max eseq.seq-core-state/param-mode))

(def param-slider-min ()
  (if (= eseq.seq-core-state/param-mode 1) 0 (param-min)))

(def param-slider-max ()
  (if (= eseq.seq-core-state/param-mode 1) 1 (param-max)))

(def param-slider-value (step)
  (if (= eseq.seq-core-state/param-mode 1)
    (eseq.step-grid-interactions/duration-slider-position (nth (param-values) step))
    (nth (param-values) step)))

(def param-haptic-pivot-position ()
  (if (= eseq.seq-core-state/param-mode 1) 0.5 1))

(def param-haptic-pivot-value ()
  (if (= eseq.seq-core-state/param-mode 1) 2 (param-max)))

(def param-haptic-exponent ()
  (if (= eseq.seq-core-state/param-mode 1) 4 1))

(def param-keyword ()
  (eseq.seqv-track-params/seqv-param-keyword eseq.seq-core-state/param-mode))

(def param-color ()
  (eseq.seqv-track-params/seqv-param-color eseq.seq-core-state/param-mode))

(def param-name ()
  (eseq.seqv-track-params/seqv-param-name eseq.seq-core-state/param-mode))

(def param-origin ()
  (eseq.seqv-track-params/seqv-param-origin eseq.seq-core-state/param-mode))

(def sync-current-label ()
  (nth SEQ.sync-labels (floor (+ 0.5 (nth SEQ.syncs (core/current-step))))))
