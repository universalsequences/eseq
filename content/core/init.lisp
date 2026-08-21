;; init.lisp — loaded at editor startup
;; Define commands and key bindings here.

;; Evaluate the s-expression at the cursor and show the result in the minibuffer.
;; (eval ...) is a native that schedules the string to run after this handler returns.
(def eval-sexp ()
  (let ((form (s-expression-at-cursor)))
    (if (= form "")
      (status "No s-expression at cursor")
      (let ((result (eval form)))
        (host-command "sync-current-buffer" true)
        result))))

(def eval-buffer-command ()
  (eval-current-buffer))

(def save-current-buffer ()
  (save-buffer))

(def compile-instrument ()
  (status "Compiling instrument...")
  (host-command "compile-instrument"
    (dict :source (current-buffer-text)
          :path (current-buffer-path))))

(def compile-effect ()
  (status "Compiling effect...")
  (host-command "compile-effect"
    (dict :source (current-buffer-text)
          :path (current-buffer-path))))

(def compile-current ()
  (status "Compiling...")
  (host-command "compile-current"
    (dict :source (current-buffer-text)
          :path (current-buffer-path))))

(def every (unit interval form)
  (host-command "register-hook"
    (dict :unit unit
          :interval interval
          :callback form)))

(def clear-hooks ()
  (host-command "clear-hooks"))

;; Switch to a buffer by name, creating it if it doesn't exist
(def switch-or-create-buffer (name)
  (let ((bufs (buffer-list))
        (exists (reduce |acc b| (if (= b name) true acc) false bufs)))
    (if exists
      (switch-to-buffer name)
      (create-buffer name))))

;; ── Themes ─────────────────────────────────────────────────────────────────
(load "./themes.lisp")

(bind-key "C-x C-e" "eval-sexp")
(bind-key "C-x C-b" "eval-buffer-command")
(bind-key "C-x C-s" "save-current-buffer")
(bind-key "C-c C-k" "compile-current")
(bind-key "C-x d" "dired-here")

;; ── Dired mode ──────────────────────────────────────────────────────────────
;; A simple file browser inspired by Emacs dired.

(def dired-current-dir "")
(def dired-entries '())

(define-mode "dired-mode" :read-only true)
(mode-bind-key "dired-mode" "g" "dired-refresh")
(mode-bind-key "dired-mode" "q" "dired-quit")
(mode-bind-key "dired-mode" "-" "dired-up")
(mode-bind-key "dired-mode" "RET" "dired-open-at-point")

(def style-fg (line start end color)
  (dict :line line :start start :end end :fg color))

(def style-bg-current-line (color)
  (dict :current-line true :full-line true :bg color))

(def style-bold-fg (line start end color)
  (dict :line line :start start :end end :fg color :bold true))

(def dired-row-styles (line is-dir)
  (append
    (list
      (style-fg line 0 10 :fg-muted)
      (style-fg line 11 13 :fg-muted)
      (style-fg line 14 20 :fg-muted)
      (style-fg line 21 33 :fg-muted))
    (list
      (if is-dir
        (style-bold-fg line 34 200 :blue)
        (style-fg line 34 200 :fg)))))

(def dired-entry-styles (entries line)
  (if (empty? entries)
    '()
    (append
      (dired-row-styles line (get (first entries) :directory))
      (dired-entry-styles (rest entries) (+ line 1)))))

(def dired-styles ()
  (append
    (list
      (style-bg-current-line :widget-focus-bg)
      (style-bold-fg 0 0 1 :yellow)
      (style-bold-fg 0 2 200 :blue))
    (dired-row-styles 2 true)
    (dired-entry-styles dired-entries 3)))

;; Open the dired entry under the text cursor.
(def dired-open-entry (entry)
  (let ((name (get entry :name))
        (is-dir (get entry :directory)))
    (if is-dir
      (do
        (set! dired-current-dir (path-join dired-current-dir name))
        (dired-refresh))
      (open-file (path-join dired-current-dir name)))))

(def dired-current-entry ()
  (let ((line (current-line-number)))
    (if (= line 3)
      :parent
      (if (>= line 4)
        (nth dired-entries (- line 4))
        nil))))

(def dired-format-entry (entry)
  (get entry :display))

;; Refresh the *dired* buffer with a plain text directory listing.
(def dired-refresh ()
  (let ((entries (list-directory dired-current-dir))
        (dirs (filter |e| (get e :directory) entries))
        (files (filter |e| (not (get e :directory)) entries)))
    (set! dired-entries (append dirs files))
    (let ((lines (append
                   (list (str "∨ > " dired-current-dir)
                         ""
                         "drwxr-xr-x  1      0            .. ../")
                   (map dired-format-entry dired-entries))))
      (render-widget nil)
      (set-buffer-lines lines)
      (set-buffer-styles (dired-styles))
      (goto-line 3)
      (status (fmt "{} entries" (len dired-entries))))))

;; Navigate to parent directory
(def dired-up ()
  (let ((parent (path-parent dired-current-dir)))
    (if (= parent nil)
      (status "Already at root")
      (do
        (set! dired-current-dir parent)
        (dired-refresh)))))

(def dired-open-at-point ()
  (let ((entry (dired-current-entry)))
    (if (= entry :parent)
      (dired-up)
      (if entry
        (dired-open-entry entry)
        (status "No entry on this line")))))

;; Close dired and switch to previous buffer
(def dired-quit ()
  (let ((bufs (buffer-list)))
    (if (> (len bufs) 1)
      (switch-to-buffer (nth bufs 0))
      (status "No other buffer"))))

;; Entry point: open dired in current directory
(def dired-here ()
  (let ((dir (current-directory)))
    (set! dired-current-dir dir)
    (switch-or-create-buffer "*dired*")
    (set-view-mode "text")
    (set-buffer-mode "dired-mode")
    (dired-refresh)))

;; ── Buffer List mode ────────────────────────────────────────────────────────
;; Shows all open buffers and lets you switch between them.

(def buflist-source-buffer "")
(def buflist-filter "")

(define-mode "buffer-list-mode" :read-only true :on-key "buflist-handle-key")
(mode-bind-key "buffer-list-mode" "q" "buflist-quit")
(mode-bind-key "buffer-list-mode" "g" "buflist-refresh")
(mode-bind-key "buffer-list-mode" "RET" "buflist-open-at-point")

(def buflist-entry-styles (bufs line)
  (if (empty? bufs)
    '()
    (append
      (list
        (style-fg line 0 1 :blue)
        (style-bold-fg line 2 200 :fg))
      (buflist-entry-styles (rest bufs) (+ line 1)))))

(def buflist-styles ()
  (append
    (list
      (style-bold-fg 0 0 10 :status-accent)
      (style-bold-fg 0 11 200 :syn-number)
      (style-bg-current-line :comp-selected-bg))
    (buflist-entry-styles (buflist-visible-buffers) 1)))

(def buflist-visible-buffers ()
  (let ((bufs (buffer-list))
        (query (string-downcase buflist-filter)))
    (if (= query "")
      (filter |name| (not (= name "*buffers*")) bufs)
      (filter |name|
        (and (not (= name "*buffers*"))
             (string-contains? (string-downcase name) query))
        bufs))))

(def buflist-format-entry (name)
  (let ((is-current (= name buflist-source-buffer))
        (prefix (if is-current "> " "  ")))
    (str prefix name)))

(def buflist-current-buffer-name ()
  (let ((line (current-line-number)))
    (if (>= line 2)
      (nth (buflist-visible-buffers) (- line 2))
      nil)))

(def buflist-refresh ()
  (let ((bufs (buflist-visible-buffers))
        (lines (append
                 (list (fmt "Switch to: {}" buflist-filter))
                 (map buflist-format-entry bufs))))
    (render-widget nil)
    (set-buffer-lines lines)
    (set-buffer-styles (buflist-styles))
    (goto-line (if (> (len bufs) 0) 2 1))
    (status (fmt "{} buffers" (len bufs)))))

(def buflist-handle-key (key text)
  (if text
    (do
      (set! buflist-filter (str buflist-filter text))
      (buflist-refresh)
      true)
    (if (= key "BS")
      (do
        (if (> (len buflist-filter) 0)
          (set! buflist-filter (substring buflist-filter 0 (- (len buflist-filter) 1)))
          nil)
        (buflist-refresh)
        true)
      (if (= key "ESC")
        (do
          (set! buflist-filter "")
          (buflist-refresh)
          true)
        false))))

(def buflist-open-at-point ()
  (let ((name (buflist-current-buffer-name)))
    (if name
      (switch-to-buffer name)
      (status "No buffer on this line"))))

(def buflist-quit ()
  (if (not (= buflist-source-buffer ""))
    (switch-to-buffer buflist-source-buffer)
    (let ((bufs (buffer-list)))
      (if (> (len bufs) 1)
        (switch-to-buffer (nth bufs 0))
        (status "No other buffer")))))

(def buffer-list-here ()
  (set! buflist-source-buffer (current-buffer-name))
  (set! buflist-filter "")
  (switch-or-create-buffer "*buffers*")
  (set-view-mode "text")
  (set-buffer-mode "buffer-list-mode")
  (buflist-refresh))

(bind-key "C-x b" "buffer-list-here")

(bind-key "C-x v" "cycle-view-mode")

(mac-osx-theme)

(mac-osx-theme)
