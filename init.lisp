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

;; Switch to a buffer by name, creating it if it doesn't exist
(def switch-or-create-buffer (name)
  (let ((bufs (buffer-list))
        (exists (reduce |acc b| (if (= b name) true acc) false bufs)))
    (if exists
      (switch-to-buffer name)
      (create-buffer name))))

;; ── Theme helpers ───────────────────────────────────────────────────────────

(def light-theme ()
  (apply-theme (dict
    :bg "#e1e2e7" :fg "#3760bf" :fg_muted "#848cb5"
    :black "#dcdcde" :red "#f52a65" :green "#587539"
    :yellow "#8c6c3e" :blue "#2e7de9" :magenta "#9854f1"
    :cyan "#007197" :white "#3760bf"
    :bright_black "#a1a6c5" :bright_red "#c64343" :bright_yellow "#a27629"
    :purple "#7847bd" :cursor "#2e7de9"
    :syn_comment "#848cb5" :syn_string "#587539" :syn_number "#b15c00"
    :syn_keyword "#9854f1" :syn_builtin "#2e7de9" :syn_special "#007197"
    :syn_delimiter "#8990b3"
    :bg_region "#b6d6fd" :bg_sexp "#d7d8df" :bg_eval_flash "#c4d6b0"
    :bg_match_paren "#2e7de9" :fg_match_paren "#e1e2e7"
    :status_fg "#3760bf" :status_bg "#dcdcde" :status_edge "#c4c8da"
    :status_chip_bg "#d5d6db" :status_mode_bg "#cbdaf6"
    :status_chip_muted "#d9dadf"
    :status_ui_bg "#2e7de9" :status_ui_fg "#e1e2e7"
    :status_mix_bg "#2e7de9" :status_mix_fg "#e1e2e7"
    :status_dirty_bg "#b15c00" :status_dirty_fg "#f7efe4"
    :status_pos_bg "#d5d6db" :status_accent "#007197"
    :comp_selected_bg "#cbdaf6" :comp_unselected_bg "#dcdcde"
    :comp_fg "#3760bf" :comp_doc_bg "#d5d6db"
    :comp_doc_fg "#3760bf" :comp_doc_title_fg "#2e7de9"
    :widget_focus_bg "#cbdaf6"
    :widget_label_fg "#3760bf"
    :widget_slider_filled "#2e7de9" :widget_slider_track "#a8aecb"
    :widget_knob_filled "#9854f1" :widget_knob_track "#a8aecb"
    :widget_toggle_on "#2e7de9" :widget_toggle_off "#8990b3"
    :widget_toggle_knob_on "#ffffff" :widget_toggle_knob_off "#f1f3f7"))
  (status "TokyoNight Day theme applied"))

(def tokyonight-storm-theme ()
  (apply-theme (dict
    :bg "#24283b" :fg "#c0caf5" :fg_muted "#565f89"
    :black "#1f2335" :red "#f7768e" :green "#9ece6a"
    :yellow "#e0af68" :blue "#7aa2f7" :magenta "#bb9af7"
    :cyan "#7dcfff" :white "#c0caf5"
    :bright_black "#545c7e" :bright_red "#db4b4b" :bright_yellow "#ff9e64"
    :purple "#9d7cd8" :cursor "#7aa2f7"
    :syn_comment "#565f89" :syn_string "#9ece6a" :syn_number "#ff9e64"
    :syn_keyword "#bb9af7" :syn_builtin "#7aa2f7" :syn_special "#7dcfff"
    :syn_delimiter "#545c7e"
    :bg_region "#292e42" :bg_sexp "#292e42" :bg_eval_flash "#394b70"
    :bg_match_paren "#7aa2f7" :fg_match_paren "#1f2335"
    :status_fg "#c0caf5" :status_bg "#1f2335" :status_edge "#292e42"
    :status_chip_bg "#292e42" :status_mode_bg "#394b70"
    :status_chip_muted "#292e42"
    :status_ui_bg "#7aa2f7" :status_ui_fg "#1f2335"
    :status_mix_bg "#7aa2f7" :status_mix_fg "#1f2335"
    :status_dirty_bg "#db4b4b" :status_dirty_fg "#c0caf5"
    :status_pos_bg "#292e42" :status_accent "#7dcfff"
    :comp_selected_bg "#292e42" :comp_unselected_bg "#1f2335"
    :comp_fg "#c0caf5" :comp_doc_bg "#1b1e2d"
    :comp_doc_fg "#c0caf5" :comp_doc_title_fg "#7aa2f7"
    :widget_focus_bg "#394b70"
    :widget_label_fg "#c0caf5"
    :widget_slider_filled "#7aa2f7" :widget_slider_track "#394b70"
    :widget_knob_filled "#bb9af7" :widget_knob_track "#545c7e"
    :widget_toggle_on "#7aa2f7" :widget_toggle_off "#545c7e"
    :widget_toggle_knob_on "#c0caf5" :widget_toggle_knob_off "#c0caf5"))
  (status "TokyoNight Storm theme applied"))

(def aura-theme ()
  (apply-theme (dict
    :bg "#15141b" :fg "#edecee" :fg_muted "#6d6d6d"
    :black "#110f18" :red "#ff6767" :green "#61ffca"
    :yellow "#ffca85" :blue "#82e2ff" :magenta "#f694ff"
    :cyan "#61ffca" :white "#edecee"
    :bright_black "#6d6d6d" :bright_red "#ff6767" :bright_yellow "#ffca85"
    :purple "#a277ff" :cursor "#a277ff"
    :syn_comment "#6d6d6d" :syn_string "#61ffca" :syn_number "#ffca85"
    :syn_keyword "#a277ff" :syn_builtin "#82e2ff" :syn_special "#f694ff"
    :syn_delimiter "#6d6d6d"
    :bg_region "#5a4b8a" :bg_sexp "#28253c" :bg_eval_flash "#3d375e"
    :bg_match_paren "#a277ff" :fg_match_paren "#15141b"
    :status_fg "#adacae" :status_bg "#121016" :status_edge "#3b334b"
    :status_chip_bg "#2e2b38" :status_mode_bg "#3b334b"
    :status_chip_muted "#2e2b38"
    :status_ui_bg "#a277ff" :status_ui_fg "#15141b"
    :status_mix_bg "#82e2ff" :status_mix_fg "#15141b"
    :status_dirty_bg "#ff6767" :status_dirty_fg "#15141b"
    :status_pos_bg "#2e2b38" :status_accent "#61ffca"
    :comp_selected_bg "#2e2b38" :comp_unselected_bg "#15141b"
    :comp_fg "#edecee" :comp_doc_bg "#15141b"
    :comp_doc_fg "#cdccce" :comp_doc_title_fg "#a277ff"
    :widget_focus_bg "#3b334b"
    :widget_label_fg "#edecee"
    :widget_slider_filled "#a277ff" :widget_slider_track "#6d6d6d"
    :widget_knob_filled "#a277ff" :widget_knob_track "#6d6d6d"
    :widget_toggle_on "#a277ff" :widget_toggle_off "#6d6d6d"
    :widget_toggle_knob_on "#ffffff" :widget_toggle_knob_off "#edecee"))
  (status "Aura theme applied"))

(def aqua-dark-theme ()
  (apply-theme (dict
    :bg "#0a0612" :fg "#c8d8f0" :fg_muted "#5a6e8a"
    :black "#061020" :red "#ff6b8a" :green "#5ec4b0"
    :yellow "#f0c060" :blue "#4a9ef5" :magenta "#a88bfa"
    :cyan "#5ccfe6" :white "#ffffff"
    :bright_black "#3a4e6a" :bright_red "#ff8da0" :bright_yellow "#ffd080"
    :purple "#3080e0" :cursor "#4a9ef5"
    :syn_comment "#4a5e7a" :syn_string "#5ec4b0" :syn_number "#f0c060"
    :syn_keyword "#a88bfa" :syn_builtin "#4a9ef5" :syn_special "#5ccfe6"
    :syn_delimiter "#3a4e6a"
    :bg_region "#1a2e48" :bg_sexp "#1a2e48" :bg_eval_flash "#1e3a5a"
    :bg_match_paren "#4a9ef5" :fg_match_paren "#0a1628"
    :status_fg "#c8d8f0" :status_bg "#0e1a30" :status_edge "#162640"
    :status_chip_bg "#162640" :status_mode_bg "#1e3a5a"
    :status_chip_muted "#162640"
    :status_ui_bg "#2060c0" :status_ui_fg "#e0eaff"
    :status_mix_bg "#2060c0" :status_mix_fg "#e0eaff"
    :status_dirty_bg "#cc4466" :status_dirty_fg "#ffffff"
    :status_pos_bg "#162640" :status_accent "#5ccfe6"
    :comp_selected_bg "#1a2e48" :comp_unselected_bg "#0e1a30"
    :comp_fg "#c8d8f0" :comp_doc_bg "#0a1224"
    :comp_doc_fg "#c8d8f0" :comp_doc_title_fg "#4a9ef5"
    :widget_focus_bg "#1e3a5a"
    :widget_label_fg "#c8d8f0"
    :widget_slider_filled "#3080e0" :widget_slider_track "#1e3a5a"
    :widget_knob_filled "#a88bfa" :widget_knob_track "#3a4e6a"
    :widget_toggle_on "#3080e0" :widget_toggle_off "#3a4e6a"
    :widget_toggle_knob_on "#ffffff" :widget_toggle_knob_off "#c8d8f0"))
  (status "Aqua Dark theme applied"))

;; (load "file.lisp") — read and evaluate a Lisp file, like Common Lisp's load.
(def load (path)
  (eval (read-file-to-string path)))

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
      (style-fg line 11 13 :syn-number)
      (style-fg line 14 24 :green)
      (style-fg line 25 33 :green)
      (style-fg line 34 40 :syn-number)
      (style-fg line 41 53 :fg-muted))
    (list
      (if is-dir
        (style-bold-fg line 53 200 :blue)
        (style-bold-fg line 53 200 :syn-number)))))

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
                         "drwxr-xr-x  1 parent     parent      0            .. ../")
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
      bufs
      (filter |name|
        (string-contains? (string-downcase name) query)
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
