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
(mode-bind-key "dired-mode" "RET" "dired-open-at-cursor")
(mode-bind-key "dired-mode" "g" "dired-refresh")
(mode-bind-key "dired-mode" "q" "dired-quit")
(mode-bind-key "dired-mode" "-" "dired-up")

;; Format a single directory entry for display
(def dired-format-size (size)
  (if (> size 1048576)
    (fmt "{:.1}M" (/ size 1048576))
    (if (> size 1024)
      (fmt "{:.0}K" (/ size 1024))
      (fmt "{}" size))))

(def dired-format-entry (entry)
  (let ((name (get entry :name))
        (is-dir (get entry :directory))
        (size (get entry :size)))
    (if is-dir
      (str "  d  " name "/")
      (str "     " name))))

;; Refresh the *dired* buffer with the contents of dired-current-dir
(def dired-refresh ()
  (let ((entries (list-directory dired-current-dir))
        (dirs (filter |e| (get e :directory) entries))
        (files (filter |e| (not (get e :directory)) entries)))
    ;; Store entries for cursor-based lookup (line 1 = header, line 2 = ..)
    (set! dired-entries
      (append
        (map |e| (merge e :display-name (str (get e :name) "/")) dirs)
        (map |e| (merge e :display-name (get e :name)) files)))
    ;; Build display lines
    (let ((header (str "  " dired-current-dir ":"))
          (parent "  d  ../")
          (dir-lines (map dired-format-entry dirs))
          (file-lines (map dired-format-entry files)))
      (set-buffer-lines
        (cons header
          (cons parent
            (append dir-lines file-lines))))
      (goto-line 2)
      (status (fmt "{} entries" (len dired-entries))))))

;; Open the file or directory under the cursor
(def dired-open-at-cursor ()
  (let ((l (current-line-number)))
    (if (= l 1)
      (status "Header line")
      (if (= l 2)
        (dired-up)
        (if (>= (- l 3) (len dired-entries))
          (status "No entry at cursor")
          (let ((entry (nth dired-entries (- l 3))))
            (let ((name (get entry :name))
                  (is-dir (get entry :directory))
                  (full-path (path-join dired-current-dir (get entry :name))))
              (if is-dir
                (do
                  (set! dired-current-dir full-path)
                  (dired-refresh))
                (open-file full-path)))))))))

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
