;; Latent Kick A/B panel (kick/scripts/build_instrument.py, from E6's Latent Snare panel). The map IS the latent space: two latent dims
;; as the axes, the 33 VILLAIN kicks at their learned codes, the ring at the z
;; the decoder will decode on the next hit = the A->B line at Blend plus the
;; z1..z4 offsets (the ring uses the plain blend; the DSP walks the line at
;; the perceptually even speed, so the ring can lead or lag by up to ~0.1 of
;; the segment). The dashed lead runs from Kick A. A click on a dot loads it
;; into the armed slot (A or B; Shift flips); a drag moves the ring by setting
;; the two map-axis offsets (one undo step per drag).
;; One accent, flat panels. The map's data is data/family.json, written with
;; the codes block below by kick/scripts/build_instrument.py.
;; transport.recording / transport.playing for the map's "recording" badge
(import eseq.kinds :refer (transport))
(def lk-accent () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def lk-text () :fg)
(def lk-surf () :instrument-group-bg)

;; BEGIN GENERATED LATENT CODES
;; Written by kick/scripts/build_instrument.py; do not edit by hand.
(def lk-family-file () "instruments/Drums/Latent Kick/data/family.json")
(def lk-x-param () "z1")
(def lk-y-param () "z2")
(def lk-x-dim () 1)
(def lk-y-dim () 2)
(def lk-size-dim () 3)
(def lk-knob-params () '("z3" "z4"))
(def lk-slot-labels () '((0 "K51") (1 "K52") (2 "K54") (3 "K55") (4 "K56") (5 "K57") (6 "K58") (7 "K59") (8 "K60") (9 "K61") (10 "K62") (11 "K63") (12 "K64") (13 "K65") (14 "K66") (15 "K67") (16 "K68") (17 "K69") (18 "K70") (19 "K71") (20 "K72") (21 "K73") (22 "K74") (23 "K75") (24 "K76") (25 "K77") (26 "K78") (27 "K79") (28 "K81") (29 "K85") (30 "K86") (31 "K98") (32 "K99")))
;; (name z1 z2 z3 z4) in knob space, kick param order.
(def lk-codes ()
  '(
    ("K51" 0.05894 -0.06089 2.37795 -0.52032)
    ("K52" 1.42291 0.09593 -0.37135 -0.50597)
    ("K54" 0.52485 -0.95272 -1.14174 -0.23235)
    ("K55" 2.01662 1.35400 -1.02335 -0.31946)
    ("K56" 0.22208 2.50000 -0.02451 1.11895)
    ("K57" 2.30657 0.95663 0.00841 1.13170)
    ("K58" -0.45966 0.20230 0.81977 -1.74578)
    ("K59" 0.68678 0.70364 -0.54074 -1.12661)
    ("K60" 1.29162 -1.39160 -0.76984 1.17581)
    ("K61" -0.11826 -0.82413 1.28365 -0.16805)
    ("K62" 0.58853 -0.35093 -0.28474 -1.44638)
    ("K63" -1.04296 -1.16407 -1.27207 0.15308)
    ("K64" -0.23174 -0.04854 1.03732 1.38421)
    ("K65" -0.37690 -0.45948 -0.28130 0.04013)
    ("K66" -0.60526 0.23358 -0.82963 -1.30729)
    ("K67" -0.05917 2.21557 1.30081 1.34759)
    ("K68" -0.21500 0.16736 0.00909 0.31691)
    ("K69" -1.01103 0.06629 1.39063 -0.51258)
    ("K70" -1.08629 -0.52738 -0.84065 1.27465)
    ("K71" 0.18083 0.68880 -0.21352 0.25884)
    ("K72" -0.71403 -1.34916 0.26005 -0.23334)
    ("K73" -0.34363 -1.30228 0.62810 1.33639)
    ("K74" -0.02790 0.98426 -0.03054 -1.32792)
    ("K75" -1.38022 0.90391 -0.78633 0.16900)
    ("K76" 0.16345 0.20600 -0.06653 -0.35344)
    ("K77" -1.56868 -0.30045 0.32407 1.08773)
    ("K78" -1.14126 -0.61731 0.51888 -0.45110)
    ("K79" 1.75165 -0.81331 -0.47667 0.30766)
    ("K81" -1.26146 0.95697 0.55130 -0.71019)
    ("K85" 1.28944 -1.38203 0.97367 0.68774)
    ("K86" 1.81564 -1.02509 0.51780 -1.00715)
    ("K98" -1.14550 0.29052 -2.30838 0.33879)
    ("K99" -1.53097 0.04363 -0.73958 -0.16125)))
;; END GENERATED LATENT CODES


(def lk-gap () (eseq.effects.custom-ui-lego/ui-lego-gap))
(def lk-dense-h () (eseq.effects.custom-ui-lego/ui-lego-dense-h))
(def lk-tall-h () (+ (lk-dense-h) (eseq.effects.custom-ui-lego/ui-lego-small-h) (lk-gap)))
(def lk-total-h () (+ (lk-dense-h) (lk-tall-h) (lk-gap)))
(def lk-left-w () 15.0)
(def lk-slot-w () 6.4)
(def lk-map-w () 32.0)
(def lk-right-w () 26.0)

(def lk-param (name) (eseq.effects.custom-ui-runtime/custom-ui-current-param name))

(def lk-effective (name fallback)
  (let ((p (lk-param name)))
    (if p (eseq.effects.param-controls/param-effective-value p) fallback)))

(def lk-clip3 (x) (min 3 (max -3 x)))
(def lk-index (name) (min 32 (max 0 (floor (+ (lk-effective name 0) 0.5)))))

;; Per-scope UI state: which slot (A or B) a map click loads.
(defstate lk-ui-state '())
(def lk-state-get (key fallback)
  (let ((scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name)))
    (let ((entry (nth (filter |item| (= (get item :scope) scope-name) lk-ui-state) 0)))
      (if (and entry (get entry key)) (get entry key) fallback))))
(def lk-state-set-in (scope-name key value)
  (let ((entry (nth (filter |item| (= (get item :scope) scope-name) lk-ui-state) 0)))
    (set! lk-ui-state
      (cons
        (merge (if entry entry (dict :scope scope-name)) key value)
        (filter |item| (not (= (get item :scope) scope-name)) lk-ui-state)))))
(def lk-armed () (lk-state-get :armed "a"))

;; Latent dim d (1-based) of the A->B line at the plain blend.
(def lk-base (d)
  (let ((ca (nth (lk-codes) (lk-index "kick_a"))) (cb (nth (lk-codes) (lk-index "kick_b")))
        (bl (min 1 (max 0 (lk-effective "blend" 0)))))
    (+ (* (- 1 bl) (nth ca d)) (* bl (nth cb d)))))

;; z as the decoder sees it (approximately: plain blend), clipped to [-3, 3].
(def lk-zdim (d) (lk-clip3 (+ (lk-base d) (lk-effective (str "z" d) 0))))

(def lk-panel (section width height body)
  (box :width width :height height :padding 0.18
    :background-color (lk-surf)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)
    body))

;; A kick slot: its letter arms the map (a click on a dot loads that slot);
;; the box under it is the param itself, showing the kick's name.
(def lk-slot-box (slot name)
  (let ((p (lk-param name))
        (scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (armed (= (lk-armed) slot)))
    (if p
      (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "lk-slot-mod-" scope-name "-" name)
        (subtree :key (str "lk-slot-" scope-name (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name "-" (if armed 1 0))
          (v-stack :width (lk-slot-w) :gap 0.16 :align :stretch
            (button slot :width (lk-slot-w) :height 0.8 :font-size 9.5 :corner-radius 3 :padding 0
              :debug-name (str "lk-arm-" slot)
              :color (if armed :black (lk-text))
              :background-color (if armed (lk-accent) :mixer-strip-bg)
              :on-click (lambda (x y r) (lk-state-set-in scope-name :armed slot)))
            (number-picker :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
              :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
              :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
              :decimals 0 :step 1
              :value-labels (lk-slot-labels)
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
              :width (lk-slot-w) :height 1.45
              :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s 0 p)))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def lk-kicks-block ()
  (lk-panel 0 (lk-left-w) (lk-dense-h)
    (v-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 0 "KICKS" 4.0 (lk-accent))
      (h-stack :width :fill :gap 0.40 :align :start
        (lk-slot-box "a" "kick_a")
        (lk-slot-box "b" "kick_b")))))

(def lk-blend-block ()
  (lk-panel 0 (lk-left-w) (lk-tall-h)
    (v-stack :width :fill :height :fill :gap 0.20 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 0 "BLEND" 4.0 (lk-accent))
      (h-stack :width :fill :gap 0.30 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "blend" "blend" 6.4 (lk-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "even" "even" 6.4 (lk-accent) 0)))))

;; A drag puts the ring at (x, y): the two map-axis offsets become x - base,
;; y - base as one coalesced param gesture (one undo step per drag).
(def lk-move (scope x y phase)
  (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
    (list (list :x (lk-x-param)) (list :y (lk-y-param)))
    (dict :x (- x (lk-base (lk-x-dim))) :y (- y (lk-base (lk-y-dim)))
      :active (< phase 2))))

(def lk-map ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope))
        (pa (lk-param "kick_a")) (pb (lk-param "kick_b")))
    (box :width (lk-map-w) :height (lk-total-h) :corner-radius 12 :padding 0.18
      :background-color (lk-surf)
      (family-map
        :file (lk-family-file)
        :recording #'transport.recording :playing #'transport.playing
        :a (lk-index "kick_a")
        :armed (if (= (lk-armed) "b") 1 0)
        :x (lk-zdim (lk-x-dim)) :y (lk-zdim (lk-y-dim)) :size (lk-zdim (lk-size-dim))
        :accent (lk-accent)
        :dot-color :curve-editor-fg
        :background-color :curve-editor-bg
        :grid-color :curve-editor-grid
        :b-color :curve-editor-fg
        :label-color :curve-editor-fg
        :line-color :curve-editor-fg
        :x-label (lk-x-param) :y-label (lk-y-param)
        :width (- (lk-map-w) 0.4) :height (- (lk-total-h) 0.4)
        :on-pick (lambda (index slot)
          (if (and pa pb) (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope (if (= slot 1) pb pa) index)))
        :on-move (lambda (x y phase) (lk-move scope x y phase))))))

(def lk-offsets-block ()
  (lk-panel 1 (lk-right-w) (lk-dense-h)
    (h-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 1 "OFFSET" 4.6 (lk-accent))
      (h-stack :gap 0.10 :align :start
        (each '("z1" "z2" "z3" "z4") |name|
          (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 name name 4.8 (lk-accent) 2))))))

;; The rest of VILLAIN Kick's transforms (drop_time, bend, knock, tick, ...) stay
;; params at their defaults: reachable from mods and automation, off the panel.
(def lk-tone-block ()
  (lk-panel 2 (lk-right-w) (lk-tall-h)
    (v-stack :width :fill :height :fill :gap 0.20 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-section-badge-s 2 "TONE" 4.0 (lk-accent))
      (h-stack :width :fill :gap 0.20 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "tune" "tune" 5.2 (lk-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "decay" "decay" 5.2 (lk-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "drive" "drive" 5.2 (lk-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "level" "level" 5.2 (lk-accent) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch :debug-name "lk-surface"
    (v-stack :width (lk-left-w) :gap (lk-gap)
      (lk-kicks-block)
      (lk-blend-block))
    (lk-map)
    (v-stack :width (lk-right-w) :gap (lk-gap)
      (lk-offsets-block)
      (lk-tone-block))))
