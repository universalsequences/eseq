; Minimal Metal Sequencer - Step Grid UI
; C-p to toggle play/stop, Esc to clear step selection

(load "mac-osx-dark.lisp")
(mac-osx-theme)
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

(def seq-clear-ui-selection ()
  (do
    (seq-clear-selection)))

(bind-key "C-p" "seq-toggle-play")
(bind-key "ESC" "seq-clear-ui-selection")

(defstate lower-panel-buffer "*fx*")

(def lower-fx-layout-height 10)

(def seq-lower-panel-layout-spec (lower-buffer lower-ratio lower-min-height lower-max-height)
  (list :rows :gap 1
    0.05 (list :buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
    0.95 (list :cols :gap 1
      0.2 (list :buf "*samples*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 34 :max-width 42)
      0.8 (list :rows :gap 1
        0.55 (list :cols :gap 1
          0.78 (list :buf "*metal*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 25)
          0.22 (list :buf "*track*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-width 28 :max-width 44))
        0.45 (list :buf "*mixer*" :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height 12 :max-height 12)))
    lower-ratio (list :buf lower-buffer :hide-status true :border-radius 12 :border-width 4 :background-color :buffer-bg :min-height lower-min-height :max-height lower-max-height)))

(def seq-apply-lower-panel-layout (lower-buffer lower-ratio lower-min-height lower-max-height)
  (set-layout (seq-lower-panel-layout-spec lower-buffer lower-ratio lower-min-height lower-max-height)))

(def seq-apply-fx-layout ()
  (seq-apply-lower-panel-layout "*fx*" 0.33 lower-fx-layout-height lower-fx-layout-height))

(def seq-apply-piano-roll-layout ()
  (seq-apply-lower-panel-layout "*piano-roll*" 1.0 13 50))

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
  (if (= (current-buffer-name) "*metal*")
    (set-window-buffer "*sequencer*")
    (if (= (current-buffer-name) "*sequencer*")
      (set-window-buffer "*metal*")
      (seq-toggle-fx-piano-roll))))

(bind-key "Tab" "seq-toggle-main-or-piano-roll")

; 0=vel 1=dur 2=aux_a 3=transpose 4=pan 5=sync
(defstate param-mode 0)

(def page-size 16)

;; ── Step cursor ──
(defstate cursor-step 0)

(def current-step ()
  (min cursor-step (- (max 1 SEQ.tp-num-steps) 1)))

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
      (set! cursor-step (mod (- (current-step) 1) (max 1 SEQ.tp-num-steps))))))

(def cursor-right ()
  (if (seq-has-selection?)
    (do
      (cool-off-follow)
      (if (seq-has-selected-bus?)
        (bus-shift-selected-steps 1)
        (seq-shift-selected-steps 1)))
    (do
      (cool-off-follow)
      (set! cursor-step (mod (+ (current-step) 1) (max 1 SEQ.tp-num-steps))))))

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

(defstate step-drag-anchor nil)
(defstate step-click-pending nil)
(defstate step-move-last nil)

(def step-select-drag-start (step evt)
  (do
    (cool-off-follow)
    (set! cursor-step step)
    (set! step-click-pending nil)
    (set! step-drag-anchor step)
    (seq-select-step-range step step)))

(def step-select-drag-over (step evt)
  (if (selection-click? evt)
    (do
      (set! step-click-pending nil)
      (set! step-move-last nil)
      (cool-off-follow)
      (if (= step-drag-anchor nil) (set! step-drag-anchor step) nil)
      (set! cursor-step step)
      (seq-select-step-range step-drag-anchor step))
    (if (= step-move-last nil)
      nil
      (if (= step step-move-last)
        nil
        (do
          (set! step-click-pending nil)
          (cool-off-follow)
          (seq-move-step-drag step-move-last step)
          (set! step-move-last step)
          (set! cursor-step step))))))

(def step-pointer-down (step evt)
  (if (selection-click? evt)
    (step-select-drag-start step evt)
    (do
      (cool-off-follow)
      (set! cursor-step step)
      (set! step-drag-anchor nil)
      (set! step-move-last step)
      (set! step-click-pending step))))

(def step-pointer-up (step evt)
  (do
    (if (and (= step-click-pending step) (not (selection-click? evt)))
      (seq-toggle-step step)
      nil)
    (set! step-click-pending nil)
    (set! step-drag-anchor nil)
    (set! step-move-last nil)))

(def select-all-steps ()
  (do
    (cool-off-follow)
    (if (seq-has-selected-bus?)
      (bus-select-all-steps)
      (seq-select-all-steps))))

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
            false))))))

(def goto-page (page)
  (do
    (cool-off-follow)
    (set! cursor-step (min (* page page-size) (- (max 1 SEQ.tp-num-steps) 1)))))

(def double-track-pattern ()
  (do
    (cool-off-follow)
    (seq-double-track-pattern)
    (set! cursor-step (min (current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

(def halve-track-pattern ()
  (do
    (cool-off-follow)
    (seq-halve-track-pattern)
    (set! cursor-step (min (current-step) (- (max 1 SEQ.tp-num-steps) 1)))))

;; Cursor keys scoped to *metal* buffer via mode
(define-mode "seq-grid-mode" :read-only true :on-key "seq-grid-handle-key")
(mode-bind-key "seq-grid-mode" "LEFT" "cursor-left")
(mode-bind-key "seq-grid-mode" "RIGHT" "cursor-right")
(mode-bind-key "seq-grid-mode" "C-a" "select-all-steps")
(mode-bind-key "seq-grid-mode" "BS" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "Delete" "delete-selected-steps")
(mode-bind-key "seq-grid-mode" "RET" "cursor-toggle")

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


(def param-values ()
  (if (= param-mode 0) SEQ.velocities
    (if (= param-mode 1) SEQ.durations
      (if (= param-mode 2) SEQ.auxas
        (if (= param-mode 3) SEQ.transposes
          (if (= param-mode 4) SEQ.pans
            SEQ.syncs))))))

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
            (- (len SEQ.sync-labels) 1)))))))

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
            :sync))))))

(def param-color ()
  (if (= param-mode 0) :blue
    (if (= param-mode 1) :green
      (if (= param-mode 2) :magenta
        (if (= param-mode 3) :yellow
          (if (= param-mode 4) :red
            :green))))))

(def param-name ()
  (if (= param-mode 0) "Velocity"
    (if (= param-mode 1) "Duration"
      (if (= param-mode 2) "Aux A"
        (if (= param-mode 3) "Transpose"
          (if (= param-mode 4) "Pan"
            "Sync"))))))

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
    (if (= step-move-last nil)
      nil
      (if (= step step-move-last)
        nil
        (do
          (set! step-click-pending nil)
          (cool-off-follow)
          (bus-move-step-drag step-move-last step)
          (set! step-move-last step)
          (set! cursor-step step))))))

(def bus-step-pointer-down (step evt)
  (if (selection-click? evt)
    (bus-step-select-drag-start step evt)
    (do
      (cool-off-follow)
      (set! cursor-step step)
      (set! step-drag-anchor nil)
      (set! step-move-last step)
      (set! step-click-pending step))))

(def bus-step-pointer-up (step evt)
  (do
    (if (and (= step-click-pending step) (not (selection-click? evt)))
      (bus-toggle-step step)
      nil)
    (set! step-click-pending nil)
    (set! step-drag-anchor nil)
    (set! step-move-last nil)))

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
