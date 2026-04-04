;; themes.lisp — color themes for the editor
;; Load from init.lisp via (load "./themes.lisp")

(def light-theme ()
  (apply-theme (dict
    :bg "#fafafa" :fg "#1a1a2e" :fg_muted "#8b8fa0"
    :black "#f0f0f3" :red "#d1345b" :green "#2e8555"
    :yellow "#9a6700" :blue "#0969da" :magenta "#8250df"
    :cyan "#0c7d9d" :white "#1a1a2e"
    :bright_black "#9ca0b0" :bright_red "#cf222e" :bright_yellow "#9a6700"
    :purple "#8250df" :cursor "#0969da"
    :syn_comment "#8b8fa0" :syn_string "#2e8555" :syn_number "#9a6700"
    :syn_keyword "#8250df" :syn_builtin "#0969da" :syn_special "#0c7d9d"
    :syn_delimiter "#8b8fa0"
    :bg_region "#ddf4ff" :bg_sexp "#f3f4f6" :bg_eval_flash "#dafbe1"
    :bg_match_paren "#0969da" :fg_match_paren "#ffffff"
    :status_fg "#57606a" :status_bg "#f6f8fa" :status_edge "#d0d7de"
    :status_chip_bg "#eaeef2" :status_mode_bg "#ddf4ff"
    :status_chip_muted "#eaeef2"
    :status_ui_bg "#0969da" :status_ui_fg "#ffffff"
    :status_mix_bg "#0969da" :status_mix_fg "#ffffff"
    :status_dirty_bg "#9a6700" :status_dirty_fg "#ffffff"
    :status_pos_bg "#eaeef2" :status_accent "#0c7d9d"
    :comp_selected_bg "#ddf4ff" :comp_unselected_bg "#f6f8fa"
    :comp_fg "#1a1a2e" :comp_doc_bg "#f6f8fa"
    :comp_doc_fg "#1a1a2e" :comp_doc_title_fg "#0969da"
    :widget_focus_bg "#ddf4ff"
    :widget_label_fg "#1a1a2e"
    :widget_slider_filled "#0969da" :widget_slider_track "#d0d7de"
    :widget_knob_filled "#8250df" :widget_knob_track "#d0d7de"
    :widget_toggle_on "#0969da" :widget_toggle_off "#8b8fa0"
    :widget_toggle_knob_on "#ffffff" :widget_toggle_knob_off "#f6f8fa"
    :border_active "#8b8fa0" :border_inactive "#d0d7de"))
  (status "Light theme applied"))

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
    :widget_toggle_knob_on "#c0caf5" :widget_toggle_knob_off "#c0caf5"
    :border_active "#7aa2f7" :border_inactive "#292e42"))
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
    :widget_toggle_knob_on "#ffffff" :widget_toggle_knob_off "#edecee"
    :border_active "#a277ff" :border_inactive "#3b334b"))
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
    :widget_toggle_knob_on "#ffffff" :widget_toggle_knob_off "#c8d8f0"
    :border_active "#4a9ef5" :border_inactive "#162640"))
  (status "Aqua Dark theme applied"))

(def mac-osx-theme ()
  (apply-theme (dict
      :bg             '(0.11 0.11 0.12)
      :fg             '(0.88 0.88 0.89)
      :fg-muted       '(0.56 0.56 0.58)
      :black          '(0.07 0.07 0.07)
      :white          '(0.92 0.92 0.94)
      :bright-black   '(0.45 0.45 0.45)
      :blue           '(0.00 0.48 0.95)
      :accent         '(0.00 0.48 0.95)
      :green          '(0.20 0.78 0.35)
      :red            '(1.00 0.23 0.19)
      :yellow         '(1.00 0.80 0.00)
      :cyan           '(0.35 0.78 0.98)
      :magenta        '(0.69 0.32 0.87)
      :purple         '(0.69 0.32 0.87)
      :cursor         '(0.00 0.48 0.95)
      :syn-comment    '(0.42 0.42 0.44)
      :syn-string     '(0.99 0.42 0.36)
      :syn-number     '(0.85 0.65 0.33)
      :syn-keyword    '(0.80 0.50 0.90)
      :syn-builtin    '(0.00 0.48 0.95)
      :syn-special    '(0.35 0.78 0.98)
      :syn-delimiter  '(0.40 0.40 0.42)
      :bg-region      '(0.00 0.35 0.82 1)
      :bg-sexp        '(0.13 0.14 0.17 1)
      :bg-eval-flash  '(0.00 0.48 0.95 0.15)
      :bg-match-paren '(0.00 0.48 0.95)
      :fg-match-paren '(1.00 1.00 1.00)
      :status-fg         '(0.58 0.58 0.60)
      :status-bg         '(0.14 0.14 0.15)
      :status-edge       '(0.05 0.05 0.06)
      :status-chip-bg    '(0.14 0.14 0.15)
      :status-mode-bg    '(0.18 0.18 0.19)
      :status-chip-muted '(0.12 0.12 0.13)
      :status-ui-bg      '(0.14 0.14 0.15)
      :status-ui-fg      '(0.58 0.58 0.60)
      :status-mix-bg     '(0.14 0.14 0.15)
      :status-mix-fg     '(0.58 0.58 0.60)
      :status-dirty-bg   '(0.18 0.16 0.12)
      :status-dirty-fg   '(0.80 0.70 0.50)
      :status-pos-bg     '(0.14 0.14 0.15)
      :status-accent     '(0.58 0.58 0.60)
      :border-active   '(0.00 0.48 0.95)
      :border-inactive '(0.05 0.05 0.06)
      :widget-focus-bg      '(0.00 0.35 0.82 1)
      :widget-label-fg      '(0.88 0.88 0.89)
      :widget-slider-filled '(0.00 0.48 0.95)
      :widget-slider-track  '(0.25 0.25 0.27)))
  (status "macOS Dark theme applied"))
