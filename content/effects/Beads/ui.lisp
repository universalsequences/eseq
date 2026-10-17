;; Beads — the module's front panel as laid out on the module:
;;
;;   FREEZE        DENSITY        SEED
;;   TIME          quality        PITCH
;;   feedback   SIZE     SHAPE    dry/wet
;;   latch   t±   sz±   sh±   p±   reverb
;;
;; The icon knobs keep the module's colors (blue loop = feedback, orange
;; cloud = dry/wet, red cube = reverb), latch sits where the module's
;; CV-assign button is, and the panel's dotted lines run from TIME, SIZE,
;; SHAPE and PITCH down to their attenurandomizers. Input and output level,
;; which the module sets on its own, sit in a narrow strip beside it. Every
;; control appears exactly once.
;;
;; The layout is fixed-size so the dotted lines, painted by bfx-panel-bg
;; behind the controls, land between them: moving a control means moving
;; its line in bfx-panel-bg too.

(def bfx-knob () :fg)
(def bfx-btn () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def bfx-fdbk () :blue)
(def bfx-wet () :control-on-bg)
(def bfx-verb () :red)

;; Geometry, in cells: a 27-wide face inside a 0.2 pad.
(def bfx-pad () 0.2)
(def bfx-face-w () 27.0)
(def bfx-w () 27.4)
(def bfx-h () 10.0)

;; The module's four media (QUALITY): recording rate, bit depth, buffer time.
(def bfx-quality-options ()
  '("dirty tape" "clean tape" "vintage" "hi-fi"))

;; Panel coordinates (cells from the top-left) to shader units.
(defmacro bfx-x (c) `(* (- (/ (* 2.0 ,c) 27.4) 1.0) width))
(defmacro bfx-y (c) `(* (- (/ (* 2.0 ,c) 10.0) 1.0) height))
(defmacro bfx-cells (c) `(* (/ (* 2.0 ,c) 10.0) height))

;; A dotted segment between two panel points: dots every 0.3 cells.
(defmacro bfx-dots (ax ay bx by)
  `(let ((dax (bfx-x ,ax)) (day (bfx-y ,ay)))
     (let ((pax (- x dax)) (pay (- y day))
           (bax (- (bfx-x ,bx) dax)) (bay (- (bfx-y ,by) day)))
       (let ((len (length (vec2 bax bay))))
         (let ((h (clamp (/ (+ (* pax bax) (* pay bay)) (* len len)) 0 1))
               (period (bfx-cells 0.3)))
           (let ((along (* h len)))
             (- (length (vec2 (- along (* period (round (/ along period))))
                              (length (vec2 (- pax (* h bax)) (- pay (* h bay))))))
                (bfx-cells 0.06))))))))

;; Control centers (x, from the panel's left edge, pad included):
;; TIME 2.7, PITCH 24.7; SIZE 10.2, SHAPE 17.2; time± 6.85, size± 11.15,
;; shape± 15.45, pitch± 19.75. Rows: top block to 4.4, SIZE row to 7.2,
;; attenurandomizer row to 9.8.
(defwidget bfx-panel-bg
  :width 27.4 :height 10.0
  :shader
  (sdf/layer
    (sdf/fill (sdf/rounded-rect width height (bfx-cells 0.33)) :instrument-group-bg)
    (sdf/fill
      (min
        ;; TIME and PITCH curve in past feedback and dry/wet...
        (bfx-dots 4.6 4.2 6.0 5.0)
        (bfx-dots 6.0 5.0 6.0 7.2)
        (bfx-dots 6.0 7.2 6.85 7.5)
        (bfx-dots 22.8 4.2 21.4 5.0)
        (bfx-dots 21.4 5.0 21.4 7.2)
        (bfx-dots 21.4 7.2 19.75 7.5)
        ;; ...and SIZE and SHAPE drop to theirs.
        (bfx-dots 10.4 7.15 11.15 7.5)
        (bfx-dots 17.0 7.15 15.45 7.5))
      :dimmer)))

;; Cap sizes as on the module: big (DENSITY, TIME, PITCH), medium (SIZE,
;; SHAPE), small (the icon knobs), smallest (the attenurandomizers).
(def bfx-big (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 5.0 3.1 3.0 (bfx-knob) decimals))
(def bfx-mid (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 4.6 2.7 2.4 (bfx-knob) decimals))
(def bfx-small (name title accent decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 4.4 2.7 2.0 accent decimals))
(def bfx-tiny (name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 3.3 2.5 1.7 (bfx-knob) 2))

(def bfx-gap (w) (box :width w :height 0.1))

(def bfx-button (name text width height)
  (box :width width :height height :v-align :center :h-align :center
    (eseq.effects.custom-ui-lego/ui-lego-chip-toggle-s 0 name text 4.0 (bfx-btn))))

(def bfx-row (height &rest children)
  (h-stack :width (bfx-face-w) :height height :gap 0 :align :center children))

;; FREEZE over TIME, DENSITY over QUALITY, SEED over PITCH.
(def bfx-top ()
  (h-stack :width (bfx-face-w) :height 4.2 :gap 0 :align :start
    (v-stack :width 5.0 :gap 0
      (bfx-button "freeze" "FREEZE" 5.0 1.1)
      (bfx-big "time" "time" 2))
    (bfx-gap 5.0)
    (v-stack :width 7.0 :gap 0 :align :center
      (bfx-big "density" "density" 2)
      (box :width 6.4 :height 1.1 :v-align :center
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "quality" "quality" 6.4 (bfx-quality-options) :fg)))
    (bfx-gap 5.0)
    (v-stack :width 5.0 :gap 0
      (bfx-button "seed" "SEED" 5.0 1.1)
      (bfx-big "pitch" "pitch" 1))))

;; Attenurandomizers: past noon they scale modulation, before noon they
;; randomize each grain (positive every grain, negative only some). In delay
;; mode (SIZE fully clockwise) size ± is the LP/HP filter, pitch ± a vibrato.
;; SEED fires a grain (or taps the delay tempo); latch keeps grains coming
;; without it, as the module's held SEED button does.
(defeffect-ui
  (h-stack :width :fill :gap 0.05 :align :start
    (box :width (bfx-w) :height (bfx-h) :background "bfx-panel-bg" :padding (bfx-pad)
      (v-stack :width (bfx-face-w) :gap 0.1
        (bfx-top)
        (bfx-row 2.7
          (bfx-small "feedback" "feedback" (bfx-fdbk) 2)
          (bfx-gap 3.3)
          (bfx-mid "size" "size" 2)
          (bfx-gap 2.4)
          (bfx-mid "shape" "shape" 2)
          (bfx-gap 3.3)
          (bfx-small "dry_wet" "dry/wet" (bfx-wet) 2))
        (bfx-row 2.5
          (bfx-button "latch" "latch" 4.4 2.5)
          (bfx-gap 0.6)
          (bfx-tiny "time_amt" "time ±")
          (bfx-gap 1.0)
          (bfx-tiny "size_amt" "size ±")
          (bfx-gap 1.0)
          (bfx-tiny "shape_amt" "shape ±")
          (bfx-gap 1.0)
          (bfx-tiny "pitch_amt" "pitch ±")
          (bfx-gap 1.4)
          (bfx-small "reverb" "reverb" (bfx-verb) 2))))
    (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 0 7.0 (bfx-h) :instrument-group-bg :border-inactive false
      (v-stack :width :fill :height :fill :gap 0.3 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "input_db" "input" 5.0 1 "dB" :fg)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level_db" "output" 5.0 1 "dB" :fg)))))
