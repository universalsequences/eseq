; Minimal Metal Sequencer - Step Grid UI
; C-p to toggle play/stop, Esc to deselect

(load "../eseqlisp/themes.lisp")
(mac-osx-theme)

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
(load "metal-seq-mixer.lisp")
(load "metal-seq-transport.lisp")

(def seq-clear-ui-selection ()
  (do
    (set! selected-bus -1)
    (seq-clear-selection)))

(bind-key "C-p" "seq-toggle-play")
(bind-key "ESC" "seq-clear-ui-selection")

(defstate lower-panel-buffer "*fx*")

(def seq-toggle-fx-piano-roll ()
  (if (= (current-buffer-name) "*fx*")
    (do
      (set-window-buffer "*piano-roll*")
      (set! lower-panel-buffer "*piano-roll*"))
    (if (= (current-buffer-name) "*piano-roll*")
      (do
        (set-window-buffer "*fx*")
        (set! lower-panel-buffer "*fx*"))
      (if (= lower-panel-buffer "*fx*")
        (do
          (set-window-buffer-for "*fx*" "*piano-roll*")
          (set! lower-panel-buffer "*piano-roll*"))
        (do
          (set-window-buffer-for "*piano-roll*" "*fx*")
          (set! lower-panel-buffer "*fx*"))))))

(bind-key "Tab" "seq-toggle-fx-piano-roll")

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
      (seq-shift-selected-steps -1))
    (do
      (cool-off-follow)
      (set! cursor-step (mod (- (current-step) 1) (max 1 SEQ.tp-num-steps))))))

(def cursor-right ()
  (if (seq-has-selection?)
    (do
      (cool-off-follow)
      (seq-shift-selected-steps 1))
    (do
      (cool-off-follow)
      (set! cursor-step (mod (+ (current-step) 1) (max 1 SEQ.tp-num-steps))))))

(def cursor-toggle ()
  (do
    (cool-off-follow)
    (seq-toggle-step (current-step))))

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
    (seq-select-all-steps)))

(def delete-selected-steps ()
  (do
    (cool-off-follow)
    (seq-delete-selected-steps)))

(def step-param-value (v)
  (if (= param-mode 3)
    (round v)
    v))

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
            (if (= key "+")
              (do (double-track-pattern) true)
              (if (or (= key "_") (= key "-"))
                (do (halve-track-pattern) true)
                false))))))))

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
(mode-bind-key "seq-grid-mode" "+" "double-track-pattern")
(mode-bind-key "seq-grid-mode" "_" "halve-track-pattern")
(mode-bind-key "seq-grid-mode" "-" "halve-track-pattern")

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
    (if (= param-mode 1) 0.1
      (if (= param-mode 2) 0
        (if (= param-mode 3) -12
          (if (= param-mode 4) -1
            0))))))

(def param-max ()
  (if (= param-mode 0) 1
    (if (= param-mode 1) 2
      (if (= param-mode 2) 16
        (if (= param-mode 3) 12
          (if (= param-mode 4) 1
            (- (len SEQ.sync-labels) 1)))))))

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

;; ── Step cursor highlight ──

(defwidget cursor-highlight
  :width 1 :height 1
  :shader (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.3)
      (material :color (rgba 0.18 0.25 0.35 0.9)))))

;; ── Aqua material for sliders ──


(defmacro aqua-slider-material2 ()
    `(material
       :lighting (lighting :edge-min -0.2015 :edge-max 0.01413
         :light (vec3 -0.1 -1.1 0.5) :shininess 71.0)
       :color
       (let ((base (mix (rgba 0.1 0.1 0.1 1) (rgba 1.0 1.0 1.0 1)
                        (smoothstep -0.02 0 d)))
             (lit (+ 0.6 (* 0.4 diffuse)))
             (shine (* 0.25 specular)))
         (+ (* base (rgba lit lit lit 1.0))
            (rgba shine shine shine 0.0)))))

(defmacro aqua-slider-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.413
       :light (vec3 -0.1 -1.1 1.5) :shininess 81.0)
     :color (aqua-color (rgba 0.15 0.15 0.88 1.0) (rgba 0.50 0.50 0.92 1.0))))

     

;; ── Aqua widgets ──

(defmacro aqua-color (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
            (__base (mix ,base1
                ,base2
                (smoothstep -0.1 3 __ny)))
            (__glass (smoothstep 0.05 -0.65 __ny))
            (__edge-fade (smoothstep 0.01 -0.16 d))
            (__hi (* __glass __edge-fade 0.2655))
            (__spec (* specular __edge-fade 0.3))
            (__bot (* (smoothstep 0.3 0.15 __ny)
                (smoothstep 0.65 0.5 __ny)
                __edge-fade 0.02))
            (__rim (smoothstep 0.9 -0.0183 d)))
          (+ (* __base (rgba __rim __rim __rim 1.0))
            (rgba (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              0.0))))

(defwidget aqua-button
  :width 4 :height 3
  :paint-margin 1
  :state (active plocked selected)
  :shader
  (let ((sel-y (if (= selected 1) (* 0.03 (cos (* 3 itime))) 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (+ (* 0.001 (smoothstep 0 0.1 (* y x))) (sdf/fill-rounded-rect -0.01 0.85))
          (material
            :lighting
            (lighting :edge-min -0.15 :edge-max 0.5
              :light (vec3 0.1 -1.0 1.5) :shininess 62.0)
            :color
            (* (if (= active 1) 1 0.7) (aqua-color (rgba 0.15 0.15 0.15 1.0) (rgba 0.30 0.30 0.92 1.0)))
            :shadow (shadow
              :color (rgba 0 0 0 0.3)
              :blur 0.15
              :offset (vec2 0 0.05))))))))

(defwidget tick
  :width 1.5 :height 1.5
  :state (active plocked selected)
  :shader
  (let ((sel-y (if (= selected 1) (* 0.1 (cos (* 3 itime))) 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (sdf/circle 1)
          (material
            :lighting (lighting :edge-min -0.35 :edge-max 0.5
              :light (vec3 0.0 -1.0 1.5) :shininess 32.0)
            :color
            (* (if (= active 1) 1 0.3)
               (aqua-color
                 (if (= plocked 1) (rgba 0.75 0.15 0.5 1.0) (rgba 0.3 0.3 0.85 1.0))
                 (if (= plocked 1) (rgba 0.4 0.135 0.95 1.0) (rgba 0.90 0.50 0.82 1.0))))))))))

(defwidget page-playhead-dot
  :width 0.7 :height 0.7
  :state (active)
  :shader
  (if (= active 1)
    (sdf/layer
      (sdf/fill (sdf/circle 0.45)
        (material :color (rgba 1 1 1 1))))
    (rgba 0 0 0 0)))

(defwidget step-playhead-dot
  :width 1.0 :height 0.7
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.45)
      (material :color (if (= active 1) (rgba 1 1 1 1) (rgba 0 0 0 0))))))

;; ── Main UI ──

(def metal-empty-track-fallback ()
  (v-stack :width :fill :padding 1 :gap 0
    (box :flex 1)
    (h-stack :width :fill :align :center
      (box :flex 1)
      (v-stack :gap 0.35 :align :center
        (label "Select a sound to create a track"
          :font-size 14 :color :gray :bg :transparent)
        (label "Sampler, instruments, and projects are in the left browser."
          :font-size 10 :color :dark-gray :bg :transparent))
      (box :flex 1))
    (box :flex 1)))

(def metal-bus-selection-panel ()
  (v-stack :width :fill :padding 1 :gap 0
    (box :flex 1)
    (h-stack :width :fill :align :center
      (box :flex 1)
      (v-stack :gap 0.4 :align :center
        (label (selected-bus-name)
          :font-size 13 :color :white :bg :transparent)
        (label "Bus sequencing"
          :font-size 12 :color :gray :bg :transparent))
      (box :flex 1))
    (box :flex 1)))

(def track-bus-send-control (send)
  (v-stack :align :center :gap 0.25
    (h-stack :gap 0.25 :align :baseline
      (label (substring (get send :name) 0 8) :font-size 9 :color :gray :bg :transparent)
      (number-picker :value (get send :amount) :min 0 :max 1 :decimals 2
        :noui true :font-size 9 :text-color :gray
        :on-change (lambda (v)
          (do
            (cool-off-follow)
            (host-command "set-track-bus-send"
              (dict :bus (get send :bus-idx) :amount v))))
        :width 4 :height 1))
    (box :width 8 :height 2
      (hslider :min 0 :max 1
        :value (get send :amount)
        :material (aqua-slider-material)
        :on-change (lambda (v)
          (do
            (cool-off-follow)
            (host-command "set-track-bus-send"
              (dict :bus (get send :bus-idx) :amount v))))))))

(effect-buffer "*metal*"
  (if (seq-has-selected-bus?)
    (metal-bus-selection-panel)
    (if (= SEQ.num-tracks 0)
    (metal-empty-track-fallback)
    (v-stack
      :padding 1
      :gap 1
      
      ; Param mode selector
      (h-stack :gap 0.5
        (box :width 8 :height 2
          :bg (if (= param-mode 0) :blue :dark-gray)
          :on-click |x y r| (set! param-mode 0)
          (label "vel" :font-size 12
            :color (if (= param-mode 0) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 1) :green :dark-gray)
          :on-click |x y r| (set! param-mode 1)
          (label "dur" :font-size 12
            :color (if (= param-mode 1) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 2) :magenta :dark-gray)
          :on-click |x y r| (set! param-mode 2)
          (label "aux_a" :font-size 12
            :color (if (= param-mode 2) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 3) :yellow :dark-gray)
          :on-click |x y r| (set! param-mode 3)
          (label "xpose" :font-size 12
            :color (if (= param-mode 3) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 4) :red :dark-gray)
          :on-click |x y r| (set! param-mode 4)
          (label "pan" :font-size 12
            :color (if (= param-mode 4) :white :gray)
            :bg :transparent))
        (box :width 8 :height 2
          :bg (if (= param-mode 5) :green :dark-gray)
          :on-click |x y r| (set! param-mode 5)
          (label "syn" :font-size 12
            :color (if (= param-mode 5) :white :gray)
            :bg :transparent)))
    
    ; Step columns: vslider + aqua step toggle + step number
    (grid :cols 16 :col-width 4
      (each (range 0 page-size) |i|
        (let ((step (step-index i))
              (visible (step-visible? i)))
          (box :padding 0.25 :background (if visible (if (= (current-step) step) "cursor-highlight" nil) nil)
            :on-click (lambda (evt)
              (if visible
                (do
                  (cool-off-follow)
                  (set! cursor-step step)
                  (if (selection-click? evt)
                    (step-select-drag-start step evt)
                    (seq-clear-selection)))
                nil))
            :on-drag (lambda (evt)
              (if visible
                (step-select-drag-over step evt)
                nil))
            (v-stack :align :center :gap 0.5
              (vslider :height 4
                :width (if (= param-mode 5) 3 2)
                :min (param-min) :max (param-max)
                :origin (param-origin)
                :value (nth (param-values) step)
                :items (if (= param-mode 5) SEQ.sync-labels '())
                :font-size 11
                :color (if visible
                         (if (nth SEQ.steps step) :white :gray)
                         :gray)
                :material (aqua-slider-material)
                :on-change (lambda (v)
                  (if visible
                    (do
                      (cool-off-follow)
                      (set! cursor-step step)
                      (let ((value (step-param-value v)))
                      (if (seq-has-selection?)
                        (seq-set-step-param-plock (param-keyword) value)
                        (seq-set-step-param step (param-keyword) value))))
                    nil)))
              (box
                :active (if visible (if (nth SEQ.steps step) 1 0) 0)
                :plocked 1
                :selected (if visible (if (nth SEQ.selected-steps step) 1 0) 0)
                :background "aqua-button"
                :align :center :width 3 :height 1.5
                :on-mouse-down (lambda (evt)
                  (if visible
                    (step-pointer-down step evt)
                    nil))
                :on-drag (lambda (evt)
                  (if visible
                    (step-select-drag-over step evt)
                    nil))
                :on-mouse-up (lambda (evt)
                  (if visible
                    (step-pointer-up step evt)
                    nil))
                (tick :active (if visible (if (nth SEQ.steps step) 1 0) 0)
                      :plocked (if visible (if (nth SEQ.step-has-plocks step) 1 0) 0)
                      :selected (if visible (if (nth SEQ.selected-steps step) 1 0) 0)))
              (label (if visible (str (+ step 1)) "")
                :font-size 10 :bg :transparent
                :color (if visible
                        (if (nth SEQ.selected-steps step) :yellow
                          :gray)
                        :gray))
              (subtree :key (str "step-playhead-probe-" step)
                (step-playhead-dot
                  :active (if (reactive-get "SEQ" (str "playhead-active-" step)) 1 0)))))))) 

    ; Step cursor info
    (h-stack :gap 1 :align :center
      (box :width 11.5 :height 1.3
        (label (fmt "Step {}  {}" (+ (current-step) 1) (param-name))
          :font-size 11 :width 11.5 :color :gray :bg :transparent))
      (if (= param-mode 5)
        (box :width 8 :height 1.3
          (label (sync-current-label)
            :font-size 11 :color :white :bg :transparent))
        (number-picker :value (nth (param-values) (current-step))
          :min (param-min) :max (param-max) :decimals (param-decimals)
          :on-change (lambda (v)
            (do
              (cool-off-follow)
              (seq-set-step-param (current-step) (param-keyword) (step-param-value v))))
          :width 8 :height 1.3 :font-size 11))
      (h-stack :gap 0.4 :align :center
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :on-click |x y r| (halve-track-pattern)
          (v-stack :align :center
            (label "-"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :on-click |x y r| (double-track-pattern)
          (v-stack :align :center
            (label "+"
              :font-size 12
              :color :white
              :bg :transparent)))
        (box :background "transport-btn-bg" :padding 0 :height 1.8
          (box :width (page-panel-width) :height 1.7 :padding 0.0525
            (h-stack :gap 0.4 :padding 0.3
              (h-stack :gap 0.4
                (each (range 0 (page-count)) |page|
                  (box :width page-button-width :height 1.25 :align :center
                      :bg (if (= page (visible-page)) :blue :dark-gray)
                      :on-click |x y r| (goto-page page)
                      (v-stack :gap 0.02 :align :center
                        (label (str (+ page 1))
                          :font-size 10
                          :color (if (= page (visible-page)) :white :gray)
                          :bg :transparent)
                        (page-playhead-dot :active (if (= page (playhead-page)) 1 0)))))))))))

    ; Track parameters — row 1: gate, poly, fts, timebase, swing, sw-res
    (h-stack :gap 1.5
      ; Gate toggle
      (v-stack :align :center :gap 0.25
        (label "gate" :font-size 9 :color :gray :bg :transparent)
        (box :width 4 :height 2
          :bg (if SEQ.tp-gate :blue :dark-gray)
          :on-click |x y r|
            (do
              (cool-off-follow)
              (seq-set-track-param :gate (if SEQ.tp-gate 0 1)))
          (label (if SEQ.tp-gate "ON" "OFF")
            :font-size 11 :color :white :bg :transparent)))
      ; Poly toggle
      (v-stack :align :center :gap 0.25
        (label "poly" :font-size 9 :color :gray :bg :transparent)
        (box :width 4 :height 2
          :bg (if SEQ.tp-poly :blue :dark-gray)
          :on-click |x y r|
            (do
              (cool-off-follow)
              (seq-set-track-param :poly (if SEQ.tp-poly 0 1)))
          (label (if SEQ.tp-poly "ON" "OFF")
            :font-size 11 :color :white :bg :transparent)))
      ; Fit-to-scale
      (v-stack :align :center :gap 0.25
        (label "fts" :font-size 9 :color :gray :bg :transparent)
        (dropdown :value SEQ.tp-fts
          :options SEQ.fts-options
          :on-change (lambda (v) (do (cool-off-follow) (seq-set-fts v)))
          :width 10 :height 1.5 :font-size 10))
      ; Timebase
      (v-stack :align :center :gap 0.25
        (label "timebase" :font-size 9 :color :gray :bg :transparent)
        (dropdown :value SEQ.tp-timebase
          :options '("1" "2" "4" "8" "16" "32" "64" "2T" "4T" "8T" "16T" "32T" "64T" "Prh")
          :on-change (lambda (v)
            (do
              (cool-off-follow)
              (if (seq-has-selection?)
                (seq-plock-timebase v)
                (seq-set-timebase v))))
          :width 6 :height 1.5 :font-size 11))
      ; Swing
      (v-stack :align :center :gap 0.25
        (h-stack :gap 0.25 :align :baseline
          (label "swg" :font-size 9 :color :gray :bg :transparent)
          (number-picker :value SEQ.tp-swing :min 50 :max 75 :decimals 1
            :noui true :font-size 9 :text-color :gray
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :swing v)))
            :width 4 :height 1))
        (box :width 8 :height 2
          (hslider :min 50 :max 75
            :value SEQ.tp-swing
            :material (aqua-slider-material)
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :swing v))))))
      ; Swing resolution
      (v-stack :align :center :gap 0.25
        (label "swg resolution" :font-size 9 :color :gray :bg :transparent)
        (dropdown :value SEQ.tp-swing-resolution
          :options '("1/16" "1/8" "1/4" "1/2")
          :on-change (lambda (v) (do (cool-off-follow) (seq-set-swing-resolution v)))
          :width 5 :height 1.5 :font-size 11))
      ; Steps
      (v-stack :align :center :gap 0.25
        (h-stack :gap 0.25 :align :baseline
          (label "steps" :font-size 9 :color :gray :bg :transparent)
          (number-picker :value SEQ.tp-num-steps :min 1 :max 256 :decimals 0
            :noui true :font-size 9 :text-color :gray
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :num-steps v)))
            :width 3 :height 1))
        (box :width 8 :height 2
          (hslider :min 1 :max 256
            :value SEQ.tp-num-steps
            :material (aqua-slider-material)
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-track-param :num-steps v)))))))

    ; Track parameters — row 2: output, bus sends, acc fn/mode/limit
    (h-stack :gap 1.5
      ; Output routing
      (v-stack :align :center :gap 0.25
        (label "out" :font-size 9 :color :gray :bg :transparent)
        (dropdown :value SEQ.tp-output
          :options SEQ.track-output-options
          :on-change (lambda (v)
            (do
              (cool-off-follow)
              (host-command "set-track-output" (dict :label v))))
          :width 10 :height 1.5 :font-size 10))
      (each SEQ.tp-bus-sends |send idx|
        (track-bus-send-control send))
      ; Accumulator function
      (v-stack :align :center :gap 0.25
        (label "acc fn" :font-size 9 :color :gray :bg :transparent)
        (dropdown :value SEQ.tp-accumulator
          :options SEQ.accumulator-options
          :on-change (lambda (v) (do (cool-off-follow) (seq-set-accumulator v)))
          :width 10 :height 1.5 :font-size 10))
      ; Accumulator mode
      (v-stack :align :center :gap 0.25
        (label "acc mode" :font-size 9 :color :gray :bg :transparent)
        (dropdown :value SEQ.tp-accum-mode
          :options SEQ.accum-mode-options
          :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-mode v)))
          :width 8 :height 1.5 :font-size 10))
      ; Accumulator limit
      (v-stack :align :center :gap 0.25
        (h-stack :gap 0.25 :align :baseline
          (label "acc lim" :font-size 9 :color :gray :bg :transparent)
          (number-picker :value SEQ.tp-accum-limit :min 0 :max 127 :decimals 0
            :noui true :font-size 9 :text-color :gray
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-limit v)))
            :width 4 :height 1))
        (box :width 8 :height 2
          (hslider :min 0 :max 127
            :value SEQ.tp-accum-limit
            :material (aqua-slider-material)
            :on-change (lambda (v) (do (cool-off-follow) (seq-set-accum-limit v)))))))))))

; Layout: samples | metal | mixer on top, fx on bottom
(set-layout '(:rows
  0.05 (:buf "*transport*" :hide-status true :borderless true :min-height 2.4 :max-height 2.4)
  0.8 (:cols 0.2 (:buf "*samples*" :hide-status true :borderless true :min-width 25 :max-width 32)
         0.6 (:buf "*metal*" :hide-status false :min-width 25)
         0.2 (:buf "*mixer*" :hide-status true :borderless true :min-width 25 :max-width 25))
  0.2 (:buf "*fx*" :hide-status true :min-height 13 :max-height 50)))

; Set mode after buffer exists (effect-buffer creates it above)
(set-buffer-mode-for "*metal*" "seq-grid-mode")
