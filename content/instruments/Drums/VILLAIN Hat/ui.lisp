;; VILLAIN Hat panel. The 12 hats on a map measured from renders of this
;; instrument (bright across, long up, tone as dot size), laid out like the
;; VILLAIN Kick: HATS / MORPH on the left, the map in the middle, NOTE and the
;; paged detail knobs on the right. One accent. data/family.json (map
;; positions + the preview's render lattice) is written by
;; .local/research/mf-doom-hats/master/export_family.py.
(def vh-accent () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def vh-text () :fg)
(def vh-surf () :instrument-group-bg)
(def vh-bord () :border-inactive)

(def vh-family-file () "instruments/Drums/VILLAIN Hat/data/family.json")

;; Hat param index -> its name in the family (data/family.json :names).
(def vh-slot-labels ()
  '((0 "CL81") (1 "CL93") (2 "CL94") (3 "CL95") (4 "CL96") (5 "OP54") (6 "OP78")
    (7 "OP83") (8 "OP85") (9 "OP89") (10 "OP100") (11 "OP104")))

(def vh-left-w () 15.0)
(def vh-slot-w () 6.4)
(def vh-map-w () 30.0)
(def vh-right-w () 31.0)
(def vh-gap () (eseq.effects.custom-ui-lego/ui-lego-gap))
(def vh-dense-h () (eseq.effects.custom-ui-lego/ui-lego-dense-h))
(def vh-pages-h () (+ (vh-dense-h) (eseq.effects.custom-ui-lego/ui-lego-small-h) (vh-gap)))
(def vh-total-h () (+ (vh-dense-h) (vh-pages-h) (vh-gap)))

;; Per-scope UI state (which hat slot a map click loads, which detail page
;; shows), like Digi Wave's oscillator tab: one entry per instrument scope.
(defstate vh-ui-state '())

(def vh-state-get (key fallback)
  (let ((scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name)))
    (let ((entry (nth (filter |item| (= (get item :scope) scope-name) vh-ui-state) 0)))
      (if (and entry (get entry key)) (get entry key) fallback))))

(def vh-state-set-in (scope-name key value)
  (let ((entry (nth (filter |item| (= (get item :scope) scope-name) vh-ui-state) 0)))
    (set! vh-ui-state
      (cons
        (merge (if entry entry (dict :scope scope-name)) key value)
        (filter |item| (not (= (get item :scope) scope-name)) vh-ui-state)))))

(def vh-armed () (vh-state-get :armed "a"))
(def vh-page () (vh-state-get :page "metal"))

(def vh-panel (section width height body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section width height (vh-surf) (vh-bord) false body))

;; A hat slot: its letter arms the map (a click on a dot loads that slot);
;; the big box under it is the param itself, showing the hat's name.
(def vh-slot-box (slot name)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (armed (= (vh-armed) slot)))
    (if p
      (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "vh-slot-mod-" scope-name "-" name)
        (subtree :key (str "vh-slot-" scope-name (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name "-" (if armed 1 0))
          (v-stack :width (vh-slot-w) :gap 0.16 :align :stretch
            (button slot :width (vh-slot-w) :height 0.8 :font-size 9.5 :corner-radius 3 :padding 0
              :debug-name (str "vh-arm-" slot)
              :color (if armed :black (vh-text))
              :background-color (if armed (vh-accent) :mixer-strip-bg)
              :on-click (lambda (x y r) (vh-state-set-in scope-name :armed slot)))
            (number-picker :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
              :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
              :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
              :decimals 0 :step 1
              :value-labels (vh-slot-labels)
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
              :width (vh-slot-w) :height 1.45
              :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s 0 p)))))
      (label (str "missing: " name) :font-size 8 :color :red :bg :transparent))))

(def vh-slots-block ()
  (vh-panel 0 (vh-left-w) (vh-dense-h)
    (v-stack :width :fill :height :fill :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "HATS" 4.0 (vh-accent))
      (h-stack :width :fill :gap 0.40 :align :start
        (vh-slot-box "a" "hat_a")
        (vh-slot-box "b" "hat_b")))))

(def vh-morph-block ()
  (vh-panel 0 (vh-left-w) (vh-pages-h)
    (v-stack :width :fill :height :fill :gap 0.20 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "MORPH" 4.0 (vh-accent))
      (h-stack :width :fill :gap 0.30 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "blend" "blend" 6.4 (vh-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "exaggerate" "exagg" 6.4 (vh-accent) 2)))))

(def vh-effective (name fallback)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (if p (eseq.effects.param-controls/param-effective-value p) fallback)))

;; The map. Every input is the param's effective value (base + live
;; modulation), so the morph point and preview follow modulation too.
(def vh-map ()
  (let ((pa (eseq.effects.custom-ui-runtime/custom-ui-current-param "hat_a"))
        (pb (eseq.effects.custom-ui-runtime/custom-ui-current-param "hat_b"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if (and pa pb)
      (box :width (vh-map-w) :height (vh-total-h) :corner-radius 12 :padding 0.18
           :background-color (vh-surf)
        (family-map
          :file (vh-family-file)
          :a (vh-effective "hat_a" 0) :b (vh-effective "hat_b" 0)
          :blend (vh-effective "blend" 0) :exaggerate (vh-effective "exaggerate" 0)
          :armed (if (= (vh-armed) "b") 1 0)
          :accent (vh-accent)
          :background-color :instrument-control-bg
          :x-label "bright" :y-label "long"
          :width (- (vh-map-w) 0.4) :height (- (vh-total-h) 0.4)
          :on-pick (lambda (index slot)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope (if (= slot 1) pb pa) index))))
      (label "missing hat params" :font-size 8 :color :red :bg :transparent))))

(def vh-family-block ()
  (vh-panel 1 (vh-right-w) (vh-dense-h)
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 5.0 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "NOTE" 5.0 (vh-accent)))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 1 "release" "release" 5.6 (vh-accent) 0)
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 1 "decay" "decay" 5.6 (vh-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "pedal" "pedal" 5.6 (vh-accent) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "keytrack" "keytrk" 5.6 (vh-accent) 2)))))

;; Detail pages: (page-id section knobs...), each knob (param title decimals).
;; Detail pages: (page-id section knobs...), each knob (param title decimals).
;; test_freeze is a research switch (frozen noise), deliberately not on the panel.
(def vh-pages ()
  (list
    (list "metal" 2 '(("tune" "tune" 2) ("metal" "metal dB" 1) ("ring" "ring" 2) ("motion" "motion" 2) ("tick" "tick" 2) ("sizzle" "sizzle" 2)))
    (list "record" 3 '(("tone" "tone" 2) ("grit" "grit" 2) ("codec" "codec" 2) ("record" "record" 2)))
    (list "out" 4 '(("drive" "drive" 2) ("length" "length" 2) ("level" "level" 2)))))

(def vh-page-entry ()
  (let ((entry (nth (filter |page| (= (nth page 0) (vh-page)) (vh-pages)) 0)))
    (if entry entry (nth (vh-pages) 0))))

(def vh-pages-block ()
  (let ((scope-name (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
        (entry (vh-page-entry))
        (tab-w (/ (- (vh-right-w) 1.0) (len (vh-pages)))))
    (vh-panel (nth entry 1) (vh-right-w) (vh-pages-h)
      (v-stack :width :fill :height :fill :gap 0.0 :align :stretch
        (h-stack :width :fill :height 1.02 :gap 0.0 :align :stretch
          (each (vh-pages) |page|
            (eseq.effects.custom-ui-lego/ui-lego-underline-tab
              (nth page 0) tab-w (= (nth page 0) (nth entry 0)) (vh-accent)
              (lambda (info) (vh-state-set-in scope-name :page (nth page 0)))
              (str "vh-page-" (nth page 0)))))
        (eseq.effects.custom-ui-lego/ui-detail-adsr-divider "vh-pages-divider")
        (subtree :key (str "vh-page-body-" (nth entry 0))
          (h-stack :width :fill :flex 1 :gap 0.10 :align :center
            (each (nth entry 2) |k|
              (eseq.effects.custom-ui-lego/ui-lego-knob-full-s (nth entry 1) (nth k 0) (nth k 1) 4.9 (vh-accent) (nth k 2)))))))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch :debug-name "vh-surface"
    (v-stack :width (vh-left-w) :gap (vh-gap)
      (vh-slots-block)
      (vh-morph-block))
    (vh-map)
    (v-stack :width (vh-right-w) :gap (vh-gap)
      (vh-family-block)
      (vh-pages-block))))
