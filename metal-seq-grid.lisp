; Minimal Metal Sequencer - Step Grid UI
; C-p to toggle play/stop, Esc to deselect

(bind-key "C-p" "seq-toggle-play")
(bind-key "ESC" "seq-clear-selection")

; 0=vel 1=dur 2=transpose 3=pan
(defstate param-mode 0)

(def param-values ()
  (if (= param-mode 0) SEQ.velocities
    (if (= param-mode 1) SEQ.durations
      (if (= param-mode 2) SEQ.transposes
        SEQ.pans))))

(def param-min ()
  (if (= param-mode 0) 0
    (if (= param-mode 1) 0.1
      (if (= param-mode 2) -12
        -1))))

(def param-max ()
  (if (= param-mode 0) 1
    (if (= param-mode 1) 2
      (if (= param-mode 2) 12
        1))))

(def param-keyword ()
  (if (= param-mode 0) :velocity
    (if (= param-mode 1) :duration
      (if (= param-mode 2) :transpose
        :pan))))

(def param-color ()
  (if (= param-mode 0) :blue
    (if (= param-mode 1) :green
      (if (= param-mode 2) :yellow
        :red))))

(def param-origin ()
  (if (= param-mode 2) 0
    (if (= param-mode 3) 0
      (param-min))))

;; ── Aqua material for sliders ──

(defmacro aqua-slider-material ()
  `(material
     :lighting (lighting :edge-min -0.35 :edge-max 0.5
       :light (vec3 -0.5 -1.0 1.5) :shininess 32.0)
     :color (aqua-color (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0))))

;; ── Aqua widgets ──

(defmacro aqua-color (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
            (__base (mix ,base1
                ,base2
                (smoothstep -0.5 0.5 __ny)))
            (__glass (smoothstep 0.1 -0.65 __ny))
            (__edge-fade (smoothstep 0.0 -0.26 d))
            (__hi (* __glass __edge-fade 0.655))
            (__spec (* specular __edge-fade 0.3))
            (__bot (* (smoothstep 0.3 0.5 __ny)
                (smoothstep 0.65 0.5 __ny)
                __edge-fade 0.12))
            (__rim (smoothstep -0.03 -0.0983 d)))
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
  (let ((sel-y (if (= selected 1) (* 0.3 (cos (* 3 itime))) 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (+ (* 0.001 (smoothstep 0 0.1 (* y x))) (sdf/fill-rounded-rect -0.01 0.85))
          (material
            :lighting
            (lighting :edge-min -0.35 :edge-max 0.5
              :light (vec3 (cos (* 0.3 itime)) -1.0 (+ (* 0.2 (cos itime)) 1.5)) :shininess 32.0)
            :color
            (* (if (= active 1) 1 0.7) (aqua-color (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0)))
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
                 (if (= plocked 1) (rgba 0.05 0.15 0.1 1.0) (rgba 0.3 0.3 0.85 1.0))
                 (if (= plocked 1) (rgba 0.4 0.135 0.95 1.0) (rgba 0.90 0.50 0.82 1.0))))))))))

;; ── Play/Pause buttons ──

(defwidget play-btn
  :width 4 :height 3
  :paint-margin 1
  :shader
  (sdf/layer
    (sdf/fill
      (let ((p1x -0.5) (p1y -0.7) (p2x -0.5) (p2y 0.7) (p3x 0.8) (p3y 0.0))
        (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
              (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
              (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
          (max (max d1 d2) d3)))
      (material
        :lighting (lighting :edge-min -0.35 :edge-max 0.5
          :light (vec3 0.0 -1.0 1.5) :shininess 32.0)
        :color (aqua-color (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0))
        :shadow (shadow :color (rgba 0 0 0 0.3) :blur 0.15 :offset (vec2 0 0.05))))))

(defwidget pause-btn
  :width 4 :height 3
  :paint-margin 1
  :shader
  (sdf/layer
    (sdf/fill
      (sdf/union
        (sdf/translate -0.35 0 (sdf/rect 0.18 0.6))
        (sdf/translate 0.35 0 (sdf/rect 0.18 0.6)))
      (material
        :lighting (lighting :edge-min -0.35 :edge-max 0.5
          :light (vec3 0.0 -1.0 1.5) :shininess 32.0)
        :color (aqua-color (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0))
        :shadow (shadow :color (rgba 0 0 0 0.3) :blur 0.15 :offset (vec2 0 0.05))))))

;; ── LED display panel ──

(defwidget led-panel
  :width 20 :height 3
  :paint-margin 0.5
  :shader
  (sdf/layer
    (sdf/fill (sdf/fill-rounded-rect 0.02 0.083)
      (material
        :color
        (let ((__ny y)
              (__base (mix (rgba 0.08 0.08 0.08 1.0)
                           (rgba 0.13 0.19 0.19 1.0)
                           (smoothstep -0.35 1.95 __ny)))
              (__vignette (* (smoothstep -0.02 -0.013 (sdf/fill-rounded-rect 0.02 0.083)) 0.15)))
          (+ __base (rgba __vignette __vignette __vignette 0)))
        :shadow (shadow :color (rgba 0 0 0 0.5) :blur 0.1 :offset (vec2 0 0.03))))))

;; ── Main UI ──

(effect-buffer "*metal*"
  (v-stack
    :padding 1
    :gap 1
    
    ; Transport bar: play button + LED display
    (h-stack :gap 1 :align :center
      (box :width 4 :height 3
        :background (if SEQ.playing "pause-btn" "play-btn")
        :on-click |x y r| (seq-toggle-play))
      (box :background "led-panel" :height 3 :width 40
        (h-stack :gap 0 :align :baseline :padding 1
          (label (fmt "{:>2}" (+ (floor (/ (mod SEQ.playhead 16) 4)) 1))
            :font-size 20 :width 4
            :color '(rgba 0.1 0.7 0.9 1)
            :bg :transparent)
          (label "|" :font-size 20 :width 2
            :color '(rgba 0.05 0.4 0.5 1)
            :bg :transparent)
          (label (fmt "{:>2}" (+ (mod (mod SEQ.playhead 16) 4) 1))
            :font-size 20 :width 4
            :color '(rgba 0.1 0.7 0.9 1)
            :bg :transparent)
          (label "|" :font-size 20 :width 2
            :color '(rgba 0.05 0.4 0.5 1)
            :bg :transparent)
          (label (fmt "{:>2}" (+ (mod SEQ.playhead 16) 1))
            :font-size 20 :width 4
            :color '(rgba 0.1 0.7 0.9 1)
            :bg :transparent)
          (label "" :width 4 :bg :transparent)
          (label (str "BPM " SEQ.bpm)
            :font-size 16 :width 12
            :color '(rgba 0.1 0.7 0.9 1)
            :bg :transparent))))
    
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
        :bg (if (= param-mode 2) :yellow :dark-gray)
        :on-click |x y r| (set! param-mode 2)
        (label "xpose" :font-size 12
          :color (if (= param-mode 2) :white :gray)
          :bg :transparent))
      (box :width 8 :height 2
        :bg (if (= param-mode 3) :red :dark-gray)
        :on-click |x y r| (set! param-mode 3)
        (label "pan" :font-size 12
          :color (if (= param-mode 3) :white :gray)
          :bg :transparent)))
    
    ; Step columns: vslider + aqua step toggle + step number
    (grid :cols 16 :col-width 3
      (each (zip SEQ.steps (range 0 16)) |(active i)|
        (v-stack :align :center :gap 0.5
          (vslider :height 4
            :min (param-min) :max (param-max)
            :origin (param-origin)
            :value (nth (param-values) i)
            :material (aqua-slider-material)
            :on-change (lambda (v)
              (if (seq-has-selection?)
                (seq-set-step-param-plock (param-keyword) v)
                (seq-set-step-param i (param-keyword) v))))
          (box :on-click (lambda (evt)
                (if (get evt :shift)
                  (seq-select-step i)
                  (do (seq-clear-selection) (seq-toggle-step i))))
            :active (if active 1 0)
            :plocked 1
            :selected (if (= 1 (nth SEQ.selected-steps i)) 1 0)
            :background "aqua-button"
            :align :center :width 3 :height 1.5
            (tick :active (if active 1 0)
                  :plocked (if (nth SEQ.step-has-plocks i) 1 0)
                  :selected (if (nth SEQ.selected-steps i) 1 0)))
          (label (str (+ i 1)) :font-size 10
            :color (if (nth SEQ.selected-steps i) :yellow
                    (if (= (mod SEQ.playhead 16) i) :white :gray))))))

    ; Mixer — track list with volume sliders
    (v-stack :gap 0.5
      (each SEQ.track-names |name i|
        (h-stack :gap 1 :align :center
          (box :width 14 :height 1
               :bg (if (= SEQ.current-track i) :blue :dark-gray)
               :on-click |x y r| (seq-set-track i)
            (label name :font-size 11
                   :color (if (= SEQ.current-track i) :white :gray)
                   :bg :transparent))
          (box :flex 1
            (hslider :min 0 :max 1
                     :value (nth SEQ.track-volumes i)
                     :material (aqua-slider-material)
                     :on-change (lambda (v) (seq-set-track-volume i v)))))))

    ; Effect chain for current track
    (h-stack :gap 1
      (each SEQ.effects |fx slot-idx|
        (box :width 20 :bg '(rgba 0.08 0.1 0.12 1)
             :padding 0.5
          (v-stack :gap 0.5
            (label (get fx :name) :font-size 12 :color :white :bg :transparent)
            (each (get fx :params) |p pi|
              (h-stack :gap 0.5 :align :center
                (label (get p :name) :font-size 9 :width 8
                       :color :gray :bg :transparent)
                (box :width 10
                  (hslider :min (get p :min) :max (get p :max)
                           :value (get p :value)
                           :material (aqua-slider-material)
                           :on-change (lambda (v)
                             (if (seq-has-selection?)
                               (seq-set-effect-plock slot-idx (get p :idx) v)
                               (seq-set-effect-param slot-idx (get p :idx) v)))))))))))))


(delete-other-windows)
(split-window-right "*metal*")