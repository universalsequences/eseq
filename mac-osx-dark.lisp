;; mac-osx-dark — macOS-inspired dark theme
;; Modeled after Finder/Notes dark mode

(def mac-osx-theme () 
  (apply-theme (dict
      ;; Main editor
      :bg             '(0.14 0.14 0.14)     ; #1c1c1e — System dark bg
      :fg             '(0.88 0.88 0.89)     ; #e0e0e3
      :fg-muted       '(0.56 0.56 0.58)     ; #8e8e93 — System gray
      :dim            '(0.66 0.66 0.68)     ; Secondary sequencer text
      :black          '(0.07 0.07 0.07)     ;
      :white          '(0.92 0.92 0.94)     ;
      :bright-black   '(0.30 0.30 0.32)     ; Subtle separators
      
      ;; Accent colors
      :blue           '(0.00 0.48 0.95)     ; #007AFF — System blue
      :accent         '(0.00 0.48 0.95)     ; System blue
      :green          '(0.20 0.78 0.35)     ; #32C759 — System green
      :red            '(1.00 0.23 0.19)     ; #FF3B30 — System red
      :yellow         '(1.00 0.80 0.00)     ; #FFCC00 — System yellow
      :cyan           '(0.35 0.78 0.98)     ; #5AC8FA — System teal
      :magenta        '(0.69 0.32 0.87)     ; #AF52DE — System purple
      :purple         '(0.69 0.32 0.87)     ;
      
      ;; Cursor
      :cursor         '(0.00 0.48 0.95)     ; System blue
      
      ;; Syntax
      :syn-comment    '(0.42 0.42 0.44)     ;
      :syn-string     '(0.99 0.42 0.36)     ; Warm red for strings
      :syn-number     '(0.85 0.65 0.33)     ; Warm gold
      :syn-keyword    '(0.80 0.50 0.90)     ; Purple keywords
      :syn-builtin    '(0.00 0.48 0.95)     ; Blue builtins
      :syn-special    '(0.35 0.78 0.98)     ; Teal
      :syn-delimiter  '(0.40 0.40 0.42)     ;
      
      ;; Selection / region
      :bg-region      '(0.00 0.35 0.82 1)   ; System selection blue
      :bg-sexp        '(0.13 0.14 0.17 1)   ;
      :bg-eval-flash  '(0.00 0.48 0.95 0.15) ;
      :bg-match-paren '(0.00 0.48 0.95)     ;
      :fg-match-paren '(1.00 1.00 1.00)     ;
      
      ;; Buffer/panel surfaces
      :buffer-bg        '(0.18 0.18 0.180)  ; Main rounded buffer surface

      ;; Status bar — Xcode-style dark gray bar with black borders
      :status-fg         '(0.58 0.58 0.60)   ; Muted text
      :status-bg         '(0.14 0.14 0.15)   ; Dark gray bar
      :status-edge       '(0.05 0.05 0.06)   ; Near-black border
      :status-chip-bg    '(0.14 0.14 0.15)   ; Same as bar
      :status-mode-bg    '(0.18 0.18 0.19)   ; Slightly lighter chip
      :status-chip-muted '(0.12 0.12 0.13)   ;
      :status-ui-bg      '(0.14 0.14 0.15)   ; No blue — match bar
      :status-ui-fg      '(0.58 0.58 0.60)   ;
      :status-mix-bg     '(0.14 0.14 0.15)   ;
      :status-mix-fg     '(0.58 0.58 0.60)   ;
      :status-dirty-bg   '(0.18 0.16 0.12)   ; Subtle warm for unsaved
      :status-dirty-fg   '(0.80 0.70 0.50)   ;
      :status-pos-bg     '(0.14 0.14 0.15)   ;
      :status-accent     '(0.58 0.58 0.60)   ; Gray accent, not blue
      
      ;; Tile borders
      :border-active   '(0.42 0.42 0.44)     ; Lighter gray for active tile
      :border-inactive '(0.07 0.07 0.075)    ; Match buffer background
      :tree-row-alt-bg '(0.225 0.225 0.22)   ; Subtle tree zebra stripe

      ;; Sequencer panels
      :fx-panel-bg       '(0.21 0.21 0.21)
      :fx-inner-panel-bg '(0.17 0.17 0.175)
      :fx-panel-selected-bg '(0.21 0.21 0.21)
      :fx-panel-header-bg '(0.27 0.27 0.27)
      :fx-panel-header-selected-bg '(0.32 0.32 0.63)
      :fx-panel-border   '(0.26 0.26 0.27)
      :instrument-panel-bg '(0.22 0.22 0.23)
      :instrument-control-bg '(0.04 0.04 0.045)
      :instrument-group-bg '(0.23 0.23 0.235)
      :instrument-group-selected-bg '(0.28 0.285 0.28)
      :mixer-strip-bg    '(0.20 0.20 0.20)
      :mixer-strip-selected-bg '(0.23 0.23 0.26)
      :mixer-strip-muted-bg '(0.095 0.095 0.10)
      :mixer-strip-border '(0.14 0.15 0.16)
      :mixer-strip-selected-border '(0.92 0.92 0.94)
      :mixer-control-bg  '(0.07 0.075 0.08)
      :mixer-label-bg    '(0.13 0.13 0.14)
      :mixer-label-muted-bg '(0.09 0.09 0.09)
      :button-primary-bg '(0.00 0.48 0.95)
      :button-primary-fg '(0.96 0.96 0.98)
      :button-secondary-bg '(0.22 0.23 0.25)
      :button-secondary-fg '(0.94 0.94 0.96)
      :button-ghost-bg   '(0.16 0.165 0.18)
      :button-ghost-fg   '(0.94 0.94 0.96)
      :button-danger-bg  '(1.00 0.23 0.19)
      :button-danger-fg  '(0.96 0.96 0.98)
      :dropdown-bg       '(0.22 0.23 0.25)
      :dropdown-fg       '(0.94 0.94 0.96)
      :dropdown-ring     '(0.00 0.48 0.95)
      :dropdown-chevron  '(0.96 0.96 0.98)
      :dropdown-badge-bg '(0.00 0.48 0.95)
      :dropdown-menu-bg  '(0.16 0.165 0.18)
      :dropdown-menu-border '(0.36 0.36 0.38)
      :dropdown-hover-bg '(0.00 0.35 0.82)
      :dropdown-check    '(0.96 0.96 0.98)
      :dropdown-scrollbar '(1.00 1.00 1.00 0.25)
      
      ;; Widgets
      :widget-focus-bg      '(0.00 0.35 0.82 1) ; Selection blue
      :widget-label-fg      '(0.88 0.88 0.89)   ;
      :widget-slider-filled '(0.00 0.48 0.95)   ;
      :widget-slider-track  '(0.25 0.25 0.27)   ;
      :widget-slider-dot    '(0.36 0.36 0.38)   ;
      :widget-knob-filled   '(0.00 0.48 0.95)   ;
      :widget-knob-track    '(0.04 0.04 0.04)   ;
      )))
