;; seq-grid-mode key handling, page/pattern commands, param-mode setters and current-param accessors.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; This file is the mode keyspace's end-to-end exerciser (spec §10 hazard d):
;; it is the only *defining* file for a mode whose keymap binds handlers it
;; does not own. Three distinct rungs fire here, all pre-built infra:
;;
;;   1. `define-mode` qualifies the registry key to
;;      `eseq.seq-grid-mode/seq-grid-mode`, so the two flat
;;      `(set-buffer-mode-for … "seq-grid-mode")` callers — ui/step-grid.lisp
;;      (headerless) and ui/sequencer.lisp (a converted module, whose bare
;;      reference qualifies against *itself*, misses, and falls to the same
;;      base-name rung) — both reach it through the identity alias below.
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

;; The mode name itself. Both callers are listed in rung 1 above.
(module-compat-alias seq-grid-mode seq-grid-mode)

;; ui/step-grid.lisp (headerless, and the only consumer of the param-*
;; family) plus src/ui/input.rs, which invokes the two pattern commands by
;; their flat names.
(module-compat-alias goto-page goto-page)
(module-compat-alias double-track-pattern double-track-pattern)
(module-compat-alias halve-track-pattern halve-track-pattern)
(module-compat-alias param-values param-values)
(module-compat-alias param-min param-min)
(module-compat-alias param-max param-max)
(module-compat-alias param-slider-min param-slider-min)
(module-compat-alias param-slider-max param-slider-max)
(module-compat-alias param-slider-value param-slider-value)
(module-compat-alias param-haptic-pivot-position param-haptic-pivot-position)
(module-compat-alias param-haptic-pivot-value param-haptic-pivot-value)
(module-compat-alias param-haptic-exponent param-haptic-exponent)
(module-compat-alias param-keyword param-keyword)
(module-compat-alias param-color param-color)
(module-compat-alias param-name param-name)
(module-compat-alias param-origin param-origin)
(module-compat-alias sync-current-label sync-current-label)

(def seq-grid-handle-key (key text)
  (if (= (current-buffer-name) "*sequencer*")
    (seqv-handle-key key text)
    (if (= key "LEFT")
      (do (cursor-left) true)
      (if (= key "RIGHT")
        (do (cursor-right) true)
        (if (= key "C-a")
          (do (select-all-steps) true)
          (if (or (= key "BS") (= key "Delete"))
            (do (delete-selected-steps) true)
            (if (= key "RET")
              (do (cursor-toggle) true)
              false)))))))

(def goto-page (page)
  (do
    (core/cool-off-follow)
    (set-track-cursor-step (min (* page core/page-size) (- (max 1 SEQ.tp-num-steps) 1)))))

(def double-track-pattern ()
  (do
    (core/cool-off-follow)
    (seq-double-track-pattern)
    (set-track-cursor-step (min (core/current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

(def halve-track-pattern ()
  (do
    (core/cool-off-follow)
    (seq-halve-track-pattern)
    (set-track-cursor-step (min (core/current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

;; Cursor keys scoped to *metal* buffer via mode
(define-mode "seq-grid-mode" :read-only true :on-key "seq-grid-handle-key")
(mode-bind-key "seq-grid-mode" "LEFT" "cursor-left")
(mode-bind-key "seq-grid-mode" "RIGHT" "cursor-right")
(mode-bind-key "seq-grid-mode" "C-a" "select-all-steps")
(mode-bind-key "seq-grid-mode" "BS" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "Delete" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "RET" "cursor-toggle")
(mode-bind-key "seq-grid-mode" "C-h" "seqv-collapse-all-tracks")

;; `param-mode` stays BARE, deliberately, even though eseq.seq-core-state now
;; owns it and is imported above: it is a `defstate`, which resolves through
;; `state_bindings` on the flat key at compile time and never touches the
;; global ladder (hazard m's immunity clause). Its owner's identity alias
;; covers that keyspace too. A qualified write to another module's `defstate`
;; is not a path any test pins, so the documented one is the one used.
(def set-vel-mode () (set! param-mode 0))
(mode-bind-key "seq-grid-mode" "v" "set-vel-mode")
(def set-dur-mode () (set! param-mode 1))
(mode-bind-key "seq-grid-mode" "d" "set-dur-mode")
(def set-aux-mode () (set! param-mode 2))
(mode-bind-key "seq-grid-mode" "a" "set-aux-mode")
(def set-transpose-mode () (set! param-mode 3))
(mode-bind-key "seq-grid-mode" "t" "set-transpose-mode")
(def set-pan-mode () (set! param-mode 4))
(mode-bind-key "seq-grid-mode" "p" "set-pan-mode")
(def set-sync-mode () (set! param-mode 5))
(mode-bind-key "seq-grid-mode" "s" "set-sync-mode")
(def set-delay-mode () (set! param-mode 6))
(mode-bind-key "seq-grid-mode" "l" "set-delay-mode")
(def set-process-lane-mode ()
  (if (> (len SEQ.process-lanes) 0)
    (set! param-mode seqv-process-lane-mode-offset)
    nil))
(mode-bind-key "seq-grid-mode" "x" "set-process-lane-mode")


(def param-values ()
  (seqv-current-param-values param-mode))

(def param-min ()
  (seqv-param-min param-mode))

(def param-max ()
  (seqv-param-max param-mode))

(def param-slider-min ()
  (if (= param-mode 1) 0 (param-min)))

(def param-slider-max ()
  (if (= param-mode 1) 1 (param-max)))

(def param-slider-value (step)
  (if (= param-mode 1)
    (duration-slider-position (nth (param-values) step))
    (nth (param-values) step)))

(def param-haptic-pivot-position ()
  (if (= param-mode 1) 0.5 1))

(def param-haptic-pivot-value ()
  (if (= param-mode 1) 2 (param-max)))

(def param-haptic-exponent ()
  (if (= param-mode 1) 4 1))

(def param-keyword ()
  (seqv-param-keyword param-mode))

(def param-color ()
  (seqv-param-color param-mode))

(def param-name ()
  (seqv-param-name param-mode))

(def param-origin ()
  (seqv-param-origin param-mode))

(def sync-current-label ()
  (nth SEQ.sync-labels (floor (+ 0.5 (nth SEQ.syncs (core/current-step))))))
