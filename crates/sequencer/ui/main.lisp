; Minimal Metal Sequencer - Step Grid UI
; C-p to toggle play/stop, Esc to clear step selection

(load "@/ui/themes.lisp")
(seq-theme-mac-osx-dark)
(load "@/ui/materials.lisp")

(load "@/ui/seq-core-state.lisp")

;; Must precede browser/mixer/sequencer: they call its helpers bare, and a
;; converted module's compat aliases only reach callers that compile after
;; it is evaluated. Those three still (load …) it themselves at their own
;; top — harmless re-evals that keep the standalone test harnesses working,
;; but too late to be the load that counts (spec §10 step 0).
(load "@/ui/track-collapse.lisp")

(load "@/ui/browser.lisp")
(load "@/ui/mixer.lisp")
(load "@/ui/patch-macros.lisp")
(load "@/ui/effects.lisp")
(load "@/ui/macros.lisp")
(load "@/ui/piano-roll.lisp")
;; Must precede the patcher buffers, which mount choose-model-panel.
(load "@/ui/choose-model.lisp")
(load "@/ui/transport.lisp")
(load "@/ui/agent.lisp")

(def seq-clear-ui-selection ()
  (do
    (seq-clear-selection)))

(bind-key "C-p" "seq-toggle-play")
(bind-key "ESC" "seq-clear-ui-selection")

;; Code editor: compile + hot-swap the current dsp code buffer. The host
;; command no-ops unless a code edit session is active.
(def seq-eval-editor-code ()
  (host-command "evaluate-editor-source" (dict)))
(bind-key "C-c C-c" "seq-eval-editor-code")

(load "@/ui/seq-step-tabs.lisp")
(load "@/ui/seq-script-picker.lisp")
(load "@/ui/seq-layout.lisp")
(load "@/ui/seq-macro-mapping-hooks.lisp")
(load "@/ui/seq-panels.lisp")
(load "@/ui/step-grid-interactions.lisp")
(load "@/ui/seqv-track-params.lisp")
(load "@/ui/seq-grid-mode.lisp")
(load "@/ui/bus-grid.lisp")

;; ui/step-grid.lisp (the legacy *metal* step grid) is intentionally NOT
;; loaded: no tile shows it, but as a loaded effect-buffer its whole-list
;; SEQ reads (steps/velocities/...) forced a full hidden-buffer rerun on
;; every step or scene edit (~5ms per launch/edit). The file is kept for
;; reference; `metal-track-tick` (the one widget the live UI still uses)
;; now lives in ui/sequencer.lisp.
(load "@/ui/sequencer.lisp")
(load "@/ui/sound-palette.lisp")
(load "@/ui/arrangement.lisp")
(load "@/ui/effects/step-buffer.lisp")

; Startup layout is applied by Rust after this file loads. Keep this file free of
; top-level layout side effects so hot reload and buffer re-evaluation do not
; replace the active editor layout.
