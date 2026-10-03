;; VILLAIN Snare panel. The 32 snares on a map of the family's principal
;; axes (sustain across, tension up, crisp as dot size), laid out like the
;; VILLAIN Kick: SNARES / MORPH on the left, the map in the middle, FAMILY and
;; the paged detail knobs on the right. One accent. data/family.json (the
;; map's data) is read out of dsp.lisp by
;; .local/research/mf-doom-snares/master/export_family.py.
(def vs-accent () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def vs-text () :fg)
(def vs-surf () :instrument-group-bg)
(def vs-bord () :border-inactive)

(def vs-family-file () "instruments/Drums/VILLAIN Snare/data/family.json")

;; Snare param index -> its name in the family (data/family.json :names).
(def vs-slot-labels ()
  '((0 "SP01") (1 "SP02") (2 "SP03") (3 "SP04") (4 "SP05") (5 "SP06") (6 "SP07")
    (7 "SP08") (8 "SP09") (9 "SP10") (10 "SP11") (11 "SP12") (12 "SP13") (13 "SP14")
    (14 "SP15") (15 "SP16") (16 "SP17") (17 "SP18") (18 "SP19") (19 "SP20") (20 "XCF")
    (21 "XDXDFGHS") (22 "YETPRE") (23 "YNG") (24 "YUTE") (25 "YYUUU") (26 "ZDSZS")
    (27 "ZETWY") (28 "ZFG") (29 "ZSFG") (30 "ZV") (31 "ZXDVFB")))

(def vs-left-w () 15.0)
(def vs-slot-w () 6.4)
(def vs-map-w () 30.0)
(def vs-right-w () 31.0)
(def vs-gap () (eseq.effects.custom-ui-lego/ui-lego-gap))
(def vs-dense-h () (eseq.effects.custom-ui-lego/ui-lego-dense-h))
(def vs-pages-h () (+ (vs-dense-h) (eseq.effects.custom-ui-lego/ui-lego-small-h) (vs-gap)))
(def vs-total-h () (+ (vs-dense-h) (vs-pages-h) (vs-gap)))

;; Per-scope UI state (which snare slot a map click loads, which detail page
;; shows), like Digi Wave's oscillator tab: one entry per instrument scope.
(defstate vs-ui-state '())

(def vs-state-get (key fallback)
  (let ((scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name)))
    (let ((entry (nth (filter |item| (= (get item :scope) scope-name) vs-ui-state) 0)))
      (if (and entry (get entry key)) (get entry key) fallback))))

(def vs-state-set-in (scope-name key value)
  (let ((entry (nth (filter |item| (= (get item :scope) scope-name) vs-ui-state) 0)))
    (set! vs-ui-state
      (cons
        (merge (if entry entry (dict :scope scope-name)) key value)
        (filter |item| (not (= (get item :scope) scope-name)) vs-ui-state)))))

(def vs-armed () (vs-state-get :armed "a"))
(def vs-page () (vs-state-get :page "head"))

(def vs-panel (section width height body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section width height (vs-surf) (vs-bord) false body))

;; A snare slot: its letter arms the map (a click on a dot loads that slot);
;; the big box under it is the param itself, showing the snare's name.
(def vs-slot-box (slot name)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (armed (= (vs-armed) slot)))
    (if p
      (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "vs-slot-mod-" scope-name "-" name)
        (subtree :key (str "vs-slot-" scope-name (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name "-" (if armed 1 0))
          (v-stack :width (vs-slot-w) :gap 0.16 :align :stretch
            (button slot :width (vs-slot-w) :height 0.8 :font-size 9.5 :corner-radius 3 :padding 0
              :debug-name (str "vs-arm-" slot)
              :color (if armed :black (vs-text))
              :background-color (if armed (vs-accent) :mixer-strip-bg)
              :on-click (lambda (x y r) (vs-state-set-in scope-name :armed slot)))
            (number-picker :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
              :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
              :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
              :decimals 0 :step 1
              :value-labels (vs-slot-labels)
              :mode :plain :noui true
              :corner-radius 0
              :border-color :black
              :background-color :mixer-strip-bg
              :font-size 18
              :text-color (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) :edit-color :yellow
              :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
              :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
              :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
              :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
              :text-align :center
              :width (vs-slot-w) :height 1.45
              :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s 0 p)))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def vs-slots-block ()
  (vs-panel 0 (vs-left-w) (vs-dense-h)
    (v-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "SNARES" 4.0 (vs-accent))
      (h-stack :width :fill :gap 0.40 :align :start
        (vs-slot-box "a" "snare_a")
        (vs-slot-box "b" "snare_b")))))

(def vs-morph-block ()
  (vs-panel 0 (vs-left-w) (vs-pages-h)
    (v-stack :width :fill :height :fill :gap 0.20 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "MORPH" 4.0 (vs-accent))
      (h-stack :width :fill :gap 0.30 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "blend" "blend" 6.4 (vs-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "exaggerate" "exagg" 6.4 (vs-accent) 2)))))

(def vs-effective (name fallback)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (if p (eseq.effects.param-controls/param-effective-value p) fallback)))

;; The map. Every input is the param's effective value (base + live
;; modulation), so the morph point and preview follow modulation too.
(def vs-map ()
  (let ((pa (eseq.effects.custom-ui-runtime/custom-ui-current-param "snare_a"))
        (pb (eseq.effects.custom-ui-runtime/custom-ui-current-param "snare_b"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if (and pa pb)
      (box :width (vs-map-w) :height (vs-total-h) :corner-radius 12 :padding 0.18
           :background-color (vs-surf)
        (family-map
          :file (vs-family-file)
          :a (vs-effective "snare_a" 0) :b (vs-effective "snare_b" 0)
          :blend (vs-effective "blend" 0) :exaggerate (vs-effective "exaggerate" 0)
          :pc1 (vs-effective "pc1" 0) :pc2 (vs-effective "pc2" 0) :pc3 (vs-effective "pc3" 0)
          :decay (vs-effective "decay" 1) :bend (vs-effective "bend" 1)
          :body (vs-effective "body" 1) :ring (vs-effective "ring" 1) :tune (vs-effective "tune" 1)
          :armed (if (= (vs-armed) "b") 1 0)
          :accent (vs-accent)
          :background-color :instrument-control-bg
          :x-label "sustain" :y-label "tension"
          :width (- (vs-map-w) 0.4) :height (- (vs-total-h) 0.4)
          :on-pick (lambda (index slot)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope (if (= slot 1) pb pa) index))))
      (label "missing snare params" :font-size 8 :color :red :bg :transparent))))

(def vs-family-block ()
  (vs-panel 1 (vs-right-w) (vs-dense-h)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 5.0 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "FAMILY" 5.0 (vs-accent)))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "pc1" "sustain" 5.6 (vs-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "pc2" "tension" 5.6 (vs-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "pc3" "crisp" 5.6 (vs-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 1 "release" "release" 5.6 (vs-accent) 0)))))

;; Detail pages: (page-id section knobs...), each knob (param title decimals).
;; Detail pages: (page-id section knobs...), each knob (param title decimals).
(def vs-pages ()
  (list
    (list "head" 2 '(("body" "body" 2) ("ring" "ring" 2) ("bend" "bend" 2) ("decay" "decay" 2) ("tune" "tune" 2)))
    (list "wires" 2 '(("rattle" "rattle" 2) ("buzz" "buzz" 2) ("crack" "crack" 2) ("strokes" "strokes" 2)))
    (list "air" 3 '(("room" "room" 2) ("extra" "extra" 2) ("hiss" "hiss" 2) ("dust" "dust" 2)))
    (list "out" 4 '(("drive" "drive" 2) ("grit" "grit" 2) ("length" "length" 2) ("level" "level" 2)))))

(def vs-page-entry ()
  (let ((entry (nth (filter |page| (= (nth page 0) (vs-page)) (vs-pages)) 0)))
    (if entry entry (nth (vs-pages) 0))))

(def vs-pages-block ()
  (let ((scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (entry (vs-page-entry))
        (tab-w (/ (- (vs-right-w) 1.0) (len (vs-pages)))))
    (vs-panel (nth entry 1) (vs-right-w) (vs-pages-h)
      (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
        (h-stack :width :fill :height 1.02 :gap 0.0 :align :stretch
          (each (vs-pages) |page|
            (eseq.effects.custom-ui-lego/ui-lego-underline-tab
              (nth page 0) tab-w (= (nth page 0) (nth entry 0)) (vs-accent)
              (lambda (info) (vs-state-set-in scope-name :page (nth page 0)))
              (str "vs-page-" (nth page 0)))))
        (eseq.effects.custom-ui-lego/ui-detail-adsr-divider "vs-pages-divider")
        (subtree :key (str "vs-page-body-" (nth entry 0))
          (h-stack :width :fill :flex 1 :gap 0.10 :align :center
            (each (nth entry 2) |k|
              (eseq.effects.custom-ui-lego/ui-lego-knob-full-s (nth entry 1) (nth k 0) (nth k 1) 4.9 (vs-accent) (nth k 2)))))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch :debug-name "vs-surface"
    (v-stack :width (vs-left-w) :gap (vs-gap)
      (vs-slots-block)
      (vs-morph-block))
    (vs-map)
    (v-stack :width (vs-right-w) :gap (vs-gap)
      (vs-family-block)
      (vs-pages-block))))
