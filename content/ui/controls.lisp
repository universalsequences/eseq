;; ui/controls.lisp — value controls bound to an instance field.
;;
;; A control takes an instance and a field name and configures itself from
;; what the kind declares about the field (field-info): a slider's range and
;; reset value, a choice's options. Reads go through (field x :f), writes
;; through (set-field! x :f v), as x.f and (set! x.f v) do.
;;
;; Unopinionated about looks: each draws as a box whose :background is the
;; widget named by :look, handed the control's state (:pos and :origin for a
;; slider, 0..1 along it; :on for a toggle or a lit button) plus whatever
;; :look-state adds. Sizes are the caller's, in cells.
;;
;;   (field-slider s :velocity :look "my-bar")      range, reset from the field
;;   (field-slider t :swing :name "SWING")          name left, value right
;;   (field-choice t :swing-resolution)             click on, right-click back
;;   (field-toggle t :poly :name "POLY")
;;   (text-button "+" (lambda (e) …))
;; (Not `slider` / `toggle` / `button`: those are built-in widgets.)

(module eseq.controls)

(import eseq.view-kit :refer (clamp index-of without))

(export field-slider field-choice field-toggle text-button along)

;; How far along a control the pointer is, 0..1: across it for :h, up it for
;; :v (e.v grows downward).
(def along (e axis) (clamp (if (= axis :v) (- 1 e.v) e.u) 0 1))

;; Text for a value: whole numbers bare, others to two places.
(def number-text (v)
  (if (number? v)
    (if (= v (round v)) (str (round v)) (str (/ (round (* 100 v)) 100)))
    (str v)))

;; A control's label: `name` on the left and `text` on the right, or `text`
;; centered without a name; nothing for a nil text and no name.
(def control-label (name text font active)
  (let ((text-label (lambda (t c)
                      (label t :font-size font :bg :transparent :v-align :center :color c
                        :active active :active-color :black))))
    (if name
      (h-stack :width :fill :height :fill :v-align :center :padding (* 0.06 font)
        (text-label name :dim)
        (box :flex 1 :height 0)
        (if text (text-label text :white) (list)))
      (if text (text-label text :white) (list)))))

;; A slider for field f of x. A numeric field spans its :range (field-info,
;; or :range (lo hi) given); a field with :options (a label) slides over
;; them. :to-unit / :from-unit map value <-> 0..1 for a curved field. :axis
;; :h tracks the pointer across, :v up; :capture keeps the drag past the box's
;; edges (off: a drag sweeps from box to box, editing each it crosses). A
;; double-click puts the default back. :text (a function of the value, or
;; false) is what it prints; :on-set writes instead of (set-field! x f v).
(def field-slider (x f &key (width 10) (height 3) (axis :h) (look "control-bar") name
                 range default to-unit from-unit (capture true) text (font 11) look-state
                 on-set)
  (let ((info (field-info x f))
        (opts (get info :options))
        (v (field x f))
        (labels (and opts (not (number? v))))
        (r (if range range (if labels (list 0 (- (len opts) 1)) (get info :range))))
        (lo (if r (nth r 0) 0))
        (hi (if r (nth r 1) 1))
        (reset (if (= default nil) (get info :default) default))
        (whole (or labels (= (get info :type) ":int")))
        (write (if on-set on-set (lambda (v) (set-field! x f v))))
        (unit (lambda (v)
                (if to-unit (to-unit v)
                  (/ (- (if labels (max 0 (index-of opts v)) v) lo) (max 0.000001 (- hi lo))))))
        (value-at (lambda (u)
                    (let ((v (if from-unit (from-unit u) (+ lo (* u (- hi lo)))))
                          (v (if whole (round v) v)))
                      (if labels (nth opts v) v))))
        (set-at (lambda (e) (write (value-at (along e axis))))))
    (apply box :width width :height height :background look
      :pos (unit v) :origin (if (and (< lo 0) (> hi 0) (not to-unit)) (unit 0) 0)
      :capture-pointer capture
      :on-mouse-down set-at :on-drag set-at
      :on-double-click (lambda (e) (when (not (= reset nil)) (write reset)))
      :h-align :center :v-align :center
      (control-label name (if (= text false) nil (if text (text v) (number-text v))) font false)
      (if look-state look-state (list)))))

;; A choice among the field's :options (or :options given): a click steps to
;; the next, a right-click to the previous, a double-click to the default.
;; With :on-open a click calls (on-open event) instead (to open a menu).
;; An :int field holds an index into the options, a :string field a label.
(def field-choice (x f &key (width 10) (height 3) (look "control-cell") name options
                 (font 11) look-state on-open)
  (let ((info (field-info x f))
        (xs (if options options (get info :options)))
        (v (field x f))
        (indexed (number? v))
        (i (if indexed v (max 0 (index-of xs v))))
        (pick (lambda (j) (let ((j (mod (+ j (len xs)) (len xs))))
                            (set-field! x f (if indexed j (nth xs j))))))
        (reset (get info :default)))
    (apply box :width width :height height :background look
      :on-click (lambda (e) (if on-open (on-open e) (pick (+ i 1))))
      :on-right-click (lambda (e) (pick (- i 1)))
      :on-double-click (lambda (e) (when (not (= reset nil)) (set-field! x f reset)))
      :h-align :center :v-align :center
      (control-label name (nth xs i) font false)
      (if look-state look-state (list)))))

;; A toggle for boolean field f of x: lit (:on) while set; a click flips it.
(def field-toggle (x f &key (width 10) (height 3) (look "control-cell") name text (font 11)
                 look-state)
  (let ((on (field x f)))
    (apply box :width width :height height :background look
      :on (if on 1 0)
      :on-click (lambda (e) (set-field! x f (not on)))
      :h-align :center :v-align :center
      ;; (= text nil), not (if text …): "" (no text) is false in eseqlisp
      (control-label name (if (= text nil) (if on "ON" "OFF") text) font on)
      (if look-state look-state (list)))))

;; A button: `text` centered, lit (:on) while :active (a value or a #'
;; binding), `on-click` (nil: none) on a click. Any other box props
;; (:on-right-click, :queued for its look, …) go to the box.
(def text-button (text on-click &rest props &key (width 10) (height 3) (look "control-cell")
                  active (font 11) &allow-other-keys)
  (apply box :width width :height height :background look
    :on active
    :h-align :center :v-align :center
    (control-label nil text font active)
    (append (if on-click (list :on-click on-click) (list))
            (without props (list :width :height :look :active :font)))))
