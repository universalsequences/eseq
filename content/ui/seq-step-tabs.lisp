;; Main-panel step-tab registry: layout states, tab records, register/unregister.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; This is a state/accessor hub: its nine `defstate`s and its tab accessors are
;; read and written by flat spelling from ui/seq-panels.lisp, ui/seq-layout.lisp,
;; ui/seq-script-picker.lisp, ui/arrangement.lisp, ui/sequencer.lisp,
;; ui/transport.lisp and from production Rust (`eval_str` by name in
;; src/ui/input.rs, src/ui/edit_sessions.rs, src/ui/host_commands/scripts.rs,
;; src/ui/state_values/project_state.rs). It therefore converts with NO renames
;; and a full set of *identity* compat aliases (the seq-core-state precedent):
;; an unconverted caller matches the alias key flat, and a converted module's
;; bare reference qualifies against itself, misses, and lands on the same alias
;; by base name. Every aliased name is a function, a `defstate`, or a write-once
;; plain `def`, all three of which are immune to hazard (m) — nothing here is a
;; mutable plain `def`, so nothing needs pinning to eseq.vanilla.
;;
;; No imports on purpose. The converted modules whose names this file touches
;; are consumers of THIS hub (seq-script-picker, seq-layout import us) or
;; render roots (piano-roll), and every cross-module reference below is an
;; event-time call, which the identity compat aliases resolve at dispatch:
;;   seq-refresh-current-layout            — eseq.seq-layout (alias → refresh-current-layout)
;;   seq-delete-script-sequencer-by-buffer — eseq.seq-script-picker
;; (`piano-roll-default-pane-height` used to be a LOAD-time read of
;; eseq.piano-roll; eseq-mods.12 moved the constant home to this hub because
;; a render root must never be imported.)
;;
;; Hazard (n)/(n2): no Rust harness reads or slices this file's source, so
;; neither the fragment-eval nor the standalone-eval restriction applies.
(module eseq.seq-step-tabs)

(export piano-roll-placement
        seq-main-view
        step-panel-buffer
        remembered-step-panel-buffer
        lower-panel-buffer
        seq-layout-mode
        seq-patcher-buffer
        seq-patcher-source-buffer
        seq-patcher-learn-buffer
        seq-registered-step-tabs
        piano-roll-default-pane-height
        lower-fx-layout-height
        seq-tile-border-width
        seq-step-tab-buffer
        seq-step-tab-sequencer-name
        seq-step-tab-source-path
        seq-step-tab-matches-buffer?
        seq-main-step-tabs
        seq-main-step-tab-buffer?
        seq-sanitized-step-buffer
        seq-arrangement-view?
        seq-visible-main-panel-buffer
        seq-main-step-tile-layout-spec
        seq-refresh-step-tabs-if-present
        seq-register-step-sequencer-tab
        seq-register-script-step-sequencer-tab
        seq-unregister-step-sequencer-tab
        seq-clear-project-script-tabs
        seq-select-main-step-tab-by-index)

;; Identity aliases — one per name with a caller outside this file (verified by
;; a whole-file sweep of content/ui, crates/sequencer/src and
;; crates/eseqlisp/src). Names with no external caller are `%`-private below.

(defstate piano-roll-placement :bottom)
(defstate seq-main-view :session)
(defstate step-panel-buffer "*sequencer*")
(defstate remembered-step-panel-buffer "*sequencer*")
(defstate lower-panel-buffer "*fx*")
(defstate seq-layout-mode :lower-panel)
(defstate seq-patcher-buffer "")
(defstate seq-patcher-source-buffer "")
(defstate seq-patcher-learn-buffer "")
(defstate seq-registered-step-tabs '())

;; Moved home from eseq.piano-roll (eseq-mods.12): the layout hub owning
;; this constant lets piano-roll import it instead of this hub reading a
;; render root at load time (roots must never be imported).
(def piano-roll-default-pane-height 11.5)
(def lower-fx-layout-height piano-roll-default-pane-height)

;; One border width for every tile chrome stroke. Home is this hub for the
;; same reason as piano-roll-default-pane-height: eseq.seq-layout reads it at
;; load time and imports us, so it cannot be imported back.
(def seq-tile-border-width 2)

(def seq-step-tab-label (tab)
  (nth tab 0))

(def seq-step-tab-buffer (tab)
  (nth tab 1))

(def seq-step-tab-sequencer-name (tab)
  (if (> (len tab) 2) (nth tab 2) ""))

(def seq-step-tab-source-path (tab)
  (if (> (len tab) 3) (nth tab 3) ""))

(def seq-script-step-tab? (tab)
  (> (len tab) 2))

(def seq-step-tab-matches-buffer? (tab buffer)
  (= (seq-step-tab-buffer tab) buffer))

(def seq-render-step-tab (tab)
  (let ((buffer (seq-step-tab-buffer tab)))
    (if (seq-script-step-tab? tab)
      (list (seq-step-tab-label tab)
        buffer
        :on-close
        (lambda (closed-buffer tab-index)
          (eseq.seq-script-picker/seq-delete-script-sequencer-by-buffer closed-buffer)))
      (list (seq-step-tab-label tab) buffer))))

(def seq-main-step-tabs ()
  (append (list (list "Seq" "*sequencer*"))
    (map seq-render-step-tab seq-registered-step-tabs)))

(def seq-main-step-tab-buffer? (buffer)
  (> (len (filter (lambda (tab) (seq-step-tab-matches-buffer? tab buffer))
            (seq-main-step-tabs))) 0))

(def seq-step-buffer? (buffer)
  (or (= buffer "*metal*") (seq-main-step-tab-buffer? buffer)))

(def seq-sanitized-step-buffer (buffer)
  (if (seq-step-buffer? buffer) buffer "*sequencer*"))

(def seq-visible-step-panel-buffer ()
  (seq-sanitized-step-buffer step-panel-buffer))

(def seq-arrangement-view? ()
  (= seq-main-view :arrangement))

(def seq-visible-main-panel-buffer ()
  (if (seq-arrangement-view?)
    "*arrangement*"
    (seq-visible-step-panel-buffer)))

(def seq-main-step-tile-layout-spec ()
  (let ((buffer (seq-visible-step-panel-buffer))
        (tabs (seq-main-step-tabs)))
    (if (and (> (len tabs) 1) (seq-main-step-tab-buffer? buffer))
      (list :buf buffer
        :tabs tabs
        :hide-status true :border-radius 12 :border-width seq-tile-border-width :background-color :buffer-bg :min-width 25)
      (list :buf buffer :hide-status true :border-radius 12 :border-width seq-tile-border-width :background-color :buffer-bg :min-width 25))))

(def seq-refresh-step-tabs-if-present ()
  (let ((tabs (seq-main-step-tabs)))
    (do
      (if (> (len tabs) 1)
        (do
          (set-window-tabs-for "*sequencer*" tabs)
          (for-each
            (lambda (tab) (set-window-tabs-for (seq-step-tab-buffer tab) tabs))
            seq-registered-step-tabs))
        (clear-window-tabs-for "*sequencer*"))
      (clear-window-tabs-for "*arrangement*"))))

(def seq-register-step-sequencer-tab (label buffer)
  (do
    (set! seq-registered-step-tabs
      (append
        (filter (lambda (tab) (not (seq-step-tab-matches-buffer? tab buffer)))
          seq-registered-step-tabs)
        (list (list label buffer))))
    (seq-refresh-step-tabs-if-present)))

(def seq-register-script-step-sequencer-tab (label buffer sequencer-name source-path)
  (let ((project-source-path
          (if (= source-path "") (current-source-path) source-path)))
    (do
      (set! seq-registered-step-tabs
        (append
          (filter (lambda (tab) (not (seq-step-tab-matches-buffer? tab buffer)))
            seq-registered-step-tabs)
          (list (list label buffer sequencer-name project-source-path))))
      (seq-refresh-step-tabs-if-present))))

(def seq-unregister-step-sequencer-tab (buffer)
  (do
    (set! seq-registered-step-tabs
      (filter (lambda (tab) (not (seq-step-tab-matches-buffer? tab buffer)))
        seq-registered-step-tabs))
    (if (= step-panel-buffer buffer) (set! step-panel-buffer "*sequencer*") nil)
    (if (= remembered-step-panel-buffer buffer) (set! remembered-step-panel-buffer "*sequencer*") nil)
    (set-window-buffer-for buffer "*sequencer*")
    ;; The static Seq tab remains, so a refresh selects the tabless layout
    ;; automatically when the final custom sequencer is removed.
    (seq-refresh-step-tabs-if-present)))

(def seq-clear-project-script-tabs ()
  (let ((script-tabs (filter seq-script-step-tab? seq-registered-step-tabs)))
    (do
      (for-each
        (lambda (tab) (seq-unregister-step-sequencer-tab (seq-step-tab-buffer tab)))
        script-tabs)
      true)))

(def seq-select-main-step-tab-by-index (index)
  (let ((tab-index (- index 1))
        (tabs (seq-main-step-tabs)))
    (if (and (>= tab-index 0) (< tab-index (len tabs)))
      (let ((buffer (seq-step-tab-buffer (nth tabs tab-index))))
        (do
          (set! step-panel-buffer buffer)
          (set! remembered-step-panel-buffer buffer)
          (set! seq-main-view :session)
          (set-window-buffer buffer)
          (eseq.seq-layout/refresh-current-layout)
          (seq-refresh-step-tabs-if-present)
          true))
      false)))
