; Minimal Metal Sequencer - Step Grid UI
; C-p to toggle play/stop, Esc to clear step selection
;
; This file is the DISTRO ROOT (module spec §4/§7): it assembles the vanilla
; eseq UI out of entry modules and nothing else. Every module a module needs
; is now a declared `import` edge inside that module, so the manifest lists
; only the roots — files nothing imports because their top level *is* the
; side effect (they register effect-buffers, modes and keymaps).
;
; `load` survives for the three files whose evaluation is the point and which
; must therefore re-run every time: themes.lisp (loading a theme file IS
; applying the theme), effects.lisp (its own nested manifest) and
; effects/step-buffer.lisp (a headerless side-effect root).

(load "@/ui/themes.lisp")
(seq-theme-mac-osx-dark)
(import eseq.materials)
(import eseq.seq-core-state)

(import eseq.browser)
(import eseq.mixer)
(import eseq.patch-macros)
(load "@/ui/effects.lisp")
(import eseq.macros)
(import eseq.piano-roll)
(import eseq.choose-model)
(import eseq.patch-learn)
(import eseq.packages)
(import eseq.transport)
(import eseq.agent)

;; Deliberately command-only for now: Patch Learn has no button in the patch
;; editor. Invoke this while the instrument patcher buffer is active.
(def open-learn-patch ()
  (host-command "open-learn-patch"
    (dict :patcher-buffer (current-buffer-name))))

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

;; Library anchors: nothing imports these, but each one imports the layer
;; below it, so they are what pulls in seq-layout and friends.
;; (seq-script-picker's read of eseq.seq-step-tabs `defstate`s is now its own
;; declared import.) step-grid-interactions / seqv-track-params /
;; seq-grid-mode used to ride in on ui/bus-grid.lisp; that file is gone with
;; the bus gate step sequencer, so they are anchored directly here.
(import eseq.seq-script-picker)
(import eseq.seq-macro-mapping-hooks)
(import eseq.step-grid-interactions)
(import eseq.seqv-track-params)
(import eseq.seq-grid-mode)

;; ui/step-grid.lisp (the legacy *metal* step grid) is intentionally NOT
;; loaded: no tile shows it, but as a loaded effect-buffer its whole-list
;; SEQ reads (steps/velocities/...) forced a full hidden-buffer rerun on
;; every step or scene edit (~5ms per launch/edit). The file is kept for
;; reference; `metal-track-tick` (the one widget the live UI still uses)
;; now lives in ui/sequencer.lisp.
(import eseq.sequencer)
(import eseq.arrangement)
(load "@/ui/effects/step-buffer.lisp")

; Startup layout is applied by Rust after this file loads. Keep this file free of
; top-level layout side effects so hot reload and buffer re-evaluation do not
; replace the active editor layout.
