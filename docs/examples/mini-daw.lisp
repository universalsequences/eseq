;; mini-daw — the kind-bindings reference example (docs/kind-bindings-spec.md
;; §13 stage 6): a small DAW for `metal_seq noui`. Rows of 8 steps per track,
;; mixer + preset + device slots beside each track, scenes or the open device
;; on top. Everything it shows comes from eseq.kinds instances: `t.name`
;; reads a value (the view re-renders when it changes), `#'t.armed` binds a
;; field (only the widget repaints), and a widget given `:step s :track t`
;; binds whichever fields its shader reads. The plumbing lives in libraries:
;; step gestures and keys in eseq.step-grid-interactions, device panels in
;; eseq.effects, scene and preset actions in eseq.kinds.
;;
;;   C-c l  show the view     C-c o  projects-mode     C-c s  save project
;;   Space play/pause   Esc clear selection   Cmd/Ctrl-A select all   BS delete

(import eseq.kinds :refer (tracks banks scenes transport selection
                           launch! clone-scene! delete-scene! step-preset!))
(import eseq.effects :as fx)
(import eseq.step-grid-interactions :as sgi)
(import eseq.file-dialogs)

(defcustom mini-daw-scale 1.0
  :type :number :min 0.4 :max 2.5 :step 0.05
  :doc "Size of the mini DAW's own elements (not the device panels).")
(def sc (n) (* mini-daw-scale n))

;; View state.
(def-kind view
  :key ()
  :state ((bank -1)                            ; bank shown; -1 follows the playing scene
          (open-device device :default nil)))  ; device whose panel fills the top tile

(def-kind scene-menu
  :key ()
  :state ((open false)
          (scene scene :default nil)
          (at :point :default nil)))

;; ── Shader helpers ──
;; A sheen over `color`, brighter toward the sides, banded by a sine of the
;; cell's `phase`. Reads `spin` and `phase` from the shader's let.
(defmacro gradient (color ratio alpha)
  `(mix ,color (* ,color 1.7 ,alpha (abs x))
     (smoothstep 0 (* ,ratio 3)
       (pow (abs (* ,ratio x (sin (+ (cos spin) phase y)))) 0.3))))
;; Full color at the top, 35% darker at the bottom (y: -1 top, 1 bottom).
(defmacro vgrad (c) `(* ,c (gray (- 1 (* 0.175 (+ y 1))))))
(defmacro gray (v) `(rgba ,v ,v ,v 1))
(defmacro outline (w h) `(sdf/stroke (sdf/rounded-rect ,w ,h 0.3) 0.03 (rgba 1 1 1 0.9)))
(defmacro frame () `(outline (* height 0.95) (* height 0.95)))
;; Full strength while the track is heard; dim while muted or soloed away.
(defmacro heard () `(if (= track.audible 1) 1 0.35))

;; ── Widgets ──
;; The beanie step: a square whose outline an active step warps into a bean,
;; with a bean body and a track-colored core inside.
(defwidget daw-step
  :width 8 :height 4 :paint-margin 2 :animates true
  :state (step track seed)
  :shader
  (let ((spin 112)
        (phase (+ seed (* 0.1 (cos itime))))
        (on step.active)
        (sel step.selected)
        ;; the playhead lightens the cell while the transport runs
        (shade (mix (smoothstep -1 4 y) 0.4 (* step.playing transport.playing)))
        ;; how far an active step's shapes bulge out of the square
        (wobble (* 4.0 on (cos spin)
                  (sin (+ (mix (+ (abs x) (abs y) on) (+ (abs x) (abs y)) (cos spin)) spin phase))))
        ;; corner roundness of the bean body and core
        (corner (* 0.3 (+ 1 (* (sin spin)
                                (sin (* (sin spin) (+ (cos (* 0.3 (- (abs x) (abs y)) y)) phase) 0.1))))))
        (lit (mix 0.02 1.0 on))
        ;; selection: a shimmer sweeping diagonally across the cell
        (shimmer (+ 0.55 (* 0.45 (sin (+ (* 5 itime) (* -2.5 (+ x y)))))))
        (cell (sdf/rounded-rect (* width 0.98) (* 0.98 width) 0.3)))
    (sdf/layer
      ;; selection halo + tint
      (sdf/stroke (+ (* 0.8 wobble) (sdf/rounded-rect (* width 1.04) (* 1.04 width) 0.34))
        (* 0.16 sel) (rgba track.color (* 0.4 sel shimmer)))
      (sdf/fill (+ (* 0.1 wobble) cell) (rgba track.color (* 0.14 sel)))
      ;; playhead
      (sdf/fill (+ (* 9.8 wobble) cell) (gray shade))
      ;; outline: white, or shimmering track color while selected
      (sdf/stroke (+ (* 0.1 wobble) cell) (+ 0.03 (* 0.04 sel))
        (mix (gradient :white 0.3 0.2) (* (+ 0.6 (* 0.6 shimmer)) (rgba track.color 1)) sel))
      ;; bean body
      (sdf/fill (+ (* 0.03 wobble) (sdf/rounded-rect (* 0.8 width) (* 0.8 width) (* 0.8 width corner)))
        (* lit (gradient (rgba 0.3 (cos phase) 0.1 1) 1 0.1)))
      ;; bean core, in the track's color
      (sdf/fill (+ (* 0.3 wobble (cos (+ phase wobble))) (sdf/rounded-rect (* 0.5 width) (* width 0.5) (* 0.5 width corner)))
        (* lit (gradient (rgba track.color 1) 1.2 1.9))))))

;; Volume bar with a thin level meter under it; fills are the shape clipped at the value.
(defwidget daw-fader
  :width 24 :height 4 :paint-margin 0.5
  :state (track)
  :shader
  (let ((w (* width 0.98))
        (bar (sdf/translate 0 (* -0.12 height) (sdf/rounded-rect w (* height 0.76) 0.3)))
        (meter (sdf/translate 0 (* 0.86 height) (sdf/rounded-rect w (* height 0.05) 0.04))))
    (sdf/layer
      (sdf/fill bar (vgrad (gray 0.07)))
      (sdf/fill (max bar (- x (+ (- w) (* 2 w (min 1 (max 0 track.volume))))))
        (* (heard) (vgrad (rgba track.color 1))))
      (sdf/stroke bar 0.03 (rgba 1 1 1 0.9))
      (sdf/fill meter (rgba 1 1 1 0.08))
      (sdf/fill (max meter (- x (+ (- w) (* 2 w (min 1 (max 0 track.peak))))))
        (vgrad (rgba 0.45 0.9 0.4 1))))))

(defwidget daw-mute
  :width 8 :height 4 :paint-margin 0.5
  :state (track)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect (* height 0.6) (* height 0.6) 0.25)
      (* (- 1 track.muted) (heard) (vgrad (rgba track.color 1))))
    (frame)))

(defwidget daw-arm
  :width 8 :height 4 :paint-margin 0.5
  :state (armed)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle (* height 0.45)) (* armed (vgrad (rgba 0.95 0.25 0.22 1))))
    (sdf/stroke (sdf/circle (* height 0.45)) 0.03 (rgba 0.95 0.35 0.3 (+ 0.5 (* 0.5 armed))))
    (frame)))

(defwidget daw-play
  :width 8 :height 4 :paint-margin 0.5
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect (* height 0.95) (* height 0.95) 0.3)
      (vgrad (rgba 0.85 0.85 0.85 (* 0.9 transport.playing))))
    (sdf/fill (max (- (+ x (* 0.3 height))) (- (+ (* 0.55 x) (* 0.85 (abs y))) (* 0.3 height)))
      (vgrad (gray (if (= transport.playing 1) 0.05 0.9))))
    (frame)))

(defwidget daw-chevron
  :width 6 :height 3 :paint-margin 0.5
  :state (dir)
  :shader
  (sdf/layer
    (sdf/fill (- (min (sdf/line (* -0.22 dir) -0.45 (* 0.22 dir) 0)
                      (sdf/line (* 0.22 dir) 0 (* -0.22 dir) 0.45)) 0.07)
      (vgrad (gray 0.9)))
    (outline (* width 0.95) (* height 0.92))))

;; Every pill (scenes, banks, preset name, device slots, rack slots): lit when
;; active, pulsing when queued, faint outline when dim.
(defwidget daw-pill
  :width 10 :height 4 :paint-margin 0.5 :animates true
  :state (active queued dim)
  :shader
  (let ((fill (if (= active 1) 0.85 (if (= queued 1) (+ 0.25 (* 0.2 (+ 1 (cos (* itime 6))))) 0.07)))
        (shape (sdf/rounded-rect (* width 0.97) (* height 0.9) 0.3)))
    (sdf/layer
      (sdf/fill shape (vgrad (gray fill)))
      (sdf/stroke shape 0.03 (rgba 1 1 1 (if (= dim 1) 0.35 0.9))))))

(defwidget daw-panel-frame
  :width 40 :height 12
  :shader
  (let ((spin 112) (phase (+ 82.213 (* 0.1 (cos itime))))
        (shape (sdf/rounded-rect (* width 0.995) (* height 0.985) 0.25)))
    (sdf/layer
      (sdf/fill shape (vgrad (gray 0.13)))
      (sdf/stroke shape 0.012 (* 0.45 (gradient :white 0.3 0.2))))))

;; ── Layout helpers ──
;; A w × h control (scaled) painted by `widget`; props go to the box.
(defmacro ctl (w h widget &rest props)
  `(box :width (sc ,w) :height (sc ,h) :background ,widget ,@props))

;; A pointer handler that selects track t, then runs body (`e` is the event).
(defmacro on-track (t &rest body)
  `(lambda (e) (do (set! selection.track ,t) ,@body)))

;; Takes the rest of a row.
(def fill () (box :flex 1 :height 0))

;; A pill with a centered label, dark text while `lit` (a value or a #'
;; binding). Extra props (:queued, :dim, handlers) go to the box.
(defmacro pill (w h text font lit &rest props)
  `(box :width (sc ,w) :height (sc ,h) :background "daw-pill" :active ,lit
     :h-align :center :v-align :center ,@props
     (label ,text :font-size (sc ,font) :bg :transparent
       :color :white :active ,lit :active-color :black)))
;; Scene and bank pills.
(defmacro big-pill (text lit &rest props) `(pill 10 4 ,text 14 ,lit ,@props))
;; Device and rack slot pills, w wide.
(defmacro slot-pill (w text lit &rest props) `(pill ,w 3 ,text 10 ,lit ,@props))

;; ── Steps ──
;; Pointer handling is the main grid's (sgi/down …): click empty = on (drag
;; paints), click on = select, drag = move, hold+drag = sweep-select,
;; shift/cmd-drag = range/add, double-click = off.
(def step-view (s t)
  (box :width (sc 8) :height (sc 4)
    :on-mouse-down   (lambda (e) (sgi/down s e))
    :on-drag         (lambda (e) (sgi/drag s e))
    :on-mouse-up     (lambda (e) (sgi/up s e))
    :on-double-click (lambda (e) (sgi/double-click s e))
    (daw-step :step s :track t :seed (~slider 68.457 :min 0 :max 100))))

;; Esc / Cmd-A / BS act on the steps in widget views.
(sgi/bind-step-keys)

;; ── Track: name, 8 steps a row, then mixer / preset / slots ──
(def track-view (t)
  (subtree :key t
    (box :on-mouse-down (on-track t)
      (h-stack :v-align :top
        ;; three letters fill the name column at this size
        (label (substring t.name 0 3)
          :width (sc 8) :font-size (sc 32) :v-align :center :bg :transparent
          :color :dim :active #'t.selected :active-color :white)
        ;; columns are a step plus the stack's 1-cell gap
        (grid :cols 8 :col-width (+ (sc 8) 1) :row-height (sc 4)
          (each t.steps |s| (step-view s t)))
        (box :width (- (sc 3) 1))  ; the last column's gap is the other 1
        (v-stack :gap (sc 0.5)
          (mixer-view t)
          (preset-view t)
          (devices-view t))))))

(def mixer-view (t)
  (h-stack :gap (sc 1) :v-align :center
    (ctl 8 4 "daw-mute" :track t :on-click (on-track t (toggle! t.muted)))
    (ctl 8 4 "daw-arm" :armed #'t.armed :on-click (on-track t (toggle! t.armed)))
    (ctl 24 4 "daw-fader" :track t
      :on-mouse-down (on-track t (set-volume t e))
      :on-drag (on-track t (set-volume t e)))
    (fill)))

(def set-volume (t e) (when e.u (set! t.volume e.u)))

;; ‹ preset › — steps through the track's preset list (wrapping).
(def preset-view (t)
  (subtree :key (list :preset t)
    (h-stack :gap (sc 0.5)
      (box :width (sc 16.5))
      (ctl 6 3 "daw-chevron" :dir -1 :on-click (on-track t (step-preset! t -1)))
      ;; 14 characters fit the pill at font 12
      (pill 12 3 (if (= t.preset "") "—" (substring t.preset 0 14)) 12 false)
      (ctl 6 3 "daw-chevron" :dir 1 :on-click (on-track t (step-preset! t 1)))
      (fill))))

;; ── Device slots: click one to open its panel in the top tile ──
(def toggle-device (d)
  (if (= view.open-device d)
    (set! view.open-device nil)
    (do (set! selection.track d.track) (set! view.open-device d))))

(def devices-view (t)
  (subtree :key (list :devices t)
    (wrap :width (sc 42) :gap (sc 0.5) :row-gap (sc 0.5)
      (each t.devices |d|
        ;; wide enough for the name at font 10
        (slot-pill (max 8 (+ 3 (* 0.8 (len d.name)))) d.name (= view.open-device d)
          :dim (not d.enabled)
          :on-click (lambda (e) (toggle-device d)))))))

;; ── Scenes ──
(def shown-bank ()
  (let ((all (banks)))
    (if (< view.bank 0)
      (if transport.scene transport.scene.bank (first all))
      (nth all (min view.bank (- (len all) 1))))))

(def scenes-view ()
  (let ((shown (shown-bank)))
    (v-stack :gap (sc 1)
      (h-stack :gap (sc 1)
        (each (banks) |b|
          (big-pill b.label (= b shown)
            :queued (and b.playing (not (= b shown)))
            :on-click (lambda (e) (set! view.bank b.index))))
        (fill))
      (h-stack :gap (sc 1)
        (each (if shown shown.scenes (list)) |s|
          (big-pill (str s.number) #'s.active
            :queued #'s.queued
            :on-click (lambda (e) (launch! s))
            :on-right-click (lambda (e)
              (do (set! scene-menu.scene s)
                  (set! scene-menu.at e.at)
                  (set! scene-menu.open true)))))
        (fill))
      (subtree :key :scene-menu (scene-menu-view)))))

;; Right-click a scene: Clone (appended to its bank) or Delete.
(def scene-menu-act (act)
  (do (set! scene-menu.open false)
      (act scene-menu.scene)))

(def scene-menu-view ()
  (context-menu :is-open scene-menu.open :anchor scene-menu.at
    :on-close (lambda () (set! scene-menu.open false))
    (menu-item "Clone scene" :key "mini-daw-scene-clone"
      :on-select (lambda (e) (scene-menu-act clone-scene!)))
    (menu-item "Delete scene" :key "mini-daw-scene-delete"
      :disabled (<= (len (scenes)) 1)
      :on-select (lambda (e) (scene-menu-act delete-scene!)))))

;; ── Open device panel ──
(def framed (body)
  (box :background "daw-panel-frame" :padding 2 body))

;; A rack's slots, beside the selected slot's synth.
(def rack-slots (rack)
  (scroll :key "mini-daw-rack-slots" :width (sc 34) :height (+ fx/panel-height 4)
    (wrap :width (sc 33) :gap (sc 0.5) :row-gap (sc 0.5)
      (each (get rack :slots) |slot|
        (slot-pill 16 (str (+ (get slot :idx) 1) "  " (get slot :display-name))
          (= (get slot :idx) (get rack :selected-slot))
          :dim (not (get slot :enabled))
          :on-click (lambda (e) (fx/rack-slot-select slot)))))))

(def device-view (d)
  (let ((panel (fx/device-panel d)))
    (box :flex 1 :h-align :center :v-align :center
      (if (= (get panel :type) "rack")
        (h-stack :gap (sc 1.5) :v-align :center
          (rack-slots panel)
          (framed (fx/device-panel-body d)))
        (framed (fx/device-panel-body d))))))

;; ── Buffers + layout ──
(effect-buffer "*sequencer*"
  (v-stack :padding (sc 1) :gap (sc 1)
    (each (tracks) |t| (track-view t))
    ;; the Save / Save As dialog C-c s opens on an unsaved project
    (subtree :key :file-dialogs (eseq.file-dialogs/panel))))

;; Top tile: transport, then the open device (while the host publishes its
;; panel: its track is selected) or the scenes. "*fx*" is fx/panel-buffer.
(effect-buffer "*fx*"
  (let ((d view.open-device))
    (h-stack :padding (sc 3.5) :width :fill :height :fill :gap (sc 1)
      (v-stack :gap (sc 1)
        (ctl 8 4 "daw-play" :on-click (lambda (e) (toggle! transport.playing)))
        (ctl 8 4 "daw-arm" :armed #'transport.recording
          :on-click (lambda (e) (toggle! transport.recording))))
      (if (fx/device-panel d)
        (subtree :key (list :device d) (device-view d))
        (subtree :key :scenes (scenes-view))))))

;; The top tile's height: tall enough for an unscaled device panel while one
;; shows (scaling never shrinks a panel), else the scaled scenes.
(def top-height ()
  (if (fx/device-panel view.open-device) (max 14 (sc 14)) (sc 14)))

(def mini-daw-shown-height nil)  ; plain global: writing it re-runs nothing

(def mini-daw-show ()
  (let ((h (top-height)))
    (do (set! mini-daw-shown-height h)
        (set-layout
          (list :rows :gap 1
            0.1 (list :buf fx/panel-buffer :borderless true :hide-status true :min-height h :max-height h)
            0.9 (list :buf "*sequencer*" :borderless false :hide-status true))))))

;; Once shown, re-apply the layout only when the top height changes (a panel
;; opens or closes, or leaves with its track's selection).
(observe
  (let ((h (top-height)))
    (when (and mini-daw-shown-height (not (= h mini-daw-shown-height)))
      (mini-daw-show))))

(def mini-daw-save-project () (host-command "project-save-open" (dict :mode "save")))
(bind-key "C-c l" "mini-daw-show")
(bind-key "C-c o" "projects-mode")
(bind-key "C-c s" "mini-daw-save-project")

;; ── projects-mode: RET loads, g refreshes, q closes ──
(def projects '())

(define-mode "projects-mode" :read-only true)
(mode-bind-key "projects-mode" "RET" "projects-open-at-point")
(mode-bind-key "projects-mode" "g" "projects-refresh")
(mode-bind-key "projects-mode" "q" "mini-daw-show")

(def projects-refresh ()
  (do (set! projects (seq-project-tree ""))
      (render-widget nil)
      (set-buffer-lines (map |p| (str (get p :label) "    " (get p :detail)) projects))
      (goto-line 1)
      (status (str (len projects) " projects — RET to load"))))

(def projects-open-at-point ()
  (let ((i (- (current-line-number) 1)))
    (if (and (>= i 0) (< i (len projects)))
      (do (host-command "load-project" (dict :name (get (nth projects i) :label)))
          (mini-daw-show))
      (status "No project on this line"))))

(def projects-mode ()
  (do (switch-or-create-buffer "*projects*")
      (set-view-mode "text")
      (set-buffer-mode "projects-mode")
      (projects-refresh)))
