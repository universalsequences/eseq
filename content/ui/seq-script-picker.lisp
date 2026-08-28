;; Script sequencer picker + the script-buffer-name/script-init-fn host contract stubs.
;; Extracted from ui/main.lisp (module-system spec slice S2), converted in S3b.
;;
;; Two keyspaces meet in this file and they convert differently:
;;
;;  1. The **host→script contract** (`script-buffer-name`, `script-tab-label`,
;;     `script-sequencer-name`, `script-source-tab-label`,
;;     `script-source-tab-requested`, `script-source-tab-opened`,
;;     `script-init-fn`) is NOT this module's API — it is a protocol spoken by
;;     arbitrary legacy scripts under `content/scripts/**`. Those scripts are
;;     headerless and
;;     *re-def* the names: `(def script-buffer-name "*16x16*")`,
;;     `(def script-init-fn () …)`.  That is hazard (i)/(m)'s codegen-re-def
;;     variant, so all seven stay flat through the §3 escape hatch
;;     (`(def eseq.vanilla/<name> …)`) and get **no** compat alias.  Every
;;     in-file reference must use the `eseq.vanilla/` spelling — a bare
;;     reference here would intern this module's own slot, a different cell,
;;     and the divergence is silent.
;;
;;  2. The picker itself converts as a normal module.  Its two mutable plain
;;     defs (`seq-script-picker-current-dir`, `seq-script-picker-source-buffer`)
;;     are also pinned: `src/ui/state_values/tests.rs` drives the picker with
;;     headerless `(set! seq-script-picker-current-dir …)` /
;;     `(set! seq-script-picker-source-buffer …)` evals, and wave 7's rule is
;;     that a mutable plain `def` is pinned, never aliased.  The remaining
;;     public names are functions, so identity compat aliases are safe
;;     (a function slot is written once by its `def`).
;;
;; The mode name qualifies to `eseq.seq-script-picker/seq-script-picker-mode`
;; and needs no alias: the only `set-buffer-mode` caller is in this file (and
;; it qualifies against this same module), and no other lisp or Rust file
;; names the mode.  Likewise the four `mode-bind-key` handlers and the
;; `bind-key "C-c s"` handler are all defined here, so their qualified
;; registration strings are exact hits.
(module eseq.seq-script-picker)
;; Compile-time edge (spec §4): this file reads eseq.seq-step-tabs
;; `defstate`s (seq-registered-step-tabs) and its accessors; the import
;; evaluates that module before the readers below compile. Before import's
;; compile-time half this had to be ordered by main.lisp (hazard (p)).
(import eseq.seq-step-tabs)

(export seq-script-reset-contract
        seq-register-script-source-tab
        seq-script-default-dir
        seq-script-entry-visible?
        seq-script-picker-refresh
        seq-script-picker-up
        seq-script-scratch-entry
        seq-script-append-to-scratch
        seq-script-remove-from-scratch
        seq-script-register-loaded-tab
        seq-script-register-loaded-tab-from-path
        seq-script-register-source-tab-from-path
        seq-delete-step-sequencer-tab
        seq-delete-script-sequencer-by-buffer
        seq-delete-script-sequencer
        seq-delete-script-sequencer-with-buffer
        seq-script-remember-source-buffer
        seq-script-return-to-source-buffer
        seq-script-load-file
        seq-script-picker-open-at-point
        seq-script-picker-quit
        seq-script-picker)

;; Identity aliases (hazard-free: every one is a function).  Why each:
;;   seq-register-script-source-tab  — called by headerless scripts under
;;       content/scripts/** and stubbed/evaled flat by src/lisp_host/tests.rs.
;;   seq-script-load-file            — used by the legacy Scripts browser while
;;       its content is being curated into modules (eseq-mods.17).
;;   seq-delete-script-sequencer-by-buffer — ui/seq-step-tabs.lisp (eseq.seq-step-tabs,
;;       calls it from the step-tab :on-close lambda.
;;   seq-delete-script-sequencer, seq-script-default-dir,
;;   seq-script-entry-visible?, seq-script-scratch-entry,
;;   seq-script-append-to-scratch, seq-script-picker — driven by flat name from
;;       src/ui/state_values/tests.rs.
;; Browser imports this module explicitly and calls its two picker operations
;; through an alias; that compile-time edge is required now that transactional
;; main.lisp loading rejects undeclared cross-module load-order dependencies.


;; ── Script picker ──────────────────────────────────────────────────────────
;; Scripts can expose this lightweight contract. The picker resets it before each
;; load, then calls script-init-fn once after a successful load. Scripts that
;; don't register a UI tab are opened as source tabs by default.
;;
;; All seven are PINNED to eseq.vanilla — see the header note (1).
(def eseq.vanilla/script-buffer-name "")
(def eseq.vanilla/script-tab-label "")
(def eseq.vanilla/script-sequencer-name "")
(def eseq.vanilla/script-source-tab-label "")
(def eseq.vanilla/script-source-tab-requested false)
(def eseq.vanilla/script-source-tab-opened false)
(def eseq.vanilla/script-init-fn () false)

(def seq-script-reset-contract ()
  (do
    (set! eseq.vanilla/script-buffer-name "")
    (set! eseq.vanilla/script-tab-label "")
    (set! eseq.vanilla/script-sequencer-name "")
    (set! eseq.vanilla/script-source-tab-label "")
    (set! eseq.vanilla/script-source-tab-requested false)
    (set! eseq.vanilla/script-source-tab-opened false)
    (def eseq.vanilla/script-init-fn () false)))

(def seq-register-script-source-tab (label)
  (do
    (set! eseq.vanilla/script-source-tab-label label)
    (set! eseq.vanilla/script-source-tab-requested true)
    (let ((path (current-source-path)))
      (if (not (= path ""))
        (do
          (set! eseq.vanilla/script-source-tab-opened true)
          (host-command "open-script-source-tab"
            (dict
              :path path
              :label label)))
        false))))

(def seq-script-default-dir ()
  (seq-factory-path "scripts"))

;; PINNED (mutable plain defs, written by headerless Rust test evals — see
;; header note (2)).  Every in-file reference uses the `eseq.vanilla/` spelling.
(def eseq.vanilla/seq-script-picker-current-dir (seq-script-default-dir))
(def eseq.vanilla/seq-script-picker-source-buffer "")

;; Module-private: no reference anywhere outside this file.
(def picker-entries '())

(def switch-or-create-buffer (name)
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

(def picker-styles ()
  (append
    (list
      (style-bg-current-line :widget-focus-bg)
      (style-bold-fg 0 0 8 :status-accent)
      (style-bold-fg 0 9 200 :blue))
    (dired-row-styles 2 true)
    (dired-entry-styles picker-entries 3)))

(def picker-current-entry ()
  (let ((line (current-line-number)))
    (if (= line 3)
      :parent
      (if (>= line 4)
        (nth picker-entries (- line 4))
        nil))))

(def seq-script-picker-refresh ()
  (if (not (directory? eseq.vanilla/seq-script-picker-current-dir))
    (do
      (set! picker-entries '())
      (render-widget nil)
      (set-buffer-lines
        (list (str "Scripts > " eseq.vanilla/seq-script-picker-current-dir)
          ""
          "script directory not found"))
      (status (fmt "Script directory not found: {}" eseq.vanilla/seq-script-picker-current-dir)))
    (let ((entries (filter (lambda (entry) (seq-script-entry-visible? entry)) (list-directory eseq.vanilla/seq-script-picker-current-dir)))
          (dirs (filter |e| (get e :directory) entries))
          (files (filter |e| (not (get e :directory)) entries)))
      (set! picker-entries (append dirs files))
      (render-widget nil)
      (set-buffer-lines
        (append
          (list (str "Scripts > " eseq.vanilla/seq-script-picker-current-dir)
            ""
            "drwxr-xr-x  1      0            .. ../")
          (map dired-format-entry picker-entries)))
      (set-buffer-styles (picker-styles))
      (goto-line 3)
      (status (fmt "{} scripts" (len files))))))

(def seq-script-picker-up ()
  (let ((parent (path-parent eseq.vanilla/seq-script-picker-current-dir)))
    (if (= parent nil)
      (status "Already at root")
      (do
        (set! eseq.vanilla/seq-script-picker-current-dir parent)
        (seq-script-picker-refresh)))))

(def scratch-path (path)
  (seq-project-content-path path))

(def seq-script-scratch-entry (path)
  (list (source (list 'load (scratch-path path)))))

(def seq-script-append-to-scratch (path)
  (append-buffer-lines-for "*scratch*" (seq-script-scratch-entry path)))

(def seq-script-remove-from-scratch (path)
  (do
    (remove-buffer-lines-for "*scratch*" (seq-script-scratch-entry path))
    (host-command "remove-project-script-from-scratch" (dict :path path))))

;; `seq-register-script-step-sequencer-tab`, `seq-registered-step-tabs` and the
;; `seq-step-tab-*` accessors live in ui/seq-step-tabs.lisp (eseq.seq-step-tabs),
;; a declared import edge above: the compile-time half of that import
;; guarantees the defstate keyspace and aliases exist before these readers
;; compile, wherever this file sits in the load order.
(def seq-script-register-loaded-tab ()
  (if (not (= eseq.vanilla/script-buffer-name ""))
    (eseq.seq-step-tabs/seq-register-script-step-sequencer-tab
      (if (= eseq.vanilla/script-tab-label "")
        eseq.vanilla/script-buffer-name
        eseq.vanilla/script-tab-label)
      eseq.vanilla/script-buffer-name
      eseq.vanilla/script-sequencer-name
      "")
    false))

(def seq-script-register-loaded-tab-from-path (path)
  (if (not (= eseq.vanilla/script-buffer-name ""))
    (eseq.seq-step-tabs/seq-register-script-step-sequencer-tab
      (if (= eseq.vanilla/script-tab-label "")
        eseq.vanilla/script-buffer-name
        eseq.vanilla/script-tab-label)
      eseq.vanilla/script-buffer-name
      eseq.vanilla/script-sequencer-name
      path)
    false))

(def seq-script-register-source-tab-from-path (path)
  (if (and (not eseq.vanilla/script-source-tab-opened)
        (or eseq.vanilla/script-source-tab-requested
          (= eseq.vanilla/script-buffer-name "")))
    (host-command "open-script-source-tab"
      (dict
        :path path
        :label (if (= eseq.vanilla/script-source-tab-label "")
                 (path-filename path)
                 eseq.vanilla/script-source-tab-label)))
    false))

(def tab-matches-sequencer? (tab name)
  (= (eseq.seq-step-tabs/seq-step-tab-sequencer-name tab) name))

(def tab-for-sequencer (name)
  (let ((hits (filter (lambda (tab) (tab-matches-sequencer? tab name))
                eseq.seq-step-tabs/seq-registered-step-tabs)))
    (if (> (len hits) 0) (nth hits 0) nil)))

(def seq-delete-step-sequencer-tab (buffer)
  (eseq.seq-step-tabs/seq-unregister-step-sequencer-tab buffer))

(def seq-delete-script-sequencer-by-buffer (buffer)
  (let ((hits (filter (lambda (tab) (eseq.seq-step-tabs/seq-step-tab-matches-buffer? tab buffer))
                eseq.seq-step-tabs/seq-registered-step-tabs)))
    (if (> (len hits) 0)
      (let ((tab (nth hits 0))
            (name (eseq.seq-step-tabs/seq-step-tab-sequencer-name (nth hits 0)))
            (path (eseq.seq-step-tabs/seq-step-tab-source-path (nth hits 0))))
        (do
          (if (not (= name "")) (seq-unpublish-sequencer name) false)
          (if (not (= path "")) (seq-script-remove-from-scratch path) false)
          (eseq.seq-step-tabs/seq-unregister-step-sequencer-tab buffer)
          (status (fmt "Deleted sequencer tab {}" buffer))
          true))
      false)))

(def seq-delete-script-sequencer (name)
  (let ((tab (tab-for-sequencer name)))
    (do
      (seq-unpublish-sequencer name)
      (if tab
        (do
          (if (not (= (eseq.seq-step-tabs/seq-step-tab-source-path tab) ""))
            (seq-script-remove-from-scratch (eseq.seq-step-tabs/seq-step-tab-source-path tab))
            false)
          (eseq.seq-step-tabs/seq-unregister-step-sequencer-tab (eseq.seq-step-tabs/seq-step-tab-buffer tab)))
        false)
      (status (fmt "Deleted sequencer {}" name))
      true)))

(def seq-delete-script-sequencer-with-buffer (name buffer)
  (let ((hits (filter (lambda (tab) (eseq.seq-step-tabs/seq-step-tab-matches-buffer? tab buffer))
                eseq.seq-step-tabs/seq-registered-step-tabs)))
    (do
      (seq-unpublish-sequencer name)
      (if (> (len hits) 0)
        (let ((path (eseq.seq-step-tabs/seq-step-tab-source-path (nth hits 0))))
          (if (not (= path ""))
            (seq-script-remove-from-scratch path)
            false))
        false)
      (eseq.seq-step-tabs/seq-unregister-step-sequencer-tab buffer)
      (status (fmt "Deleted sequencer {}" name))
      true)))

;; Owner-side accessor for the pinned `seq-script-picker-source-buffer` (module
;; spec §10 hazard m).  ui/browser.lisp (module eseq.browser) calls this rather
;; than writing the global, so the write lands in exactly one cell no matter
;; which module the caller lives in.
(def seq-script-remember-source-buffer ()
  (set! eseq.vanilla/seq-script-picker-source-buffer (current-buffer-name)))

(def seq-script-return-to-source-buffer ()
  (if (not (= eseq.vanilla/seq-script-picker-source-buffer ""))
    (switch-to-buffer eseq.vanilla/seq-script-picker-source-buffer)
    (switch-to-buffer "*sequencer*")))

(def seq-script-load-file (path)
  (do
    (seq-script-reset-contract)
    (let ((load-result (load path)))
      (if (string-starts-with? (str load-result) "load:")
        (status (str load-result))
        (do
          (seq-script-append-to-scratch path)
          (eseq.vanilla/script-init-fn)
          (seq-script-register-loaded-tab-from-path path)
          (seq-script-register-source-tab-from-path path)
          (seq-script-return-to-source-buffer)
          (status (fmt "Loaded script {}" (path-filename path))))))))

(def picker-open-entry (entry)
  (let ((name (get entry :name))
        (is-dir (get entry :directory)))
    (if is-dir
      (do
        (set! eseq.vanilla/seq-script-picker-current-dir
          (path-join eseq.vanilla/seq-script-picker-current-dir name))
        (seq-script-picker-refresh))
      (seq-script-load-file (path-join eseq.vanilla/seq-script-picker-current-dir name)))))

(def seq-script-picker-open-at-point ()
  (let ((entry (picker-current-entry)))
    (if (= entry :parent)
      (seq-script-picker-up)
      (if entry
        (picker-open-entry entry)
        (status "No script on this line")))))

(def seq-script-picker-quit ()
  (if (not (= eseq.vanilla/seq-script-picker-source-buffer ""))
    (switch-to-buffer eseq.vanilla/seq-script-picker-source-buffer)
    (switch-to-buffer "*sequencer*")))

(def seq-script-picker ()
  (do
    (set! eseq.vanilla/seq-script-picker-source-buffer (current-buffer-name))
    (set! eseq.vanilla/seq-script-picker-current-dir (seq-script-default-dir))
    (switch-or-create-buffer "*scripts*")
    (set-view-mode "text")
    (set-buffer-mode "seq-script-picker-mode")
    (seq-script-picker-refresh)))

(bind-key "C-c s" "seq-script-picker")
