; Minimal Metal Sequencer - Step Grid UI
; C-p to toggle play/stop, Esc to clear step selection

(load "@/ui/themes.lisp")
(seq-theme-mac-osx-dark)
(load "@/ui/materials.lisp")

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

(defstate piano-roll-placement :bottom)
(defstate seq-main-view :session)
(defstate step-panel-buffer "*sequencer*")
(defstate remembered-step-panel-buffer "*sequencer*")
(defstate lower-panel-buffer "*fx*")
(defstate seq-layout-mode :lower-panel)
(defstate seq-patcher-buffer "")
(defstate seq-patcher-source-buffer "")
(defstate seq-registered-step-tabs '())

(def lower-fx-layout-height piano-roll-default-pane-height)

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
          (seq-delete-script-sequencer-by-buffer closed-buffer)))
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
        :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25)
      (list :buf buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25))))

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
          (seq-refresh-current-layout)
          (seq-refresh-step-tabs-if-present)
          true))
      false)))

;; ── Script picker ──────────────────────────────────────────────────────────
;; Scripts can expose this lightweight contract. The picker resets it before each
;; load, then calls script-init-fn once after a successful load. Scripts that
;; don't register a UI tab are opened as source tabs by default.
(def script-buffer-name "")
(def script-tab-label "")
(def script-sequencer-name "")
(def script-source-tab-label "")
(def script-source-tab-requested false)
(def script-source-tab-opened false)
(def script-init-fn () false)

(def seq-script-reset-contract ()
  (do
    (set! script-buffer-name "")
    (set! script-tab-label "")
    (set! script-sequencer-name "")
    (set! script-source-tab-label "")
    (set! script-source-tab-requested false)
    (set! script-source-tab-opened false)
    (def script-init-fn () false)))

(def seq-register-script-source-tab (label)
  (do
    (set! script-source-tab-label label)
    (set! script-source-tab-requested true)
    (let ((path (current-source-path)))
      (if (not (= path ""))
        (do
          (set! script-source-tab-opened true)
          (host-command "open-script-source-tab"
            (dict
              :path path
              :label label)))
        false))))

(def seq-script-default-dir ()
  (if (directory? "scripts")
    "scripts"
    (if (directory? "crates/sequencer/scripts")
      "crates/sequencer/scripts"
      "scripts")))

(def seq-script-picker-current-dir (seq-script-default-dir))
(def seq-script-picker-entries '())
(def seq-script-picker-source-buffer "")

(def seq-switch-or-create-buffer (name)
  (let ((bufs (buffer-list))
        (exists (reduce |acc b| (if (= b name) true acc) false bufs)))
    (if exists
      (switch-to-buffer name)
      (create-buffer name))))

(define-mode "seq-script-picker-mode" :read-only true)
(mode-bind-key "seq-script-picker-mode" "g" "seq-script-picker-refresh")
(mode-bind-key "seq-script-picker-mode" "q" "seq-script-picker-quit")
(mode-bind-key "seq-script-picker-mode" "-" "seq-script-picker-up")
(mode-bind-key "seq-script-picker-mode" "RET" "seq-script-picker-open-at-point")

(def seq-script-entry-visible? (entry)
  (or (get entry :directory)
    (string-ends-with? (get entry :name) ".lisp")))

(def seq-script-picker-styles ()
  (append
    (list
      (style-bg-current-line :widget-focus-bg)
      (style-bold-fg 0 0 8 :status-accent)
      (style-bold-fg 0 9 200 :blue))
    (dired-row-styles 2 true)
    (dired-entry-styles seq-script-picker-entries 3)))

(def seq-script-picker-current-entry ()
  (let ((line (current-line-number)))
    (if (= line 3)
      :parent
      (if (>= line 4)
        (nth seq-script-picker-entries (- line 4))
        nil))))

(def seq-script-picker-refresh ()
  (if (not (directory? seq-script-picker-current-dir))
    (do
      (set! seq-script-picker-entries '())
      (render-widget nil)
      (set-buffer-lines
        (list (str "Scripts > " seq-script-picker-current-dir)
          ""
          "script directory not found"))
      (status (fmt "Script directory not found: {}" seq-script-picker-current-dir)))
    (let ((entries (filter (lambda (entry) (seq-script-entry-visible? entry)) (list-directory seq-script-picker-current-dir)))
          (dirs (filter |e| (get e :directory) entries))
          (files (filter |e| (not (get e :directory)) entries)))
      (set! seq-script-picker-entries (append dirs files))
      (render-widget nil)
      (set-buffer-lines
        (append
          (list (str "Scripts > " seq-script-picker-current-dir)
            ""
            "drwxr-xr-x  1      0            .. ../")
          (map dired-format-entry seq-script-picker-entries)))
      (set-buffer-styles (seq-script-picker-styles))
      (goto-line 3)
      (status (fmt "{} scripts" (len files))))))

(def seq-script-picker-up ()
  (let ((parent (path-parent seq-script-picker-current-dir)))
    (if (= parent nil)
      (status "Already at root")
      (do
        (set! seq-script-picker-current-dir parent)
        (seq-script-picker-refresh)))))

(def seq-script-scratch-path (path)
  (if (string-starts-with? path "crates/sequencer/")
    path
    (if (string-starts-with? path "scripts/")
      (str "crates/sequencer/" path)
      (if (string-starts-with? path "/")
        path
        (str "crates/sequencer/" path)))))

(def seq-script-scratch-entry (path)
  (list (source (list 'load (seq-script-scratch-path path)))))

(def seq-script-append-to-scratch (path)
  (append-buffer-lines-for "*scratch*" (seq-script-scratch-entry path)))

(def seq-script-remove-from-scratch (path)
  (do
    (remove-buffer-lines-for "*scratch*" (seq-script-scratch-entry path))
    (host-command "remove-project-script-from-scratch" (dict :path path))))

(def seq-script-register-loaded-tab ()
  (if (not (= script-buffer-name ""))
    (seq-register-script-step-sequencer-tab
      (if (= script-tab-label "") script-buffer-name script-tab-label)
      script-buffer-name
      script-sequencer-name
      "")
    false))

(def seq-script-register-loaded-tab-from-path (path)
  (if (not (= script-buffer-name ""))
    (seq-register-script-step-sequencer-tab
      (if (= script-tab-label "") script-buffer-name script-tab-label)
      script-buffer-name
      script-sequencer-name
      path)
    false))

(def seq-script-register-source-tab-from-path (path)
  (if (and (not script-source-tab-opened)
        (or script-source-tab-requested (= script-buffer-name "")))
    (host-command "open-script-source-tab"
      (dict
        :path path
        :label (if (= script-source-tab-label "") (path-filename path) script-source-tab-label)))
    false))

(def seq-script-tab-matches-sequencer? (tab name)
  (= (seq-step-tab-sequencer-name tab) name))

(def seq-script-tab-for-sequencer (name)
  (let ((hits (filter (lambda (tab) (seq-script-tab-matches-sequencer? tab name))
                seq-registered-step-tabs)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def seq-delete-step-sequencer-tab (buffer)
  (seq-unregister-step-sequencer-tab buffer))

(def seq-delete-script-sequencer-by-buffer (buffer)
  (let ((hits (filter (lambda (tab) (seq-step-tab-matches-buffer? tab buffer))
                seq-registered-step-tabs)))
    (if (> (len hits) 0)
      (let ((tab (nth hits 0))
            (name (seq-step-tab-sequencer-name (nth hits 0)))
            (path (seq-step-tab-source-path (nth hits 0))))
        (do
          (if (not (= name "")) (seq-unpublish-sequencer name) false)
          (if (not (= path "")) (seq-script-remove-from-scratch path) false)
          (seq-unregister-step-sequencer-tab buffer)
          (status (fmt "Deleted sequencer tab {}" buffer))
          true))
      false)))

(def seq-delete-script-sequencer (name)
  (let ((tab (seq-script-tab-for-sequencer name)))
    (do
      (seq-unpublish-sequencer name)
      (if tab
        (do
          (if (not (= (seq-step-tab-source-path tab) ""))
            (seq-script-remove-from-scratch (seq-step-tab-source-path tab))
            false)
          (seq-unregister-step-sequencer-tab (seq-step-tab-buffer tab)))
        false)
      (status (fmt "Deleted sequencer {}" name))
      true)))

(def seq-delete-script-sequencer-with-buffer (name buffer)
  (let ((hits (filter (lambda (tab) (seq-step-tab-matches-buffer? tab buffer))
                seq-registered-step-tabs)))
    (do
      (seq-unpublish-sequencer name)
      (if (> (len hits) 0)
        (let ((path (seq-step-tab-source-path (nth hits 0))))
          (if (not (= path ""))
            (seq-script-remove-from-scratch path)
            false))
        false)
      (seq-unregister-step-sequencer-tab buffer)
      (status (fmt "Deleted sequencer {}" name))
      true)))

(def seq-script-return-to-source-buffer ()
  (if (not (= seq-script-picker-source-buffer ""))
    (switch-to-buffer seq-script-picker-source-buffer)
    (switch-to-buffer "*sequencer*")))

(def seq-script-load-file (path)
  (do
    (seq-script-reset-contract)
    (let ((load-result (load path)))
      (if (string-starts-with? (str load-result) "load:")
        (status (str load-result))
        (do
          (seq-script-append-to-scratch path)
          (script-init-fn)
          (seq-script-register-loaded-tab-from-path path)
          (seq-script-register-source-tab-from-path path)
          (seq-script-return-to-source-buffer)
          (status (fmt "Loaded script {}" (path-filename path))))))))

(def seq-script-picker-open-entry (entry)
  (let ((name (get entry :name))
        (is-dir (get entry :directory)))
    (if is-dir
      (do
        (set! seq-script-picker-current-dir (path-join seq-script-picker-current-dir name))
        (seq-script-picker-refresh))
      (seq-script-load-file (path-join seq-script-picker-current-dir name)))))

(def seq-script-picker-open-at-point ()
  (let ((entry (seq-script-picker-current-entry)))
    (if (= entry :parent)
      (seq-script-picker-up)
      (if entry
        (seq-script-picker-open-entry entry)
        (status "No script on this line")))))

(def seq-script-picker-quit ()
  (if (not (= seq-script-picker-source-buffer ""))
    (switch-to-buffer seq-script-picker-source-buffer)
    (switch-to-buffer "*sequencer*")))

(def seq-script-picker ()
  (do
    (set! seq-script-picker-source-buffer (current-buffer-name))
    (set! seq-script-picker-current-dir (seq-script-default-dir))
    (seq-switch-or-create-buffer "*scripts*")
    (set-view-mode "text")
    (set-buffer-mode "seq-script-picker-mode")
    (seq-script-picker-refresh)))

(bind-key "C-c s" "seq-script-picker")

(def seq-step-and-track-panel-layout-spec ()
  (list :cols :gap 1
    0.78 (seq-main-step-tile-layout-spec)
    0.22 (list :rows :gap 1
      0.48 (list :buf "*step*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 28 :max-width 28)
      0.52 (list :buf "*track*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :max-height 7 :min-height 7 :min-width 28 :max-width 28))))

(def seq-main-panel-layout-spec ()
  (if (seq-arrangement-view?)
    (list :buf "*arrangement*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25)
    (seq-step-and-track-panel-layout-spec)))

(def seq-collapsible-panel-layout-spec (buffer on-collapse min-width max-width min-height max-height)
  (list :buf buffer
    :hide-status true
    :border-radius 12
    :border-width 4
    :background-color :buffer-bg
    :min-width min-width
    :max-width max-width
    :min-height min-height
    :max-height max-height
    :collapse-threshold 0.25
    :on-collapse on-collapse))

(def seq-samples-panel-layout-spec (min-width max-width min-height max-height)
  (seq-collapsible-panel-layout-spec "*samples*"
    (lambda () (seq-hide-samples-sidebar))
    min-width max-width min-height max-height))

(def seq-mixer-panel-layout-spec (min-width max-width min-height max-height)
  (seq-collapsible-panel-layout-spec "*mixer*"
    (lambda () (seq-hide-mixer-panel))
    min-width max-width min-height max-height))

;; Patch-editor bottom bar variant of the mixer panel: a single compact
;; channel strip for the current track (see patch-mixer-strip in mixer.lisp).
(def seq-patch-mixer-panel-layout-spec (min-width max-width min-height max-height)
  (seq-collapsible-panel-layout-spec "*patch-mixer*"
    (lambda () (seq-hide-mixer-panel))
    min-width max-width min-height max-height))

(def seq-fx-panel-layout-spec (min-width max-width min-height max-height)
  (seq-collapsible-panel-layout-spec "*fx*"
    (lambda () (seq-hide-fx-panel))
    min-width max-width min-height max-height))

(def seq-samples-sidebar-layout-spec ()
  (if (param-macro-mapping-active?)
    (seq-collapsible-panel-layout-spec "*macro-mappings*"
      (lambda () (macro-clear-mapping-arm))
      46 64 nil nil)
    (seq-samples-panel-layout-spec 34 42 nil nil)))

(def seq-sidebar-ratio ()
  (if (param-macro-mapping-active?) 0.34 0.2))

(def seq-main-and-mixer-layout-spec ()
  (if mixer-panel-visible
    (list :rows :gap 1
      0.55 (seq-main-panel-layout-spec)
      0.45 (seq-mixer-panel-layout-spec nil nil 14.5 14.5))
    (seq-main-panel-layout-spec)))

(def seq-lower-panel-layout-spec (lower-buffer lower-ratio lower-min-height lower-max-height)
  (let ((main-layout
          (if samples-sidebar-visible
            (list :cols :gap 1
              (seq-sidebar-ratio) (seq-samples-sidebar-layout-spec)
              (- 1.0 (seq-sidebar-ratio)) (seq-main-and-mixer-layout-spec))
            (seq-main-and-mixer-layout-spec))))
    (if lower-panel-visible
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 (list :rows :gap 1 :remember (str "sequencer-lower-panel:" lower-buffer)
          0.95 main-layout
          lower-ratio (if (= lower-buffer "*fx*")
            (seq-fx-panel-layout-spec nil nil lower-min-height lower-max-height)
            (list :buf lower-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height lower-min-height :max-height lower-max-height))))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

;; Every patcher bottom-bar panel shares the regular fx-panel height so the
;; instrument preview isn't padded out by a taller neighbor.
(def seq-patcher-bottom-bar-panel-height lower-fx-layout-height)

(def seq-patcher-bottom-bar-layout-spec ()
  (let ((h seq-patcher-bottom-bar-panel-height))
    (if (and samples-sidebar-visible mixer-panel-visible lower-panel-visible)
      (list :cols :gap 1
        0.30 (seq-samples-panel-layout-spec 28 28 h h)
        0.15 (seq-patch-mixer-panel-layout-spec 10 10 h h)
        0.55 (seq-fx-panel-layout-spec nil nil h h))
      (if (and samples-sidebar-visible mixer-panel-visible)
        (list :cols :gap 1
          0.6 (seq-samples-panel-layout-spec 28 28 h h)
          0.4 (seq-patch-mixer-panel-layout-spec 10 10 h h))
      (if samples-sidebar-visible
        (if lower-panel-visible
          (list :cols :gap 1
            0.5 (seq-samples-panel-layout-spec 28 28 h h)
            0.5 (seq-fx-panel-layout-spec nil nil h h))
          (seq-samples-panel-layout-spec 28 28 h h))
        (if mixer-panel-visible
          (if lower-panel-visible
            (list :cols :gap 1
              0.4 (seq-patch-mixer-panel-layout-spec 10 10 h h)
              0.6 (seq-fx-panel-layout-spec nil nil h h))
            (seq-patch-mixer-panel-layout-spec 15 15 h h))
          (seq-fx-panel-layout-spec nil nil h h)))))))

(def seq-patcher-bottom-bar-visible? ()
  (or samples-sidebar-visible mixer-panel-visible lower-panel-visible))

;; Macro sidebar to the left of the patch editor: defmacros in the patch +
;; the saved macro library (see ui/patch-macros.lisp).
(def seq-patch-macros-panel-layout-spec ()
  (seq-collapsible-panel-layout-spec "*patch-macros*"
    (lambda () (seq-hide-patch-macros-panel))
    22 26 nil nil))

(def seq-patcher-canvas-layout-spec (patcher-buffer)
  (list :buf patcher-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20))

(def seq-patcher-main-layout-spec (patcher-buffer)
  (if patch-macros-panel-visible
    (list :cols :gap 1
      0.15 (seq-patch-macros-panel-layout-spec)
      0.85 (seq-patcher-canvas-layout-spec patcher-buffer))
    (seq-patcher-canvas-layout-spec patcher-buffer)))

(def seq-instrument-patcher-layout-spec (patcher-buffer)
  (if (seq-patcher-bottom-bar-visible?)
    (list :rows :gap 1
      0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
      0.80 (seq-patcher-main-layout-spec patcher-buffer)
      0.15 (seq-patcher-bottom-bar-layout-spec))
    (list :rows :gap 1
      0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
      0.95 (seq-patcher-main-layout-spec patcher-buffer))))

(def seq-instrument-patcher-source-layout-spec (patcher-buffer source-buffer)
  (let ((main-layout
          (if patch-macros-panel-visible
            (list :cols :gap 1
              0.14 (seq-patch-macros-panel-layout-spec)
              0.53 (seq-patcher-canvas-layout-spec patcher-buffer)
              0.33 (list :buf source-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20))
            (list :cols :gap 1
              0.62 (seq-patcher-canvas-layout-spec patcher-buffer)
              0.38 (list :buf source-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20)))))
    (if (seq-patcher-bottom-bar-visible?)
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.80 main-layout
        0.15 (seq-patcher-bottom-bar-layout-spec))
      (list :rows :gap 1
        0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
        0.95 main-layout))))

(def seq-apply-lower-panel-layout (lower-buffer lower-ratio lower-min-height lower-max-height)
  (do
    (set! seq-layout-mode :lower-panel)
    (set-layout (seq-lower-panel-layout-spec lower-buffer lower-ratio lower-min-height lower-max-height))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-apply-fx-layout ()
  (do
    (set! lower-panel-buffer "*fx*")
    (seq-apply-lower-panel-layout "*fx*" 0.33 lower-fx-layout-height lower-fx-layout-height)))

(def seq-apply-piano-roll-layout ()
  (do
    (set! lower-panel-buffer "*piano-roll*")
    (seq-apply-lower-panel-layout "*piano-roll*" 0.33 lower-fx-layout-height 50)))

(def seq-apply-instrument-patcher-layout (patcher-buffer)
  (do
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set! seq-layout-mode :instrument-patcher)
    (set! seq-patcher-buffer patcher-buffer)
    (set! seq-patcher-source-buffer "")
    (set-layout (seq-instrument-patcher-layout-spec patcher-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-apply-instrument-patcher-source-layout (patcher-buffer source-buffer)
  (do
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set! seq-layout-mode :instrument-patcher-source)
    (set! seq-patcher-buffer patcher-buffer)
    (set! seq-patcher-source-buffer source-buffer)
    (set-layout (seq-instrument-patcher-source-layout-spec patcher-buffer source-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-refresh-current-layout ()
  (if (and (= seq-layout-mode :instrument-patcher-source) (not (= seq-patcher-buffer "")) (not (= seq-patcher-source-buffer "")))
    (seq-apply-instrument-patcher-source-layout seq-patcher-buffer seq-patcher-source-buffer)
    (if (and (= seq-layout-mode :instrument-patcher) (not (= seq-patcher-buffer "")))
      (seq-apply-instrument-patcher-layout seq-patcher-buffer)
      (if (= lower-panel-buffer "*piano-roll*")
        (seq-apply-piano-roll-layout)
        (seq-apply-fx-layout)))))

(defstate macro-mapping-sidebar-was-visible true)

(def macro-mapping-sidebar-open-hook ()
  (do
    (set! macro-mapping-sidebar-was-visible samples-sidebar-visible)
    (set! samples-sidebar-visible true)))

(def macro-mapping-sidebar-refresh-hook ()
  (seq-refresh-current-layout))

(def macro-mapping-sidebar-close-hook ()
  (do
    (set! samples-sidebar-visible macro-mapping-sidebar-was-visible)
    (seq-refresh-current-layout)))

(def seq-hide-samples-sidebar ()
  (if samples-sidebar-visible
    (do
      (set! samples-sidebar-visible false)
      (seq-refresh-current-layout))
    nil))

(def seq-hide-mixer-panel ()
  (if mixer-panel-visible
    (do
      (seq-sync-step-panel-buffer-from-current-window)
      (set! mixer-panel-visible false)
      (seq-refresh-current-layout))
    nil))

(def seq-hide-fx-panel ()
  (if lower-panel-visible
    (do
      (set! lower-panel-visible false)
      (seq-refresh-current-layout))
    nil))

(def seq-hide-patch-macros-panel ()
  (if patch-macros-panel-visible
    (do
      (set! patch-macros-panel-visible false)
      (seq-refresh-current-layout))
    nil))

(def seq-toggle-patch-macros-panel ()
  (do
    (set! patch-macros-panel-visible (not patch-macros-panel-visible))
    (seq-refresh-current-layout)))

(def seq-toggle-samples-sidebar ()
  (do
    (set! samples-sidebar-visible (not samples-sidebar-visible))
    (seq-refresh-current-layout)))

(def seq-toggle-mixer-panel ()
  (do
    (seq-sync-step-panel-buffer-from-current-window)
    (set! mixer-panel-visible (not mixer-panel-visible))
    (seq-refresh-current-layout)))

(def seq-toggle-fx-panel ()
  (do
    (set! lower-panel-visible (not lower-panel-visible))
    (seq-refresh-current-layout)))

(def seq-restore-instrument-patcher-layout ()
  (do
    (set! step-panel-buffer remembered-step-panel-buffer)
    (if (= lower-panel-buffer "*piano-roll*")
      (seq-apply-piano-roll-layout)
      (seq-apply-fx-layout))))

(def seq-current-step-buffer ()
  (seq-sanitized-step-buffer
    (if (= step-panel-buffer "*piano-roll*")
      remembered-step-panel-buffer
      step-panel-buffer)))

(def seq-sync-step-panel-buffer-from-current-window ()
  (let ((buffer (current-buffer-name)))
    (if (seq-main-step-tab-buffer? buffer)
      (do
        (set! step-panel-buffer buffer)
        (set! remembered-step-panel-buffer buffer))
      nil)))

(def seq-piano-roll-open? ()
  (or (= step-panel-buffer "*piano-roll*")
    (= lower-panel-buffer "*piano-roll*")))

(def seq-close-piano-roll ()
  (if (= step-panel-buffer "*piano-roll*")
    (do
      (set! step-panel-buffer (seq-current-step-buffer))
      (set-window-buffer step-panel-buffer)
      (seq-apply-fx-layout))
    (do
      (if (= (current-buffer-name) "*piano-roll*")
        (set-window-buffer "*fx*")
        (set-window-buffer-for "*piano-roll*" "*fx*"))
      (seq-apply-fx-layout))))

(def seq-open-piano-roll-bottom-for-track-core (track)
  (do
    (set! lower-panel-visible true)
    (if (= step-panel-buffer "*piano-roll*")
      (set! step-panel-buffer (seq-current-step-buffer))
      nil)
    (if (= (current-buffer-name) "*fx*")
      (set-window-buffer "*piano-roll*")
      (set-window-buffer-for "*fx*" "*piano-roll*"))
    (piano-roll-request-fit-for-track track)
    (seq-apply-piano-roll-layout)))

(def seq-open-piano-roll-bottom-for-track (track)
  (do
    (reactive-set "SEQV" "piano-roll-arrangement-mode" 0)
    (seq-open-piano-roll-bottom-for-track-core track)))

(def seq-open-arrangement-piano-roll-bottom-for-track (track)
  (do
    (reactive-set "SEQV" "piano-roll-arrangement-mode" 1)
    (seq-open-piano-roll-bottom-for-track-core track)))

(def seq-open-piano-roll-bottom ()
  (seq-open-piano-roll-bottom-for-track SEQ.current-track))

(def seq-open-piano-roll-main ()
  (seq-open-piano-roll-bottom))

(def seq-open-piano-roll-preferred ()
  (seq-open-piano-roll-bottom))

(def seq-switch-main-view (view)
  (let ((old-buffer (seq-visible-main-panel-buffer)))
    (do
      (set! seq-main-view view)
      (if (= seq-layout-mode :lower-panel)
        ;; Reapplying the layout when nothing changed clobbers incremental
        ;; relayout dirty tracking (e.g. expand/collapse row diffs), so only
        ;; refresh when the visible main panel actually switched buffers.
        (if (not (= old-buffer (seq-visible-main-panel-buffer)))
          (do
            (set-window-buffer-for old-buffer (seq-visible-main-panel-buffer))
            (seq-refresh-current-layout))
          nil)
        ;; An explicit view switch leaves the instrument patcher workspace and
        ;; returns to the normal sequencer workspace.
        (seq-apply-fx-layout)))))

(def seq-show-sequencer-main ()
  (do
    (reactive-set "SEQV" "piano-roll-arrangement-mode" 0)
    (set! remembered-step-panel-buffer "*sequencer*")
    (set! step-panel-buffer "*sequencer*")
    (seq-switch-main-view :session)))

;; Arrangement is an app view, not a sequencer tile tab. It owns a wider main
;; layout without the step and track context panes.
(def seq-open-arrangement ()
  (seq-switch-main-view :arrangement))

(def seq-toggle-arrangement ()
  (if (seq-arrangement-view?)
    (seq-show-sequencer-main)
    (seq-open-arrangement)))

(def seq-toggle-current-track-expanded-main ()
  (do
    (seq-show-sequencer-main)
    (seqv-toggle-current-track-expanded)))

(def seq-toggle-piano-roll-main ()
  (seq-toggle-main-or-piano-roll))

(def seq-toggle-piano-roll-placement ()
  (do
    (set! piano-roll-placement :bottom)
    (if (= step-panel-buffer "*piano-roll*")
      (seq-open-piano-roll-bottom)
      nil)))

(def seq-toggle-fx-piano-roll ()
  (do
    (set! lower-panel-visible true)
    (if (= (current-buffer-name) "*fx*")
    (do
      (set-window-buffer "*piano-roll*")
      (set! lower-panel-buffer "*piano-roll*")
      (piano-roll-request-fit)
      (seq-apply-piano-roll-layout))
    (if (= (current-buffer-name) "*piano-roll*")
      (do
        (set-window-buffer "*fx*")
        (set! lower-panel-buffer "*fx*")
        (seq-apply-fx-layout))
      (if (= lower-panel-buffer "*fx*")
        (do
          (set-window-buffer-for "*fx*" "*piano-roll*")
          (set! lower-panel-buffer "*piano-roll*")
          (piano-roll-request-fit)
          (seq-apply-piano-roll-layout))
        (do
          (set-window-buffer-for "*piano-roll*" "*fx*")
          (set! lower-panel-buffer "*fx*")
          (seq-apply-fx-layout)))))))

(def seq-toggle-main-or-piano-roll ()
  (if (or (= SEQ.editor-mode "new-instrument")
          (= SEQ.editor-mode "edit-instrument")
          (= SEQ.editor-mode "new-effect")
          (= SEQ.editor-mode "edit-effect"))
    (host-command "toggle-instrument-patcher-source" (dict))
    (if (seq-piano-roll-open?)
      (seq-close-piano-roll)
      (seq-open-piano-roll-bottom))))

(def seq-show-fx-lower-panel ()
  (do
    ;; This is an explicit mode transition even when the FX buffer is already
    ;; visible; do not leave a stale arrangement-editor mode behind.
    (reactive-set "SEQV" "piano-roll-arrangement-mode" 0)
    (if (or (not lower-panel-visible) (= lower-panel-buffer "*piano-roll*"))
      (do
        (set! lower-panel-visible true)
        (if (= lower-panel-buffer "*piano-roll*")
          (if (= (current-buffer-name) "*piano-roll*")
            (set-window-buffer "*fx*")
            (set-window-buffer-for "*piano-roll*" "*fx*"))
          nil)
        (seq-apply-fx-layout))
      nil)))

(def seq-toggle-current-track-mods-view ()
  (do
    (set! selected-bus -1)
    (instrument-toggle-mods-view)
    (seq-show-fx-lower-panel)))

(bind-key "Tab" "seq-toggle-arrangement")
;; Sound palette overlay (takes spec 17.6): toggles on the bound clip, or
;; the current track's binding outside the timeline.
(bind-key "C-x p" "seq-toggle-sound-palette")
(bind-key "BackTab" "seq-toggle-main-or-piano-roll")

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

(def seqv-track-list (lists track)
  (if (< track (len lists))
    (nth lists track)
    '()))

;; A drum rack's sequencer pitch is its pad note.  The host publishes the
;; occupied pads as {transpose, label} pairs so every sequencer surface can
;; present the same names that appear on the drum-pad grid.
(def seqv-track-drum-rack? (track)
  (seqv-list-ref SEQ.track-drum-racks track false))

(def seqv-track-drum-sounds (track)
  (seqv-list-ref SEQ.track-drum-sounds track '()))

(def seqv-drum-sound-mode? (track mode)
  (and (= mode 3) (seqv-track-drum-rack? track)))

(def seqv-drum-sound-count (track)
  (len (seqv-track-drum-sounds track)))

(def seqv-drum-sound-labels (track)
  (map (lambda (sound) (get sound :label)) (seqv-track-drum-sounds track)))

(def seqv-drum-sound-short-labels (track)
  (map (lambda (sound) (get sound :short-label)) (seqv-track-drum-sounds track)))

(def seqv-drum-sound-indices-for-transpose (track transpose)
  (let ((sounds (seqv-track-drum-sounds track)))
    (filter
      (lambda (idx)
        (= (round (get (nth sounds idx) :transpose)) (round transpose)))
      (range 0 (len sounds)))))

(def seqv-drum-sound-index-for-transpose (track transpose)
  (let ((indices (seqv-drum-sound-indices-for-transpose track transpose)))
    (if (> (len indices) 0) (nth indices 0) 0)))

(def seqv-drum-sound-label-for-transpose (track transpose)
  (let ((sounds (seqv-track-drum-sounds track))
        (indices (seqv-drum-sound-indices-for-transpose track transpose)))
    (if (> (len indices) 0)
      (get (nth sounds (nth indices 0)) :label)
      (str "Unassigned " (round transpose)))))

(def seqv-drum-sound-transpose-at-index (track index)
  (let ((sounds (seqv-track-drum-sounds track)))
    (if (> (len sounds) 0)
      (get (nth sounds (max 0 (min (round index) (- (len sounds) 1)))) :transpose)
      0)))

(def seqv-drum-sound-transpose-for-label (track label)
  (let ((sounds (seqv-track-drum-sounds track))
        (matches
          (filter (lambda (sound) (= (get sound :label) label))
            (seqv-track-drum-sounds track))))
    (if (> (len matches) 0)
      (get (nth matches 0) :transpose)
      (if (> (len sounds) 0) (get (nth sounds 0) :transpose) 0))))

(def seqv-process-lane-mode-offset 7)

(def seqv-process-lane-mode? (mode)
  (>= mode seqv-process-lane-mode-offset))

(def seqv-process-lane-index (mode)
  (- mode seqv-process-lane-mode-offset))

(def seqv-empty-process-lane ()
  (dict
    :values '()
    :min 0
    :max 1
    :default 0
    :decimals 2
    :label "Process"
    :short-label "proc"
    :instance-id 0
    :inlet ""))

(def seqv-list-ref (items idx fallback)
  (if (and (>= idx 0) (< idx (len items)))
    (nth items idx)
    fallback))

(def seqv-track-process-lanes (track)
  (seqv-list-ref SEQ.track-process-lanes track '()))

(def seqv-track-process-slots (track)
  (seqv-list-ref SEQ.track-process-slots track '()))

(def seqv-current-process-lane (mode)
  (seqv-list-ref
    SEQ.process-lanes
    (seqv-process-lane-index mode)
    (seqv-empty-process-lane)))

(def seqv-track-process-lane (track mode)
  (seqv-list-ref
    (seqv-track-process-lanes track)
    (seqv-process-lane-index mode)
    (seqv-empty-process-lane)))

(def seqv-current-param-values (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :values)
    (if (= mode 0) SEQ.velocities
      (if (= mode 1) SEQ.durations
        (if (= mode 2) SEQ.auxas
          (if (= mode 3) SEQ.transposes
            (if (= mode 4) SEQ.pans
              (if (= mode 5) SEQ.syncs
                SEQ.delays))))))))

(def seqv-track-param-values (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :values)
    (if (= mode 0) (seqv-track-list SEQ.track-velocities track)
      (if (= mode 1) (seqv-track-list SEQ.track-durations track)
        (if (= mode 2) (seqv-track-list SEQ.track-auxas track)
          (if (= mode 3) (seqv-track-list SEQ.track-transposes track)
            (if (= mode 4) (seqv-track-list SEQ.track-pans track)
              (if (= mode 5) (seqv-track-list SEQ.track-syncs track)
                (seqv-track-list SEQ.track-delays track)))))))))

(def seqv-param-values (track mode)
  (if (= track SEQ.current-track)
    (seqv-current-param-values mode)
    (seqv-track-param-values track mode)))

(def seqv-param-value-at (track mode step)
  (let ((values (seqv-param-values track mode)))
    (if (< step (len values))
      (nth values step)
      0)))

(def seqv-param-min (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :min)
    (if (= mode 0) 0
      (if (= mode 1) 0
        (if (= mode 2) 0
          (if (= mode 3) -12
            (if (= mode 4) -1
              0)))))))

(def seqv-param-max (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :max)
    (if (= mode 0) 1
      (if (= mode 1) 32
        (if (= mode 2) 16
          (if (= mode 3) 12
            (if (= mode 4) 1
              (if (= mode 5) (- (len SEQ.sync-labels) 1)
                1))))))))

(def seqv-param-slider-min (mode)
  (if (= mode 1) 0 (seqv-param-min mode)))

(def seqv-param-slider-max (mode)
  (if (= mode 1) 1 (seqv-param-max mode)))

(def seqv-param-slider-value (track mode step)
  (if (= mode 1)
    (duration-slider-position (seqv-param-value-at track mode step))
    (seqv-param-value-at track mode step)))

(def seqv-param-haptic-pivot-position (mode)
  (if (= mode 1) 0.5 1))

(def seqv-param-haptic-pivot-value (mode)
  (if (= mode 1) 2 (seqv-param-max mode)))

(def seqv-param-haptic-exponent (mode)
  (if (= mode 1) 4 1))

(def seqv-param-keyword (mode)
  (if (seqv-process-lane-mode? mode) :process-lane
    (if (= mode 0) :velocity
      (if (= mode 1) :duration
        (if (= mode 2) :aux-a
          (if (= mode 3) :transpose
            (if (= mode 4) :pan
              (if (= mode 5) :sync
                :delay))))))))

(def seqv-param-color (mode)
  (if (seqv-process-lane-mode? mode) :orange
    (if (= mode 0) :blue
      (if (= mode 1) :green
        (if (= mode 2) :magenta
          (if (= mode 3) :yellow
            (if (= mode 4) :red
              (if (= mode 5) :green
                :cyan))))))))

(def seqv-param-name (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :label)
    (if (= mode 0) "Velocity"
      (if (= mode 1) "Duration"
        (if (= mode 2) "Aux A"
          (if (= mode 3) "Transpose"
            (if (= mode 4) "Pan"
              (if (= mode 5) "Sync"
                "Delay"))))))))

(def seqv-range-origin (min-value max-value)
  (if (and (< min-value 0) (= (abs min-value) max-value))
    0
    min-value))

(def seqv-param-origin (mode)
  (if (seqv-process-lane-mode? mode)
    (seqv-range-origin (seqv-param-min mode) (seqv-param-max mode))
    (if (= mode 3) 0
      (if (= mode 4) 0
        (if (= mode 5) 0
          (seqv-param-min mode))))))

(def seqv-param-decimals (mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-current-process-lane mode) :decimals)
    (if (= mode 3) 0 2)))

(def seqv-track-param-min (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :min)
    (seqv-param-min mode)))

(def seqv-track-param-max (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :max)
    (seqv-param-max mode)))

(def seqv-track-param-name (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :label)
    (seqv-param-name mode)))

(def seqv-track-param-origin (track mode)
  (if (seqv-process-lane-mode? mode)
    (seqv-range-origin (seqv-track-param-min track mode) (seqv-track-param-max track mode))
    (seqv-param-origin mode)))

(def seqv-track-param-decimals (track mode)
  (if (seqv-process-lane-mode? mode)
    (get (seqv-track-process-lane track mode) :decimals)
    (seqv-param-decimals mode)))

(def seqv-track-param-slider-min (track mode)
  (if (= mode 1) 0 (seqv-track-param-min track mode)))

(def seqv-track-param-slider-max (track mode)
  (if (= mode 1) 1 (seqv-track-param-max track mode)))

(def seqv-track-param-haptic-pivot-value (track mode)
  (if (= mode 1) 2 (seqv-track-param-max track mode)))

(def seqv-step-param-value (mode value)
  (if (or (= mode 3) (= (seqv-param-decimals mode) 0))
    (round value)
    value))

(def seqv-step-slider-param-value (mode value)
  (if (= mode 1)
    (duration-slider-value value)
    (seqv-step-param-value mode value)))

(def seqv-track-step-param-value (track mode value)
  (if (or (= mode 3) (= (seqv-track-param-decimals track mode) 0))
    (round value)
    value))

(def seqv-track-step-slider-param-value (track mode value)
  (if (= mode 1)
    (duration-slider-value value)
    (seqv-track-step-param-value track mode value)))

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
    (cool-off-follow)
    (set-track-cursor-step (min (* page page-size) (- (max 1 SEQ.tp-num-steps) 1)))))

(def double-track-pattern ()
  (do
    (cool-off-follow)
    (seq-double-track-pattern)
    (set-track-cursor-step (min (current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

(def halve-track-pattern ()
  (do
    (cool-off-follow)
    (seq-halve-track-pattern)
    (set-track-cursor-step (min (current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

;; Cursor keys scoped to *metal* buffer via mode
(define-mode "seq-grid-mode" :read-only true :on-key "seq-grid-handle-key")
(mode-bind-key "seq-grid-mode" "LEFT" "cursor-left")
(mode-bind-key "seq-grid-mode" "RIGHT" "cursor-right")
(mode-bind-key "seq-grid-mode" "C-a" "select-all-steps")
(mode-bind-key "seq-grid-mode" "BS" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "Delete" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "RET" "cursor-toggle")
(mode-bind-key "seq-grid-mode" "C-h" "seqv-collapse-all-tracks")

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
  (nth SEQ.sync-labels (floor (+ 0.5 (nth SEQ.syncs (current-step))))))

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
