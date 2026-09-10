;; Shared framing for performance-first drum instruments. Each voice supplies
;; explicit controls and analytic graphics; no fitted parameter auto-grids.
(module eseq.effects.drum-surface)
(export panel bind value envelope dsr-envelope strike pitch-view burst-view cut-view parameter-gesture)
(def bind (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-binding
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
(def value (name) (reactive-value (bind name)))
(def section () eseq.vanilla/custom-ui-selected-section)
(def ink (p)
  (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white :black))
(def control (spec)
  (let ((name (nth spec 0)) (title (nth spec 1)) (decimals (nth spec 2))
        (options (nth spec 3))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (p (eseq.effects.custom-ui-runtime/custom-ui-current-param (nth spec 0))))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "drum-mod-" name)
      (v-stack :width 8.4 :height 1.3 :gap 0
        (label title :height 0.65 :v-align :center :font-size 9 :color (ink p) :bg :transparent)
        (if options
          (dropdown :width 8.4 :height 0.65 :font-size 9 :options options
            :value-index (- (value name) (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p))
            :bg-color :instrument-control-bg :text-color :cyan :chevron-color :cyan :border-color :transparent
            :on-change (lambda (v)
              (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
                (+ (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
                  (eseq.effects.param-controls/custom-ui-option-index options v)))))
          (number-picker :debug-name (str "drum-detail-" name)
            :width 8.4 :height 0.65 :noui true :decimals decimals :font-size 10 :text-align :left
            :step (if (= name "bank_harm") 0.5 (pow 10 (- 0 decimals)))
            :value (bind name) :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) (ink p))
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p)))))))
(def knob (spec section)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section (nth spec 0) (nth spec 1)
    7.0 3.6 2.35 :cyan (nth spec 2) :linear :widget-knob-track 10 9.5 :center))
(def block (spec index active pages)
  (let ((target (if (> (len (filter |p| (= (get p :id) index) pages)) 0) index active)))
  (box :width 14.8 :height 4.8 :padding 0.2
    :background-color (if (= active index) :instrument-panel-bg :instrument-group-bg)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback target)
    (v-stack :gap 0.15
      (box :width 8 :height 0.7 :background-color :cyan
        (label (nth spec 0) :width 8 :height 0.7 :h-align :center :v-align :center
          :font-size 8 :color :black :bg :transparent))
      (h-stack :gap 0.2 (knob (nth spec 1) target) (knob (nth spec 2) target))))))
(def details (specs)
  (v-stack :gap 0.1
    (h-stack :gap 0.3
      (each (range 0 (min 4 (len specs))) |i| (control (nth specs i))))
    (h-stack :gap 0.3
      (each (range 4 (max 4 (len specs))) |i| (control (nth specs i))))))
(def extra-controls (blocks specs)
  (filter |spec|
    (= 0 (len (filter |block|
      (or (= (nth spec 0) (nth (nth block 1) 0))
          (= (nth spec 0) (nth (nth block 2) 0))) blocks))) specs))
(def panel (title blocks pages)
  ;; Original page IDs survive filtering and engine changes. A page has a
  ;; reason to exist only when it exposes controls beyond the persistent knobs.
  (let ((resolved (map |id|
          (let ((p (nth pages id)))
            (dict :id id :title (get p :title) :visual (get p :visual)
              :controls (extra-controls blocks ((get p :controls))))) (range (len pages))))
      (visible (filter |p| (> (len (get p :controls)) 0) resolved))
      (selected (filter |p| (= (get p :id) (section)) visible))
      (page (if (> (len selected) 0) (nth selected 0) (nth visible 0)))
      (active (get page :id)))
    (h-stack :height 9.8 :gap 0.3 :align :start
      (v-stack :gap 0.2 (block (nth blocks 0) 0 active visible) (block (nth blocks 1) 1 active visible))
      (box :debug-name "drum-display" :width 35.8 :height 9.8 :padding 0.3 :background-color :cyan
        (v-stack :gap 0.15
          (label (str title " / " (get page :title)) :width 35.2 :height 0.7
            :h-align :center :v-align :center :font-size 9 :color :cyan :bg :black)
          (box :width 35.2 :height 4.2 ((get page :visual)))
          (box :width 35.2 :height 3.05 (details (get page :controls)))
          (h-stack :gap 0.15
            (each visible |p|
              (button (get p :title) :debug-name (str "drum-page-" (get p :id))
                :width (/ (- 35 (* 0.15 (- (len visible) 1))) (len visible))
                :height 0.8 :padding 0 :font-size 8.6
                :border-color :transparent
                :color (if (= (get p :id) active) :cyan :black)
                :background-color (if (= (get p :id) active) :black :cyan)
                :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback (get p :id)))))))
      (v-stack :gap 0.2 (block (nth blocks 2) 2 active visible) (block (nth blocks 3) 3 active visible)))))

;; Per-instance gesture state preserves the grab offset. X adjusts positive
;; time parameters proportionally; negative exponential rates approach zero
;; when dragged right (a longer decay). Other zero-based ranges use linear motion.
;; Y, when supplied, adjusts the secondary parameter across its actual range.
(def parameter-gesture (x-name y-name)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (xp (eseq.effects.custom-ui-runtime/custom-ui-current-param x-name))
        (yp (if y-name (eseq.effects.custom-ui-runtime/custom-ui-current-param y-name) false))
        (gesture (dict :start nil)))
    (let ((write (lambda (p v)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
              (max (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
                (min (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p) v))))))
      (dict
        :down (lambda (x y region)
          (set! gesture.start (list x y
            (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding xp))
            (if yp (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding yp)) 0))))
        :up (lambda (x y region) (set! gesture.start nil))
        :drag (lambda (x y region)
          (if gesture.start
            (let ((dx (- x (nth gesture.start 0))) (dy (- (nth gesture.start 1) y))
                  (xmin (eseq.effects.custom-ui-runtime/custom-ui-param-control-min xp))
                  (xmax (eseq.effects.custom-ui-runtime/custom-ui-param-control-max xp)))
              (do (write xp (if (> xmin 0)
                    (* (nth gesture.start 2) (exp (* dx 3)))
                    (if (<= xmax 0)
                      (* (nth gesture.start 2) (exp (* dx -3)))
                      (+ (nth gesture.start 2) (* dx (- xmax xmin) 0.25)))))
                  (if yp (write yp (+ (nth gesture.start 3) (* dy 0.25
                    (- (eseq.effects.custom-ui-runtime/custom-ui-param-control-max yp)
                       (eseq.effects.custom-ui-runtime/custom-ui-param-control-min yp))))) false))) false))))))

;; T60 envelope: meaningful for isolated decay parameters, not an audio trace.
(defwidget eseq-drum-decay-envelope
  :width 35.2 :height 1.5 :state (duration) :bindable (duration)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.96) 1.92) 0 1))
        (a (exp (/ (* -6907.7553 u u) (max duration 0.01))))
        (curve (- 0.8 (* 1.55 a))))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :cyan)
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0 0 0 0.16))
      (sdf/paint (- (abs (- y curve)) 0.023) :black))))
(def envelope (title a second-title b edit-a edit-b)
  (let ((ga (parameter-gesture edit-a false)) (gb (parameter-gesture edit-b false)))
  (v-stack :gap 0.12
    (label title :height 0.4 :v-align :center :font-size 8.5 :color :black :bg :transparent)
    (eseq-drum-decay-envelope :debug-name "drum-envelope-a" :duration a
      :on-mouse-down (get ga :down) :on-drag (get ga :drag) :on-mouse-up (get ga :up))
    (label second-title :height 0.4 :v-align :center :font-size 8.5 :color :black :bg :transparent)
    (eseq-drum-decay-envelope :debug-name "drum-envelope-b" :duration b
      :on-mouse-down (get gb :down) :on-drag (get gb :drag) :on-mouse-up (get gb :up)))))
(defwidget eseq-drum-strike-position
  :width 35.2 :height 4.2 :state (sx sy) :bindable (sx sy)
  :shader
  (let ((u (/ x aspect)))
    (sdf/layer
      (sdf/region :head (sdf/rect width height) :cyan)
      (sdf/paint (min
        (- (abs (- (fract (* (+ u 1) 3)) 0.5)) 0.006)
        (- (abs (- (fract (* (+ y 1) 3)) 0.5)) 0.006)) (rgba 0 0 0 0.2))
      (sdf/paint (sdf/translate (* aspect (- (* sx 2) 1)) (- (* sy 2) 1) (sdf/circle 0.065)) :black))))
(def strike ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (let ((change (lambda (x y region)
      (do (eseq.effects.custom-ui-runtime/custom-ui-set-param-by-name-in-scope scope "strike_x" (max 0 (min 1 (/ (+ x 1) 2))))
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-by-name-in-scope scope "strike_y" (max 0 (min 1 (/ (+ y 1) 2))))))))
      (eseq-drum-strike-position :debug-name "drum-strike" :sx (bind "strike_x") :sy (bind "strike_y")
        :on-click change :on-drag change))))

;; Derivative of the identified two-exponential phase law; held fast segment
;; is used by Orbit, zero hold by Virus. Horizontal axis is 250 ms squared.
(defwidget eseq-drum-pitch-trajectory
  :width 35.2 :height 4.1
  :state (base a1 a2 r1 r2 scale tune hold)
  :bindable (base a1 a2 r1 r2 scale tune hold)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.96) 1.92) 0 1))
        (t (* 0.25 u u))
        (f (* (pow 2 (/ tune 12)) (+ base
          (* a1 (exp (/ (* r1 (max 0 (- t (* hold scale)))) scale)))
          (* a2 (exp (/ (* r2 t) scale))))))
        (v (/ f (+ 500 f)))
        (line (- 0.8 (* 1.6 v))))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :cyan)
      (sdf/paint (max (- y 0.8) (- line y)) (rgba 0 0 0 0.13))
      (sdf/paint (- (abs (- y line)) 0.016) :black))))
(def pitch-view (base a1 a2 r1 r2 scale tune hold edit-time)
  (let ((g (parameter-gesture edit-time "tune")))
  (v-stack :gap 0.1
    (eseq-drum-pitch-trajectory :debug-name "drum-pitch"
      :base base :a1 a1 :a2 a2 :r1 r1 :r2 r2 :scale scale :tune tune :hold hold
      :on-mouse-down (get g :down) :on-drag (get g :drag) :on-mouse-up (get g :up))
    (label "Drag X: time / Y: tune / 0–250 ms" :height 0.4 :v-align :center :font-size 8.2 :color :black :bg :transparent))))
(defwidget eseq-drum-burst-timing
  :width 35.2 :height 4.1 :state (count spread decay tail snap body)
  :bindable (count spread decay tail snap body)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.96) 1.92) 0 1))
        (ms (* 500 u u))
        (end (* (- (round count) 1) spread))
        (burst (if (< ms (+ end (* spread 0.999))) (* snap (exp (/ (* -6.907755 (fract (/ ms spread)) spread) decay))) 0))
        (wash (if (< ms end) 0 (* body (exp (/ (* -6.907755 (- ms end)) tail)))))
        (a (min 1 (+ burst wash))))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :cyan)
      (sdf/paint (max (- y 0.8) (- (- 0.8 (* 1.6 a)) y)) :black))))
(def burst-view ()
  (let ((g (parameter-gesture "sprd" "bdec")))
  (v-stack :gap 0.1
    (eseq-drum-burst-timing :debug-name "drum-bursts" :count (bind "bursts") :spread (bind "sprd")
      :decay (bind "bdec") :tail (bind "dec") :snap (bind "snap") :body (bind "body")
      :on-mouse-down (get g :down) :on-drag (get g :drag) :on-mouse-up (get g :up))
    (label "Drag X: spread / Y: burst decay / 0–500 ms" :height 0.4 :v-align :center :font-size 8.2 :color :black :bg :transparent))))
(defwidget eseq-drum-layer-cut
  :width 35.2 :height 1.5 :state (length scale) :bindable (length scale)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.96) 1.92) 0 1))
        (t (* 1200 u u))
        (end (* length scale))
        (start (* end (/ 0.17 0.180514)))
        (v (clamp (/ (- t start) (max 0.01 (- end start))) 0 1))
        (a (- 1 (* v v (- 3 (* 2 v))))))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :cyan)
      (sdf/paint (- (abs (- y (- 0.8 (* 1.55 a)))) 0.024) :black))))
(def cut-view ()
  (let ((ga (parameter-gesture "decay" false)) (gb (parameter-gesture "ring" false)))
  (v-stack :gap 0.12
    (label "Body cut / 0–1.2 s" :height 0.4 :v-align :center :font-size 8.5 :color :black :bg :transparent)
    (eseq-drum-layer-cut :length (bind "length") :scale (bind "decay")
      :on-mouse-down (get ga :down) :on-drag (get ga :drag) :on-mouse-up (get ga :up))
    (label "Shell cut / 0–1.2 s" :height 0.4 :v-align :center :font-size 8.5 :color :black :bg :transparent)
    (eseq-drum-layer-cut :length (bind "length") :scale (bind "ring")
      :on-mouse-down (get gb :down) :on-drag (get gb :drag) :on-mouse-up (get gb :up)))))

;; Orbit's analytic attack-normalized quadratic body envelope, before sidebands
;; and output saturation. This is a mechanism view, not a mixed audio waveform.
(defwidget eseq-drum-quadratic-envelope
  :width 35.2 :height 4.1 :state (attack decay linear quadratic)
  :bindable (attack decay linear quadratic)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.96) 1.92) 0 1))
        (t (* 0.5 u u))
        (ramp (/ (- 1 (exp (/ (* -1 t) attack))) (- 1 (exp (/ -0.05 attack)))))
        (a (clamp (* ramp (exp (/ (+ (* linear t) (* quadratic t t)) decay))) 0 1)))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :cyan)
      (sdf/paint (max (- y 0.8) (- (- 0.8 (* 1.6 a)) y)) (rgba 0 0 0 0.16))
      (sdf/paint (- (abs (- y (- 0.8 (* 1.6 a)))) 0.016) :black))))
(export quadratic-envelope)
(def quadratic-envelope (attack decay linear quadratic)
  (let ((g (parameter-gesture "decay" "attack")))
  (v-stack :gap 0.1
    (eseq-drum-quadratic-envelope :attack attack :decay decay :linear linear :quadratic quadratic
      :on-mouse-down (get g :down) :on-drag (get g :drag) :on-mouse-up (get g :up))
    (label "Drag X: decay / Y: attack / 0–500 ms" :height 0.4 :v-align :center :font-size 8.2 :color :black :bg :transparent))))


;; Held amplitude approaches Sustain; release starts at the note-off level.
;; The release plot is normalized to that level, since note length is external.
(defwidget eseq-drum-sustain-envelope
  :width 35.2 :height 1.5 :state (duration sustain) :bindable (duration sustain)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.96) 1.92) 0 1))
        (a (+ sustain (* (- 1 sustain) (exp (/ (* -6907.7553 u u) (max duration 0.01))))))
        (curve (- 0.8 (* 1.55 a))))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :cyan)
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0 0 0 0.16))
      (sdf/paint (- (abs (- y curve)) 0.023) :black))))
(def dsr-envelope (decay-time)
  (let ((decay (parameter-gesture "dec" "sustain"))
        (release (parameter-gesture "release" false)))
    (v-stack :gap 0.12
      (label "Held: decay to sustain / 0–1 s" :height 0.4 :v-align :center :font-size 8.5 :color :black :bg :transparent)
      (eseq-drum-sustain-envelope :duration decay-time :sustain (bind "sustain")
        :on-mouse-down (get decay :down) :on-drag (get decay :drag) :on-mouse-up (get decay :up))
      (label "Note-off: release from current level / 0–1 s" :height 0.4 :v-align :center :font-size 8.5 :color :black :bg :transparent)
      (eseq-drum-decay-envelope :duration (bind "release")
        :on-mouse-down (get release :down) :on-drag (get release :drag) :on-mouse-up (get release :up)))))
