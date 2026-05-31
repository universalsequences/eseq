;; metal-seq-transport.lisp — Transport bar UI (Logic Pro style)
;; Renders to *transport* buffer. Loaded by metal-seq-grid.lisp.

;; ── Shared container backgrounds ──

(defwidget transport-btn-bg
  :width 1 :height 1
  :paint-margin 0.3
  :state (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.7)
      (material :color (if active (rgba 0.00 0.35 0.82 1.0) (rgba 0.18 0.18 0.20 1.0))
        :shadow (shadow :color (rgba 0 0 0 0.4) :blur 0.08 :offset (vec2 0 0.03))))))

(defwidget transport-led-bg
  :width 1 :height 1
  :paint-margin 0.3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.7)
      (material
        :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
          :light (vec3 -0.31 -0.4851 1.0) :shininess 51.0)
        :color
        (let ((base (rgba 0.003 0.003 0.004 1.0))
              (lit (+ 0.02 (* 0.02 diffuse)))
              (shine (* 0.10 specular)))
          (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
        :shadow (shadow :color (rgba 0 0 0 0.5) :blur 0.06 :offset (vec2 0 0.02))))))

(defwidget transport-master-meter
  :width 10.5 :height 0.34
  :paint-margin 0.012
  :state (level)
  :bindable (level)
  :shader
  (let ((lvl (min 1.0 (max 0.0 level)))
        (track (sdf/rounded-rect width height height))
        (green-end (min lvl 0.60))
        (yellow-end (min lvl 0.85))
        (red-end lvl))
    (sdf/layer
      (sdf/fill track
        (material :color (rgba 0.06 0.07 0.08 1)))
      (if (> green-end 0.005)
        (sdf/fill
          (let ((__start 0.0)
                (__end green-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.32)
                (__radius (min 0.16 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.34 0.86 0.40 1)))
        (rgba 0 0 0 0))
      (if (> (- yellow-end 0.60) 0.005)
        (sdf/fill
          (let ((__start 0.60)
                (__end yellow-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.32)
                (__radius (min 0.16 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.86 0.72 0.22 1)))
        (rgba 0 0 0 0))
      (if (> (- red-end 0.85) 0.005)
        (sdf/fill
          (let ((__start 0.85)
                (__end red-end)
                (__half_w (* 0.5 aspect (- __end __start)))
                (__half_h 0.32)
                (__radius (min 0.16 (min __half_h (max __half_w 0.001)))))
            (let ((x (+ (* 0.5 x) (* 0.5 aspect (- 1.0 (+ __start __end)))))
                  (y (* 0.5 y)))
              (sdf/rounded-rect __half_w __half_h __radius)))
          (material :color (rgba 0.92 0.24 0.22 1)))
        (rgba 0 0 0 0))
      (sdf/fill
        track
        (material :color
          (rgba
            (+ 0.02 (* 0.03 (smoothstep 0.0 0.8 (- y))))
            (+ 0.02 (* 0.03 (smoothstep 0.0 0.8 (- y))))
            (+ 0.03 (* 0.05 (smoothstep 0.0 0.8 (- y))))
            0.18))))))

(defwidget pattern-pill-bg
  :width 1 :height 1
  :state (active)
  :bindable (active)
  :paint-margin 0.3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height 0.54)
      (material 
        :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
          :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
        :color 
        (if (> active 0)
          (let ((base (rgba 0.00 0.01 0.42 1.0))
                (lit (+ 0.06 (* 0.03 diffuse)))
                (shine (* 0.25 specular)))
            (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
          (if hit/hover
            (let ((base (rgba 0.10 0.10 0.12 0.72))
                  (lit (+ 0.06 (* 0.03 diffuse)))
                  (shine (* 0.25 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
            (rgba 0 0 0 0)))))))


(defwidget pattern-pill-btn-bg
 :width 1 :height 1
  :state (active)
  :paint-margin 0.3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height height)
      (material 
        :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
          :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
        :color 
        (if (> active 0)
          (let ((base (rgba 0.00 0.01 0.02 1.0))
                (lit (+ 0.06 (* 0.03 diffuse)))
                (shine (* 0.25 specular)))
            (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
          (if hit/hover
            (let ((base (rgba 0.10 0.10 0.12 0.72))
                  (lit (+ 0.06 (* 0.03 diffuse)))
                  (shine (* 0.25 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))
            (rgba 0 0 0 0)))))))

(defwidget add-track-icon
  :width 2.5 :height 2.5
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) (rgba 1 1 1 1.0) (rgba 0.75 0.75 0.78 1.0))))
    (sdf/layer
        (rgba 0 0 0 0)
      (sdf/fill (sdf/rounded-rect 0.12 0.72 0.05)
        (material :color fg-col))
      (sdf/fill (sdf/rounded-rect 0.72 0.12 0.05)
        (material :color fg-col)))))

(defwidget save-icon
  :width 2.8 :height 1.4
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (rgba 0.92 0.92 0.96 1.0))
      (bg-col (if (= active 1)
          (rgba 0.00 0.35 0.82 1.0)
          (rgba 0.18 0.18 0.20 1.0))))
    (sdf/layer
      (sdf/fill
        (sdf/rounded-rect width height 0.4)
        (material :color bg-col))
      
      (sdf/fill
        (sdf/translate 0.0 -0.60
          (sdf/rounded-rect 0.42 0.32 0.12))
        (material :color fg-col))
            (sdf/fill
        (sdf/translate 0.22 -0.60
          (sdf/rounded-rect 0.14 0.26 0.1))
        (material :color bg-col))
      (sdf/fill
        (sdf/translate 0.0 0.38
          (sdf/rounded-rect 0.48 0.33 0.12))
        (material :color fg-col)))))

(defwidget transport-tool-chip-bg
  :width 1 :height 1
  :state (active)
  :paint-margin 0.3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height height)
      (material
        :color (if (= active 1)
                 (rgba 0.00 0.35 0.82 1.0)
                 (rgba 0.18 0.18 0.20 1.0))
        :shadow (shadow :color (rgba 0 0 0 0.42) :blur 0.06 :offset (vec2 0 0.02))))))

;; ── Button widgets — icons scaled 2x ──

;; Rewind: two left-pointing triangles (mirrored play triangle)
(defwidget rw-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :shader
  (sdf/layer
    ;; Left triangle
    (sdf/fill
      (sdf/translate -0.25 0
        (let ((p1x 0.35) (p1y 0.5) (p2x 0.35) (p2y -0.5) (p3x -0.35) (p3y 0.0))
          (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
            (max (max d1 d2) d3))))
      (material :color (rgba 0.75 0.75 0.78 1.0)))
    ;; Right triangle
    (sdf/fill
      (sdf/translate 0.35 0
        (let ((p1x 0.35) (p1y 0.5) (p2x 0.35) (p2y -0.5) (p3x -0.35) (p3y 0.0))
          (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
            (max (max d1 d2) d3))))
      (material :color (rgba 0.75 0.75 0.78 1.0)))))

(defwidget stop-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect 0.44 0.44 0.05)
      (material :color (rgba 0.75 0.75 0.78 1.0)))))

(defwidget play-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) (rgba 1 1 1 1.0) (rgba 0.75 0.75 0.78 1.0))))
    (sdf/layer
      (if (= active 1)
        (sdf/fill (sdf/rounded-rect (* 0.75 height) (* 0.75 height) 0.4)
          (material
            :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
              :light (vec3 -0.31 -0.851 1.3) :shininess 51.0)
            :color
            (let ((base (rgba 0.05 0.28 0.03 1.0))
                  (lit (+ 0.025 (* 0.05 diffuse)))
                  (shine (* 0.18 specular)))
              (+ base (rgba lit lit lit 1) (rgba shine shine shine 0)))))
        (rgba 0 0 0 0))
      (sdf/fill
        (let ((p1x -0.35) (p1y -0.5) (p2x -0.35) (p2y 0.5) (p3x 0.55) (p3y 0.0))
          (let ((d1 (- (* (- p2x p1x) (- y p1y)) (* (- p2y p1y) (- x p1x))))
                (d2 (- (* (- p3x p2x) (- y p2y)) (* (- p3y p2y) (- x p2x))))
                (d3 (- (* (- p1x p3x) (- y p3y)) (* (- p1y p3y) (- x p3x)))))
            (max (max d1 d2) d3)))
        (material :color fg-col)))))

(defwidget rec-icon
  :width 2.5 :height 1.8
  :paint-margin 0.5
  :state (active)
  :shader
  (let ((fg-col (if (= active 1) (rgba 1 1 1 1.0) (rgba 0.65 0.18 0.18 1.0))))
    (sdf/layer
      (if (= active 1)
        (sdf/fill (sdf/rounded-rect (* 0.75 height) (* 0.75 height) 0.4)
          (material
            :lighting (lighting :edge-min -0.1015 :edge-max 0.9413
              :light (vec3 -0.31 -0.851 1.5) :shininess 51.0)
            :color
            (let ((base (rgba 0.12 0.001 0.001 1.0))
                  (lit (+ 0.06 (* 0.40 diffuse)))
                  (shine (* 0.25 specular)))
              (+ base (rgba lit 0 0 1) (rgba shine shine shine 0)))))
        (rgba 0 0 0 0))
      (sdf/fill (sdf/circle 0.4)
        (material :color fg-col)))))

(def seq-switch-pattern (idx)
  (host-command "switch-pattern" (dict :idx idx)))

(def seq-clone-pattern ()
  (host-command "clone-pattern" (dict)))

(def seq-delete-pattern ()
  (host-command "delete-pattern" (dict)))

(def transport-icon-style
  (ui/style
    :pressed (dict
      :scale 1.08
      :transition (dict :scale 0.12 :ease :smoothstep))
    :hover (dict
      :brightness 1.10
      :transition (dict :brightness 0.12 :ease :smoothstep))))

(def pattern-control-style
  (ui/style
    :pressed (dict
      :scale 1.06
      :transition (dict :scale 0.10 :ease :smoothstep))
    :hover (dict
      :brightness 1.12
      :transition (dict :brightness 0.12 :ease :smoothstep))))

;; ── Transport layout ──

(effect-buffer "*transport*"
  (h-stack :gap 0.5 :padding 0.5 :align :center
    
    (save-icon 
      :on-click |x y r| (sbrowser-open-project-save)
      :style transport-icon-style
      :active (if (sbrowser-project-save-mode?) 1 0))
    
    ;; Transport buttons in a shared rounded-rect container
    (box :background "transport-btn-bg" :padding 0.015 :height 1.4
      (h-stack :gap 0.2 :align :center
        (box :width 2.5 
          :on-click |x y r| (if SEQ.playing (seq-toggle-play) nil)
          (rw-icon))
        (box :width 2.5 
          :on-click |x y r| (if SEQ.playing (seq-toggle-play) nil)
          (stop-icon))
        (box :width 2.5 
          :on-click |x y r| (seq-toggle-play)
          (play-icon :active (if SEQ.playing 1 0)))
        (box :width 2.5 
          :on-click |x y r| (seq-toggle-record)
          (rec-icon :active (if SEQ.recording 1 0)))))
    
    ;; Single continuous LED panel
    (box :background "transport-led-bg" :height 1.4 :width 41
      (h-stack
        (subtree :key "transport-clock"
          (h-stack :gap 0 :align :center :padding 0.5
            (transport-clock :playhead (bind-seq "transport-playhead")
              :font-size 15 :width 10 :height 1.2
              :color '(rgba 0.85 0.85 0.85 1)
              :bg :transparent)
            (label "" :width 1 :bg :transparent)
            (number-picker :value SEQ.bpm :min 20 :max 300 :decimals 1
              :noui true
              :font-size 15
              :text-color (rgba 0.85 0.85 0.85 1)
              :on-change (lambda (v) (seq-set-bpm v))
              :width 7 :height 1.2)))
        (v-stack :gap 0.08 :padding 0.05
          (label "L"
            :font-size 5 :width 0.9
            :color '(rgba 0.63 0.88 0.41 1)
            :bg :transparent)         
          
          (label "R"
            :font-size 5 :width 0.9
            :color '(rgba 0.63 0.88 0.41 1)
            :bg :transparent)          )
        
        
        (v-stack :gap 0.08 :padding 0.05
          (h-stack :gap 0.25 
            
            (v-stack
              (box :height 0.2)
              (subtree :key "master-meter-l"
                (transport-master-meter :level (bind-seq "master-peak-l")))))
          (h-stack :gap 0.25 :align :center
            
            (v-stack (box :height 0.1)
              (subtree :key "master-meter-r"
                (transport-master-meter :level (bind-seq "master-peak-r"))))))
        (subtree :key "transport-cpu"
          (h-stack :gap 0 :align :center :padding 0.3
            (box :height 1.3
              (label "cpu"
                :font-size 12 :width 2.5
                :color '(rgba 0.30 0.30 0.32 1)
                :bg :transparent))
            (number-label :value (bind-seq "cpu-load-pct")
              :decimals 0 :min-integer-digits 2 :suffix "%"
              :font-size 12 :width 4.5 :height 1
              :color :gray
              :bg :transparent)))))
    
    (box :background "transport-btn-bg" :padding 0.2 :height 1.4
      (h-stack :gap 0.1 :align :center
        (each (range 0 SEQ.num-patterns) |i|
          (box :width 2.5 :height 1.1 
            :background "pattern-pill-bg"
            :active (if (= i SEQ.current-pattern) 1 0)
            :style pattern-control-style
            :on-click |x y r| (seq-switch-pattern i)
            (v-stack :align :center
              (label (fmt " {} " (+ i 1))
                :font-size 11
                :color (if (= i SEQ.current-pattern) :white :gray)
                :hover-color :white
                :bg :transparent))))
        (label "" :width 0.2 :bg :transparent)
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :style pattern-control-style
          :on-click |x y r| (seq-clone-pattern)
          (v-stack :align :center
            (label "+"
              :font-size 12
              
              :color :white
              :bg :transparent)))
        
        (box :background "pattern-pill-btn-bg" :width 2.5 :height 1.1 :active true
          :style (if (> SEQ.num-patterns 1) pattern-control-style nil)
          :on-click |x y r| (if (> SEQ.num-patterns 1) (seq-delete-pattern) nil)
          (v-stack :align :center
            (label "-"
              :font-size 12
              
              :color (if (> SEQ.num-patterns 1) :white :dark-gray)
              :bg :transparent)))))))
