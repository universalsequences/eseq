; Minimal Metal Sequencer - Step Grid UI
; C-p to toggle play/stop, Esc to clear step selection

(load "metal-seq-themes.lisp")
(seq-theme-mac-osx-dark)
(load "metal-seq-materials.lisp")

(defstate selected-bus -1)

(def selected-bus-name ()
  (if (and (>= selected-bus 0) (< selected-bus (len SEQ.bus-names)))
    (nth SEQ.bus-names selected-bus)
    "Bus"))

(def seq-has-selected-bus? ()
  (and (>= selected-bus 0) (< selected-bus (len SEQ.bus-names))))

(load "metal-seq-browser.lisp")
(load "metal-seq-fx.lisp")
(load "metal-seq-piano-roll.lisp")
(load "metal-seq-mixer-v2.lisp")
(load "metal-seq-transport.lisp")
(load "metal-seq-agent.lisp")

(def seq-clear-ui-selection ()
  (do
    (seq-clear-selection)))

(bind-key "C-p" "seq-toggle-play")
(bind-key "ESC" "seq-clear-ui-selection")

(defstate piano-roll-placement :bottom)
(defstate step-panel-buffer "*sequencer*")
(defstate remembered-step-panel-buffer "*sequencer*")
(defstate lower-panel-buffer "*fx*")
(defstate seq-registered-step-tabs '())

(def lower-fx-layout-height 10.5)

(def seq-step-tab-buffer (tab)
  (nth tab 1))

(def seq-step-tab-matches-buffer? (tab buffer)
  (= (seq-step-tab-buffer tab) buffer))

(def seq-main-step-tabs ()
  (append (list (list "Seq" "*sequencer*")) seq-registered-step-tabs))

(def seq-main-step-tab-buffer? (buffer)
  (> (len (filter (lambda (tab) (seq-step-tab-matches-buffer? tab buffer))
            (seq-main-step-tabs))) 0))

(def seq-step-buffer? (buffer)
  (or (= buffer "*metal*") (seq-main-step-tab-buffer? buffer)))

(def seq-sanitized-step-buffer (buffer)
  (if (seq-step-buffer? buffer) buffer "*sequencer*"))

(def seq-visible-step-panel-buffer ()
  (seq-sanitized-step-buffer step-panel-buffer))

(def seq-main-step-tile-layout-spec ()
  (let ((buffer (seq-visible-step-panel-buffer))
        (tabs (seq-main-step-tabs)))
    (if (and (> (len tabs) 1) (seq-main-step-tab-buffer? buffer))
      (list :buf buffer
        :tabs tabs
        :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25)
      (list :buf buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25))))

(def seq-refresh-step-tabs-if-present ()
  (do
    (set-window-tabs-for "*sequencer*" (seq-main-step-tabs))
    (for-each
      (lambda (tab) (set-window-tabs-for (seq-step-tab-buffer tab) (seq-main-step-tabs)))
      seq-registered-step-tabs)))

(def seq-register-step-sequencer-tab (label buffer)
  (do
    (set! seq-registered-step-tabs
      (append
        (filter (lambda (tab) (not (seq-step-tab-matches-buffer? tab buffer)))
          seq-registered-step-tabs)
        (list (list label buffer))))
    (seq-refresh-step-tabs-if-present)))

(def seq-step-and-track-panel-layout-spec (lower-buffer)
  (if (= lower-buffer "*piano-roll*")
    (list :buf "*track*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25)
    (list :cols :gap 1
      0.78 (seq-main-step-tile-layout-spec)
      0.22 (list :buf "*track*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 28 :max-width 44))))

(def seq-lower-panel-layout-spec (lower-buffer lower-ratio lower-min-height lower-max-height)
  (list :rows :gap 1
    0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
    0.95 (list :cols :gap 1
      0.2 (list :buf "*samples*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 34 :max-width 42)
      0.8 (list :rows :gap 1
        0.55 (seq-step-and-track-panel-layout-spec lower-buffer)
        0.45 (list :buf "*mixer*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 13 :max-height 13)))
    lower-ratio (list :buf lower-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height lower-min-height :max-height lower-max-height)))

(def seq-patcher-bottom-bar-layout-spec ()
  (list :cols :gap 1
    0.333 (list :buf "*samples*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 28 :max-width 28 :min-height 13 :max-height 13)
    0.334 (list :buf "*mixer*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25 :max-width 30 :min-height 13 :max-height 13)
    0.333 (list :buf "*fx*" :hide-status true :border-radius 12 :border-width 40 :background-color :buffer-bg :min-height 13 :max-height 13)))

(def seq-instrument-patcher-layout-spec (patcher-buffer)
  (list :rows :gap 1
    0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
    0.80 (list :buf patcher-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20)
    0.15 (seq-patcher-bottom-bar-layout-spec)))

(def seq-instrument-patcher-source-layout-spec (patcher-buffer source-buffer)
  (list :rows :gap 1
    0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
    0.80 (list :cols :gap 1
      0.62 (list :buf patcher-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20)
      0.38 (list :buf source-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 20))
    0.15 (seq-patcher-bottom-bar-layout-spec)))

(def seq-apply-lower-panel-layout (lower-buffer lower-ratio lower-min-height lower-max-height)
  (do
    (set-layout (seq-lower-panel-layout-spec lower-buffer lower-ratio lower-min-height lower-max-height))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-apply-fx-layout ()
  (do
    (set! lower-panel-buffer "*fx*")
    (seq-apply-lower-panel-layout "*fx*" 0.33 lower-fx-layout-height lower-fx-layout-height)))

(def seq-apply-piano-roll-layout ()
  (do
    (set! lower-panel-buffer "*piano-roll*")
    (seq-apply-lower-panel-layout "*piano-roll*" 1.0 13 50)))

(def seq-apply-instrument-patcher-layout (patcher-buffer)
  (do
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set-layout (seq-instrument-patcher-layout-spec patcher-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-apply-instrument-patcher-source-layout (patcher-buffer source-buffer)
  (do
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set-layout (seq-instrument-patcher-source-layout-spec patcher-buffer source-buffer))
    (host-command "refresh-mixer-ui" (dict))))

(def seq-restore-instrument-patcher-layout ()
  (do
    (set! step-panel-buffer remembered-step-panel-buffer)
    (if (= lower-panel-buffer "*piano-roll*")
      (seq-apply-piano-roll-layout)
      (seq-apply-fx-layout))))

(def seq-current-step-buffer ()
  (seq-sanitized-step-buffer
    (if (= step-panel-buffer "*piano-roll*")
      remembered-step-panel-buffer
      step-panel-buffer)))

(def seq-piano-roll-open? ()
  (or (= step-panel-buffer "*piano-roll*")
    (= lower-panel-buffer "*piano-roll*")))

(def seq-close-piano-roll ()
  (if (= step-panel-buffer "*piano-roll*")
    (do
      (set! step-panel-buffer (seq-current-step-buffer))
      (set-window-buffer step-panel-buffer)
      (seq-apply-fx-layout))
    (do
      (set-window-buffer "*fx*")
      (seq-apply-fx-layout))))

(def seq-open-piano-roll-bottom ()
  (do
    (if (= step-panel-buffer "*piano-roll*")
      (set! step-panel-buffer (seq-current-step-buffer))
      nil)
    (set-window-buffer "*piano-roll*")
    (seq-apply-piano-roll-layout)))

(def seq-open-piano-roll-main ()
  (do
    (if (= lower-panel-buffer "*piano-roll*")
      (set! lower-panel-buffer "*fx*")
      nil)
    (set! remembered-step-panel-buffer (seq-current-step-buffer))
    (set! step-panel-buffer "*piano-roll*")
    (set-window-buffer "*piano-roll*")
    (seq-apply-fx-layout)))

(def seq-open-piano-roll-preferred ()
  (if (= piano-roll-placement :main)
    (seq-open-piano-roll-main)
    (seq-open-piano-roll-bottom)))

(def seq-show-sequencer-main ()
  (do
    (set! remembered-step-panel-buffer "*sequencer*")
    (if (= step-panel-buffer "*piano-roll*")
      nil
      (set! step-panel-buffer "*sequencer*"))
    (set-window-buffer "*sequencer*")
    (if (= lower-panel-buffer "*piano-roll*")
      (seq-apply-piano-roll-layout)
      (seq-apply-fx-layout))))

(def seq-toggle-current-track-expanded-main ()
  (do
    (seq-show-sequencer-main)
    (seqv-toggle-current-track-expanded)))

(def seq-toggle-piano-roll-main ()
  (if (= step-panel-buffer "*piano-roll*")
    (seq-close-piano-roll)
    (seq-open-piano-roll-main)))

(def seq-toggle-piano-roll-placement ()
  (if (= piano-roll-placement :main)
    (do
      (set! piano-roll-placement :bottom)
      (if (seq-piano-roll-open?)
        (seq-open-piano-roll-bottom)
        nil))
    (do
      (set! piano-roll-placement :main)
      (if (seq-piano-roll-open?)
        (seq-open-piano-roll-main)
        nil))))

(def seq-toggle-fx-piano-roll ()
  (if (= (current-buffer-name) "*fx*")
    (do
      (set-window-buffer "*piano-roll*")
      (set! lower-panel-buffer "*piano-roll*")
      (seq-apply-piano-roll-layout))
    (if (= (current-buffer-name) "*piano-roll*")
      (do
        (set-window-buffer "*fx*")
        (set! lower-panel-buffer "*fx*")
        (seq-apply-fx-layout))
      (if (= lower-panel-buffer "*fx*")
        (do
          (set-window-buffer-for "*fx*" "*piano-roll*")
          (set! lower-panel-buffer "*piano-roll*")
          (seq-apply-piano-roll-layout))
        (do
          (set-window-buffer-for "*piano-roll*" "*fx*")
          (set! lower-panel-buffer "*fx*")
          (seq-apply-fx-layout))))))

(def seq-toggle-main-or-piano-roll ()
  (if (or (= SEQ.editor-mode "new-instrument")
          (= SEQ.editor-mode "edit-instrument")
          (= SEQ.editor-mode "new-effect")
          (= SEQ.editor-mode "edit-effect"))
    (host-command "toggle-instrument-patcher-source" (dict))
    (if (seq-piano-roll-open?)
      (seq-close-piano-roll)
      (seq-open-piano-roll-preferred))))

(def seq-show-fx-lower-panel ()
  (if (= lower-panel-buffer "*piano-roll*")
    (do
      (if (= (current-buffer-name) "*piano-roll*")
        (set-window-buffer "*fx*")
        (set-window-buffer-for "*piano-roll*" "*fx*"))
      (seq-apply-fx-layout))
    nil))

(def seq-toggle-current-track-mods-view ()
  (do
    (set! selected-bus -1)
    (instrument-toggle-mods-view)
    (seq-show-fx-lower-panel)))

(bind-key "Tab" "seq-toggle-current-track-expanded-main")
(bind-key "BackTab" "seq-toggle-main-or-piano-roll")

; 0=vel 1=dur 2=aux_a 3=transpose 4=pan 5=sync 6=delay
(defstate param-mode 0)

(def page-size 16)

;; ── Step cursor ──
(defstate cursor-step 0)

(def cursor-num-steps ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-num-steps selected-bus)
    SEQ.tp-num-steps))

(def current-step ()
  (min cursor-step (- (max 1 (cursor-num-steps)) 1)))

(def page-count ()
  (max 1 (floor (/ (+ SEQ.tp-num-steps (- page-size 1)) page-size))))

(def current-page ()
  (min (floor (/ (current-step) page-size)) (- (page-count) 1)))

(def visible-page ()
  (if (and SEQ.playing SEQ.auto-follow (not (seq-has-selection?)))
    (playhead-page)
    (current-page)))

(def playhead-page ()
  (min SEQ.playhead-page
    (- (page-count) 1)))

(def page-offset ()
  (* (visible-page) page-size))

(def cool-off-follow ()
  (seq-pause-auto-follow))

(def page-button-width 2.8)

(def page-button-gap 0.4)

(def page-slot-width ()
  (+ page-button-width page-button-gap))

(def page-panel-width ()
  (+ 0.4 (* (page-count) (page-slot-width))))

(def step-index (i)
  (+ (page-offset) i))

(def step-visible? (i)
  (< (step-index i) SEQ.tp-num-steps))

(def cursor-left ()
  (if (seq-has-selection?)
    (do
      (cool-off-follow)
      (if (seq-has-selected-bus?)
        (bus-shift-selected-steps -1)
        (seq-shift-selected-steps -1)))
    (do
      (cool-off-follow)
      (let ((num-steps (max 1 (cursor-num-steps))))
        (set-track-cursor-step
          (if (= (current-step) 0)
            (- num-steps 1)
            (- (current-step) 1)))))))

(def cursor-right ()
  (if (seq-has-selection?)
    (do
      (cool-off-follow)
      (if (seq-has-selected-bus?)
        (bus-shift-selected-steps 1)
        (seq-shift-selected-steps 1)))
    (do
      (cool-off-follow)
      (let ((num-steps (max 1 (cursor-num-steps))))
        (set-track-cursor-step
          (if (>= (current-step) (- num-steps 1))
            0
            (+ (current-step) 1)))))))

(def cursor-toggle ()
  (do
    (cool-off-follow)
    (if (seq-has-selected-bus?)
      (bus-toggle-step (bus-current-step))
      (seq-toggle-step (current-step)))))

(def selection-click? (evt)
  (or (get evt :shift)
    (get evt :cmd)
    (get evt :super)
    (get evt :meta)
    (get evt :ctrl)))

(def sequencer-cursor-step-changed (track step)
  nil)

(def set-track-cursor-step (step)
  (do
    (set! cursor-step step)
    (sequencer-cursor-step-changed SEQ.current-track step)))

(defstate step-drag-anchor nil)
(defstate step-click-pending nil)
(defstate step-move-last nil)
(defstate step-toggle-drag-value nil)

(def step-selected? (step)
  (and (seq-has-selection?) (nth SEQ.selected-steps step)))

(def step-select-drag-start (step evt)
  (do
    (cool-off-follow)
    (set-track-cursor-step step)
    (set! step-click-pending nil)
    (set! step-drag-anchor step)
    (seq-select-step-range step step)))

(def step-set-cursor-if (update-cursor step)
  (if update-cursor
    (set-track-cursor-step step)
    nil))

(def step-select-drag-over-for-track-with-cursor (track step evt update-cursor)
  (if (selection-click? evt)
    (do
      (set! step-click-pending nil)
      (set! step-move-last nil)
      (set! step-toggle-drag-value nil)
      (cool-off-follow)
      (if (= step-drag-anchor nil) (set! step-drag-anchor step) nil)
      (step-set-cursor-if update-cursor step)
      (seq-select-step-range step-drag-anchor step))
    (if (not (= step-toggle-drag-value nil))
      (do
        (set! step-click-pending nil)
        (cool-off-follow)
        (step-set-cursor-if update-cursor step)
        (if (= (seq-track-step-active? track step) step-toggle-drag-value)
          nil
          (seq-toggle-step step)))
      (if (= step-move-last nil)
        nil
        (if (= step step-move-last)
          nil
          (do
            (set! step-click-pending nil)
            (cool-off-follow)
            (seq-move-step-drag step-move-last step)
            (set! step-move-last step)
            (step-set-cursor-if update-cursor step)))))))

(def step-select-drag-over-for-track (track step evt)
  (step-select-drag-over-for-track-with-cursor track step evt true))

(def step-select-drag-over-for-track-no-cursor (track step evt)
  (step-select-drag-over-for-track-with-cursor track step evt false))

(def step-select-drag-over (step evt)
  (step-select-drag-over-for-track SEQ.current-track step evt))

(def step-pointer-down-for-track (track step evt use-selection)
  (if (selection-click? evt)
    (step-select-drag-start step evt)
    (do
      (cool-off-follow)
      (set-track-cursor-step step)
      (set! step-drag-anchor nil)
      (if (or (seq-track-step-active? track step) (and use-selection (step-selected? step)))
        (do
          (set! step-move-last step)
          (set! step-click-pending step)
          (set! step-toggle-drag-value nil))
        (do
          (set! step-move-last nil)
          (set! step-click-pending nil)
          (set! step-toggle-drag-value true)
          (step-select-drag-over-for-track track step evt))))))

(def step-pointer-down (step evt)
  (step-pointer-down-for-track SEQ.current-track step evt true))

(def step-pointer-up (step evt)
  (do
    (if (and (= step-click-pending step) (not (selection-click? evt)))
      (seq-toggle-step step)
      nil)
    (set! step-click-pending nil)
    (set! step-drag-anchor nil)
    (set! step-move-last nil)
    (set! step-toggle-drag-value nil)))

(def seq-set-step-param-from-step (step param value)
  (if (step-selected? step)
    (seq-set-step-param-plock param value)
    (do
      (if (seq-has-selection?) (seq-clear-selection) nil)
      (seq-set-step-param step param value))))

(def select-all-steps ()
  (do
    (cool-off-follow)
    (if (seq-has-selected-bus?)
      (bus-select-all-steps)
      (seq-select-all-steps))))

(def seq-global-select-all-steps ()
  (if (and
        (or (buffer-read-only?) (= (current-buffer-name) "*transport*"))
        (not (= (current-buffer-name) "*piano-roll*")))
    (select-all-steps)
    false))

(bind-key "C-a" "seq-global-select-all-steps")

(def seq-global-toggle-record ()
  (if (or (buffer-read-only?) (= (view-mode) "ui"))
    (seq-toggle-record)
    false))

(bind-key "." "seq-global-toggle-record")

(def delete-selected-steps ()
  (do
    (cool-off-follow)
    (if (seq-has-selected-bus?)
      (bus-delete-selected-steps)
      (seq-delete-selected-steps))))

(def duration-slider-position (duration)
  (let ((d (max 0 (min duration 32))))
    (if (<= d 2)
      (/ d 4)
      (+ 0.5 (* 0.5 (pow (/ (- d 2) 30) 0.25))))))

(def duration-slider-value (position)
  (let ((p (max 0 (min position 1))))
    (if (<= p 0.5)
      (* p 4)
      (+ 2 (* 30 (pow (* 2 (- p 0.5)) 4))))))

(def step-param-value (v)
  (if (= param-mode 3)
    (round v)
    v))

(def step-slider-param-value (v)
  (if (= param-mode 1)
    (duration-slider-value v)
    (step-param-value v)))

(def param-decimals ()
  (if (= param-mode 3) 0 2))

(def seq-grid-handle-key (key text)
  (if (= (current-buffer-name) "*sequencer*")
    (seqv-handle-key key text)
    (if (= key "LEFT")
      (do (cursor-left) true)
      (if (= key "RIGHT")
        (do (cursor-right) true)
        (if (= key "C-a")
          (do (select-all-steps) true)
          (if (or (= key "BS") (= key "Delete"))
            (do (delete-selected-steps) true)
            (if (= key "RET")
              (do (cursor-toggle) true)
              false)))))))

(def goto-page (page)
  (do
    (cool-off-follow)
    (set-track-cursor-step (min (* page page-size) (- (max 1 SEQ.tp-num-steps) 1)))))

(def double-track-pattern ()
  (do
    (cool-off-follow)
    (seq-double-track-pattern)
    (set-track-cursor-step (min (current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

(def halve-track-pattern ()
  (do
    (cool-off-follow)
    (seq-halve-track-pattern)
    (set-track-cursor-step (min (current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

;; Cursor keys scoped to *metal* buffer via mode
(define-mode "seq-grid-mode" :read-only true :on-key "seq-grid-handle-key")
(mode-bind-key "seq-grid-mode" "LEFT" "cursor-left")
(mode-bind-key "seq-grid-mode" "RIGHT" "cursor-right")
(mode-bind-key "seq-grid-mode" "C-a" "select-all-steps")
(mode-bind-key "seq-grid-mode" "BS" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "Delete" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "RET" "cursor-toggle")
(mode-bind-key "seq-grid-mode" "C-h" "seqv-collapse-all-tracks")

(def set-vel-mode () (set! param-mode 0))
(mode-bind-key "seq-grid-mode" "v" "set-vel-mode")
(def set-dur-mode () (set! param-mode 1))
(mode-bind-key "seq-grid-mode" "d" "set-dur-mode")
(def set-aux-mode () (set! param-mode 2))
(mode-bind-key "seq-grid-mode" "a" "set-aux-mode")
(def set-transpose-mode () (set! param-mode 3))
(mode-bind-key "seq-grid-mode" "t" "set-transpose-mode")
(def set-pan-mode () (set! param-mode 4))
(mode-bind-key "seq-grid-mode" "p" "set-pan-mode")
(def set-sync-mode () (set! param-mode 5))
(mode-bind-key "seq-grid-mode" "s" "set-sync-mode")
(def set-delay-mode () (set! param-mode 6))
(mode-bind-key "seq-grid-mode" "l" "set-delay-mode")


(def param-values ()
  (if (= param-mode 0) SEQ.velocities
    (if (= param-mode 1) SEQ.durations
      (if (= param-mode 2) SEQ.auxas
        (if (= param-mode 3) SEQ.transposes
          (if (= param-mode 4) SEQ.pans
            (if (= param-mode 5) SEQ.syncs
              SEQ.delays)))))))

(def param-min ()
  (if (= param-mode 0) 0
    (if (= param-mode 1) 0
      (if (= param-mode 2) 0
        (if (= param-mode 3) -12
          (if (= param-mode 4) -1
            0))))))

(def param-max ()
  (if (= param-mode 0) 1
    (if (= param-mode 1) 32
      (if (= param-mode 2) 16
        (if (= param-mode 3) 12
          (if (= param-mode 4) 1
            (if (= param-mode 5) (- (len SEQ.sync-labels) 1)
              1)))))))

(def param-slider-min ()
  (if (= param-mode 1) 0 (param-min)))

(def param-slider-max ()
  (if (= param-mode 1) 1 (param-max)))

(def param-slider-value (step)
  (if (= param-mode 1)
    (duration-slider-position (nth (param-values) step))
    (nth (param-values) step)))

(def param-haptic-pivot-position ()
  (if (= param-mode 1) 0.5 1))

(def param-haptic-pivot-value ()
  (if (= param-mode 1) 2 (param-max)))

(def param-haptic-exponent ()
  (if (= param-mode 1) 4 1))

(def param-keyword ()
  (if (= param-mode 0) :velocity
    (if (= param-mode 1) :duration
      (if (= param-mode 2) :aux-a
        (if (= param-mode 3) :transpose
          (if (= param-mode 4) :pan
            (if (= param-mode 5) :sync
              :delay)))))))

(def param-color ()
  (if (= param-mode 0) :blue
    (if (= param-mode 1) :green
      (if (= param-mode 2) :magenta
        (if (= param-mode 3) :yellow
          (if (= param-mode 4) :red
            (if (= param-mode 5) :green
              :cyan)))))))

(def param-name ()
  (if (= param-mode 0) "Velocity"
    (if (= param-mode 1) "Duration"
      (if (= param-mode 2) "Aux A"
        (if (= param-mode 3) "Transpose"
          (if (= param-mode 4) "Pan"
            (if (= param-mode 5) "Sync"
              "Delay")))))))

(def param-origin ()
  (if (= param-mode 3) 0
    (if (= param-mode 4) 0
      (if (= param-mode 5) 0
        (param-min)))))

(def sync-current-label ()
  (nth SEQ.sync-labels (floor (+ 0.5 (nth SEQ.syncs (current-step))))))

(def bus-seq-list (lists)
  (if (seq-has-selected-bus?)
    (nth lists selected-bus)
    '()))

(def bus-seq-playhead ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-playheads selected-bus)
    0))

(def bus-seq-num-steps ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-num-steps selected-bus)
    16))

(def bus-seq-timebase ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-timebases selected-bus)
    "16"))

(def bus-seq-swing ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-swings selected-bus)
    50))

(def bus-seq-swing-resolution ()
  (if (seq-has-selected-bus?)
    (nth SEQ.bus-swing-resolutions selected-bus)
    "1/16"))

(def bus-seq-param-values ()
  (if (= param-mode 1) (bus-seq-list SEQ.bus-durations)
    (if (= param-mode 2) (bus-seq-list SEQ.bus-syncs)
      (bus-seq-list SEQ.bus-velocities))))

(def bus-seq-param-name ()
  (if (= param-mode 1) "Duration"
    (if (= param-mode 2) "Sync"
      "Gate Amount")))

(def bus-seq-param-key ()
  (if (= param-mode 1) "duration"
    (if (= param-mode 2) "sync"
      "velocity")))

(def bus-seq-param-min ()
  (if (= param-mode 1) 0.1 0))

(def bus-seq-param-max ()
  (if (= param-mode 1) 2
    (if (= param-mode 2) (- (len SEQ.sync-labels) 1)
      1)))

(def bus-page-count ()
  (max 1 (floor (/ (+ (bus-seq-num-steps) (- page-size 1)) page-size))))

(def bus-current-step ()
  (min cursor-step (- (max 1 (bus-seq-num-steps)) 1)))

(def bus-current-page ()
  (min (floor (/ (bus-current-step) page-size)) (- (bus-page-count) 1)))

(def bus-page-offset ()
  (* (bus-current-page) page-size))

(def bus-step-index (i)
  (+ (bus-page-offset) i))

(def bus-step-visible? (i)
  (< (bus-step-index i) (bus-seq-num-steps)))

(def bus-page-panel-width ()
  (+ 0.4 (* (bus-page-count) (page-slot-width))))

(def bus-goto-page (page)
  (do
    (cool-off-follow)
    (set! cursor-step (min (* page page-size) (- (max 1 (bus-seq-num-steps)) 1)))))

(def bus-set-step-param (step value)
  (host-command "set-bus-step-param"
    (dict :bus selected-bus :step step :param (bus-seq-param-key) :value value)))

(def bus-set-selected-step-param (value)
  (host-command "set-selected-bus-step-param"
    (dict :bus selected-bus :param (bus-seq-param-key) :value value)))

(def bus-toggle-step (step)
  (do
    (cool-off-follow)
    (set! cursor-step step)
    (host-command "toggle-bus-step" (dict :bus selected-bus :step step))))

(def bus-set-step-active (step active)
  (do
    (cool-off-follow)
    (set! cursor-step step)
    (host-command "set-bus-step-active"
      (dict :bus selected-bus :step step :active active))))

(def bus-step-active? (step)
  (nth (bus-seq-list SEQ.bus-steps) step))

(def bus-select-step-range (start end)
  (host-command "select-bus-step-range"
    (dict :bus selected-bus :start start :end end)))

(def bus-select-all-steps ()
  (host-command "select-all-bus-steps" (dict :bus selected-bus)))

(def bus-delete-selected-steps ()
  (host-command "delete-selected-bus-steps" (dict :bus selected-bus)))

(def bus-move-step-drag (start target)
  (host-command "move-bus-step-drag"
    (dict :bus selected-bus :start start :target target)))

(def bus-shift-selected-steps (direction)
  (host-command "shift-selected-bus-steps"
    (dict :bus selected-bus :direction direction)))

(def bus-step-select-drag-start (step evt)
  (do
    (cool-off-follow)
    (set! cursor-step step)
    (set! step-click-pending nil)
    (set! step-drag-anchor step)
    (bus-select-step-range step step)))

(def bus-step-select-drag-over (step evt)
  (if (selection-click? evt)
    (do
      (set! step-click-pending nil)
      (set! step-move-last nil)
      (cool-off-follow)
      (if (= step-drag-anchor nil) (set! step-drag-anchor step) nil)
      (set! cursor-step step)
      (bus-select-step-range step-drag-anchor step))
    (if (not (= step-toggle-drag-value nil))
      (do
        (set! step-click-pending nil)
        (cool-off-follow)
        (set! cursor-step step)
        (if (= (bus-step-active? step) step-toggle-drag-value)
          nil
          (bus-set-step-active step step-toggle-drag-value)))
      (if (= step-move-last nil)
        nil
        (if (= step step-move-last)
          nil
          (do
            (set! step-click-pending nil)
            (cool-off-follow)
            (bus-move-step-drag step-move-last step)
            (set! step-move-last step)
            (set! cursor-step step)))))))

(def bus-step-pointer-down (step evt)
  (if (selection-click? evt)
    (bus-step-select-drag-start step evt)
    (do
      (cool-off-follow)
      (set! cursor-step step)
      (set! step-drag-anchor nil)
      (if (or (bus-step-active? step) (step-selected? step))
        (do
          (set! step-move-last step)
          (set! step-click-pending step)
          (set! step-toggle-drag-value nil))
        (do
          (set! step-move-last nil)
          (set! step-click-pending nil)
          (set! step-toggle-drag-value true)
          (bus-step-select-drag-over step evt))))))

(def bus-step-pointer-up (step evt)
  (do
    (if (and (= step-click-pending step) (not (selection-click? evt)))
      (bus-toggle-step step)
      nil)
    (set! step-click-pending nil)
    (set! step-drag-anchor nil)
    (set! step-move-last nil)
    (set! step-toggle-drag-value nil)))

(def bus-set-sequencer-param (param value)
  (host-command "set-bus-sequencer-param"
    (dict :bus selected-bus :param param :value value)))

(def bus-set-sequencer-label (param label)
  (host-command "set-bus-sequencer-param"
    (dict :bus selected-bus :param param :label label)))

(load "metal-seq-metal.lisp")
(load "metal-seq-sequencer.lisp")

; Layout: samples on the left; metal + mixer on the right; fx spans the bottom.
(seq-apply-fx-layout)
