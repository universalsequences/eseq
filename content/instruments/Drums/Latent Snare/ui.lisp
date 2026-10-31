;; Latent Snare A/B panel (E6). The map IS the latent space: two latent dims
;; as the axes, the 32 VILLAIN snares at their learned codes, the ring at the z
;; the decoder will decode on the next hit = the A->B line at Blend plus the
;; z1..z4 offsets (the ring uses the plain blend; the DSP walks the line at
;; the perceptually even speed, so the ring can lead or lag by up to ~0.1 of
;; the segment). The dashed lead runs from Snare A. A click on a dot loads it
;; into the armed slot (A or B; Shift flips); a drag moves the ring by setting
;; the two map-axis offsets (one undo step per drag).
;; One accent, flat panels. The map's data is data/family.json, written with
;; the codes block below by e6/build_panel.py.
;; transport.recording / transport.playing for the map's "recording" badge
(import eseq.kinds :refer (transport))
(def ls-accent () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def ls-text () :fg)
(def ls-surf () :instrument-group-bg)

;; BEGIN GENERATED LATENT CODES
;; Written by e6/build_panel.py; do not edit by hand.
(def ls-family-file () "instruments/Drums/Latent Snare/data/family.json")
(def ls-x-param () "z1")
(def ls-y-param () "z3")
(def ls-x-dim () 1)
(def ls-y-dim () 3)
(def ls-size-dim () 2)
(def ls-knob-params () '("z2" "z4"))
(def ls-slot-labels () '((0 "SP01") (1 "SP02") (2 "SP03") (3 "SP04") (4 "SP05") (5 "SP06") (6 "SP07") (7 "SP08") (8 "SP09") (9 "SP10") (10 "SP11") (11 "SP12") (12 "SP13") (13 "SP14") (14 "SP15") (15 "SP16") (16 "SP17") (17 "SP18") (18 "SP19") (19 "SP20") (20 "XCF") (21 "XDXDFGHS") (22 "YETPRE") (23 "YNG") (24 "YUTE") (25 "YYUUU") (26 "ZDSZS") (27 "ZETWY") (28 "ZFG") (29 "ZSFG") (30 "ZV") (31 "ZXDVFB")))
;; (name z1 z2 z3 z4) in knob space, snare param order.
(def ls-codes ()
  '(
    ("SP01" -0.48858 0.21800 0.63605 0.19747)
    ("SP02" -0.90261 -0.56335 0.73687 0.07331)
    ("SP03" -0.74773 0.29799 -0.04349 -0.15361)
    ("SP04" -0.83685 -0.60949 -0.19629 -0.34156)
    ("SP05" -0.05395 0.47960 -0.58905 -0.34612)
    ("SP06" 0.21510 -0.59668 1.06556 -1.23941)
    ("SP07" 0.58937 0.44282 1.32992 -0.59526)
    ("SP08" -0.72808 0.02543 0.46675 0.95018)
    ("SP09" -0.29219 -0.25404 -1.10314 -0.27719)
    ("SP10" 0.23823 0.04959 0.17567 0.24956)
    ("SP11" -0.50246 -0.61294 -0.57516 0.11876)
    ("SP12" 0.61118 0.57553 0.31710 -0.44478)
    ("SP13" 1.40389 0.44852 0.24063 0.60344)
    ("SP14" -0.19180 0.17883 0.01783 0.26035)
    ("SP15" 0.43311 -0.09565 -1.12408 0.17019)
    ("SP16" -0.92226 -0.07505 -0.68169 -0.28151)
    ("SP17" 0.41451 0.50140 0.86889 0.25610)
    ("SP18" -0.17039 -0.14693 0.08784 -0.02938)
    ("SP19" 0.02822 -0.50833 -0.17399 0.47509)
    ("SP20" 0.99263 -2.50000 0.50435 1.17824)
    ("XCF" -1.23179 0.03675 0.28226 0.30009)
    ("XDXDFGHS" 0.58521 0.89020 0.28464 1.06995)
    ("YETPRE" 0.55059 -0.72832 -0.52608 -0.67933)
    ("YNG" -0.25864 0.13848 -0.74443 0.45950)
    ("YUTE" 0.72913 -0.05651 -1.18695 -0.77405)
    ("YYUUU" 0.23343 1.01980 -0.41895 0.31831)
    ("ZDSZS" 0.00341 0.55039 0.29403 -0.11168)
    ("ZETWY" 0.07372 0.29934 0.21110 -0.58150)
    ("ZFG" 0.52762 0.33505 -0.60475 -0.11471)
    ("ZSFG" 0.62352 -0.29795 0.09998 -1.41047)
    ("ZV" -0.00143 0.41316 -0.31026 1.27397)
    ("ZXDVFB" -0.92410 0.14436 0.65885 -0.57394)))
;; END GENERATED LATENT CODES


(def ls-gap () (eseq.effects.custom-ui-lego/ui-lego-gap))
(def ls-dense-h () (eseq.effects.custom-ui-lego/ui-lego-dense-h))
(def ls-tall-h () (+ (ls-dense-h) (eseq.effects.custom-ui-lego/ui-lego-small-h) (ls-gap)))
(def ls-total-h () (+ (ls-dense-h) (ls-tall-h) (ls-gap)))
(def ls-left-w () 15.0)
(def ls-slot-w () 6.4)
(def ls-map-w () 32.0)
(def ls-right-w () 26.0)

(def ls-param (name) (eseq.effects.custom-ui-runtime/custom-ui-current-param name))

(def ls-effective (name fallback)
  (let ((p (ls-param name)))
    (if p (eseq.effects.param-controls/param-effective-value p) fallback)))

(def ls-clip3 (x) (min 3 (max -3 x)))
(def ls-index (name) (min 31 (max 0 (floor (+ (ls-effective name 0) 0.5)))))

;; Per-scope UI state: which slot (A or B) a map click loads.
(defstate ls-ui-state '())
(def ls-state-get (key fallback)
  (let ((scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name)))
    (let ((entry (nth (filter |item| (= (get item :scope) scope-name) ls-ui-state) 0)))
      (if (and entry (get entry key)) (get entry key) fallback))))
(def ls-state-set-in (scope-name key value)
  (let ((entry (nth (filter |item| (= (get item :scope) scope-name) ls-ui-state) 0)))
    (set! ls-ui-state
      (cons
        (merge (if entry entry (dict :scope scope-name)) key value)
        (filter |item| (not (= (get item :scope) scope-name)) ls-ui-state)))))
(def ls-armed () (ls-state-get :armed "a"))

;; Latent dim d (1-based) of the A->B line at the plain blend.
(def ls-base (d)
  (let ((ca (nth (ls-codes) (ls-index "snare_a"))) (cb (nth (ls-codes) (ls-index "snare_b")))
        (bl (min 1 (max 0 (ls-effective "blend" 0)))))
    (+ (* (- 1 bl) (nth ca d)) (* bl (nth cb d)))))

;; z as the decoder sees it (approximately: plain blend), clipped to [-3, 3].
(def ls-zdim (d) (ls-clip3 (+ (ls-base d) (ls-effective (str "z" d) 0))))

(def ls-panel (section width height body)
  (box :width width :height height :padding 0.18
    :background-color (ls-surf)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    body))

;; A snare slot: its letter arms the map (a click on a dot loads that slot);
;; the box under it is the param itself, showing the snare's name.
(def ls-slot-box (slot name)
  (let ((p (ls-param name))
        (scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (armed (= (ls-armed) slot)))
    (if p
      (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "ls-slot-mod-" scope-name "-" name)
        (subtree :key (str "ls-slot-" scope-name (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name "-" (if armed 1 0))
          (v-stack :width (ls-slot-w) :gap 0.16 :align :stretch
            (button slot :width (ls-slot-w) :height 0.8 :font-size 9.5 :corner-radius 3 :padding 0
              :debug-name (str "ls-arm-" slot)
              :color (if armed :black (ls-text))
              :background-color (if armed (ls-accent) :mixer-strip-bg)
              :on-click (lambda (x y r) (ls-state-set-in scope-name :armed slot)))
            (number-picker :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
              :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
              :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
              :decimals 0 :step 1
              :value-labels (ls-slot-labels)
              :mode :plain :noui true
              :corner-radius 0
              :border-color :black
              :background-color :mixer-strip-bg
              :font-size 14
              :text-color (eseq.effects.custom-ui-runtime/custom-ui-param-knob-text-color p) :edit-color :yellow
              :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
              :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
              :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
              :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
              :text-align :center
              :width (ls-slot-w) :height 1.45
              :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s 0 p)))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def ls-snares-block ()
  (ls-panel 0 (ls-left-w) (ls-dense-h)
    (v-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 0 "SNARES" 4.0 (ls-accent))
      (h-stack :width :fill :gap 0.40 :align :start
        (ls-slot-box "a" "snare_a")
        (ls-slot-box "b" "snare_b")))))

(def ls-blend-block ()
  (ls-panel 0 (ls-left-w) (ls-tall-h)
    (v-stack :width :fill :height :fill :gap 0.20 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 0 "BLEND" 4.0 (ls-accent))
      (h-stack :width :fill :gap 0.30 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "blend" "blend" 6.4 (ls-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "even" "even" 6.4 (ls-accent) 0)))))

;; A drag puts the ring at (x, y): the two map-axis offsets become x - base,
;; y - base as one coalesced param gesture (one undo step per drag).
(def ls-move (scope x y phase)
  (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
    (list (list :x (ls-x-param)) (list :y (ls-y-param)))
    (dict :x (- x (ls-base (ls-x-dim))) :y (- y (ls-base (ls-y-dim)))
      :active (< phase 2))))

(def ls-map ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (pa (ls-param "snare_a")) (pb (ls-param "snare_b")))
    (box :width (ls-map-w) :height (ls-total-h) :corner-radius 12 :padding 0.18
      :background-color (ls-surf)
      (family-map
        :file (ls-family-file)
        :recording #'transport.recording :playing #'transport.playing
        :a (ls-index "snare_a")
        :armed (if (= (ls-armed) "b") 1 0)
        :x (ls-zdim (ls-x-dim)) :y (ls-zdim (ls-y-dim)) :size (ls-zdim (ls-size-dim))
        :accent (ls-accent)
        :dot-color :curve-editor-fg
        :background-color :curve-editor-bg
        :grid-color :curve-editor-grid
        :b-color :curve-editor-fg
        :label-color :curve-editor-fg
        :line-color :curve-editor-fg
        :x-label (ls-x-param) :y-label (ls-y-param)
        :width (- (ls-map-w) 0.4) :height (- (ls-total-h) 0.4)
        :on-pick (lambda (index slot)
          (if (and pa pb) (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope (if (= slot 1) pb pa) index)))
        :on-move (lambda (x y phase) (ls-move scope x y phase))))))

(def ls-offsets-block ()
  (ls-panel 1 (ls-right-w) (ls-dense-h)
    (h-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 1 "OFFSET" 4.6 (ls-accent))
      (h-stack :gap 0.10 :align :start
        (each '("z1" "z2" "z3" "z4") |name|
          (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 name name 4.8 (ls-accent) 2))))))

(def ls-tone-block ()
  (ls-panel 2 (ls-right-w) (ls-tall-h)
    (v-stack :width :fill :height :fill :gap 0.20 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 2 "TONE" 4.0 (ls-accent))
      (h-stack :width :fill :gap 0.30 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "tune" "tune" 6.2 (ls-accent) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "decay" "decay" 6.2 (ls-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "level" "level" 6.2 (ls-accent) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch :debug-name "ls-surface"
    (v-stack :width (ls-left-w) :gap (ls-gap)
      (ls-snares-block)
      (ls-blend-block))
    (ls-map)
    (v-stack :width (ls-right-w) :gap (ls-gap)
      (ls-offsets-block)
      (ls-tone-block))))
