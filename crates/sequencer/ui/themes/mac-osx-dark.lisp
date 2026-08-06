;; mac-osx-dark — macOS-inspired dark theme
;; Modeled after Finder/Notes dark mode

(def mac-osx-theme () 
  (apply-theme (dict
      ;; Main editor
      :bg             '(0.10 0.10 0.12)     ; #1c1c1e — System dark bg
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
      :buffer-bg        '(0.12 0.13 0.14)  ; Main rounded buffer surface
      :buffer-tab-bar-bg '(0.14 0.14 0.15)
      :buffer-tab-selected-bg '(0.28 0.28 0.30)
      :buffer-tab-selected-border '(0.43 0.43 0.46)
      :buffer-tab-fg '(0.52 0.52 0.55)
      :buffer-tab-selected-fg '(0.92 0.92 0.94)
      :buffer-tab-selected-highlight '(1.00 1.00 1.00 0.16)
      :buffer-tab-selected-shadow '(0.00 0.00 0.00 0.22)
      
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
      :tree-row-alt-bg '(0.14 0.15 0.16)   ; Subtle tree zebra stripe
      
      ;; Sequencer panels
      :fx-panel-bg       '(0.17 0.17 0.18)
      :fx-inner-panel-bg '(0.15 0.15 0.155)
      :fx-panel-selected-bg '(0.17 0.17 0.17)
      :fx-panel-header-bg '(0.21 0.21 0.22)
      :fx-panel-header-selected-bg '(0.32 0.32 0.63)
      :fx-panel-border   '(0.26 0.26 0.27)
      :instrument-panel-bg '(0.21 0.21 0.22)
      :instrument-control-bg '(0.04 0.04 0.045)
      :instrument-group-bg '(0.15 0.15 0.155)
      :instrument-group-selected-bg '(0.11 0.115 0.11)
      :mixer-strip-bg    '(0.16 0.16 0.17)
      :mixer-strip-selected-bg '(0.24 0.24 0.25)
      :mixer-strip-muted-bg '(0.095 0.095 0.10)
      :mixer-strip-border '(0.21 0.21 0.22)
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
      :button-border     '(0.43 0.43 0.46 0.82)
      :button-highlight  '(1.00 1.00 1.00 0.16)
      :button-shadow     '(0.00 0.00 0.00 0.22)
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
      :sequencer-step-border          '(0.22 0.22 0.23)
      :sequencer-step-selected-border '(0.90 0.92 0.96)
      :sequencer-step-off-fill        '(0.055 0.058 0.065)
      :sequencer-step-off-fill-alt    '(0.25 0.255 0.26)
      :patcher-bg           '(0.08 0.08 0.08)
      :patcher-grid-minor   '(0.22 0.23 0.25 0.34)
      :patcher-grid-major   '(0.32 0.33 0.36 0.46)
      :patcher-text         '(0.88 0.88 0.89)
      :patcher-text-muted   '(0.62 0.63 0.66)
      :patcher-error        '(1.00 0.35 0.39)
      :patcher-cable        '(0.74 0.76 0.84 0.92)
      :patcher-feedback-cable '(1.00 0.62 0.12 0.88)
      :patcher-marquee-fill '(0.00 0.48 0.95 0.12)
      :patcher-marquee-border '(0.00 0.48 0.95 0.72)
      :patcher-node-bg      '(0.17 0.17 0.18)
      :patcher-node-border  '(0.32 0.32 0.39)
      :patcher-node-text    '(0.30 0.98 0.75)
      :patcher-node-tail-text '(0.92 0.92 0.94)
      :patcher-io-node-bg   '(0.17 0.17 0.18)
      :patcher-io-node-border '(0.36 0.36 0.39)
      :patcher-io-node-text '(0.20 0.78 0.35)
      :patcher-param-node-bg '(0.16 0.17 0.19)
      :patcher-param-node-border '(0.00 0.48 0.95)
      :patcher-param-node-text '(0.45 0.68 1.00)
      :patcher-code-node-bg '(0.22 0.12 0.13)
      :patcher-code-node-border '(1.00 0.35 0.39)
      :patcher-code-node-text '(0.58 0.54 0.58)
      :patcher-node-hover-border '(0.58 0.58 0.62)
      :patcher-node-selected-border '(0.00 0.48 0.95)
      :patcher-port-input   '(1.00 0.80 0.00)
      :patcher-port-output  '(1.00 0.58 0.12)
      :patcher-edit-selection '(0.00 0.48 0.95 0.35)
      :patcher-edit-cursor  '(1.00 1.00 1.00)
      :patcher-back-button-bg '(0.14 0.14 0.15)
      :patcher-back-button-hover-bg '(0.16 0.20 0.28)
      :patcher-back-button-border '(0.36 0.36 0.39)
      :patcher-back-button-hover-border '(0.00 0.48 0.95)
      :patcher-back-button-text '(0.62 0.63 0.66)
      :patcher-back-button-hover-text '(0.88 0.93 1.00)
      )))

(mac-osx-theme)
