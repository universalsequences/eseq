;; VILLAIN Kick panel. The 33 kicks on a map of the family's three principal
;; axes (deep across, swell up, tension as dot size), laid out the Digi Wave
;; way: KICKS / MORPH on the left, the map in the middle, FAMILY and the paged
;; detail knobs on the right. One accent. data/family.json (the map's data)
;; is written by .local/research/mf-doom-kicks/master/build_pca.py --family.

(def vk-accent () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def vk-text () :fg)
(def vk-surf () :instrument-group-bg)
(def vk-bord () :border-inactive)

(def vk-family-file () "instruments/Drums/VILLAIN Kick/data/family.json")

;; Kick param index -> the kick's number in the family (data/family.json :kicks).
(def vk-kick-labels ()
  '((0 "51") (1 "52") (2 "54") (3 "55") (4 "56") (5 "57") (6 "58") (7 "59") (8 "60")
    (9 "61") (10 "62") (11 "63") (12 "64") (13 "65") (14 "66") (15 "67") (16 "68")
    (17 "69") (18 "70") (19 "71") (20 "72") (21 "73") (22 "74") (23 "75") (24 "76")
    (25 "77") (26 "78") (27 "79") (28 "81") (29 "85") (30 "86") (31 "98") (32 "99")))

(def vk-left-w () 15.0)
(def vk-kick-w () 6.4)
(def vk-map-w () 30.0)
(def vk-right-w () 31.0)
(def vk-gap () (eseq.effects.custom-ui-lego/ui-lego-gap))
(def vk-dense-h () (eseq.effects.custom-ui-lego/ui-lego-dense-h))
(def vk-pages-h () (+ (vk-dense-h) (eseq.effects.custom-ui-lego/ui-lego-small-h) (vk-gap)))
(def vk-total-h () (+ (vk-dense-h) (vk-pages-h) (vk-gap)))

;; Per-scope UI state (which kick slot a map click loads, which detail page
;; shows), like Digi Wave's oscillator tab: one entry per instrument scope.
(defstate vk-ui-state '())

(def vk-state-get (key fallback)
  (let ((scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name)))
    (let ((entry (nth (filter |item| (= (get item :scope) scope-name) vk-ui-state) 0)))
      (if (and entry (get entry key)) (get entry key) fallback))))

(def vk-state-set-in (scope-name key value)
  (let ((entry (nth (filter |item| (= (get item :scope) scope-name) vk-ui-state) 0)))
    (set! vk-ui-state
      (cons
        (merge (if entry entry (dict :scope scope-name)) key value)
        (filter |item| (not (= (get item :scope) scope-name)) vk-ui-state)))))

(def vk-armed () (vk-state-get :armed "a"))
(def vk-page () (vk-state-get :page "hit"))

(def vk-panel (section width height body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section width height (vk-surf) (vk-bord) false body))

;; A kick slot: its letter arms the map (a click on a dot loads that slot);
;; the big box under it is the param itself, showing the kick's number.
(def vk-kick-box (slot name)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (armed (= (vk-armed) slot)))
    (if p
      (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "vk-kick-mod-" scope-name "-" name)
        (subtree :key (str "vk-kick-" scope-name (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name "-" (if armed 1 0))
          (v-stack :width (vk-kick-w) :gap 0.16 :align :stretch
            (button slot :width (vk-kick-w) :height 0.8 :font-size 9.5 :corner-radius 3 :padding 0
              :debug-name (str "vk-arm-" slot)
              :color (if armed :black (vk-text))
              :background-color (if armed (vk-accent) :mixer-strip-bg)
              :on-click (lambda (x y r) (vk-state-set-in scope-name :armed slot)))
            (number-picker :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
              :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
              :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
              :decimals 0 :step 1
              :value-labels (vk-kick-labels)
              :mode :plain :noui true
              :corner-radius 0
              :border-color :black
              :background-color :mixer-strip-bg
              :font-size 18
              :text-color (eseq.effects.custom-ui-runtime/custom-ui-param-knob-text-color p) :edit-color :yellow
              :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
              :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
              :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
              :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
              :text-align :center
              :width (vk-kick-w) :height 1.45
              :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s 0 p)))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def vk-kicks-block ()
  (vk-panel 0 (vk-left-w) (vk-dense-h)
    (v-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "KICKS" 4.0 (vk-accent))
      (h-stack :width :fill :gap 0.40 :align :start
        (vk-kick-box "a" "kick_a")
        (vk-kick-box "b" "kick_b")))))

(def vk-morph-block ()
  (vk-panel 0 (vk-left-w) (vk-pages-h)
    (v-stack :width :fill :height :fill :gap 0.20 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "MORPH" 4.0 (vk-accent))
      (h-stack :width :fill :gap 0.30 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "blend" "blend" 6.4 (vk-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "exaggerate" "exagg" 6.4 (vk-accent) 2)))))

(def vk-effective (name fallback)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (if p (eseq.effects.param-controls/param-effective-value p) fallback)))

;; The map. Every input is the param's effective value (base + live
;; modulation), so the morph point and preview follow modulation too.
(def vk-map ()
  (let ((pa (eseq.effects.custom-ui-runtime/custom-ui-current-param "kick_a"))
        (pb (eseq.effects.custom-ui-runtime/custom-ui-current-param "kick_b"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if (and pa pb)
      (box :width (vk-map-w) :height (vk-total-h) :corner-radius 12 :padding 0.18
           :background-color (vk-surf)
        (family-map
          :file (vk-family-file)
          :a (vk-effective "kick_a" 0) :b (vk-effective "kick_b" 0)
          :blend (vk-effective "blend" 0) :exaggerate (vk-effective "exaggerate" 0)
          :pc1 (vk-effective "pc1" 0) :pc2 (vk-effective "pc2" 0) :pc3 (vk-effective "pc3" 0)
          :spread (vk-effective "spread" 1) :beat (vk-effective "beat" 1)
          :tilt (vk-effective "tilt" 0) :drop-time (vk-effective "drop_time" 1)
          :armed (if (= (vk-armed) "b") 1 0)
          :accent (vk-accent)
          :background-color :instrument-control-bg
          :x-label "deep" :y-label "swell"
          :width (- (vk-map-w) 0.4) :height (- (vk-total-h) 0.4)
          :on-pick (lambda (index slot)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope (if (= slot 1) pb pa) index))))
      (label "missing kick params" :font-size 8 :color :red :bg :transparent))))

(def vk-family-block ()
  (vk-panel 1 (vk-right-w) (vk-dense-h)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 5.0 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "FAMILY" 5.0 (vk-accent)))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "pc1" "deep" 5.6 (vk-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "pc2" "swell" 5.6 (vk-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "pc3" "tension" 5.6 (vk-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "bend" "drop" 5.6 (vk-accent) 2)))))

;; Detail pages: (page-id section knobs...), each knob (param title decimals).
(def vk-pages ()
  (list
    (list "hit" 2 '(("decay" "decay" 2) ("knock" "knock" 2) ("tick" "tick" 2)))
    (list "shape" 2 '(("drop_time" "drop t" 2) ("tilt" "tilt" 2) ("spread" "spread" 2) ("beat" "beat" 2) ("body" "body" 2) ("attack" "attack" 2)))
    (list "colour" 3 '(("shape" "shape" 2) ("growl" "growl" 2) ("grit" "grit" 2) ("drive" "drive" 2) ("ceiling" "ceiling" 2) ("clip_amt" "clip" 2)))
    (list "air" 3 '(("air" "air" 2) ("air_time" "air t" 2) ("air_tone" "tone" 2) ("click" "click" 2) ("click_time" "click t" 2) ("click_tone" "tone" 2)))
    (list "room" 3 '(("room" "room" 2) ("room_time" "room t" 2) ("room_tone" "tone" 2) ("floor" "floor" 2) ("crackle" "crackle" 2) ("hum" "hum" 2)))
    (list "out" 4 '(("tune" "tune" 2) ("hold" "hold" 2) ("release" "release" 2) ("length" "length" 2) ("level" "level" 2)))))

(def vk-page-entry ()
  (let ((entry (nth (filter |page| (= (nth page 0) (vk-page)) (vk-pages)) 0)))
    (if entry entry (nth (vk-pages) 0))))

(def vk-pages-block ()
  (let ((scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (entry (vk-page-entry))
        (tab-w (/ (- (vk-right-w) 1.0) (len (vk-pages)))))
    (vk-panel (nth entry 1) (vk-right-w) (vk-pages-h)
      (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
        (h-stack :width :fill :height 1.02 :gap 0.0 :align :stretch
          (each (vk-pages) |page|
            (eseq.effects.custom-ui-lego/ui-lego-underline-tab
              (nth page 0) tab-w (= (nth page 0) (nth entry 0)) (vk-accent)
              (lambda (info) (vk-state-set-in scope-name :page (nth page 0)))
              (str "vk-page-" (nth page 0)))))
        (eseq.effects.custom-ui-lego/ui-detail-adsr-divider "vk-pages-divider")
        (subtree :key (str "vk-page-body-" (nth entry 0))
          (h-stack :width :fill :flex 1 :gap 0.10 :align :center
            (each (nth entry 2) |k|
              (eseq.effects.custom-ui-lego/ui-lego-knob-full-s (nth entry 1) (nth k 0) (nth k 1) 4.9 (vk-accent) (nth k 2)))))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch :debug-name "vk-surface"
    (v-stack :width (vk-left-w) :gap (vk-gap)
      (vk-kicks-block)
      (vk-morph-block))
    (vk-map)
    (v-stack :width (vk-right-w) :gap (vk-gap)
      (vk-family-block)
      (vk-pages-block))))
