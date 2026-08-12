;; Script sequencer picker + the script-buffer-name/script-init-fn host contract stubs.
;; Extracted from ui/main.lisp (module-system spec slice S2). Headerless on
;; purpose: implicit eseq.vanilla until per-file (module …) headers land in S3.


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
