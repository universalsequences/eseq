;; mac-osx-dark — macOS-inspired dark theme
;; Modeled after Finder/Notes dark mode

(apply-theme (dict
  ;; Main editor
  :bg             '(0.11 0.11 0.12)     ; #1c1c1e — System dark bg
  :fg             '(0.88 0.88 0.89)     ; #e0e0e3
  :fg-muted       '(0.56 0.56 0.58)     ; #8e8e93 — System gray
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
  :border-active   '(0.00 0.48 0.95)     ; System blue for active tile
  :border-inactive '(0.05 0.05 0.06)     ; Near-black, like status edge

  ;; Widgets
  :widget-focus-bg      '(0.00 0.35 0.82 1) ; Selection blue
  :widget-label-fg      '(0.88 0.88 0.89)   ;
  :widget-slider-filled '(0.00 0.48 0.95)   ;
  :widget-slider-track  '(0.25 0.25 0.27)   ;
))
