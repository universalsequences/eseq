;; Filter Table built-in FX panel.
(module eseq.effects.builtin.filter-table)

(import eseq.effects.builtin.filter-core :refer (eseq.effects.builtin.filter-core/builtin-fx-param))
(import eseq.effects.param-controls :as pc)

;; The generic dynamics knobs only edit base values. Filter Table parameters
;; need the complete modulation contract: in the mods tab the same knob edits
;; the selected source's depth, draws all assigned modulation ranges, and is
;; wrapped by the blue modulation target affordance.
(def %knob (fx label-text p decimals value-scale)
  (pc/param-mod-wrapper fx p (str "filter-table-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "filter-table-param-" (get p :idx) (pc/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (pc/fx-param-value-for fx p)
        :min (pc/param-control-min fx p) :max (pc/param-control-max fx p)
        :value-scale value-scale :decimals decimals
        :base-value (pc/param-base-value-prop fx p)
        :base-min (pc/param-base-min-prop fx p) :base-max (pc/param-base-max-prop fx p)
        :mod-range-0-slot (pc/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (pc/param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (pc/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (pc/param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (pc/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (pc/param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (pc/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (pc/param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (pc/param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.5
        :text-color (pc/param-plock-text-color fx p) :label-color :dim
        :plock-active (if (pc/param-plock-active? fx p) 1 0)
        :plock-default (pc/param-plock-default fx p)
        :plock-color-r (pc/param-plock-color-r)
        :plock-color-g (pc/param-plock-color-g)
        :plock-color-b (pc/param-plock-color-b)
        :width 6.8 :height 2.2 :knob-size 1.25
        :track-color '(rgba 0.4, 0.4, 0.4, 1)
        :on-change (lambda (v) (pc/param-set-control-value fx p v))))))

(def %percent-knob (fx label-text p)
  (%knob fx label-text p 0 100))

(def %number-knob (fx label-text p decimals)
  (%knob fx label-text p decimals 1))

(def %spectrum-source (fx)
  (if (get fx :rack-fx)
    (dict :kind :rack-effect :index (get fx :track-idx)
          :rack-slot (get fx :rack-slot) :slot (get fx :slot-idx))
    (if (get fx :bus-fx)
      (dict :kind :bus-effect :index (get fx :bus-idx) :slot (get fx :slot-idx))
      (dict :kind :track-effect :index (get fx :track-idx) :slot (get fx :slot-idx)))))

(def %drop-table (event)
  (let ((payload (get event :payload))
        (target (get event :target)))
    (let ((path (get payload :path)))
      (if path
        (host-command "set-filter-table-source"
          (dict :track (get target :track)
                :slot (get target :slot)
                :bus (get target :bus)
                :path path))
        (status "Drop an audio sample, not a folder")))))

(def filter-table-ui (fx)
  (let ((params (get fx :params)))
    (let ((frame-p (eseq.effects.builtin.filter-core/builtin-fx-param params "frame"))
          (cutoff-p (eseq.effects.builtin.filter-core/builtin-fx-param params "cutoff"))
          (res-p (eseq.effects.builtin.filter-core/builtin-fx-param params "resonance"))
          (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
          (output-p (eseq.effects.builtin.filter-core/builtin-fx-param params "output"))
          (table-name (get fx :table-name))
          (table-key (get fx :table-data-key)))
      (v-stack :gap 0.25
        (box :width 36.4 :height 4.15 :padding 0.25
          :background-color :instrument-control-bg :corner-radius 8
          :drop-types (list "sample")
          :drop-meta (dict :kind "filter-table-source"
                           :track SEQ.current-track
                           :bus (if (get fx :bus-fx) (get fx :bus-idx) -1)
                           :slot (get fx :slot-idx))
          :drop-hover-border-color :blue
          :on-drop (lambda (event) (%drop-table event))
          (v-stack :width :fill :height :fill :gap 0.08 :align :stretch
            (h-stack :width :fill :height 0.85 :gap 0.35 :align :center
              (label "MAGNITUDE TABLE" :font-size 7.5 :color :dim :bg :transparent)
              (label (if table-name table-name "Drop an audio sample")
                :font-size 9.0 :color :fg :bg :transparent))
            (if table-key
              (wavetable-viewer
                :data-key table-key :domain :magnitude
                :waves-per-set 64 :set 0
                :wave (pc/instrument-param-base-value frame-p) :wave-normalized true
                :wave-color (rgba 0.35 0.68 1.0 1.0)
                :inactive-color (rgba 0.20 0.43 0.72 0.34)
                :background-color (rgba 0.035 0.045 0.060 1.0)
                :width 35.9 :height 2.75)
              (box :width :fill :height 2.75))))
        (if table-key
          (eq8-editor
            :width 36.4 :height 2.55
            :bands (list) :selected-band -1
            :source (%spectrum-source fx) :tap-point :pre-fx
            :mode :eq :fft-size 8192 :time-slices 128
            :min-db -96 :max-db 0 :smoothing 0.65
            :freq-min 20 :freq-max 20000
            :response-min-db -48 :response-max-db 8
            :response-data-key table-key
            :response-frame (pc/instrument-param-base-value frame-p)
            :response-cutoff (pc/instrument-param-base-value cutoff-p)
            :response-resonance (pc/instrument-param-base-value res-p)
            :background-color (rgba 0.045 0.055 0.070 1.0)
            :curve-color (rgba 0.78 0.84 0.92 0.96)
            :spectrum-color (rgba 0.18 0.38 0.64 0.30)
            :spectrum-peak-color (rgba 0.36 0.62 0.92 0.58))
          (box :width :fill :height 0))
        (h-stack :gap 0.6 :align :center
          (if frame-p (%percent-knob fx "frame" frame-p) (box :width 0 :height 0))
          (if cutoff-p (%number-knob fx "cutoff" cutoff-p 0) (box :width 0 :height 0))
          (if res-p (%percent-knob fx "resonance" res-p) (box :width 0 :height 0))
          (if mix-p (%percent-knob fx "mix" mix-p) (box :width 0 :height 0))
          (if output-p (%number-knob fx "output" output-p 2) (box :width 0 :height 0)))))))
