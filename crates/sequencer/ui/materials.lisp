;; ui/materials.lisp - Shared Metal Sequencer materials and widgets.
;; Loaded before the Metal Seq UI buffers that reference these definitions.

(module eseq.materials)

;; Migration compat aliases (spec §10 slice 3): every renamed macro with a
;; caller outside this file. `aqua-color` has ~10 call sites in
;; sequencer.lisp / step-grid.lisp / legacy/mixer.lisp shader bodies, so it
;; stays public. `aqua-color-button` and `aqua-slider-material2` have none
;; and go `%`-private. The standalone eseqlisp demos (sdf-aqua-demo.lisp,
;; slider-material-demo.lisp) and the Rust test fixtures that define their
;; own flat `aqua-color`/`aqua-slider-material` never load this file, so
;; the alias cannot shadow them.

;; NOTE: `:shader` and `:material` values are auto-quoted and expand at
;; shader-compile time in a throwaway implicit-module compiler, NOT in this
;; module — so macro references inside them must be written qualified
;; (`eseq.materials/color`), bare names would not find this module's table.

;; ── Step cursor highlight ──

(defwidget cursor-highlight
  :width 1 :height 1
  :state (active selected hide)
  :bindable (active selected hide)
  :shader
  (if (= hide 1)
    (rgba 0 0 0 0)
    (sdf/layer
      (sdf/stroke (sdf/rounded-rect (* width 0.94) (* height 0.99) 0.10)
        0.055
        (rgba 0.72 0.76 0.84 (* 0.95 active selected))))))

;; ── Aqua material for sliders ──


(defmacro %slider-material2 ()
    `(material
       :lighting (lighting :edge-min -0.2015 :edge-max 0.01413
         :light (vec3 -0.1 -1.1 0.5) :shininess 71.0)
       :color
       (let ((base (mix (rgba 0.4 0.1 0.8 1) (rgba 1.0 1.0 1.0 1)
                        (smoothstep -0.02 0 d)))
             (lit (+ 0.6 (* 0.4 diffuse)))
             (shine (* 0.25 specular)))
         (+ (* base (rgba lit lit lit 1.0))
            (rgba shine shine shine 0.0)))))

(defmacro slider-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.8413
       :light (vec3 -0.1 -0.61 3.5) :shininess 81.0)
     :color (eseq.materials/color (rgba 0.35 0.35 0.8 1.0) (rgba 0.20 0.20 0.92 1.0))))

(defmacro slider-muted-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.8413
       :light (vec3 -0.1 -0.61 2.4) :shininess 38.0)
     :color
       (* 0.38
          (eseq.materials/color
            (rgba 0.10 0.10 0.22 0.85)
            (rgba 0.08 0.08 0.30 0.85)))))

(defmacro slider-track-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.8413
       :light (vec3 -0.1 -0.61 3.5) :shininess 81.0)
     :color
       (eseq.materials/color
         (rgba (* eseq.step-grid/metal-track-r 0.55) (* eseq.step-grid/metal-track-g 0.55) (* eseq.step-grid/metal-track-b 0.55) 1.0)
         (rgba eseq.step-grid/metal-track-r eseq.step-grid/metal-track-g eseq.step-grid/metal-track-b 1.0))))

(defmacro slider-track-muted-material ()
  `(material
     :lighting (lighting :edge-min -0.215 :edge-max 0.8413
       :light (vec3 -0.1 -0.61 2.4) :shininess 38.0)
     :color
       (* 0.42
          (eseq.materials/color
            (rgba
              (+ (* eseq.step-grid/metal-track-r 0.36) 0.06)
              (+ (* eseq.step-grid/metal-track-g 0.36) 0.06)
              (+ (* eseq.step-grid/metal-track-b 0.36) 0.08)
              0.85)
            (rgba
              (+ (* eseq.step-grid/metal-track-r 0.30) 0.04)
              (+ (* eseq.step-grid/metal-track-g 0.30) 0.04)
              (+ (* eseq.step-grid/metal-track-b 0.30) 0.08)
              0.85)))))

     

;; ── Aqua widgets ──

(defmacro color (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
      (__base (mix ,base1
          ,base2
          (smoothstep 1 5 __ny)))
      (__glass (smoothstep 0.85 -0.865 __ny))
      (__edge-fade (smoothstep 0.61 -0.16 d))
      (__hi (* __glass __edge-fade 0.2655))
      (__spec (* specular __edge-fade 0.3))
      (__bot (* (smoothstep 0.29 -0.15 __ny)
          (smoothstep 0.15 0.5 __ny)
          __edge-fade 0.12))
      (__rim (smoothstep 0.8 -0.16183 d)))
    (+ (* __base (rgba __rim __rim __rim 1.0))
      (rgba (+ __hi __spec __bot)
        (+ __hi __spec __bot)
        (+ __hi __spec __bot)
        0.0))))


(defmacro %color-button (base1 base2)
  `(let ((__ny (+ y (* 0.3 (dot normal (vec3 0 1 0)))))
            (__base (mix ,base1
                ,base2
                (smoothstep 1 5 __ny)))
            (__glass (smoothstep 0.25 -0.865 __ny))
            (__edge-fade (smoothstep 0.01 -0.16 d))
            (__hi (* __glass __edge-fade 0.2655))
            (__spec (* specular __edge-fade 0.3))
            (__bot (* (smoothstep 0.9 -0.15 __ny)
                (smoothstep 0.65 0.5 __ny)
                __edge-fade 0.12))
            (__rim (smoothstep -0.30 -0.16183 d)))
          (+ (* __base (rgba __rim __rim __rim 1.0))
            (rgba (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              (+ __hi __spec __bot)
              0.0))))
(defwidget aqua-button
  :width 4 :height 3
  :paint-margin 1
  :state (active plocked selected)
  :bindable (active plocked selected)
  :shader
  (let ((sel-y (if (= selected 1) 0.03 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (+ (* (if (= selected 1) (* sel-y 1) 0.001) (smoothstep 0 0.1 (* y x))) (sdf/fill-rounded-rect -0.01 0.85))
          (material
            :lighting
            (lighting :edge-min -0.25 :edge-max 0.15
              :light (vec3 0.1 -1.0 0.5) :shininess 62.0)
            :color
            (* (if (= active 1) 1 0.7) (eseq.materials/%color-button (rgba 0.35 0.35 0.45 1.0) (rgba 0.30 0.30 0.92 1.0)))
            :shadow (shadow
              :color (rgba 0 0 0 0.3)
              :blur 0.15
              :offset (vec2 0 0.05))))
        (sdf/fill
          (sdf/translate 0 0.58
            (sdf/circle 0.17))
          (material
            :color (if (= plocked 1)
              (rgba 0.82 0.84 0.88 0.95)
              (rgba 0 0 0 0))))))))

(defwidget tick
  :width 1.5 :height 1.5
  :state (active plocked selected)
  :bindable (active plocked selected)
  :shader
  (let ((sel-y (if (= selected 1) 0.01 0)))
    (sdf/translate 0 sel-y
      (sdf/layer
        (sdf/fill (sdf/circle (+ 1 (* sel-y 100)))
          (material
            :lighting (lighting :edge-min -0.35 :edge-max 0.5
              :light (vec3 0.0 -1.0 1.5) :shininess 32.0)
            :color
            (* (if (= active 1) 1 0.3)
              (eseq.materials/color
                (rgba 0.3 0.3 0.85 1.0)
                (rgba 0.90 0.50 0.82 1.0)))))))))

(defwidget page-playhead-dot
  :width 0.7 :height 0.7
  :state (active)
  :bindable (active)
  :shader
  (if (= active 1)
    (sdf/layer
      (sdf/fill (sdf/circle 0.45)
        (material :color (rgba 1 1 1 1))))
    (rgba 0 0 0 0)))

(defwidget step-playhead-dot
  :width 1.0 :height 0.7
  :state (active)
  :bindable (active)
  :shader
  (sdf/layer
    (sdf/fill (sdf/circle 0.45)
      (material :color (if (= active 1) (rgba 1 1 1 1) (rgba 0 0 0 0))))))
