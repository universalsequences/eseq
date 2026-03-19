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
  (let ((result (eval (current-buffer-text))))
    (host-command "sync-current-buffer" true)
    result))

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

(def empty? (xs)
  (= (len xs) 0))

(def map (fn xs)
  (if (empty? xs)
    '()
    (cons (fn (first xs))
          (map fn (rest xs)))))

(def filter (fn xs)
  (if (empty? xs)
    '()
    (if (fn (first xs))
      (cons (first xs) (filter fn (rest xs)))
      (filter fn (rest xs)))))

(def reduce (fn acc xs)
  (if (empty? xs)
    acc
    (reduce fn (fn acc (first xs)) (rest xs))))

(def for-each (fn xs)
  (if (empty? xs)
    nil
    (do
      (fn (first xs))
      (for-each fn (rest xs)))))

(bind-key "C-x C-e" "eval-sexp")
(bind-key "C-x C-b" "eval-buffer-command")
(bind-key "C-x C-s" "save-current-buffer")
(bind-key "C-c C-k" "compile-current")
(bind-key "C-c C-c" "compile-current")
(bind-key "C-x d" "dired-here")

;; ── Dired mode ──────────────────────────────────────────────────────────────
;; A simple file browser inspired by Emacs dired.

(def dired-current-dir "")
(def dired-entries '())

(define-mode "dired-mode" :read-only true)
(mode-bind-key "dired-mode" "g" "dired-refresh")
(mode-bind-key "dired-mode" "q" "dired-quit")
(mode-bind-key "dired-mode" "-" "dired-up")

;; Build the action callback for a directory entry
(def dired-entry-action (entry)
  (let ((name (get entry :name))
        (is-dir (get entry :directory)))
    (if is-dir
      (lambda ()
        (set! dired-current-dir (path-join dired-current-dir name))
        (dired-refresh))
      (lambda ()
        (open-file (path-join dired-current-dir name))))))

;; Build a single label widget for a dired entry
(def dired-entry-widget (entry)
  (let ((name (get entry :name))
        (is-dir (get entry :directory)))
    (if is-dir
      (label (str "  d  " name "/")
        :width 80
        :color :cyan
        :focusable true
        :on-enter (dired-entry-action entry))
      (label (str "     " name)
        :width 80
        :focusable true
        :on-enter (dired-entry-action entry)))))

;; Refresh the *dired* buffer with a widget-based directory listing
(def dired-refresh ()
  (let ((entries (list-directory dired-current-dir))
        (dirs (filter |e| (get e :directory) entries))
        (files (filter |e| (not (get e :directory)) entries)))
    (set! dired-entries (append dirs files))
    (let ((header (label (str dired-current-dir ":")
                    :color :dim
                    :width 80))
          (parent (label "  d  ../"
                    :width 80
                    :color :purple
                    :focusable true
                    :on-enter (lambda () (dired-up))))
          (dir-widgets (map dired-entry-widget dirs))
          (file-widgets (map dired-entry-widget files))
          (all-widgets (cons header
                         (cons parent
                           (append dir-widgets file-widgets)))))
      (render-widget (v-stack all-widgets))
      (status (fmt "{} entries" (len dired-entries))))))

;; Navigate to parent directory
(def dired-up ()
  (let ((parent (path-parent dired-current-dir)))
    (if (= parent nil)
      (status "Already at root")
      (do
        (set! dired-current-dir parent)
        (dired-refresh)))))

;; Close dired and switch to previous buffer
(def dired-quit ()
  (render-widget nil)
  (let ((bufs (buffer-list)))
    (if (> (len bufs) 1)
      (switch-to-buffer (nth bufs 0))
      (status "No other buffer"))))

;; Entry point: open dired in current directory
(def dired-here ()
  (let ((dir (current-directory)))
    (set! dired-current-dir dir)
    (create-buffer "*dired*")
    (set-buffer-mode "dired-mode")
    (dired-refresh)))

;; dired-open-at-cursor is no longer needed — Enter triggers :on-enter on focused widget
