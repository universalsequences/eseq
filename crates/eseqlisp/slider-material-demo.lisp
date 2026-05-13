;; slider-material-demo.lisp — :material on built-in hslider/vslider
;; Demonstrates applying SDF materials to native slider widgets
;; while preserving track dots and interaction semantics.

;; ── Reuse aqua-color macro from aqua demo ──────────────────────────────
(defmacro aqua-color (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
        (__base (mix ,base1 ,base2 (smoothstep -0.5 0.5 __ny)))
        (__glass (smoothstep 0.1 -0.65 __ny))
        (__edge-fade (smoothstep 0.0 -0.26 d))
        (__hi (* __glass __edge-fade 0.655))
        (__spec (* specular __edge-fade 0.3))
        (__bot (* (smoothstep 0.3 0.5 __ny)
                  (smoothstep 0.65 0.5 __ny)
                  __edge-fade 0.12))
        (__rim (smoothstep -0.53 -0.033 d)))
     (+ (* __base (rgba __rim __rim __rim 1.0))
        (rgba (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              (+ __hi __spec __bot) 0.0))))

;; ── State ──────────────────────────────────────────────────────────────
(defstate vol 0.7)
(defstate pan 0.5)
(defstate cutoff 0.6)
(defstate resonance 0.3)
(defstate attack 0.2)
(defstate decay 0.5)
(defstate sustain 0.8)
(defstate release 0.4)
(defstate mix-level 0.65)
(defstate drive 0.15)

;; ── Demo layout ────────────────────────────────────────────────────────
(effect-buffer "*slider-material*"
  (v-stack :padding 2 :gap 2

    ;; Section 1: Aqua glass sliders
    (label "Aqua Glass" :font-size 16 :color :accent :bg :transparent)
    (h-stack :gap 1
      (label "vol" :color :dim :bg :transparent :width 6)
      (hslider :min 0 :max 1 :bind vol :width 20
        :material (material
          :lighting (lighting :edge-min -0.35 :edge-max 0.5
            :light (vec3 -0.5 -1.0 1.5) :shininess 32.0)
          :color (aqua-color (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0)))))
    (h-stack :gap 1
      (label "pan" :color :dim :bg :transparent :width 6)
      (hslider :min 0 :max 1 :bind pan :width 20 :origin 0.5
        :material (material
          :lighting (lighting :edge-min -0.35 :edge-max 0.5
            :light (vec3 -0.5 -1.0 1.5) :shininess 32.0)
          :color (aqua-color (rgba 0.15 0.25 0.35 1.0) (rgba 0.20 0.50 0.92 1.0)))))

    ;; Section 2: Warm amber sliders
    (label "Warm Amber" :font-size 16 :color :accent :bg :transparent)
    (h-stack :gap 1
      (label "cutoff" :color :dim :bg :transparent :width 6)
      (hslider :min 0 :max 1 :bind cutoff :width 20
        :material (material
          :lighting (lighting :edge-min -0.2 :edge-max 0.3
            :light (vec3 -0.7 -0.9 1.3) :shininess 48.0)
          :color
          (let ((__glow (smoothstep -0.1 -0.4 d))
                (__rim (smoothstep -0.4 -0.02 d)))
            (+ (* (mix (rgba 0.6 0.25 0.05 1) (rgba 0.95 0.6 0.1 1)
                       (+ (* 0.5 diffuse) 0.5))
                  (rgba __rim __rim __rim 1))
               (rgba (* specular 0.4) (* specular 0.3) (* specular 0.1) 0))))))
    (h-stack :gap 1
      (label "reso" :color :dim :bg :transparent :width 6)
      (hslider :min 0 :max 1 :bind resonance :width 20
        :material (material
          :lighting (lighting :edge-min -0.2 :edge-max 0.3
            :light (vec3 -0.7 -0.9 1.3) :shininess 48.0)
          :color
          (let ((__glow (smoothstep -0.1 -0.4 d))
                (__rim (smoothstep -0.4 -0.02 d)))
            (+ (* (mix (rgba 0.6 0.25 0.05 1) (rgba 0.95 0.6 0.1 1)
                       (+ (* 0.5 diffuse) 0.5))
                  (rgba __rim __rim __rim 1))
               (rgba (* specular 0.4) (* specular 0.3) (* specular 0.1) 0))))))

    ;; Section 3: Minimal flat — just a color keyword (simplest usage)
    (label "Simple Color" :font-size 16 :color :accent :bg :transparent)
    (h-stack :gap 1
      (label "mix" :color :dim :bg :transparent :width 6)
      (hslider :min 0 :max 1 :bind mix-level :width 20
        :material (material :color :accent)))
    (h-stack :gap 1
      (label "drive" :color :dim :bg :transparent :width 6)
      (hslider :min 0 :max 1 :bind drive :width 20
        :material (material :color (rgba 0.9 0.2 0.3 1))))

    ;; Section 4: Default sliders for comparison
    (label "Default (no material)" :font-size 16 :color :accent :bg :transparent)
    (h-stack :gap 1
      (label "vol" :color :dim :bg :transparent :width 6)
      (hslider :min 0 :max 1 :bind vol :width 20 :fill :accent))
    (h-stack :gap 1
      (label "pan" :color :dim :bg :transparent :width 6)
      (hslider :min 0 :max 1 :bind pan :width 20 :fill :secondary))

    ;; Section 5: Vertical sliders with materials
    (label "Vertical — ADSR" :font-size 16 :color :accent :bg :transparent)
    (h-stack :gap 2 :align :top
      (v-stack :align :center :gap 0.5
        (vslider :min 0 :max 1 :bind attack :height 6
          :material (material
            :lighting (lighting :edge-min -0.3 :edge-max 0.4
              :light (vec3 0 -1 1.5) :shininess 32.0)
            :color (aqua-color (rgba 0.1 0.3 0.7 1) (rgba 0.3 0.6 0.95 1))))
        (label "A" :color :dim :bg :transparent))
      (v-stack :align :center :gap 0.5
        (vslider :min 0 :max 1 :bind decay :height 6
          :material (material
            :lighting (lighting :edge-min -0.3 :edge-max 0.4
              :light (vec3 0 -1 1.5) :shininess 32.0)
            :color (aqua-color (rgba 0.1 0.3 0.7 1) (rgba 0.3 0.6 0.95 1))))
        (label "D" :color :dim :bg :transparent))
      (v-stack :align :center :gap 0.5
        (vslider :min 0 :max 1 :bind sustain :height 6
          :material (material
            :lighting (lighting :edge-min -0.3 :edge-max 0.4
              :light (vec3 0 -1 1.5) :shininess 32.0)
            :color (aqua-color (rgba 0.1 0.3 0.7 1) (rgba 0.3 0.6 0.95 1))))
        (label "S" :color :dim :bg :transparent))
      (v-stack :align :center :gap 0.5
        (vslider :min 0 :max 1 :bind release :height 6
          :material (material
            :lighting (lighting :edge-min -0.3 :edge-max 0.4
              :light (vec3 0 -1 1.5) :shininess 32.0)
            :color (aqua-color (rgba 0.1 0.3 0.7 1) (rgba 0.3 0.6 0.95 1))))
        (label "R" :color :dim :bg :transparent)))))

(delete-other-windows)
(split-window-right "*slider-material*")
