;; ui/panel-header.lisp — the right column's panel header: a rounded dim
;; pill holding a filled chip (the *step* panel's track badge, the
;; *processes* dock's instance name) and muted secondary text. One home so
;; *step* and the dock stay identical (eseq-waa9.22).
;;
;; Nearly a leaf: both eseq.effects.track-panels and eseq.processes-buffer
;; import it, and it imports only the shader materials its pill is made of.
(module eseq.panel-header)

(import eseq.materials)

(export pill chip note)

(defwidget liquid-glass
  :width 1 :ehight 1
  :shader
  (sdf/layer 
    (sdf/fill
      (sdf/rounded-rect width height height)
      (material
        :lighting (lighting :edge-min -0.115 :edge-max 0.813
          :light (vec3 0.31 4.5 3.5) :shininess 91.0)
        :color
        (eseq.materials/color-cell 
          :transport-controls-bg 
          (mix (* 1.0 :white) :transport-controls-bg
            (mix 1 0 (smoothstep 0.3 1.5 (abs y)))
            )
          )
        ))
    
    (sdf/fill
      (sdf/rounded-rect (* width 0.9999 ) (* 0.96 height) height)
      (* (mix 1 0.4 (smoothstep -0.1 0.03 d )) :transport-controls-bg )
      )
    ))
;; The pill: `chip` then `summary`, on the selected-strip background.
(def pill (chip summary)
  (box :padding 0.25 :background "liquid-glass" :corner-radius 12 :v-align :center
    (h-stack :gap 0.45 :align :start
      (box :width 0.0)
      chip
      summary)))

;; The filled chip: `text` in black on `fill` (dimmed to `muted-fill` with
;; dim text while `muted`).
(def chip (key text width fill muted muted-fill)
  (box
    :key key
    :width width :height 1.0
    :padding 0
    :corner-radius 16
    :v-align :center
    :muted muted
    :background-color fill
    :muted-background-color muted-fill
    (label text
      :width width
      :font-size 10
      :v-align :center
      :h-align :center
      :active muted
      :color :black
      :active-color :dim
      :bg :transparent)))

;; Muted secondary text beside the chip.
(def note (key text)
  (label text :key key :font-size 8 :color :dim :bg :transparent))
