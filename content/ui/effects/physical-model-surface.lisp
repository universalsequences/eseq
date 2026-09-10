;; A shared rose display and section identity for physical instruments.
;; Instrument files choose their controls and mechanism views explicitly.
(module eseq.effects.physical-model-surface)
(export panel bind envelope reed-view curved-reed-view bore-loss-view column-view flutter-view vibrato-view body-view)

;; Keep this identity independent of the current theme's synth/drum accents.
(defmacro screen () `(rgba 0.24 0.93 0.81 1))
(defmacro screen-ink () `(rgba 0.16 0.06 0.11 1))
(def accent () (eseq.effects.physical-model-surface/screen))
(def ink () (eseq.effects.physical-model-surface/screen-ink))
(def bind (name)
  (eseq.effects.custom-ui-runtime/custom-ui-param-binding
    (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
(def knob (spec section)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section (nth spec 0) (nth spec 1)
    8.5 3.5 2.55 (accent) (nth spec 2) (nth spec 3) :widget-knob-track 11 9.5 :center))
(def block (spec section active)
  (box :width 18 :height 4.8 :padding 0.2 :corner-radius 3
    :debug-name (str "pm-section-" section)
    :background-color (if (= active section) :instrument-panel-bg :instrument-group-bg)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    (v-stack :gap 0.2
      (box :width 17.6 :height 0.65 :corner-radius 1 :background-color (accent)
        (label (nth spec 0) :height 0.65 :h-align :center :v-align :center
          :font-size 8.5 :color (ink) :bg :transparent))
      (h-stack :gap 0.4 (knob (nth spec 1) section) (knob (nth spec 2) section)))))
(def control (spec section)
  (let ((name (nth spec 0)) (p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "pm-detail-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "pm-detail-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width 11.3 :height 1.15 :gap 0.05
          (label (nth spec 1) :height 0.5 :v-align :center :font-size 9 :color (ink) :bg :transparent)
          (number-picker :debug-name (str "pm-value-" name)
            :width 11.3 :height 0.6 :noui true :decimals (nth spec 2) :font-size 10 :text-align :left
            :step (pow 10 (- 0 (nth spec 2)))
            :value (bind name)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) (ink))
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)))))))
(def details (specs section)
  (v-stack :gap 0.15
    (h-stack :gap 0.35 (each (range 0 (min 3 (len specs))) |i| (control (nth specs i) section)))
    (h-stack :gap 0.35 (each (range 3 (max 3 (len specs))) |i| (control (nth specs i) section)))))
(def panel (title blocks pages)
  (let ((active (max 0 (min (- (len pages) 1) eseq.vanilla/custom-ui-selected-section)))
      (page (nth pages active)))
    (h-stack :height 9.8 :gap 0.25 :align :start :debug-name "pm-surface"
      (v-stack :gap 0.2 (block (nth blocks 0) 0 active) (block (nth blocks 1) 1 active))
      (box :debug-name "pm-display" :width 36 :height 9.8 :padding 0.35 :corner-radius 2
        :background-color (accent)
        (v-stack :gap 0.2
          (box :width 35.3 :height 0.7 :background-color (ink)
            (label (str title " / " (get page :title)) :width 35.3 :height 0.7
              :h-align :center :v-align :center :font-size 9 :color (accent) :bg :transparent))
          (box :width 35.3 :height 3.7 ((get page :view)))
          (box :width 35.3 :height 2.5 (details ((get page :controls)) active))
          (label (get page :hint) :width 35.3 :height 0.65 :v-align :center
            :font-size 8.3 :color (ink) :bg :transparent)
          (h-stack :gap 0.15 :height 0.75
            (each (range (len pages)) |i|
              (button (get (nth pages i) :title) :debug-name (str "pm-page-" i)
                :width (/ (- 35.3 (* 0.15 (- (len pages) 1))) (len pages))
                :height 0.75 :padding 0 :font-size 8.5 :corner-radius 16
                :color (if (= i active) (accent) (ink))
                :background-color (if (= i active) (ink) (accent)) :border-color (ink)
                :border-color :transparent
                :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback i))))))
      (v-stack :gap 0.2 (block (nth blocks 2) 2 active) (block (nth blocks 3) 3 active)))))

(def caption (text)
  (label text :width 35.3 :height 0.45 :v-align :center :font-size 8.5 :color (ink) :bg :transparent))
(def envelope (section)
  (envelope-titled section "Breath envelope / drag the contour"))
(def envelope-titled (section title)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (v-stack :gap 0.15
      (caption title)
      (adsr-editor :debug-name "pm-envelope" :width 35.3 :height 3.1
        :attack (bind "amp.attack") :decay (bind "amp.decay")
        :sustain (bind "amp.sustain") :release (bind "amp.release")
        :attack-max (get (eseq.effects.custom-ui-runtime/custom-ui-current-param "amp.attack") :max)
        :decay-max (get (eseq.effects.custom-ui-runtime/custom-ui-current-param "amp.decay") :max)
        :release-max (get (eseq.effects.custom-ui-runtime/custom-ui-current-param "amp.release") :max)
        :curve-color (ink) :point-color (ink) :grid-color (ink) :background-color (accent)
        :on-change (lambda (env)
          (do
            (eseq.effects.custom-ui-sections/custom-ui-select-section-in-scope scope section)
            (eseq.effects.custom-ui-sections/custom-ui-set-active-adsr scope section (get env :active))
            (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
              "amp.attack" "amp.decay" "amp.sustain" "amp.release" env)))))))

;; Analytic mechanism views, not an audio scope. Every varying shader input
;; is bindable so automation can update the drawing without rebuilding the UI.
(defwidget pm-reed-response
  :width 35.3 :height 3.1 :state (slope closure direction curvature) :bindable (slope closure curvature)
  :shader
  (let ((delta (clamp (/ x aspect) -1 1))
        (linear (clamp (+ closure (* direction (max 0.1 slope) delta)) -1 1))
        (magnitude (pow (abs linear) curvature))
        (reflection (if (< linear 0) (- magnitude) magnitude)))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (min (- (abs y) 0.005) (- (abs x) 0.012)) (rgba 0.16 0.06 0.11 0.22))
      (sdf/paint (- (abs (+ y (* 0.8 reflection))) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def reed-view (slope closure direction)
  (curved-reed-view slope closure direction 1))
(def curved-reed-view (slope closure direction curvature)
  (v-stack :gap 0.15
    (caption "Reed reflection / pressure difference -1 to +1")
    (pm-reed-response :debug-name "pm-reed" :slope slope :closure closure :direction direction :curvature curvature)))

(defwidget pm-bore-loss
  :width 35.3 :height 3.1 :state (cutoff loss) :bindable (cutoff loss)
  :shader
  (let ((u (* 0.5 (+ 1 (/ x aspect))))
        (f (* 40 (pow 400 u)))
        (ratio (/ f cutoff))
        (a (/ (- 1 loss) (sqrt (+ 1 (* ratio ratio)))))
        (curve (- 0.8 (* 1.6 a))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.12))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def bore-loss-view (cutoff loss)
  (v-stack :gap 0.15
    (caption "Bore reflection / nominal magnitude / 40 Hz-16 kHz")
    (pm-bore-loss :debug-name "pm-bore-loss" :cutoff cutoff :loss loss)))

(defwidget pm-air-column
  :width 35.3 :height 3.1 :state (position) :bindable (position)
  :shader
  (let ((marker (* aspect (+ -0.88 (* 1.76 position)))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/stroke (sdf/rect (* width 0.88) 0.45) 0.025 (eseq.effects.physical-model-surface/screen-ink))
      (sdf/paint (max (- (abs (- x marker)) 0.024) (- (abs y) 0.65)) (eseq.effects.physical-model-surface/screen-ink))
      (sdf/paint (max (max (- x marker) (- (* aspect -0.88) x)) (- (abs y) 0.43))
        (rgba 0.16 0.06 0.11 0.13)))))
(def column-view (position title)
  (v-stack :gap 0.15 (caption title)
    (pm-air-column :debug-name "pm-column" :position position)))

(defwidget pm-flutter-motion
  :width 35.3 :height 3.1 :state (rate drift depth floor) :bindable (rate drift depth floor)
  :shader
  (let ((t (+ 1 (/ x aspect)))
        (lo (- 1 depth))
        (hi (- 1 (min depth floor)))
        (bottom (mix lo hi (* 0.5 (+ 1 (cos (* 6.2831853 drift t))))))
        (a (mix bottom 1 (* 0.5 (+ 1 (cos (* 6.2831853 rate t))))))
        (curve (- 0.8 (* 1.6 a))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.12))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def flutter-view ()
  (v-stack :gap 0.15
    (caption "Breath multiplier / 0-2 seconds")
    (pm-flutter-motion :debug-name "pm-flutter" :rate (bind "flutter.rate")
      :drift (bind "flutter.drift_hz") :depth (bind "flutter.depth") :floor (bind "flutter.floor"))))

(defwidget pm-vibrato-motion
  :width 35.3 :height 3.1 :state (rate depth wait) :bindable (rate depth wait)
  :shader
  (let ((t (+ 1 (/ x aspect)))
        (fade (clamp (/ (- t (* wait 0.001)) 0.15) 0 1))
        (curve (* -0.8 (/ depth 100) fade (sin (* 6.2831853 rate t)))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (- (abs y) 0.006) (rgba 0.16 0.06 0.11 0.25))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def vibrato-view ()
  (v-stack :gap 0.15
    (caption "Pitch vibrato / +/-100 cents / 0-2 seconds")
    (pm-vibrato-motion :debug-name "pm-vibrato" :rate (bind "expression.vib_hz")
      :depth (bind "expression.vib_cent") :wait (bind "expression.vib_wait"))))

;; Normalized analogue bandpass magnitude is a nominal response preview;
;; the DSP's discrete SVF and bell lowpass determine the rendered spectrum.
(defwidget pm-body-response
  :width 35.3 :height 3.1 :state (frequency q amount) :bindable (frequency q amount)
  :shader
  (let ((u (* 0.5 (+ 1 (/ x aspect))))
        (f (* 40 (pow 400 u)))
        (ratio (/ f frequency))
        (imag (/ ratio q))
        (real (- 1 (* ratio ratio)))
        (denominator (+ (* real real) (* imag imag)))
        (mixed-real (+ (- 1 amount) (* amount (/ (* imag imag) denominator))))
        (mixed-imag (* amount (/ (* real imag) denominator)))
        (a (sqrt (+ (* mixed-real mixed-real) (* mixed-imag mixed-imag))))
        (curve (- 0.8 (* 1.6 a))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.12))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def body-view ()
  (v-stack :gap 0.15
    (caption "Body band / nominal magnitude / 40 Hz-16 kHz")
    (pm-body-response :debug-name "pm-body" :frequency (bind "color.body_hz")
      :q (bind "color.body_q") :amount (bind "color.body"))))

(export partials-view)
(defwidget pm-flute-body-partials
  :width 35.3 :height 3.1 :state (frequency stretch brightness) :bindable (frequency stretch brightness)
  :shader
  (let ((u (* 0.5 (+ 1 (/ x aspect))))
        (f (* 40 (pow 350 u)))
        (f1 (clamp frequency 40 8000))
        (f2 (clamp (* frequency 2 stretch) 60 12000))
        (f3 (clamp (* frequency 3 stretch stretch) 80 13000))
        (f4 (clamp (* frequency 4 stretch stretch stretch) 100 14000))
        (w2 (mix 0.35 0.85 brightness))
        (w3 (mix 0.15 0.65 brightness))
        (w4 (mix 0.05 0.5 brightness)))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint
        (min
          (max (- (abs (- (/ f f1) 1)) 0.018) (- (abs y) 0.8))
          (max (- (abs (- (/ f f2) 1)) 0.018) (- (abs (- y (- 0.8 w2))) w2))
          (max (- (abs (- (/ f f3) 1)) 0.018) (- (abs (- y (- 0.8 w3))) w3))
          (max (- (abs (- (/ f f4) 1)) 0.018) (- (abs (- y (- 0.8 w4))) w4)))
        (eseq.effects.physical-model-surface/screen-ink)))))
(def partials-view ()
  (v-stack :gap 0.15
    (caption "Body partials / center frequencies / 40 Hz-14 kHz")
    (pm-flute-body-partials :debug-name "pm-partials"
      :frequency (bind "freq") :stretch (bind "stretch") :brightness (bind "bright"))))

(export envelope-titled bow-view pluck-view cello-body-view section-view)
(defwidget pm-bow-friction
  :width 35.3 :height 3.1 :state (pressure curve speed) :bindable (pressure curve speed)
  :shader
  (let ((relative (* 0.8 (/ x aspect)))
        (slip (+ 0.75 (/ (abs relative) (+ 0.1 (* 0.6 pressure)))))
        (grip (min 0.999 (pow slip (- curve))))
        (line (- 0.8 (* 1.6 grip)))
        (marker (* aspect (/ speed 0.8))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- y 0.8) (- line y)) (rgba 0.16 0.06 0.11 0.12))
      (sdf/paint (- (abs (- y line)) 0.025) (eseq.effects.physical-model-surface/screen-ink))
      (sdf/paint (- (abs (- x marker)) 0.009) (rgba 0.16 0.06 0.11 0.45)))))
(def bow-view ()
  (v-stack :gap 0.15
    (caption "Bow grip / relative velocity / marker = bow speed")
    (pm-bow-friction :debug-name "pm-bow" :pressure (bind "bow.pressure")
      :curve (bind "bow.rosin") :speed (bind "bow.speed"))))

(defwidget pm-pluck-pulse
  :width 35.3 :height 3.1 :state (duration amount texture) :bindable (duration amount texture)
  :shader
  (let ((t (* 12.5 (+ 1 (/ x aspect))))
        (phase (clamp (/ t duration) 0 1))
        (pulse (* amount (sin (* 3.14159265 phase))))
        (line (- 0.8 (* 1.6 pulse))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (- (abs (- y line)) (+ 0.015 (* 0.35 pulse texture))) (rgba 0.16 0.06 0.11 0.15))
      (sdf/paint (- (abs (- y line)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def pluck-view ()
  (v-stack :gap 0.15
    (caption "Pluck force pulse / 0-25 ms / shading = texture")
    (pm-pluck-pulse :debug-name "pm-pluck" :duration (bind "pluck.width_ms")
      :amount (bind "pluck.strength") :texture (bind "pluck.texture"))))

(defwidget pm-cello-body
  :width 35.3 :height 3.1
  :state (amount size q f1 g1 f2 g2 f3 g3 f4 g4)
  :bindable (amount size q f1 g1 f2 g2 f3 g3 f4 g4)
  :shader
  (let ((u (* 0.5 (+ 1 (/ x aspect))))
        (f (* 40 (pow 400 u)))
        (r1 (/ (* f size) f1)) (r2 (/ (* f size) f2))
        (r3 (/ (* f size) f3)) (r4 (/ (* f size) f4))
        (i1 (/ r1 q)) (i2 (/ r2 q)) (i3 (/ r3 q)) (i4 (/ r4 q))
        (a1 (- 1 (* r1 r1))) (a2 (- 1 (* r2 r2)))
        (a3 (- 1 (* r3 r3))) (a4 (- 1 (* r4 r4)))
        (d1 (+ (* a1 a1) (* i1 i1))) (d2 (+ (* a2 a2) (* i2 i2)))
        (d3 (+ (* a3 a3) (* i3 i3))) (d4 (+ (* a4 a4) (* i4 i4)))
        (total (max 0.001 (+ g1 g2 g3 g4)))
        (re (/ (+ (* g1 (/ (* i1 i1) d1)) (* g2 (/ (* i2 i2) d2))
          (* g3 (/ (* i3 i3) d3)) (* g4 (/ (* i4 i4) d4))) total))
        (im (/ (+ (* g1 (/ (* a1 i1) d1)) (* g2 (/ (* a2 i2) d2))
          (* g3 (/ (* a3 i3) d3)) (* g4 (/ (* a4 i4) d4))) total))
        (real (+ (- 1 amount) (* amount re)))
        (imag (* amount im))
        (magnitude (sqrt (+ (* real real) (* imag imag))))
        (curve (- 0.8 (* 1.6 magnitude))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.12))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def cello-body-view ()
  (v-stack :gap 0.15
    (caption "Wood modes / nominal magnitude / 40 Hz-16 kHz")
    (pm-cello-body :debug-name "pm-cello-body" :amount (bind "body.wood") :size (bind "body.size")
      :q (bind "body.resonance") :f1 (bind "body.low_hz") :g1 (bind "body.low")
      :f2 (bind "body.mid_hz") :g2 (bind "body.mid") :f3 (bind "body.high_hz") :g3 (bind "body.high")
      :f4 (bind "body.air_hz") :g4 (bind "body.air"))))

(defwidget pm-string-section
  :width 35.3 :height 3.1 :state (amount spread) :bindable (amount spread)
  :shader
  (let ((offset (* aspect (/ spread 55))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- (abs x) 0.025) (- (abs y) 0.8)) (eseq.effects.physical-model-surface/screen-ink))
      (sdf/paint (max (- (min (abs (- x offset)) (abs (+ x offset))) 0.025)
        (- (abs y) (* 0.65 amount))) (rgba 0.16 0.06 0.11 0.6)))))
(def section-view ()
  (v-stack :gap 0.15
    (caption "Player tuning / center note +/-55 cents")
    (pm-string-section :debug-name "pm-section-spread" :amount (bind "section.blend") :spread (bind "section.spread"))))

(export piano-hammer-view piano-string-view piano-body-view piano-tuning-view piano-damper-view piano-motion-view piano-output-view)

(defwidget pm-piano-hammer
  :width 35.3 :height 3.1 :state (hardness contact position) :bindable (hardness contact position)
  :shader
  (let ((t (* 1.5 (+ 1 (/ x aspect))))
        (tau (* 0.12 contact (pow 2 (* 3 (- 0.5 hardness)))))
        (u (/ t tau))
        (force (* 0.25 u u (pow 2.7182818 (- 2 u))))
        (curve (- 0.78 (* 1.5 force))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- y 0.78) (- curve y)) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink))
      (sdf/paint (max (- (abs (- x (* aspect (- (* position 2) 1)))) 0.014)
        (- (abs (+ y 0.89)) 0.08)) (eseq.effects.physical-model-surface/screen-ink)))))
(def piano-hammer-view ()
  (v-stack :gap 0.15 (caption "Felt force / C4, full velocity / 0-3 ms")
    (pm-piano-hammer :debug-name "pm-piano-hammer" :hardness (bind "hammer.hardness")
      :contact (bind "hammer.contact") :position (bind "hammer.position"))))

(defwidget pm-piano-strings
  :width 35.3 :height 3.1 :state (decay damping stiffness aftersound) :bindable (decay damping stiffness aftersound)
  :shader
  (let ((u (* 0.5 (+ 1 (/ x aspect))))
        (n (+ 1 (floor (* u 16))))
        (slot (- (* u 16) (floor (* u 16))))
        (a (* (pow n (* -0.35 damping)) (min 1 (+ 0.45 (* decay 0.2)))))
        (top (- 0.8 (* 1.5 a)))
        (shift (* 0.0003 stiffness n n)))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (max (- (abs (- slot (+ 0.35 shift))) 0.14) (- top y)) (- y 0.8))
        (eseq.effects.physical-model-surface/screen-ink))
      (sdf/paint (max (max (- (abs (- slot (+ 0.62 shift))) 0.06)
        (- (- 0.8 (* a aftersound 0.5)) y)) (- y 0.8)) (rgba 0.16 0.06 0.11 0.3)))))
(def piano-string-view ()
  (v-stack :gap 0.15 (caption "String modes / decay, damping, stiffness and aftersound")
    (pm-piano-strings :debug-name "pm-piano-strings" :decay (bind "string.decay")
      :damping (bind "string.damping") :stiffness (bind "string.stiffness") :aftersound (bind "string.aftersound"))))

(defwidget pm-piano-body
  :width 35.3 :height 3.1 :state (size amount color low high) :bindable (size amount color low high)
  :shader
  (let ((u (* 0.5 (+ 1 (/ x aspect))))
        (f (* 40 (pow 400 u)))
        (r1 (/ (* f size) low)) (r2 (/ (* f size) high))
        (i1 (* 0.5 r1)) (i2 (* 0.5 r2))
        (a1 (/ i1 (sqrt (+ (* (- 1 (* r1 r1)) (- 1 (* r1 r1))) (* i1 i1)))))
        (a2 (/ i2 (sqrt (+ (* (- 1 (* r2 r2)) (- 1 (* r2 r2))) (* i2 i2)))))
        (curve (- 0.65 (* 0.65 amount (+ (* (- 1 color) a1) (* (+ 1 color) a2))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.12))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def piano-body-view ()
  (v-stack :gap 0.15 (caption "Soundboard color / nominal resonances / 40 Hz-16 kHz")
    (pm-piano-body :debug-name "pm-piano-body" :size (bind "body.size") :amount (bind "body.resonance")
      :color (bind "body.color") :low (bind "body.low_hz") :high (bind "body.high_hz"))))

(defwidget pm-piano-unison
  :width 35.3 :height 3.1 :state (spread stereo stretch) :bindable (spread stereo stretch)
  :shader
  (let ((u (+ 1 (/ x aspect)))
        (envelope (* 0.22 (sin (* 1.5707963 u))))
        (center (* envelope (sin (* 31.415927 u))))
        (left (+ (* -0.5 stereo) (* envelope (sin (* (+ 31.415927 (* spread 0.14)) u)))))
        (right (+ (* 0.5 stereo) (* envelope (sin (* (- 31.415927 (* spread 0.14)) u)))))
        (distance (min (abs (- y center)) (min (abs (- y left)) (abs (- y right))))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (- distance 0.024) (eseq.effects.physical-model-surface/screen-ink))
      (sdf/paint (max (- (abs (- y (+ 0.85 (* -0.14 stretch (/ x aspect))))) 0.012)
        (- (abs x) (* aspect 0.9))) (rgba 0.16 0.06 0.11 0.35)))))
(def piano-tuning-view ()
  (v-stack :gap 0.15 (caption "Unison / detuning, stereo spread and tuning stretch")
    (pm-piano-unison :debug-name "pm-piano-unison" :spread (bind "tuning.unison")
      :stereo (bind "tuning.width") :stretch (bind "tuning.stretch"))))

(defwidget pm-piano-dampers
  :width 35.3 :height 3.1 :state (release pedal upper) :bindable (release pedal upper)
  :shader
  (let ((t (* 0.5 (+ 1 (/ x aspect))))
        (rate (/ (* 6.907755 (- 1 pedal) (- 1 pedal)) release))
        (a (pow 2.7182818 (* (- rate) t)))
        (curve (- 0.8 (* 1.6 a))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (max (- y 0.8) (- curve y)) (rgba 0.16 0.06 0.11 0.13))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink))
      (sdf/paint (- (abs (- y (- 0.8 (* 1.6 upper)))) 0.01) (rgba 0.16 0.06 0.11 0.25)))))
(def piano-damper-view ()
  (v-stack :gap 0.15 (caption "Key-up damping / pedal lift / first second after release")
    (pm-piano-dampers :debug-name "pm-piano-dampers" :release (bind "damper.release_s")
      :pedal (bind "damper.pedal") :upper (bind "damper.upper_free"))))

(defwidget pm-piano-motion
  :width 35.3 :height 3.1 :state (depth rate pan panrate) :bindable (depth rate pan panrate)
  :shader
  (let ((t (+ 1 (/ x aspect)))
        (amp (- 1 (* depth 0.5 (+ 1 (sin (* 6.2831853 rate t))))))
        (center (* 0.48 pan (sin (* 6.2831853 panrate t))))
        (edge (* amp 0.35)))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (- (abs (- y center)) edge) (rgba 0.16 0.06 0.11 0.14))
      (sdf/paint (- (abs (- (abs (- y center)) edge)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def piano-motion-view ()
  (v-stack :gap 0.15 (caption "Tremolo and pan / 0-2 seconds")
    (pm-piano-motion :debug-name "pm-piano-motion" :depth (bind "motion.tremolo")
      :rate (bind "motion.tremolo_hz") :pan (bind "motion.pan") :panrate (bind "motion.pan_hz"))))

(defwidget pm-piano-output
  :width 35.3 :height 3.1 :state (drive gain) :bindable (drive gain)
  :shader
  (let ((input (/ x aspect))
        (v (* input (+ 1 (* drive 12))))
        (e (pow 2.7182818 (* 2 v)))
        (shaped (/ (/ (- e 1) (+ e 1)) (+ 1 (* drive 3))))
        (curve (* -0.85 gain (mix input shaped drive))))
    (sdf/layer
      (sdf/fill (sdf/rect width height) (eseq.effects.physical-model-surface/screen))
      (sdf/paint (min (- (abs y) 0.005) (- (abs x) 0.012)) (rgba 0.16 0.06 0.11 0.2))
      (sdf/paint (- (abs (- y curve)) 0.025) (eseq.effects.physical-model-surface/screen-ink)))))
(def piano-output-view ()
  (v-stack :gap 0.15 (caption "Output drive / transfer curve")
    (pm-piano-output :debug-name "pm-piano-output" :drive (bind "output.drive") :gain (bind "output.gain"))))
