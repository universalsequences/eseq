;; sdf-lighting-v2-demo.lisp — Holographic SDF lighting showcase

;; Clean shape — no ripples in the SDF itself
(defmacro mysdf (a)
  `(+
    (* 0.1 (smoothstep 0 ,a (+ 0.5 (* 0.5 (cos (* 5 (cos itime) (- x y)))))))
    (sdf/smooth-union
      0.25
      (sdf/translate
        (* 0.4 (cos itime))
        (* 1.7 (sin itime))
        (sdf/circle 0.5))
      (sdf/rounded-rect 1 1.5 0.3))))

;; Rune SDF helpers — operate on local tile coordinates (tx, ty) in [-0.5, 0.5]
;; Line segment distance from (ax,ay) to (bx,by)
(defmacro rune-line (tx ty ax ay bx by)
  `(let ((__pax (- ,tx ,ax)) (__pay (- ,ty ,ay))
         (__bax (- ,bx ,ax)) (__bay (- ,by ,ay)))
     (let ((__h (clamp (/ (+ (* __pax __bax) (* __pay __bay))
                          (+ (* __bax __bax) (* __bay __bay))) 0 1)))
       (length (vec2 (- __pax (* __h __bax)) (- __pay (* __h __bay)))))))

;; Rune glyphs — each returns a distance field for a different symbol
;; Glyph 0: cross with dot
(defmacro rune-glyph-0 (tx ty)
  `(min (min (rune-line ,tx ,ty -0.2 0 0.2 0)
             (rune-line ,tx ,ty 0 -0.25 0 0.25))
        (- (length (vec2 ,tx ,ty)) 0.06)))

;; Glyph 1: triangle
(defmacro rune-glyph-1 (tx ty)
  `(min (rune-line ,tx ,ty -0.15 0.15 0.15 0.15)
        (min (rune-line ,tx ,ty -0.15 0.15 0 -0.2)
             (rune-line ,tx ,ty 0.15 0.15 0 -0.2))))

;; Glyph 2: diamond
(defmacro rune-glyph-2 (tx ty)
  `(min (min (rune-line ,tx ,ty 0 -0.25 0.18 0)
             (rune-line ,tx ,ty 0.18 0 0 0.25))
        (min (rune-line ,tx ,ty 0 0.25 -0.18 0)
             (rune-line ,tx ,ty -0.18 0 0 -0.25))))

;; Glyph 3: parallel bars with slash
(defmacro rune-glyph-3 (tx ty)
  `(min (min (rune-line ,tx ,ty -0.15 -0.2 -0.15 0.2)
             (rune-line ,tx ,ty 0.15 -0.2 0.15 0.2))
        (rune-line ,tx ,ty -0.2 0.15 0.2 -0.15)))

;; Pick a glyph based on tile index (cheap hash)
(defmacro rune-pick (tx ty cell-id)
  `(let ((__sel (fract (* ,cell-id 0.7631))))
     (if (< __sel 0.25)
       (rune-glyph-0 ,tx ,ty)
       (if (< __sel 0.5)
         (rune-glyph-1 ,tx ,ty)
         (if (< __sel 0.75)
           (rune-glyph-2 ,tx ,ty)
           (rune-glyph-3 ,tx ,ty))))))

;; Main bump: smooth crystal base + rune etchings carved in
(defmacro holo-bump ()
  `(let (;; Smooth dome undulation
         (__wave (+ (* 0.05 (cos (+ (* 2.3 x) (* 1.7 y) (* 0.5 itime))))
                    (* 0.03 (sin (+ (* 1.9 x) (* -2.8 y) (* -0.35 itime))))))
         ;; Tile coordinates for rune grid
         (__scale 1)
         (__cx (floor (* __scale x)))
         (__cy (floor (* __scale y)))
         (__tx (- (fract (* __scale x)) 0.5))
         (__ty (- (fract (* __scale y)) 0.5))
         ;; Tile ID for glyph selection
         (__cell-id (+ __cx (* __cy 7.13))))
     (let (;; Distance to rune shape in this tile
           (__rune-d (rune-pick __tx __ty __cell-id))
           ;; Convert to etch: sharp groove where distance is small
           (__etch (smoothstep 0.04 0.01 __rune-d)))
       ;; Smooth base minus etched grooves
       (- __wave (* 0.08 __etch)))))

;; ── Widgets ─────────────────────────────────────────────────────────────

;; Holographic main panel
;; Metallic base with rainbow concentrated in specular streaks
(defwidget v2-holo
  :width 15 :height 15
  :shader
  (sdf/layer
    (sdf/fill (mysdf 0.8)
      (material
        :lighting (lighting :edge-min -0.1 :edge-max 0.3
                    :light (vec3 -0.9 -0.9 1.3) :shininess 96.0
                    :bump (holo-bump))
        :color
        (let (;; Metallic silver-green base, lit by diffuse
              (base-r (* 0.15 (+ 0.25 (* 0.45 diffuse))))
              (base-g (* 0.18 (+ 0.25 (* 0.45 diffuse))))
              (base-b (* 0.15 (+ 0.25 (* 0.45 diffuse))))
              ;; Rainbow phase from surface angle — like diffraction
              ;; Use dot products with angled vectors to create directional streaks
              (streak (+ (* 6.0 (dot normal (normalize (vec3 1.0 0.7 0.3))))
                         (* 0.5 itime)))
              (ir (+ 0.5 (* 0.5 (cos streak))))
              (ig (+ 0.5 (* 0.5 (cos (+ streak 2.09)))))
              (ib (+ 0.5 (* 0.5 (cos (+ streak 4.18)))))
              ;; Rainbow intensity: strongest in specular highlights and fresnel edges
              (fresnel (- 1.0 (dot normal (vec3 0 0 1))))
              (rainbow-strength (+ (* 2.0 specular) (* 0.6 fresnel fresnel))))
          (rgba (+ base-r (* rainbow-strength ir))
                (+ base-g (* rainbow-strength ig))
                (+ base-b (* rainbow-strength ib))
                1.0))))))

;; Small holographic gem dots
(defwidget v2-gem
  :width 2 :height 1
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.8)
      (material
        :lighting (lighting :edge-min -0.5 :edge-max 0.8
                    :light (vec3 -0.9 -0.9 1.3) :shininess 128.0
                    )
        :color
        (let ((base (* 0.3 (+ 0.5 (* 0.5 diffuse))))
              (streak (+ (* 1.0 (dot normal (normalize (vec3 1.0 0.7 0.3))))
                         (* 0.8 itime)))
              (ir (+ 0.9 (* 0.5 (cos streak))))
              (ig (+ 0.5 (* 0.5 (cos (+ streak 2.09)))))
              (ib (+ 0.5 (* 0.5 (cos (+ streak 4.18)))))
              (fresnel (- 1.0 (dot normal (vec3 0 0 1))))
              (rs (+ (* 2.5 specular) (* 0.8 fresnel fresnel))))
          (rgba (+ base (* rs ir))
                (+ base (* rs ig))
                (+ base (* rs ib))
                1.0))))))

;; Holographic button bar
(defwidget v2-holo-bar
  :width 16 :height 3
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect 1.5 0.7 0.18)
      (material
        :lighting (lighting :edge-min -0.15 :edge-max 0.5
                    :light (vec3 -0.9 -0.9 1.3) :shininess 80.0
                    )
        :color
        (let ((base-r (* 0.1 (+ 0.1 (* 0.5 diffuse))))
              (base-g (* 0.1 (+ 0.1 (* 0.5 diffuse))))
              (base-b (* 0.1 (+ 0.1 (* 0.5 diffuse))))
              (streak (+ (* 6.0 (dot normal (normalize (vec3 1.0 0.5 0.2))))
                         (* 0.4 itime)
                         (* 3.0 x)))
              (ir (+ 0.5 (* 0.5 (cos streak))))
              (ig (+ 0.5 (* 0.5 (cos (+ streak 2.09)))))
              (ib (+ 0.5 (* 0.5 (cos (+ streak 4.18)))))
              (fresnel (- 1.0 (dot normal (vec3 0 0 1))))
              (rs (+ (* 1.8 specular) (* 0.5 fresnel fresnel))))
          (rgba (+ base-r (* rs ir))
                (+ base-g (* rs ig))
                (+ base-b (* rs ib))
                1.0))))))

;; ── Demo ───────────────────────────────────────────────────────────────

(defstate v1 0.5)
(defstate v2 0.5)

(effect-buffer "*light*"
  (v-stack :padding 1 :gap 1
    (label "SDF Lighting v2" :font-size 18 :color :accent)
    (label "holographic: metallic base + rainbow in specular streaks & fresnel" :color :dim :font-size 10)
    
    (h-stack :gap 3 :align :center
      (box :background "v2-holo" :padding 2 :width 16 :height 16
        :align :center
        (v-stack :align :center
          (box :background "v2-holo-bar" :width 16 :height 3 :align :center
            (label "sampl" :bg :transparent :font-size 24))
          (hslider :min 0 :max 1 :bind v1 :fill :white)
          (hslider :min 0 :max 1 :bind v2 :fill :white)
          (label "adsr" :bg :transparent)
          (grid :cols 2 (each (range 0 4) |z| (v2-gem))))))))

(delete-other-windows)
(split-window-right "*light*")