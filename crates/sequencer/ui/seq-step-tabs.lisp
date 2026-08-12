;; Main-panel step-tab registry: layout states, tab records, register/unregister.
;; Extracted from ui/main.lisp (module-system spec slice S2). Headerless on
;; purpose: implicit eseq.vanilla until per-file (module …) headers land in S3.

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
