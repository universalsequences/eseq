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
    :bg_region "#29263c" :bg_sexp "#28253c" :bg_eval_flash "#3d375e"
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
    :widget_label_fg "#edecee"
    :widget_slider_filled "#a277ff" :widget_slider_track "#6d6d6d"
    :widget_knob_filled "#a277ff" :widget_knob_track "#6d6d6d"
    :widget_toggle_on "#a277ff" :widget_toggle_off "#6d6d6d"
    :widget_toggle_knob_on "#ffffff" :widget_toggle_knob_off "#edecee"))
  (status "Aura theme applied"))

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

;; Build a single row widget for a dired entry
(def dired-entry-widget (entry)
  (let ((name (get entry :name))
        (is-dir (get entry :directory)))
    (if is-dir
      (h-stack :gap 1 :align :center :focusable true
        :on-enter (dired-entry-action entry)
        (label "▸" :color :yellow :width 2)
        (label (str name "/") :color :cyan))
      (h-stack :gap 1 :align :center :focusable true
        :on-enter (dired-entry-action entry)
        (label " " :width 2)
        (label name)))))

;; Refresh the *dired* buffer with a widget-based directory listing
(def dired-refresh ()
  (let ((entries (list-directory dired-current-dir))
        (dirs (filter |e| (get e :directory) entries))
        (files (filter |e| (not (get e :directory)) entries)))
    (set! dired-entries (append dirs files))
    (let ((header (label (str dired-current-dir ":")
                    :color :dim))
          (parent (h-stack :gap 1 :align :center :focusable true
                    :on-enter (lambda () (dired-up))
                    (label "▸" :color :purple :width 2)
                    (label "../" :color :purple)))
          (dir-widgets (map dired-entry-widget dirs))
          (file-widgets (map dired-entry-widget files))
          (all-widgets (cons header
                         (cons parent
                           (append dir-widgets file-widgets)))))
      (render-widget (v-stack :font-size 18 :gap 0.125 :padding 0.5 :align :stretch all-widgets))
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
  (let ((bufs (buffer-list)))
    (if (> (len bufs) 1)
      (switch-to-buffer (nth bufs 0))
      (status "No other buffer"))))

;; Entry point: open dired in current directory
(def dired-here ()
  (let ((dir (current-directory)))
    (set! dired-current-dir dir)
    (switch-or-create-buffer "*dired*")
    (set-buffer-mode "dired-mode")
    (dired-refresh)))

;; ── Buffer List mode ────────────────────────────────────────────────────────
;; Shows all open buffers and lets you switch between them.

(def buflist-source-buffer "")

(define-mode "buffer-list-mode" :read-only true)
(mode-bind-key "buffer-list-mode" "q" "buflist-quit")
(mode-bind-key "buffer-list-mode" "g" "buflist-refresh")

(def buflist-make-entry (name)
  (let ((is-current (= name buflist-source-buffer))
        (prefix (if is-current " > " "   "))
        (color (if is-current :yellow :white)))
    (label (str prefix name)
      :width 80
      :color color
      :focusable true
      :on-enter (lambda ()
                  (switch-to-buffer name)))))

(def buflist-refresh ()
  (let ((bufs (buffer-list))
        (header (label "  Buffers:"
                  :color :dim
                  :width 80))
        (entries (map buflist-make-entry bufs))
        (all-widgets (cons header entries)))
    (render-widget (v-stack all-widgets))
    (status (fmt "{} buffers" (len bufs)))))

(def buflist-quit ()
  (if (not (= buflist-source-buffer ""))
    (switch-to-buffer buflist-source-buffer)
    (let ((bufs (buffer-list)))
      (if (> (len bufs) 1)
        (switch-to-buffer (nth bufs 0))
        (status "No other buffer")))))

(def buffer-list-here ()
  (set! buflist-source-buffer (current-buffer-name))
  (switch-or-create-buffer "*buffers*")
  (set-buffer-mode "buffer-list-mode")
  (buflist-refresh))

(bind-key "C-x b" "buffer-list-here")

;; dired-open-at-cursor is no longer needed — Enter triggers :on-enter on focused widget
(bind-key "C-x v" "cycle-view-mode")
