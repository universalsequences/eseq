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

;; ── Bezel: the lit surface of a button and a number picker ──

;; Distance at (px, py) to a rounded rect of half-size (sx, sy) and corner
;; radius r.
(defmacro bezel-rrect (px py sx sy r)
  `(let ((x ,px) (y ,py)) (sdf/rounded-rect ,sx ,sy ,r)))

;; Distance to an active tab: narrower at the top, its top corners rounded.
(defmacro bezel-tab (px py sx sy r)
  `(let ((__bt-px ,px) (__bt-py ,py) (__bt-sx ,sx) (__bt-sy ,sy) (__bt-r ,r)
         (__bt-top-w (max (- __bt-sx (min (* __bt-sx 0.20) 0.22)) 0.001))
         (__bt-t (smoothstep (+ (* -1.0 __bt-sy) (* __bt-r 1.40)) __bt-sy __bt-py))
         (__bt-w (mix __bt-top-w __bt-sx __bt-t))
         (__bt-d (let ((x __bt-px) (y __bt-py)) (sdf/rect __bt-w __bt-sy)))
         (__bt-top (+ (* -1.0 __bt-sy) __bt-r))
         (__bt-left (- (length (vec2 (- __bt-px (- __bt-r __bt-top-w)) (- __bt-py __bt-top))) __bt-r))
         (__bt-right (- (length (vec2 (- __bt-px (- __bt-top-w __bt-r)) (- __bt-py __bt-top))) __bt-r))
         (__bt-d1 (if (< __bt-px (- __bt-r __bt-top-w))
                    (if (< __bt-py __bt-top) __bt-left __bt-d)
                    __bt-d)))
     (if (> __bt-px (- __bt-top-w __bt-r))
       (if (< __bt-py __bt-top) __bt-right __bt-d1)
       __bt-d1)))

;; The bezel of a `button` and a `number-picker`: a rounded rect whose 1px
;; rim and face are lit from the top left, with a soft highlight and shadow.
;; `bezel-tab-face` is the same over an active `:shape :tab` button's tab;
;; `def-bezel` makes both from `dist`, a macro for the outline's distance.
;;   fill    a button's variant slot (:button-primary-bg, :button-secondary-bg,
;;           :button-ghost-bg, :button-danger-bg) or a number picker's
;;           :dropdown-bg; `fill-slot` 1..5 picks it, 0 when the control
;;           has its own :background-color
;;   border  :button-border, unless `border-slot` is 0 (its own
;;           :border-color, p-locked, or a filled primary/danger button)
;; `input-color` and `second-color` are the fill and border the control
;; paints with when not using a slot; `press` (-1..1) darkens or lightens a
;; slot's fill while a button is held. `radius` is the corner radius over
;; the half-height; hl-* and sh-* the highlight and shadow tints.
(defmacro def-bezel (name dist)
  `(defwidget ,name
     :width 4 :height 1
     :state (radius fill-slot border-slot press
             hl-r hl-g hl-b hl-a sh-r sh-g sh-b sh-a)
     :shader
     (let ((asp (max aspect 0.001))
           (k (min asp 1.0))
           (px (* x k))
           (py (* y k))
           (r (min (if (> radius 0.0) radius 0.75) (min asp 1.0)))
           (d (,dist px py asp 1.0 r))
           (edge (* (fwidth d) 1.2))
           (mask (smoothstep edge (* -1.0 edge) d))
           (pw (max (max (fwidth px) (fwidth py)) 0.001))
           (bw pw)
           (inner-d (,dist px py (max (- asp bw) 0.001) (max (- 1.0 bw) 0.001)
                                             (max (- r bw) 0.0)))
           (inner-edge (* (fwidth inner-d) 1.2))
           (inner-mask (smoothstep inner-edge (* -1.0 inner-edge) inner-d))
           (border-mask (clamp (- mask inner-mask) 0.0 1.0))
           (eps (max (* pw 1.5) 0.004))
           (right (smoothstep -0.10 0.92 (,dist (+ px eps) py asp 1.0 r)))
           (left (smoothstep -0.10 0.92 (,dist (- px eps) py asp 1.0 r)))
           (down (smoothstep -0.10 0.92 (,dist px (+ py eps) asp 1.0 r)))
           (up (smoothstep -0.10 0.92 (,dist px (- py eps) asp 1.0 r)))
           (n (normalize (vec3 (/ (- right left) (* 2.0 eps)) (/ (- down up) (* 2.0 eps)) 1.0)))
           (key-light (normalize (vec3 -0.12 -0.32 1.30)))
           (bounce-light (normalize (vec3 0.82 0.78 1.10)))
           (key-diffuse (max 0.0 (dot n key-light)))
           (bounce-diffuse (max 0.0 (dot n bounce-light)))
           (key-spec (pow (max 0.0 (dot n (normalize (+ key-light (vec3 0.0 0.0 1.0))))) 56.0))
           (bounce-spec (pow (max 0.0 (dot n (normalize (+ bounce-light (vec3 0.0 0.0 1.0))))) 42.0))
           (edge-fade (smoothstep 0.12 -0.04 d))
           (quadrant (+ (* (- key-diffuse (* 0.34 bounce-diffuse)) 0.16)
                        (* (- bounce-diffuse (* 0.30 key-diffuse)) 0.10)))
           ;; The fill and border, from their slots or the control's own.
           (slot-fill (if (< fill-slot 1.5) :button-primary-bg
                        (if (< fill-slot 2.5) :button-secondary-bg
                          (if (< fill-slot 3.5) :button-ghost-bg
                            (if (< fill-slot 4.5) :button-danger-bg :dropdown-bg)))))
           (press-t (abs press))
           (press-w (if (> press 0.0) press-t 0.0))
           (pressed-fill (+ (* slot-fill (rgba (- 1.0 press-t) (- 1.0 press-t) (- 1.0 press-t) 1.0))
                            (rgba press-w press-w press-w 0.0)))
           (fill (if (> fill-slot 0.5) pressed-fill input-color))
           (border (if (> border-slot 0.5) :button-border second-color))
           ;; Lit face: shaded by quadrant, a highlight glint, a touch of shadow.
           (glint (* hl-a key-spec edge-fade 0.10))
           (face-shadow (* sh-a (- 1.0 key-diffuse) 0.05))
           (face-k (* (+ 1.0 (* quadrant 0.08)) (- 1.0 face-shadow)))
           (fill-lit (+ (* fill (rgba face-k face-k face-k 1.0))
                        (rgba (+ (* hl-r glint (- 1.0 face-shadow)) (* sh-r face-shadow))
                              (+ (* hl-g glint (- 1.0 face-shadow)) (* sh-g face-shadow))
                              (+ (* hl-b glint (- 1.0 face-shadow)) (* sh-b face-shadow))
                              0.0)))
           ;; Lit rim: brighter toward the lights, specular, shadowed away.
           (rim-glint (* hl-a (+ (* key-spec 4.8) (* bounce-spec 2.25)) edge-fade))
           (rim-shadow (* sh-a (- 1.0 (max key-diffuse bounce-diffuse)) 0.42))
           (rim-k (* (+ 0.02 (* 0.54 key-diffuse) (* 0.26 bounce-diffuse)) (- 1.0 rim-shadow)))
           (border-lit (+ (* border (rgba rim-k rim-k rim-k 1.0))
                          (rgba (+ (* hl-r rim-glint (- 1.0 rim-shadow)) (* sh-r rim-shadow))
                                (+ (* hl-g rim-glint (- 1.0 rim-shadow)) (* sh-g rim-shadow))
                                (+ (* hl-b rim-glint (- 1.0 rim-shadow)) (* sh-b rim-shadow))
                                0.0)))
           ;; Face over rim (straight alpha, as button_surface.metal).
           (fa (* (dot fill (rgba 0.0 0.0 0.0 1.0)) inner-mask))
           (ba (* (dot border (rgba 0.0 0.0 0.0 1.0)) border-mask))
           (out-a (+ fa (* ba (- 1.0 fa))))
           (wb (* ba (- 1.0 fa)))
           (inv (/ 1.0 (max out-a 0.000001))))
       (+ (* (+ (* fill-lit (rgba fa fa fa 0.0)) (* border-lit (rgba wb wb wb 0.0)))
             (rgba inv inv inv 0.0))
          (rgba 0.0 0.0 0.0 out-a)))))

(def-bezel bezel-face eseq.widgets/bezel-rrect)
(def-bezel bezel-tab-face eseq.widgets/bezel-tab)

;; ── Dropdown: the trigger's lit surface, its badge and the chevron ──

;; The dropdown faces place points as the native dropdown shaders did, to
;; draw the same pixels: from the uv (`logical-uv`, 0..1 across the face,
;; y down) as ((u - 0.5) * 2 * aspect, (v - 0.5) * 2).
(defmacro dropdown-u () `(dot logical-uv (vec2 1.0 0.0)))
(defmacro dropdown-v () `(dot logical-uv (vec2 0.0 1.0)))

;; Distance at (px, py) to a rounded rect of half-size (sx, sy) and corner
;; radius r, in the order the native dropdown shader computes it.
(defmacro dropdown-rrect (px py sx sy r)
  `(let ((__dr-dx (+ (- (abs ,px) ,sx) ,r))
         (__dr-dy (+ (- (abs ,py) ,sy) ,r)))
     (- (+ (length (vec2 (max __dr-dx 0.0) (max __dr-dy 0.0)))
           (min (max __dr-dx __dr-dy) 0.0))
        ,r)))

;; A dropdown's lit surface, after AppKit's pop-up buttons: a rounded fill
;; with a soft top-to-bottom gradient, a 1px hairline that catches light
;; along the top edge and falls into shade along the bottom, and a faint
;; sheen just under the top. `dropdown-face` is the trigger and
;; `dropdown-badge-face` the badge behind the chevron.
;;   fill    `fill-slot` (the trigger's :dropdown-bg, the badge's
;;           :dropdown-badge-bg), 0 when the control has its own colour
;;   border  `border-slot` (:dropdown-border, :dropdown-badge-border), 0
;;           when it has its own (or none, under a p-lock or :border-color)
;; `input-color` and `second-color` are the fill and border it paints with
;; when not using a slot. `radius` is the corner radius over the
;; half-height (0 for the pill), `light` the edge light and sheen, `grad`
;; the fill's gradient.
(defmacro def-dropdown-surface (name fill-slot-color border-slot-color)
  `(defwidget ,name
     :width 4 :height 1
     :state (radius light grad fill-slot border-slot)
     :shader
     (let ((asp (max aspect 0.001))
           (px (* (- (eseq.widgets/dropdown-u) 0.5) 2.0 asp))
           (py (* (- (eseq.widgets/dropdown-v) 0.5) 2.0))
           (r (min (if (> radius 0.0) radius 0.75) (min asp 1.0)))
           (border-px 1.0)
           ;; The 1px hairline and the fill's coverage.
           (outer-d (eseq.widgets/dropdown-rrect px py asp 1.0 r))
           (pixel (max (max (fwidth px) (fwidth py)) 0.001))
           (inner-d (+ outer-d (* border-px pixel)))
           (outer (smoothstep pixel (* -1.0 pixel) outer-d))
           (inner (smoothstep pixel (* -1.0 pixel) inner-d))
           (border-mask (* outer (- 1.0 inner)))
           ;; Which way the nearest edge faces: 1 on the top, -1 on the bottom.
           (qx (- (abs px) (- asp r)))
           (qy (- (abs py) (- 1.0 r)))
           (qcx (max qx 0.0))
           (qcy (max qy 0.0))
           (ql (length (vec2 qcx qcy)))
           (ny (if (> ql 0.00001) (/ qcy ql) (if (> qx qy) 0.0 1.0)))
           (facing (* -1.0 (if (> py 0.0) ny (* -1.0 ny))))
           (top (clamp facing 0.0 1.0))
           (bottom (clamp (* -1.0 facing) 0.0 1.0))
           (up (- 1.0 (clamp (eseq.widgets/dropdown-v) 0.0 1.0)))
           (inside-px (max (- (/ (* -1.0 outer-d) pixel) border-px) 0.0))
           (fill (if (> fill-slot 0.5) ,fill-slot-color input-color))
           (border (if (> border-slot 0.5) ,border-slot-color second-color))
           (fill-a-in (dot fill (rgba 0.0 0.0 0.0 1.0)))
           (border-a-in (dot border (rgba 0.0 0.0 0.0 1.0)))
           (fill-lit (mix (* fill (+ 1.0 (* grad (- up 0.5))))
                          (rgba 1.0 1.0 1.0 1.0)
                          (* (exp (/ (* -1.0 inside-px) 1.6)) top light 0.14)))
           (border-lit (mix border (rgba 1.0 1.0 1.0 1.0) (* top light 0.6)))
           (border-a (* (clamp (+ (* border-a-in (- 1.0 (* bottom light 0.45))) (* top light 0.24)) 0.0 1.0)
                        border-mask
                        (if (>= border-a-in 0.001) 1.0 0.0)))
           ;; The hairline over the fill.
           (fill-a (* fill-a-in outer))
           (out-a (+ border-a (* fill-a (- 1.0 border-a))))
           (out-rgb (/ (+ (* border-lit border-a) (* (* fill-lit fill-a) (- 1.0 border-a))) out-a)))
       (if (< outer 0.002)
         (rgba 0.0 0.0 0.0 0.0)
         (if (<= out-a 0.002)
           (rgba 0.0 0.0 0.0 0.0)
           (+ (* out-rgb (rgba 1.0 1.0 1.0 0.0)) (rgba 0.0 0.0 0.0 out-a)))))))

(def-dropdown-surface dropdown-face :dropdown-bg :dropdown-border)
(def-dropdown-surface dropdown-badge-face :dropdown-badge-bg :dropdown-badge-border)

;; Distance at p to the segment from a to b.
(defmacro dropdown-segment (p a b)
  `(let ((__ds-pa (- ,p ,a))
         (__ds-ba (- ,b ,a))
         (__ds-h (clamp (/ (dot __ds-pa __ds-ba) (dot __ds-ba __ds-ba)) 0.0 1.0)))
     (length (- __ds-pa (* __ds-ba __ds-h)))))

;; The dropdown's chevron: a compact up "^" over a down "v", stroked in
;; `input-color`, or :dropdown-chevron when `chevron-slot` is 1 (0 when the
;; dropdown has its own :chevron-color).
(defwidget dropdown-chevron-face
  :width 1 :height 1
  :state (chevron-slot)
  :shader
  (let ((p (vec2 (* (- (eseq.widgets/dropdown-u) 0.5) 2.0 aspect)
                 (* (- (eseq.widgets/dropdown-v) 0.5) 2.0)))
        (hw (* 0.35 aspect))
        (up-pt (vec2 0.0 -0.70))
        (up-a (vec2 (* -1.0 hw) -0.22))
        (up-b (vec2 hw -0.22))
        (dn-pt (vec2 0.0 0.70))
        (dn-a (vec2 (* -1.0 hw) 0.22))
        (dn-b (vec2 hw 0.22))
        (d (min (min (eseq.widgets/dropdown-segment p up-a up-pt)
                     (eseq.widgets/dropdown-segment p up-pt up-b))
                (min (eseq.widgets/dropdown-segment p dn-a dn-pt)
                     (eseq.widgets/dropdown-segment p dn-pt dn-b))))
        (stroke 0.10)
        (edge (* (fwidth d) 1.2))
        (mask (smoothstep (+ stroke edge) (- stroke edge) d))
        (col (if (> chevron-slot 0.5) :dropdown-chevron input-color)))
    (if (< mask 0.002)
      (rgba 0.0 0.0 0.0 0.0)
      (+ (* col (rgba 1.0 1.0 1.0 0.0)) (rgba 0.0 0.0 0.0 (* (dot col (rgba 0.0 0.0 0.0 1.0)) mask))))))
