;; Shared shader widgets and selected-effect actions for the FX strip.
(def fx-select-effect (slot)
  (do
    (process-panel-clear-selection)
    (seq-set-delete-target :fx-effect (dict :chain "audio" :slot slot))))

(def fx-select-midi-effect (slot)
  (do
    (process-panel-clear-selection)
    (seq-set-delete-target :fx-effect (dict :chain "midi" :slot slot))))

(def fx-select-bus-effect (bus slot)
  (do
    (process-panel-clear-selection)
    (seq-set-delete-target :fx-effect (dict :chain "bus" :bus bus :slot slot))))

(def fx-has-selected-bus? ()
  (and (>= selected-bus 0)
       (< selected-bus (len SEQ.bus-names))
       (< selected-bus (len SEQ.bus-effects))))

(def fx-delete-selected-effect ()
  (if (process-panel-delete-selected)
    true
    (if (fx-plock-row-selected?)
      (fx-delete-selected-plock-row)
      (seq-delete-active-target))))

(defwidget fx-panel-bg
  :width 1 :height 1
  :state (selected header-r header-g header-b selected-header-r selected-header-g selected-header-b)
  :shader
  (let ((panel-radius (min (* 3 (fwidth y)) (* 0.5 (min width height))))
      (panel (sdf/rounded-rect (* 1 width) (* 1 height) (* 2 panel-radius)))
      ;; Use derivatives to convert a real pixel height into the shader's
      ;; normalized/SDF y-space. This keeps the header bar visually constant
      ;; as panels get taller/shorter.
      (header-h (* 35 (fwidth y)))
      (header-bottom (+ (- height) header-h))
      (header-shape (max panel (- y header-bottom))))
    (sdf/layer
      (sdf/fill
        panel
        (material
          :color
          (let ((header-aa (max (fwidth header-shape) (fwidth y)))
                (border-w (max (* 1.5 (fwidth d)) (fwidth y)))
                (body input-color)
                (header (rgba header-r header-g header-b 1.0))
                (base
                  (mix header body (smoothstep 0 header-aa header-shape))))
            base)))
      (if selected
        (sdf/fill header-shape
          (material :color (rgba selected-header-r selected-header-g selected-header-b 1.0)))
        (rgba 0 0 0 0)))))

(defwidget compile-progress
  :width 12 :height 0.3
  :state (active)
  :shader
  (if (= active 0)
    (rgba 0 0 0 0)
    (let ((bar-w 0.3)
          (pos (fract (* 0.5 itime)))
          (bar-x (- (* pos (+ 1 bar-w)) (/ bar-w 2)))
          (d-bar (- (abs (- x bar-x)) (/ bar-w 2)))
          (bg (sdf/rounded-rect width height 0.06))
          (mask (max bg (- d-bar))))
      (sdf/layer
        (sdf/fill bg
          (material :color (rgba 0.15 0.15 0.17 1)))
          (sdf/fill mask
          (material :color
            (mix
              (rgba 0.3 0.5 1.0 1)
              (rgba 0.2 0.35 0.8 1)
              (smoothstep -0.02 0.02 d-bar))))))))

(defwidget agent-instrument-stub-bg
  :width 70 :height 1
  :paint-margin 0.2
  :shader
  (let ((drift (* itime 0.11))
        (pulse (+ 0.5 (* 0.5 (sin (* itime 0.72)))))
        (sx (+ (* x 0.17) drift))
        (sy (+ (* y aspect 0.92) (* (sin (* itime 0.19)) 0.36)))
        (ix (floor sx))
        (iy (floor sy))
        (fx (fract sx))
        (fy (fract sy))
        (ux (smoothstep 0.0 1.0 fx))
        (uy (smoothstep 0.0 1.0 fy))
        (h00 (fract (* (sin (+ (* ix 127.1) (* iy 311.7))) 43758.5453)))
        (h10 (fract (* (sin (+ (* (+ ix 1.0) 127.1) (* iy 311.7))) 43758.5453)))
        (h01 (fract (* (sin (+ (* ix 127.1) (* (+ iy 1.0) 311.7))) 43758.5453)))
        (h11 (fract (* (sin (+ (* (+ ix 1.0) 127.1) (* (+ iy 1.0) 311.7))) 43758.5453)))
        (n0 (mix h00 h10 ux))
        (n1 (mix h01 h11 ux))
        (cloud-a (mix n0 n1 uy))
        (sx2 (+ (* x 0.39) (* (sin (* itime 0.13)) 0.8)))
        (sy2 (- (* y aspect 1.8) (* itime 0.17)))
        (ix2 (floor sx2))
        (iy2 (floor sy2))
        (fx2 (fract sx2))
        (fy2 (fract sy2))
        (ux2 (smoothstep 0.0 1.0 fx2))
        (uy2 (smoothstep 0.0 1.0 fy2))
        (k00 (fract (* (sin (+ (* ix2 269.5) (* iy2 183.3))) 24634.6345)))
        (k10 (fract (* (sin (+ (* (+ ix2 1.0) 269.5) (* iy2 183.3))) 24634.6345)))
        (k01 (fract (* (sin (+ (* ix2 269.5) (* (+ iy2 1.0) 183.3))) 24634.6345)))
        (k11 (fract (* (sin (+ (* (+ ix2 1.0) 269.5) (* (+ iy2 1.0) 183.3))) 24634.6345)))
        (m0 (mix k00 k10 ux2))
        (m1 (mix k01 k11 ux2))
        (cloud-b (mix m0 m1 uy2))
        (cloud (smoothstep 0.18 0.92 (+ (* cloud-a 0.68) (* cloud-b 0.32))))
        (body (rgba 0.055 0.060 0.072 1.0))
        (blue (rgba 0.05 0.30 0.48 1.0))
        (violet (rgba 0.48 0.20 0.56 1.0))
        (cyan (rgba 0.15 0.78 0.92 1.0))
        (magenta (rgba 0.96 0.34 0.74 1.0)))
    (sdf/layer
      (sdf/fill
        (sdf/rect width height)
        (material :color
          (mix
            (mix body (mix blue violet pulse) 0.30)
            (mix cyan magenta pulse)
            (* cloud 0.42)))))))
