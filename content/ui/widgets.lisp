;; ui/widgets.lisp - How the built-in controls are painted.
;;
;; A built-in control (`knob`) keeps its layout, clicks and drags in the
;; engine but paints with the `defwidget` of the same face name here, when
;; one is defined. Each reads its colours from theme slots, so a package can
;; restyle one part everywhere with `override-theme-slot`: the slot's widget
;; is inlined where the colour was, with `slot-color` bound to that colour.
;;
;; Inputs every face gets: `x`/`y` (-1..1 on the shorter side, y down),
;; `value_t` (the control's value, 0..1) and `design-px`.

(module eseq.widgets)

;; A knob: a 270° ring from bottom-left round to bottom-right, filled up to
;; the value, with a dot on the ring and a pointer line from the centre.
;;   :widget-knob-filled  the filled arc and the dot
;;   :widget-knob-track   the unfilled ring and the pointer line
(defwidget knob-face
  :width 2 :height 2
  :shader
  (let ((fill :widget-knob-filled)
        (track :widget-knob-track)
        (r (length (vec2 x y)))
        (start 1.57079633)
        (sweep 4.71238898)
        (rel (mod (+ (- (atan2 y x) start) 6.2831853) 6.2831853))
        (in-range (if (<= rel sweep) 1.0 0.0))
        (active (if (<= rel (* sweep value_t)) 1.0 0.0))
        (aa (max (fwidth r) 0.0015))
        (ring-mask (* (smoothstep aa (* -1.0 aa) (- (abs (- r 0.74)) 0.11)) in-range))
        (active-mask (* (smoothstep aa (* -1.0 aa) (- (abs (- r 0.74)) 0.125)) in-range active))
        (track-mask (* ring-mask (- 1.0 active)))
        (angle (+ start (* sweep value_t)))
        (dx (cos angle))
        (dy (sin angle))
        (notch (- (length (vec2 (- x (* dx 0.74)) (- y (* dy 0.74)))) 0.105))
        (notch-mask (smoothstep aa (* -1.0 aa) notch))
        (along (+ (* x dx) (* y dy)))
        (across (abs (- (* x dy) (* y dx))))
        (segment (* (if (>= along 0.0) 1.0 0.0) (if (<= along 0.68) 1.0 0.0)))
        (line-mask (* (smoothstep aa (* -1.0 aa) (- across 0.11)) segment)))
    (mix (mix (mix (mix (rgba 0 0 0 0) track track-mask) fill active-mask) track line-mask)
         fill notch-mask)))

;; The ring of a `knob-number` (the mixer's and every device panel's knobs):
;; the arc fills from the origin (the minimum, or 0 on a bipolar knob) to
;; the value. A p-locked knob (its arc then in the p-lock colour) glows in
;; that colour and marks its unlocked value with a small grey dot.
;;   :widget-knob-filled  the filled arc and the dot, unless the knob has
;;                        an :arc-color or is p-locked (`arc-slot` is 0)
;;   :widget-knob-track   the unfilled ring and the pointer line, unless
;;                        the knob has a :track-color (`track-slot` is 0)
;; `input-color` is the arc's colour and track-r/g/b/a the track's, the
;; ones the knob paints with when it is not using the slot.
(defwidget knob-number-face
  :width 2 :height 2
  :state (plock default-t origin-t arc-slot track-slot
          track-r track-g track-b track-a)
  :shader
  (let ((fill (mix input-color :widget-knob-filled arc-slot))
        (track (mix (rgba track-r track-g track-b track-a) :widget-knob-track track-slot))
        (r (length (vec2 x y)))
        (start 1.57079633)
        (sweep 4.71238898)
        (rel (mod (+ (- (atan2 y x) start) 6.2831853) 6.2831853))
        (in-range (if (<= rel sweep) 1.0 0.0))
        (value-rel (* sweep (clamp value_t 0.0 1.0)))
        (origin-rel (* sweep (clamp origin-t 0.0 1.0)))
        (lo (min value-rel origin-rel))
        (hi (max value-rel origin-rel))
        (active (* (if (>= rel lo) 1.0 0.0) (if (<= rel hi) 1.0 0.0)
                   (if (>= (- hi lo) 0.001) 1.0 0.0)))
        (locked (if (>= plock 0.5) 1.0 0.0))
        (radius 0.64)
        (aa (max (fwidth r) 0.0015))
        (ring-mask (* (smoothstep aa (* -1.0 aa) (- (abs (- r radius)) 0.070)) in-range))
        (active-mask (* (smoothstep aa (* -1.0 aa) (- (abs (- r radius)) 0.082)) in-range active))
        (glow-mask (* (smoothstep (* 4.0 aa) (* -4.0 aa) (- (abs (- r radius)) 0.150))
                      in-range active locked))
        (track-mask (* ring-mask (- 1.0 active)))
        (angle (+ start value-rel))
        (dx (cos angle))
        (dy (sin angle))
        (notch (- (length (vec2 (- x (* dx radius)) (- y (* dy radius)))) 0.070))
        (notch-mask (smoothstep aa (* -1.0 aa) notch))
        (along (+ (* x dx) (* y dy)))
        (across (abs (- (* x dy) (* y dx))))
        (segment (* (if (>= along 0.0) 1.0 0.0) (if (<= along 0.58) 1.0 0.0)))
        (line-mask (* (smoothstep aa (* -1.0 aa) (- across 0.070)) segment))
        (default-angle (+ start (* sweep (clamp default-t 0.0 1.0))))
        (default-notch (- (length (vec2 (- x (* (cos default-angle) radius))
                                        (- y (* (sin default-angle) radius))))
                          0.046))
        (default-mask (* (smoothstep aa (* -1.0 aa) default-notch) locked
                         (if (>= (abs (- default-t value_t)) 0.01) 1.0 0.0))))
    (mix (mix (mix (mix (mix (mix (rgba 0 0 0 0) (* input-color (rgba 1.0 1.0 1.0 0.20)) glow-mask)
                             track track-mask)
                        fill active-mask)
                   track line-mask)
              fill notch-mask)
         (rgba 0.36 0.36 0.41 0.95) default-mask)))
