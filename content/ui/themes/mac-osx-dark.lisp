;; mac-osx-dark — macOS-inspired dark theme
;; Modeled after Finder/Notes dark mode

(def mac-osx-theme () 
  (apply-theme (dict
      :compressor-bg                        '(0.02 0.022 0.025 1.0)
      :compressor-output                    '(0.52 0.54 0.56 0.88)
      :compressor-gr                        '(1.0 0.62 0.25 1.0)
      :compressor-threshold                 '(0.45 0.78 0.95 1.0)
      :rack-view-on                         '(1.0 0.58 0.25 1.0)
      :rack-view-off                        '(0.24 0.25 0.26 1.0)
      :rack-view-on-fg                      '(0.10 0.10 0.11 1.0)
      :rack-view-off-fg                     '(0.58 0.59 0.60 1.0)
      :rack-macro-off-fg                    '(0.72 0.73 0.74 1.0)
      :rack-row-selected-border             '(0.48 0.50 0.52 1.0)
      :rack-row-border                      '(0.16 0.17 0.19 1.0)
      :rack-mapping-border                  '(0.18 0.85 0.42 0.9)
      :rack-mapping-bg                      '(0.18 0.85 0.42 1.0)
      :plock-base                           '(0.54509807 0.54509807 0.5882353 1.0)
      :eq8-active-text                      '(1.00 0.80 0.00)
      :eq8-badge-bg                         '(1.00 0.80 0.00)
      :filter-table-badge-bg                '(1.00 0.80 0.00)
      ;; Modulation, editor actions, and effect visualizers.
      :mod-port-selected                    '(1.0 0.18 0.12 1.0)
      :mod-port-pending                     '(0.48 0.86 1.0 1.0)
      :mod-port-output                      '(0.10 0.58 1.0 1.0)
      :mod-port-input                       '(1.0 0.52 0.16 1.0)
      :mod-port-inactive                    '(0.10 0.11 0.13 1.0)
      :mod-port-output-inner                '(0.015 0.035 0.055 1.0)
      :mod-port-input-inner                 '(0.035 0.018 0.006 1.0)
      :mod-port-inactive-inner              '(0.02 0.025 0.03 1.0)
      :mod-cable                            '(0.10 0.58 1.0 0.92)
      :mod-cable-highlight                  '(0.78 0.94 1.0 0.38)
      :mod-cable-selected                   '(1.0 0.16 0.10 0.96)
      :mod-cable-selected-highlight         '(1.0 0.66 0.58 0.42)
      :mod-cable-preview                    '(0.42 0.84 1.0 0.84)
      :mod-cable-preview-highlight          '(0.88 0.98 1.0 0.36)
      :mod-cable-lane-tint                  '(1.0 0.35 0.0 0.0)
      :browser-primary-fg                   '(0.92 0.92 0.94)
      :adsr-curve                           '(0.4431372549 0.7490196078 0.8117647059 1.0)
      :adsr-bg                              '(0.035 0.038 0.043 1.0)
      :adsr-grid                            '(0.48 0.50 0.52 0.32)
      :adsr-point                           '(1.0 0.55 0.16 1.0)
      :phaser-bg                            '(0.045 0.048 0.052 0.72)
      :phaser-grid                          '(0.36 0.36 0.38 0.30)
      :phaser-left                          '(1.0 0.62 0.25 1.0)
      :phaser-right                         '(0.45 0.78 0.95 1.0)
      :phaser-spectrum-min                  '(0.02 0.025 0.03 0.28)
      :phaser-spectrum-mid                  '(0.12 0.30 0.34 0.18)
      :phaser-spectrum-max                  '(0.35 0.68 0.72 0.30)
      :phaser-spectrum-line                 '(0.42 0.76 0.78 0.28)
      :phaser-spectrum-fill                 '(0.10 0.36 0.38 0.12)
      :phaser-spectrum-bg                   '(0.02 0.025 0.03 1.0)
      :phaser-wave-on-bg                    '(0.45 0.78 0.95 1.0)
      :ott-high                             '(0.93 0.85 0.36 1.0)
      :ott-mid                              '(0.45 0.78 0.95 1.0)
      :ott-low                              '(1.00 0.62 0.25 1.0)
      :ott-control-on                       '(0.94 0.69 0.32 1.0)
      :ott-frequency-fg                     '(0.75 0.75 0.75 1.0)
      :dynamics-knob-track                       '(0.4 0.4 0.4 1.0)
      :multiband-bg                         '(0.045 0.048 0.052 1.0)
      :multiband-grid                       '(0.36 0.36 0.38 0.34)
      :multiband-level                      '(0.36 0.72 0.92 1.0)
      :multiband-gain                       '(1.0 0.62 0.25 1.0)
      :reverb-curve                         '(0.32 0.68 0.96 1.0)
      :reverb-curve-bg                      '(0.055 0.058 0.06 1.0)
      :reverb-curve-grid                    '(0.34 0.34 0.36 0.55)
      :reverb-curve-point                   '(1.0 0.62 0.25 1.0)
      :eq8-band-selected                    '(1.0 0.74 0.22 1.0)
      :eq8-curve                            '(1.0 0.54 0.14 1.0)
      :eq8-selected                         '(1.0 0.78 0.18 1.0)
      :eq8-spectrum                         '(0.08 0.52 0.54 0.30)
      :eq8-spectrum-peak                    '(0.40 0.92 0.86 0.74)
      :eq8-default-bg                       '(0.045 0.048 0.052 1.0)
      :eq8-default-spectrum                 '(0.12 0.58 0.62 0.30)
      :eq8-default-spectrum-peak            '(0.42 0.95 0.88 0.74)
      :eq8-spectrum-min                     '(0.03 0.035 0.04 1.0)
      :eq8-default-curve                    '(1.0 0.58 0.18 1.0)
      :eq8-inactive                         '(0.50 0.52 0.54 0.72)
      :eq8-grid                             '(0.42 0.43 0.45 0.34)
      :filter-table-editor-bg               '(0.030 0.040 0.055 1.0)
      :filter-table-editor-grid             '(0.30 0.32 0.36 0.5)
      :filter-table-wave                    '(1.0 0.62 0.25 1.0)
      :filter-table-wave-inactive           '(0.20 0.43 0.72 0.14)
      :filter-table-wave-bg                 '(0.035 0.045 0.060 1.0)
      :filter-table-response                '(0.78 0.84 0.92 0.96)
      :filter-table-spectrum                '(0.18 0.38 0.64 0.30)
      :filter-table-spectrum-peak           '(0.36 0.62 0.92 0.58)
      :filterbank-control-on                '(0.98 0.78 0.14 1.0)
      :filterbank-readout                   '(0.93 0.88 0.72 1.0)
      ;; Semantic control roles preserve the original UI independently of accent.
      :scene-active-base              '(0.00 0.01 0.42 1.0)
      :scene-hover-base               '(0.10 0.10 0.12 0.72)
      :scene-push-base                '(0.04 0.24 0.88 1.0)
      :scene-queued-base              '(0.01 0.03 0.38 1.0)
      :scene-queued-pulse             '(0.04 0.10 0.30 0.0)
      :scene-bank-indicator           '(0.20 0.52 1.0 1.0)
      :scene-action-base              '(0.00 0.01 0.02 1.0)
      :scene-active-fg                '(0.92 0.92 0.94)
      :scene-clip-bg                  '(0.52 0.56 0.62 1.0)
      :scene-clip-fg                  '(0.2 0.2 0.2 1.0)
      :clip-label-fg                  '(0.2 0.2 0.2 1.0)
      :icon-fg                        '(0.75 0.75 0.78 1.0)
      :icon-active-fg                 '(1.0 1.0 1.0 1.0)
      :save-icon-fg                   '(0.92 0.92 0.96 1.0)
      :play-active-base               '(0.05 0.28 0.03 1.0)
      :arrangement-return-base        '(0.80 0.38 0.16 1.0)
      :arrangement-return-fg          '(0.10 0.045 0.02 1.0)
      :record-active-base             '(0.12 0.001 0.001 1.0)
      :record-idle-fg                 '(0.65 0.18 0.18 1.0)
      :clock-fg                       '(0.85 0.85 0.85 1.0)
      :preset-save-bg                 '(0.08 0.08 0.01 1.0)
      :preset-save-active-bg          '(0.00 0.35 0.82 1.0)
      :device-enabled                 '(1.0 0.8 0.12 1.0)
      :device-disabled                '(0.0 0.0 0.0 1.0)
      :sequencer-toggle-off-bg        '(0.08 0.09 0.10 1.0)
      :sequencer-solo-on-bg           '(0.72 0.10 0.10 1.0)
      :sequencer-solo-on-fg            '(0.92 0.92 0.94)
      :sampler-toggle-off-bg          '(0.1 0.1 0.1 0.5)
      :poly-off-bg                    '(0.1 0.1 0.1 1.0)
      :poly-off-fg                    '(0.92 0.92 0.94)
      :sampler-dropdown-bg            '(0.1 0.1 0.1 0.3)
      :effect-mode-on-bg               '(1.0 0.62 0.25 1.0)
      :delay-reverb-mode-on-bg        '(0.36 0.80 0.50 1.0)
      :delay-mode-readout-fg          '(0.93 0.88 0.78 1.0)
      :number-slider-fill             '(0.1 0.2 0.6 1.0)
      :sequencer-volume-handle        '(0.90 0.91 0.93 0.64)
      :mixer-volume-handle            '(0.78 0.80 0.83 1.0)
      :arrangement-loop               '(0.92 0.72 0.25 1.0)
      :arrangement-cursor             '(0.32 0.78 0.94 1.0)
      :piano-key-border               '(0.101960784 0.101960784 0.11372549 1.0)
      :piano-black-key                '(0.019607843 0.019607843 0.023529412 1.0)
      :piano-white-lane               '(0.08627451 0.08627451 0.094117647 1.0)
      :piano-black-lane               '(0.050980392 0.050980392 0.058823529 1.0)
      :timeline-grid-minor            '(0.2 0.2 0.219607843 1.0)
      :timeline-playhead              '(1.00 0.80 0.00)
      :search-icon                    '(0.45 0.47 0.50 1.0)
      :text-input-bg                  '(0.19 0.2 0.22999999999999998 1.0)
      :control-on-bg  '(0.95 0.48 0.18)
      :control-on-fg  '(0.07 0.07 0.07)
      :track-tint     '(0.0 0.0 0.0 0.0)
      ;; Main editor
      :bg             '(0.10 0.10 0.12)     ; #1c1c1e — System dark bg
      :fg             '(0.88 0.88 0.89)     ; #e0e0e3
      :fg-muted       '(0.56 0.56 0.58)     ; #8e8e93 — System gray
      :dim            '(0.66 0.66 0.68)     ; Secondary sequencer text
      :dimmer         '(0.48 0.48 0.50)     ; Between :gray and :dim
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
      :border-inactive '(0.10 0.10 .105)    ; Match buffer background
      
      ;; Browser tree icons (solid colored silhouettes; the rail keeps white strokes)
      :list-icon-detail     '(0.12 0.13 0.14)   ; cutouts match the row bg
      :list-icon-folder     '(0.45 0.75 0.98)   ; Finder's light sky-blue folder
      :list-icon-instrument '(0.96 0.57 0.10)
      :list-icon-sample     '(0.11 0.86 0.31)
      :list-icon-kit        '(0.96 0.14 0.10)
      :list-icon-audio-fx   '(0.17 0.71 0.96)
      :list-icon-midi-fx    '(0.67 0.17 0.91)
      :list-icon-preset     '(0.96 0.79 0.10)
      :list-icon-lfo        '(0.89 0.15 0.53)
      :list-icon-misc       '(0.56 0.56 0.58)
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
      :instrument-group-bg '(0.14 0.14 0.15)
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
      :dropdown-menu-bg  '(0.26 0.265 0.28)
      :dropdown-menu-border '(0.46 0.46 0.48)
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
      :widget-knob-mod-dot  '(1.00 0.80 0.35)
      :sequencer-step-border          '(0.01 0.02 0.01)
      :sequencer-step-selected-border '(0.90 0.92 0.96)
      :sequencer-step-off-fill        '(0.015 0.028 0.025)
      :sequencer-step-off-fill-alt    '(0.15 0.155 0.16)
      ;:patcher-bg           '(0.08 0.08 0.08)
      :patcher-bg '(0.12 0.13 0.14)  ; Main rounded buffer surface
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
      ;; Flat (Zed-style) completion menu, shared by the code editor and the
      ;; patcher node editor -- one `comp-*` palette drives both surfaces.
      :comp-unselected-bg '(0.15 0.15 0.16)
      :comp-selected-bg '(0.20 0.21 0.24)
      :comp-border '(0.23 0.24 0.25)
      :comp-fg '(0.30 0.98 0.75)
      :comp-selected-fg '(0.46 1.00 0.82)
      :comp-category-fg '(0.45 0.47 0.51)
      :comp-doc-bg '(0.09 0.09 0.11)
      :comp-doc-border '(0.23 0.24 0.25)
      :comp-doc-fg '(0.66 0.67 0.72)
      :comp-doc-title-fg '(0.46 1.00 0.82)
      ;; Port tooltips
      :patcher-tooltip-bg '(0.09 0.09 0.11)
      :patcher-tooltip-border '(0.18 0.19 0.21)
      :patcher-tooltip-text '(0.82 0.84 0.87)
      :patcher-node-tail-text-hover '(1.00 0.80 0.00)
      ;; Agentic bubbles (cmd+k). The card is drawn flat (AGENTIC_CARD_FLATNESS),
      ;; so unlike the node colours these land close to as authored. The card's
      ;; own alpha is max(border, bg), so dim its border in RGB rather than by
      ;; lowering alpha -- that constraint is the card only; box and chip
      ;; borders go through a shader that mixes alpha properly.
      :patcher-agentic-card-bg '(0.165 0.160 0.152 0.975)
      :patcher-agentic-card-border '(0.11 0.11 0.11 0.975)
      :patcher-agentic-card-border-active '(0.21 0.21 0.21 0.975)
      :patcher-agentic-card-error-bg '(0.20 0.12 0.12 0.975)
      :patcher-agentic-box-bg '(0.235 0.230 0.230)
      :patcher-agentic-box-border '(0.27 0.27 0.28)
      :patcher-agentic-header-text '(0.82 0.83 0.86)
      :patcher-agentic-body-text '(0.98 0.98 0.99)
      :patcher-agentic-placeholder-text '(0.78 0.79 0.72)
      :patcher-agentic-chip-bg '(0.52 0.53 0.56 0.42)
      :patcher-agentic-chip-border '(0.32 0.33 0.36 0.88)
      :patcher-agentic-chip-text '(0.82 0.83 0.86)
      :patcher-agentic-send-bg '(0.12 0.13 0.16 0.80)
      :patcher-agentic-send-glyph '(0.92 0.92 0.94)
      :patcher-agentic-spinner '(0.22 0.83 0.66)
      :patcher-back-button-bg '(0.14 0.14 0.15)
      :patcher-back-button-hover-bg '(0.16 0.20 0.28)
      :patcher-back-button-border '(0.36 0.36 0.39)
      :patcher-back-button-hover-border '(0.00 0.48 0.95)
      :patcher-back-button-text '(0.62 0.63 0.66)
      :patcher-back-button-hover-text '(0.88 0.93 1.00)
      )))

(mac-osx-theme)
