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
