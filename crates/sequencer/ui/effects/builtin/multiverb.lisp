;; Multiverb built-in FX panel.
;;
;; Four algorithms share one control set. The selected algorithm colors the
;; controls that define its character; universal utility controls stay neutral.
;; Factory buttons deliberately use the normal effect-param authoring path, so
;; they work for track effects, bus effects, and selected-step p-locks.

(module eseq.effects.builtin.multiverb)

(import eseq.effects.builtin.filter-core :refer (builtin-fx-param))
(import eseq.effects.param-grid :refer (fx-param-grid))
(import eseq.effects.param-controls :refer
  (fx-param-numeric-value
   fx-param-value-for
   fx-set-effect-value
   instrument-mod-target-source-slot
   instrument-param-mod-targets
   param-base-max-prop
   param-base-min-prop
   param-base-value-prop
   param-control-key-mode
   param-control-max
   param-control-min
   param-knob-mod-depth-prop
   param-knob-mod-slot-prop
   param-mod-wrapper
   param-plock-active?
   param-plock-color-b
   param-plock-color-g
   param-plock-color-r
   param-plock-default
   param-plock-text-color
   param-selected-mod-slot-prop
   param-set-control-value
   param-set-option))

;; Migration alias (module spec §10). `builtin-fx-multiverb-apply-preset` is
;; eval'd by flat name from a Rust test
;; (metal_seq_fx_multiverb_layout_contains_modes_presets_and_live_knobs in
;; src/ui/state_values/tests.rs). Identity alias; delete when that test
;; qualifies.
(module-compat-alias builtin-fx-multiverb-apply-preset builtin-fx-multiverb-apply-preset)

(def %gold () (rgba 0.94 0.68 0.24 1.0))
(def %blue () (rgba 0.32 0.68 0.96 1.0))
(def %violet () (rgba 0.72 0.48 0.96 1.0))
(def %green () (rgba 0.36 0.82 0.58 1.0))

(def %accent (mode)
  (if (= mode 0) (%gold)
    (if (= mode 1) (%blue)
      (if (= mode 2) (%violet)
        (%green)))))

(def %characteristic? (mode name)
  (or (= name "decay")
      (= name "size")
      (and (= mode 0) (or (= name "damp") (= name "diffusion") (= name "era")))
      (and (= mode 1) (or (= name "bass") (= name "diffusion")
                          (= name "mod rate") (= name "mod depth") (= name "mod shape")))
      (and (= mode 2) (or (= name "damp") (= name "bass")
                          (= name "diffusion") (= name "era")))
      (and (= mode 3) (or (= name "damp") (= name "diffusion")
                          (= name "mod rate") (= name "mod depth") (= name "mod shape")))))

(def %label-color (mode-p p)
  (let ((mode (round (fx-param-numeric-value mode-p))))
    (if (%characteristic? mode (get p :name))
      (%accent mode)
      :dim)))

(def %knob (fx mode-p label-text p decimals)
  (param-mod-wrapper fx p (str "multiverb-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "multiverb-param-" (get p :idx) (param-control-key-mode fx p))
      (knob-number :label label-text
        :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p) :decimals decimals
        :base-value (param-base-value-prop fx p)
        :base-min (param-base-min-prop fx p) :base-max (param-base-max-prop fx p)
        :mod-range-0-slot (param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.0
        :text-color (param-plock-text-color fx p)
        :label-color (%label-color mode-p p)
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.25 :height 2.48 :knob-size 1.82
        :on-change (lambda (v) (param-set-control-value fx p v))))))

(def %percent-knob (fx mode-p label-text p)
  (param-mod-wrapper fx p (str "multiverb-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "multiverb-param-" (get p :idx) (param-control-key-mode fx p))
      (knob-number :label label-text
        :value (fx-param-value-for fx p)
        :min (param-control-min fx p) :max (param-control-max fx p)
        :value-scale 100 :decimals 0
        :base-value (param-base-value-prop fx p)
        :base-min (param-base-min-prop fx p) :base-max (param-base-max-prop fx p)
        :mod-range-0-slot (param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.0
        :text-color (param-plock-text-color fx p)
        :label-color (%label-color mode-p p)
        :plock-active (if (param-plock-active? fx p) 1 0)
        :plock-default (param-plock-default fx p)
        :plock-color-r (param-plock-color-r)
        :plock-color-g (param-plock-color-g)
        :plock-color-b (param-plock-color-b)
        :width 4.25 :height 2.48 :knob-size 1.82
        :on-change (lambda (v) (param-set-control-value fx p v))))))

;; ── Modes and factory settings ──

(def %mode-button (fx p index label-text)
  (let ((selected (= (round (fx-param-numeric-value p)) index)))
    (button label-text
      :debug-name (str "multiverb-mode-" label-text)
      :width 2.72 :height 1.20 :padding 0 :font-size 8.5
      :background-color (if selected (%accent index) :mixer-control-bg)
      :color (if selected :black :dim)
      :plock-active (if (param-plock-active? fx p) 1 0)
      :plock-color-r (param-plock-color-r)
      :plock-color-g (param-plock-color-g)
      :plock-color-b (param-plock-color-b)
      :on-click |x y r| (fx-set-effect-value fx p index))))

(def %clear-mod-depths (fx target-params)
  (each target-params |p|
    (each (instrument-param-mod-targets p) |target|
      (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") 0))))

(def %source-section (fx source-slot)
  (nth
    (filter |section| (= (get section :slot) source-slot) (get fx :sources))
    0))

(def %set-mod-source (fx source-slot source-name)
  (let ((section (%source-section fx source-slot)))
    (if section
      (let ((source-p (get section :source-param)))
        (if source-p (param-set-option fx source-p source-name))))))

(def %clear-mod-sources (fx)
  (each (get fx :sources) |section|
    (let ((source-p (get section :source-param)))
      (if source-p (param-set-option fx source-p "off")))))

(def %set-mod-depth (fx p source-slot depth)
  (let ((target
          (nth
            (filter |candidate|
              (= (instrument-mod-target-source-slot candidate) source-slot)
              (instrument-param-mod-targets p))
            0)))
    (if target
      (fx-set-effect-value fx (dict :idx (get target :depth-idx) :control "param") depth))))

(def %preset-values (name)
  (if (= name "Xtal Wash")
    (list (list "mode" 2) (list "decay" 0.84) (list "size" 0.52)
          (list "predelay" 18) (list "damp" 0.58) (list "bass" 0.65)
          (list "diffusion" 0.78) (list "mod rate" 0.20) (list "mod depth" 0.02)
          (list "mod shape" 0.70) (list "era" 0.70) (list "width" 1.0)
          (list "mix" 0.45))
    (if (= name "224 Bloom")
      (list (list "mode" 1) (list "decay" 0.86) (list "size" 0.62)
            (list "predelay" 28) (list "damp" 0.45) (list "bass" 0.70)
            (list "diffusion" 0.80) (list "mod rate" 0.32) (list "mod depth" 0.10)
            (list "mod shape" 0.85) (list "era" 0.55) (list "width" 1.0)
            (list "mix" 0.45))
      (if (= name "Gold Plate")
        (list (list "mode" 0) (list "decay" 0.65) (list "size" 0.50)
              (list "predelay" 12) (list "damp" 0.25) (list "bass" 0.55)
              (list "diffusion" 0.72) (list "mod rate" 0.70) (list "mod depth" 0.12)
              (list "mod shape" 0.15) (list "era" 0.28) (list "width" 0.90)
              (list "mix" 0.40))
        (list (list "mode" 3) (list "decay" 0.78) (list "size" 0.60)
              (list "predelay" 5) (list "damp" 0.30) (list "bass" 0.55)
              (list "diffusion" 0.70) (list "mod rate" 0.55) (list "mod depth" 1.0)
              (list "mod shape" 0.05) (list "era" 0.10) (list "width" 1.0)
              (list "mix" 0.48))))))

(def builtin-fx-multiverb-apply-preset (fx name)
  (let ((params (get fx :params)))
    (let ((decay-p (builtin-fx-param params "decay"))
          (size-p (builtin-fx-param params "size"))
          (depth-p (builtin-fx-param params "mod depth"))
          (mix-p (builtin-fx-param params "mix")))
      (do
        (%clear-mod-sources fx)
        (%clear-mod-depths fx (list decay-p size-p depth-p mix-p))
        (each (%preset-values name) |setting|
          (let ((p (builtin-fx-param params (nth setting 0))))
            (if p (fx-set-effect-value fx p (nth setting 1)))))
        (if (= name "Xtal Wash")
          (do
            (%set-mod-depth fx decay-p 1 0.08)
            (%set-mod-source fx 1 "drift")))))))

(def %preset-button (fx label-text)
  (button label-text
    :debug-name (str "multiverb-preset-" label-text)
    :width 5.60 :height 1.00 :padding 0 :font-size 8.0
    :background-color :mixer-control-bg :color :dim
    :on-click |x y r| (builtin-fx-multiverb-apply-preset fx label-text)))

(def %mode-box (fx mode-p)
  (box :debug-name "multiverb-mode-section"
    :width 13.2 :height 9.65 :padding 0.34
    :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.28 :align :center
      (label "MULTIVERB" :font-size 9.0 :width 11.8
        :color (%accent (round (fx-param-numeric-value mode-p))) :bg :transparent)
      (h-stack :gap 0.14
        (%mode-button fx mode-p 0 "Plate")
        (%mode-button fx mode-p 1 "Hall")
        (%mode-button fx mode-p 2 "Quad")
        (%mode-button fx mode-p 3 "Mod"))
      (label (get mode-p :text-value) :font-size 8.0 :width 11.8 :color :dim :bg :transparent)
      (box :height 0.25)
      (label "FACTORY" :font-size 8.0 :width 11.8 :color :dim :bg :transparent)
      (h-stack :gap 0.14
        (%preset-button fx "Gold Plate")
        (%preset-button fx "224 Bloom"))
      (h-stack :gap 0.14
        (%preset-button fx "Xtal Wash")
        (%preset-button fx "Seasick")))))

;; ── Control groups ──

(def %space-box (fx mode-p decay-p size-p predelay-p mix-p)
  (box :debug-name "multiverb-space-section"
       :width 9.45 :height 9.65 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.20 :align :center
      (label "SPACE" :font-size 8.0 :width 8.6 :color :dim :bg :transparent)
      (h-stack :gap 0.18
        (%percent-knob fx mode-p "decay" decay-p)
        (%percent-knob fx mode-p "size" size-p))
      (h-stack :gap 0.18
        (%knob fx mode-p "pre ms" predelay-p 0)
        (%percent-knob fx mode-p "mix" mix-p)))))

(def %tone-box (fx mode-p damp-p bass-p diffusion-p era-p)
  (box :debug-name "multiverb-tone-section"
       :width 9.45 :height 9.65 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.20 :align :center
      (label "TONE / GRAIN" :font-size 8.0 :width 8.6 :color :dim :bg :transparent)
      (h-stack :gap 0.18
        (%percent-knob fx mode-p "damp" damp-p)
        (%percent-knob fx mode-p "bass" bass-p))
      (h-stack :gap 0.18
        (%percent-knob fx mode-p "diffuse" diffusion-p)
        (%percent-knob fx mode-p "era" era-p)))))

(def %motion-box (fx mode-p rate-p depth-p shape-p width-p)
  (box :debug-name "multiverb-motion-section"
       :width 9.45 :height 9.65 :padding 0.30
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.20 :align :center
      (label "MOTION / STEREO" :font-size 8.0 :width 8.6 :color :dim :bg :transparent)
      (h-stack :gap 0.18
        (%knob fx mode-p "rate hz" rate-p 2)
        (%percent-knob fx mode-p "depth" depth-p))
      (h-stack :gap 0.18
        (%percent-knob fx mode-p "random" shape-p)
        (%percent-knob fx mode-p "width" width-p)))))

(def builtin-fx-multiverb-ui (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (builtin-fx-param params "mode"))
          (decay-p (builtin-fx-param params "decay"))
          (size-p (builtin-fx-param params "size"))
          (predelay-p (builtin-fx-param params "predelay"))
          (damp-p (builtin-fx-param params "damp"))
          (bass-p (builtin-fx-param params "bass"))
          (diffusion-p (builtin-fx-param params "diffusion"))
          (rate-p (builtin-fx-param params "mod rate"))
          (depth-p (builtin-fx-param params "mod depth"))
          (shape-p (builtin-fx-param params "mod shape"))
          (era-p (builtin-fx-param params "era"))
          (width-p (builtin-fx-param params "width"))
          (mix-p (builtin-fx-param params "mix")))
      (if (and mode-p decay-p size-p predelay-p damp-p bass-p diffusion-p
               rate-p depth-p shape-p era-p width-p mix-p)
        (h-stack :gap 0.35 :align :start
          (%mode-box fx mode-p)
          (%space-box fx mode-p decay-p size-p predelay-p mix-p)
          (%tone-box fx mode-p damp-p bass-p diffusion-p era-p)
          (%motion-box fx mode-p rate-p depth-p shape-p width-p))
        (fx-param-grid params fx)))))
