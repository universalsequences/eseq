;; mac-osx-light-theme — complete macOS-inspired light palette
;;
;; The palette follows macOS's visual hierarchy: white content surfaces,
;; softly tinted grouped backgrounds, hairline gray separators, near-black
;; text, and system blue for selection and focus. Every theme slot registered
;; by eseqlisp is specified here so switching from a dark theme cannot leak
;; stale colors into the light appearance.

(def mac-osx-light-theme ()
  (apply-theme (dict
      ;; Core canvas and semantic colors
      :accent         '(0.00 0.478 1.00)       ; #007aff — system blue
      :bg             '(0.961 0.961 0.969)     ; #f5f5f7 — grouped background
      :fg             '(0.114 0.114 0.122)     ; #1d1d1f — primary label
      :fg-muted       '(0.353 0.353 0.373)     ; Strong secondary label
      :dim            '(0.365 0.365 0.384)     ; Survives disabled-state opacity
      :black          '(0.114 0.114 0.122)
      :red            '(1.00 0.231 0.188)      ; #ff3b30
      :green          '(0.204 0.780 0.349)     ; #34c759
      :yellow         '(1.00 0.800 0.00)       ; #ffcc00
      :blue           '(0.00 0.478 1.00)       ; #007aff
      :magenta        '(0.686 0.322 0.871)     ; #af52de
      :cyan           '(0.353 0.784 0.980)     ; #5ac8fa
      :white          '(0.114 0.114 0.122)     ; ANSI white is foreground in light mode
      :bright-black   '(0.682 0.682 0.698)     ; #aeaeb2
      :bright-red     '(1.00 0.270 0.227)
      :bright-yellow  '(1.00 0.839 0.039)
      :purple         '(0.345 0.337 0.839)     ; accessible system indigo
      :cursor         '(0.00 0.478 1.00)

      ;; Editor syntax and selections
      :syn-comment    '(0.431 0.431 0.451)
      :syn-string     '(0.659 0.082 0.173)
      :syn-number     '(0.678 0.361 0.000)
      :syn-keyword    '(0.518 0.184 0.659)
      :syn-builtin    '(0.000 0.337 0.714)
      :syn-special    '(0.000 0.420 0.490)
      :syn-delimiter  '(0.557 0.557 0.576)
      :bg-region      '(0.765 0.867 1.000 1.00)
      :bg-sexp        '(0.914 0.945 0.988 1.00)
      :bg-eval-flash  '(0.204 0.780 0.349 0.20)
      :bg-match-paren '(0.00 0.478 1.00)
      :fg-match-paren '(1.00 1.00 1.00)

      ;; Window, buffer, and tab surfaces
      :buffer-bg                       '(0.992 0.992 0.996)
      :buffer-tab-bar-bg               '(0.925 0.925 0.937)
      :buffer-tab-selected-bg          '(1.00 1.00 1.00)
      :buffer-tab-selected-border      '(0.710 0.710 0.729)
      :buffer-tab-fg                   '(0.431 0.431 0.451)
      :buffer-tab-selected-fg          '(0.114 0.114 0.122)
      :buffer-tab-selected-highlight   '(1.00 1.00 1.00 0.90)
      :buffer-tab-selected-shadow      '(0.00 0.00 0.00 0.12)

      ;; Status bar and completion UI
      :status-fg         '(0.286 0.286 0.302)
      :status-bg         '(0.925 0.925 0.937)
      :status-edge       '(0.776 0.776 0.792)
      :status-chip-bg    '(0.867 0.867 0.882)
      :status-mode-bg    '(0.765 0.867 1.000)
      :status-chip-muted '(0.898 0.898 0.910)
      :status-ui-bg      '(0.00 0.478 1.00)
      :status-ui-fg      '(1.00 1.00 1.00)
      :status-mix-bg     '(0.00 0.478 1.00)
      :status-mix-fg     '(1.00 1.00 1.00)
      :status-dirty-bg   '(1.00 0.929 0.722)
      :status-dirty-fg   '(0.420 0.247 0.000)
      :status-pos-bg     '(0.867 0.867 0.882)
      :status-accent     '(0.000 0.337 0.714)
      :comp-selected-bg  '(0.765 0.867 1.000)
      :comp-unselected-bg '(0.969 0.969 0.976)
      :comp-border       '(0.710 0.710 0.729)
      :comp-fg           '(0.114 0.114 0.122)
      :comp-selected-fg  '(0.000 0.337 0.714)
      :comp-category-fg  '(0.431 0.431 0.451)
      :comp-doc-bg       '(1.00 1.00 1.00)
      :comp-doc-border   '(0.710 0.710 0.729)
      :comp-doc-fg       '(0.286 0.286 0.302)
      :comp-doc-title-fg '(0.000 0.337 0.714)

      ;; Browser, tiles, effects, instruments, and mixer
      :tree-row-alt-bg                 '(0.949 0.949 0.957)
      :fx-panel-bg                     '(0.949 0.949 0.957)
      :fx-inner-panel-bg               '(0.902 0.902 0.914)
      :fx-panel-selected-bg            '(1.00 1.00 1.00)
      :fx-panel-header-bg              '(0.914 0.914 0.925)
      :fx-panel-header-selected-bg     '(0.765 0.867 1.000)
      :fx-panel-border                 '(0.776 0.776 0.792)
      :instrument-panel-bg             '(0.949 0.949 0.957)
      :instrument-control-bg           '(0.875 0.875 0.890)
      :instrument-group-bg             '(0.914 0.914 0.925)
      :instrument-group-selected-bg    '(0.820 0.902 1.000)
      :mixer-strip-bg                  '(0.949 0.949 0.957)
      :mixer-strip-selected-bg         '(1.00 1.00 1.00)
      :mixer-strip-muted-bg            '(0.867 0.867 0.882)
      :mixer-strip-border              '(0.776 0.776 0.792)
      :mixer-strip-selected-border     '(0.00 0.478 1.00)
      :mixer-control-bg                '(0.890 0.890 0.902)
      :mixer-label-bg                  '(0.914 0.914 0.925)
      :mixer-label-muted-bg            '(0.835 0.835 0.851)

      ;; Buttons use subtle macOS-style bevel and shadow layers
      :button-primary-bg   '(0.00 0.478 1.00)
      :button-primary-fg   '(1.00 1.00 1.00)
      :button-secondary-bg '(0.902 0.902 0.914)
      :button-secondary-fg '(0.114 0.114 0.122)
      :button-ghost-bg     '(0.949 0.949 0.957)
      :button-ghost-fg     '(0.114 0.114 0.122)
      :button-danger-bg    '(1.00 0.231 0.188)
      :button-danger-fg    '(1.00 1.00 1.00)
      :button-border       '(0.600 0.600 0.620 0.60)
      :button-highlight    '(1.00 1.00 1.00 0.90)
      :button-shadow       '(0.00 0.00 0.00 0.14)

      ;; Dropdowns and inspector overlays
      :dropdown-bg          '(0.902 0.902 0.914)
      :dropdown-fg          '(0.114 0.114 0.122)
      :dropdown-ring        '(0.00 0.478 1.00)
      :dropdown-chevron     '(0.286 0.286 0.302)
      :dropdown-badge-bg    '(0.00 0.478 1.00)
      :dropdown-menu-bg     '(0.992 0.992 0.996 0.98)
      :dropdown-menu-border '(0.710 0.710 0.729)
      :dropdown-hover-bg    '(0.00 0.478 1.00)
      :dropdown-check       '(1.00 1.00 1.00)
      :dropdown-scrollbar   '(0.235 0.235 0.255 0.32)
      :inspect-overlay-fill   '(0.00 0.478 1.00 0.14)
      :inspect-overlay-border '(0.00 0.337 0.714 0.92)

      ;; Sliders, knobs, and toggles
      :widget-focus-bg       '(0.765 0.867 1.000 1.00)
      :widget-label-fg       '(0.114 0.114 0.122)
      :widget-slider-filled  '(0.00 0.478 1.00)
      :widget-slider-track   '(0.776 0.776 0.792)
      :widget-slider-dot     '(0.431 0.431 0.451)
      :widget-knob-filled    '(0.00 0.478 1.00)
      :widget-knob-track     '(0.710 0.710 0.729)
      :widget-toggle-on      '(0.204 0.780 0.349)
      :widget-toggle-off     '(0.710 0.710 0.729)
      :widget-toggle-knob-on '(1.00 1.00 1.00)
      :widget-toggle-knob-off '(1.00 1.00 1.00)
      :sequencer-step-border          '(0.58 0.58 0.60)
      :sequencer-step-selected-border '(0.00 0.478 1.00)
      :sequencer-step-off-fill        '(0.78 0.78 0.80)
      :sequencer-step-off-fill-alt    '(0.68 0.68 0.70)

      ;; Patcher canvas, nodes, cables, editing, and navigation
      :patcher-bg                    '(0.949 0.949 0.957)
      :patcher-grid-minor            '(0.431 0.431 0.451 0.16)
      :patcher-grid-major            '(0.431 0.431 0.451 0.28)
      :patcher-text                  '(0.114 0.114 0.122)
      :patcher-text-muted            '(0.431 0.431 0.451)
      :patcher-error                 '(0.780 0.080 0.075)
      :patcher-cable                 '(0.286 0.286 0.302 0.88)
      :patcher-feedback-cable        '(1.00 0.584 0.00 0.92)
      :patcher-marquee-fill          '(0.00 0.478 1.00 0.12)
      :patcher-marquee-border        '(0.00 0.337 0.714 0.78)
      :patcher-node-bg               '(1.00 1.00 1.00)
      :patcher-node-border           '(0.710 0.710 0.729)
      :patcher-node-text             '(0.086 0.486 0.180)
      :patcher-node-tail-text        '(0.114 0.114 0.122)
      :patcher-io-node-bg            '(0.925 0.925 0.937)
      :patcher-io-node-border        '(0.600 0.600 0.620)
      :patcher-io-node-text          '(0.086 0.486 0.180)
      :patcher-param-node-bg         '(0.914 0.945 0.988)
      :patcher-param-node-border     '(0.00 0.337 0.714)
      :patcher-param-node-text       '(0.00 0.286 0.620)
      :patcher-code-node-bg          '(1.00 0.925 0.925)
      :patcher-code-node-border      '(0.780 0.080 0.075)
      :patcher-code-node-text        '(0.620 0.055 0.051)
      :patcher-node-hover-border     '(0.286 0.286 0.302)
      :patcher-node-selected-border  '(0.00 0.478 1.00)
      :patcher-port-input            '(0.765 0.565 0.000)
      :patcher-port-output           '(0.835 0.365 0.000)
      :patcher-edit-selection        '(0.00 0.478 1.00 0.26)
      :patcher-edit-cursor           '(0.114 0.114 0.122)
      :patcher-alignment-guide       '(0.00 0.478 1.00 0.86)
      :patcher-tooltip-bg            '(0.992 0.992 0.996 0.98)
      :patcher-tooltip-border        '(0.710 0.710 0.729)
      :patcher-tooltip-text          '(0.114 0.114 0.122)
      :patcher-back-button-bg        '(0.925 0.925 0.937)
      :patcher-back-button-hover-bg  '(0.820 0.902 1.000)
      :patcher-back-button-border    '(0.710 0.710 0.729)
      :patcher-back-button-hover-border '(0.00 0.478 1.00)
      :patcher-back-button-text      '(0.431 0.431 0.451)
      :patcher-back-button-hover-text '(0.00 0.286 0.620)

      ;; Active content gets system-blue focus; inactive content gets a hairline.
      :border-active   '(0.00 0.478 1.00)
      :border-inactive '(0.776 0.776 0.792)
      )))

(mac-osx-light-theme)
