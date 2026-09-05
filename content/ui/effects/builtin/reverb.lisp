;; Reverb built-in FX panel (galaxy / plate / hall).
;;
;; Laid out like a classic studio reverb front panel: an input filter curve,
;; the tank (mode + its character knobs), the diffusion-network damping
;; curve, motion (chorus / tank modulation / stereo) and dry/wet. The two
;; curve editors write straight to the effect params through the batch
;; authoring path, so they p-lock like knobs. No presets live here — the
;; normal effect preset system covers that.

(module eseq.effects.builtin.reverb)

(import eseq.effects.builtin.filter-core :refer
  (builtin-fx-param
   builtin-fx-param-subtree-key
   builtin-fx-set-effect-option))
(import eseq.effects.param-grid :refer (fx-param-grid))
(import eseq.effects.panel-frame :refer (fx-clear-selected-effect))
(import eseq.effects.param-controls :refer
  (fx-param-numeric-value
   fx-param-value-for
   fx-set-effect-value
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
   param-set-control-value))

(export builtin-fx-reverb-ui
        reverb-input-curve-action
        reverb-network-curve-action)

(def accent () :reverb-curve)
(def curve-bg () :reverb-curve-bg)
(def curve-grid () :reverb-curve-grid)
(def curve-point () :reverb-curve-point)

(def section-h () 6.3)
(def curve-h () 3.5)

;; The damping shelves only cut (param range -18..0 dB), but the curve editor
;; centres 0 dB on the band's own gain range, so the band is declared
;; symmetric and writes clamp to 0.
(def shelf-gain-min () -18)
(def shelf-gain-max () 18)
(def clamp-shelf-gain (gain)
  (if (> gain 0) 0 (if (< gain (shelf-gain-min)) (shelf-gain-min) gain)))

;; Bands share the editor's full 20 Hz - 20 kHz axis (a band's own range is
;; what the widget maps its handle against), so a drag can leave the param's
;; range and is clamped back here.
(def clamp-param (p v)
  (if (< v (get p :min)) (get p :min) (if (> v (get p :max)) (get p :max) v)))

;; ── Knobs ──

(def parameter-knob (fx label-text p decimals)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "reverb-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "reverb-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (eseq.effects.param-controls/fx-param-value-for fx p)
        :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :decimals decimals
        :base-value (eseq.effects.param-controls/param-base-value-prop fx p)
        :mod-offset (eseq.effects.param-controls/param-mod-offset p)
        :mod-scale (eseq.effects.param-controls/param-mod-scale p)
        :unit (eseq.effects.param-controls/param-control-unit fx p)
        :base-min (eseq.effects.param-controls/param-base-min-prop fx p) :base-max (eseq.effects.param-controls/param-base-max-prop fx p)
        :mod-range-0-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (eseq.effects.param-controls/param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.0
        :track-color :mixer-strip-selected-bg
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p)
        :label-color :dim
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.25 :height 2.48 :knob-size 1.82
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

(def percent-knob (fx label-text p)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "reverb-param-" (get p :idx) "-mod-wrapper")
    (subtree :key (str "reverb-param-" (get p :idx) (eseq.effects.param-controls/param-control-key-mode fx p))
      (knob-number :label label-text
        :value (eseq.effects.param-controls/fx-param-value-for fx p)
        :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p)
        :value-scale 100 :decimals 0
        :base-value (eseq.effects.param-controls/param-base-value-prop fx p)
        :mod-offset (eseq.effects.param-controls/param-mod-offset p)
        :mod-scale (eseq.effects.param-controls/param-mod-scale p)
        :unit (eseq.effects.param-controls/param-control-unit fx p)
        :base-min (eseq.effects.param-controls/param-base-min-prop fx p) :base-max (eseq.effects.param-controls/param-base-max-prop fx p)
        :mod-range-0-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 0) :mod-range-0-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 0)
        :mod-range-1-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 1) :mod-range-1-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 1)
        :mod-range-2-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 2) :mod-range-2-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 2)
        :mod-range-3-slot (eseq.effects.param-controls/param-knob-mod-slot-prop fx p 3) :mod-range-3-depth (eseq.effects.param-controls/param-knob-mod-depth-prop fx p 3)
        :selected-mod-slot (eseq.effects.param-controls/param-selected-mod-slot-prop fx p)
        :font-size 9.5 :label-font-size 9.0
        :text-color (eseq.effects.param-controls/param-plock-text-color fx p)
        :label-color :dim
        :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
        :plock-default (eseq.effects.param-controls/param-plock-default fx p)
        :track-color :mixer-strip-selected-bg
        :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
        :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
        :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
        :width 4.25 :height 2.48 :knob-size 1.82
        :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v))))))

;; Placeholder the size of a knob, so mode-conditional rows keep their shape.
(def knob-gap ()
  (box :width 4.25 :height 2.48))

;; Value field in the digidrift lego style: dim title over a left-aligned
;; number picker (same metrics as `ui-lego-num`, bound to a builtin fx param).
(def num-field (fx title p decimals unit width)
  (eseq.effects.param-controls/param-mod-wrapper fx p (str "reverb-num-" (get p :idx) "-mod-wrapper")
    (subtree :key (builtin-fx-param-subtree-key fx p "num")
      (v-stack :width width :height 1.12 :gap 0.08 :align :start
        (label title :font-size 8.2 :width width :color :dim :bg :transparent :v-align :center)
        (number-picker :value (eseq.effects.param-controls/fx-param-value-for fx p)
          :min (eseq.effects.param-controls/param-control-min fx p) :max (eseq.effects.param-controls/param-control-max fx p) :decimals decimals
          :unit unit
          :noui true :font-size 10.2
          :text-color :accent
          :edit-color :yellow
          :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
          :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
          :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
          :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
          :text-align :left
          :width width :height 0.68
          :on-change (lambda (v) (eseq.effects.param-controls/param-set-control-value fx p v)))))))

;; ── Curve authoring ──

;; Write several params at once. Rack/bus/midi chains go through the
;; per-param path; track chains use one batch host command so a drag lands
;; as a single undo step (`:commit` on release). `pairs` is a list of
;; (param value) lists.
(def update-dict (pair)
  (dict :param-idx (get (nth pair 0) :idx) :value (nth pair 1)))

(def write-params (fx pairs commit?)
  (do
    (fx-clear-selected-effect)
    (if (or (get fx :rack-fx) (get fx :bus-fx) (get fx :midi-fx))
      (each pairs |pair|
        (eseq.effects.param-controls/fx-set-effect-value fx (nth pair 0) (nth pair 1)))
      (host-command
        (if (seq-has-selection?) "set-effect-plock-batch" "set-effect-param-batch")
        (dict :slot-idx (get fx :slot-idx)
              :target-node-id (get fx :target-node-id)
              :updates (if (nth pairs 1)
                         (list (update-dict (nth pairs 0)) (update-dict (nth pairs 1)))
                         (list (update-dict (nth pairs 0))))
              :commit commit?)))))

(def curve-drag? (event)
  (or (= (get event :type) :change-band) (= (get event :type) :commit-band)))

;; Input filter: band 0 = lo cut (highpass), band 1 = hi cut (lowpass).
(def reverb-input-curve-action (fx locut-p hicut-p event)
  (if (curve-drag? event)
    (write-params fx
      (let ((p (if (= (get event :id) 1) hicut-p locut-p)))
        (list (list p (clamp-param p (get event :freq)))))
      (= (get event :type) :commit-band))
    nil))

;; Diffusion network: band 0 = lo shelf, band 1 = hi shelf (freq + gain).
(def reverb-network-curve-action (fx lo-freq-p lo-gain-p hi-freq-p hi-gain-p event)
  (if (curve-drag? event)
    (if (= (get event :id) 1)
      (write-params fx
        (list (list hi-freq-p (clamp-param hi-freq-p (get event :freq)))
              (list hi-gain-p (clamp-shelf-gain (get event :gain))))
        (= (get event :type) :commit-band))
      (write-params fx
        (list (list lo-freq-p (clamp-param lo-freq-p (get event :freq)))
              (list lo-gain-p (clamp-shelf-gain (get event :gain))))
        (= (get event :type) :commit-band)))
    nil))

(def cut-band (fx id type p)
  (dict :id id :type type
        :freq (eseq.effects.param-controls/fx-param-value-for fx p)
        :freq-min 20 :freq-max 20000
        :gain 0 :q 0.71 :q-min 0.71 :q-max 0.71
        :lock-y true :enabled true :selected false))

(def shelf-band (fx id type freq-p gain-p)
  (dict :id id :type type
        :freq (eseq.effects.param-controls/fx-param-value-for fx freq-p)
        :freq-min 20 :freq-max 20000
        :gain (eseq.effects.param-controls/fx-param-value-for fx gain-p)
        :gain-min (shelf-gain-min) :gain-max (shelf-gain-max)
        :q 0.71 :q-min 0.71 :q-max 0.71
        :enabled true :selected false))

;; ── Sections ──

(def input-box (fx locut-p hicut-p)
  (box :debug-name "reverb-input-section"
    :width 23.0 :height (section-h) :padding 0.30
    :background-color :fx-inner-panel-bg :corner-radius 12
    (v-stack :gap 0.22 :align :start
      (h-stack :gap 0 (box :width 0.5)
        (label "Input Filter" :font-size 10.0 :width 22.2 :color :dim :bg :transparent :v-align :center)
        )
        (box :width 22.2 :height (curve-h)
          (subtree :key (builtin-fx-param-subtree-key fx locut-p "curve")
            (response-curve-editor
              :mode :filter
              :bands (list (cut-band fx 0 "highpass" locut-p)
                (cut-band fx 1 "lowpass" hicut-p))
              :freq-min 20 :freq-max 20000
              :gain-min -12 :gain-max 12
              :q-min 0.71 :q-max 0.71
              :background-color (curve-bg) :corner-radius 6
              :grid-color (curve-grid) :stroke-color (accent) :stroke-width 4.5 :point-color (curve-point)
              :on-action |event| (reverb-input-curve-action fx locut-p hicut-p event))))
        (h-stack :gap 0.6 :align :start
          (num-field fx "lo cut" locut-p 0 "Hz" 7.0)
          (num-field fx "hi cut" hicut-p 0 "Hz" 7.0)))))

(def mode-dropdown (fx p)
  (subtree :key (builtin-fx-param-subtree-key fx p "mode")
    (dropdown :value (get p :text-value)
      :options (get p :options)
      :debug-name "reverb-mode-dropdown"
      :on-change (lambda (v) (eseq.effects.builtin.filter-core/builtin-fx-set-effect-option fx p v))
      :plock-active (if (eseq.effects.param-controls/param-plock-active? fx p) 1 0)
      :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
      :chevron-color :accent
      :badge-color :none
      :border-color :mixer-strip-bg
      :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
      :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
      :width 6.4 :height 1.05 :font-size 9.5)))

;; Bottom row: mode dropdown plus every knob, Ableton style. Galaxy exposes its
;; own feedback/tone knobs; plate and hall share the Dattorro/224 decay +
;; diffusion pair and their tank modulation depth.
(def mode-row (fx mode-p galaxy? bright-p replace-p decay-p diffusion-p size-p predelay-p depth-p)
  (box :debug-name "reverb-mode-section"
    :width 54.3 :height 3.05 :padding 0.28
    :background-color :mixer-control-bg :corner-radius 12
    (h-stack :gap 0.3 :align :center
      (box :width 1)
      (v-stack :gap 0.15 :align :center
        (label "MODE" :font-size 8.0 :width 6.4 :color :dim :bg :transparent :v-align :center)
        (mode-dropdown fx mode-p))
      (box :width 0.4)
      (if galaxy?
        (percent-knob fx "bright" bright-p)
        (percent-knob fx "decay" decay-p))
      (if galaxy?
        (percent-knob fx "replace" replace-p)
        (percent-knob fx "diffuse" diffusion-p))
      (percent-knob fx "size" size-p)
      (parameter-knob fx "pre ms" predelay-p 1)
      (if galaxy? (knob-gap) (percent-knob fx "mod depth" depth-p)))))

;; Right-hand column, spanning both rows: the knobs you reach for most.
(def output-box (fx gain-p width-p mix-p)
  (box :debug-name "reverb-output-section"
       :width 5.4 :height (+ (section-h) 0.15 3.05) :padding 0.50
       :background-color :fx-inner-panel-bg :corner-radius 12
    (v-stack :gap 0.3 :align :center
      (label "Output" :font-size 10.0 :width 4.4 :color :dim :bg :transparent :v-align :center)
      (parameter-knob fx "wet db" gain-p 1)
      (percent-knob fx "stereo" width-p)
      (percent-knob fx "dry/wet" mix-p))))

(def network-box (fx lo-freq-p lo-gain-p hi-freq-p hi-gain-p)
  (box :debug-name "reverb-network-section"
    :width 23.0 :height (section-h) :padding 0.30
    :background-color :fx-inner-panel-bg :corner-radius 12
    (v-stack :gap 0.22 :align :start
      (h-stack :gap 0 (box :width 0.5)
        (label "Diffusion Network" :font-size 10.0 :width 22.2 :color :dim :bg :transparent :v-align :center)
        )
        (box :width 22.2 :height (curve-h)
          (subtree :key (builtin-fx-param-subtree-key fx lo-freq-p "curve")
            (response-curve-editor
              :mode :eq :combine true
              :bands (list (shelf-band fx 0 "lowshelf" lo-freq-p lo-gain-p)
                (shelf-band fx 1 "highshelf" hi-freq-p hi-gain-p))
              :freq-min 20 :freq-max 20000
              :gain-min (shelf-gain-min) :gain-max (shelf-gain-max)
              :q-min 0.71 :q-max 0.71
              :background-color (curve-bg) :corner-radius 6
              :grid-color (curve-grid) :stroke-color (accent) :stroke-width 4.5 :point-color (curve-point)
              :on-action |event| (reverb-network-curve-action fx lo-freq-p lo-gain-p hi-freq-p hi-gain-p event))))
        (h-stack :gap 0.4 :align :start
          (num-field fx "low" lo-freq-p 0 "Hz" 5.2)
          (num-field fx "low gain" lo-gain-p 1 "dB" 5.2)
          (num-field fx "high" hi-freq-p 0 "Hz" 5.2)
          (num-field fx "high gain" hi-gain-p 1 "dB" 5.2)))))

(def chorus-box (fx chorus-p rate-p)
  (box :debug-name "reverb-chorus-section"
       :width 7.6 :height (section-h) :padding 0.50
       :background-color :fx-inner-panel-bg :corner-radius 7
    (v-stack :gap 0.4 :align :start
      (label "Chorus" :font-size 10.0 :width 6.8 :color :dim :bg :transparent :v-align :center)
      (box :height 0.3)
      (num-field fx "amount" chorus-p 2 "" 6.6)
      (num-field fx "rate" rate-p 2 "Hz" 6.6))))

(def builtin-fx-reverb-ui (fx)
  (let ((params (get fx :params)))
    (let ((mode-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mode"))
          (mix-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mix"))
          (size-p (eseq.effects.builtin.filter-core/builtin-fx-param params "size"))
          (bright-p (eseq.effects.builtin.filter-core/builtin-fx-param params "brightness"))
          (replace-p (eseq.effects.builtin.filter-core/builtin-fx-param params "replace"))
          (predelay-p (eseq.effects.builtin.filter-core/builtin-fx-param params "predelay"))
          (decay-p (eseq.effects.builtin.filter-core/builtin-fx-param params "decay"))
          (diffusion-p (eseq.effects.builtin.filter-core/builtin-fx-param params "diffusion"))
          (hi-freq-p (eseq.effects.builtin.filter-core/builtin-fx-param params "hi shelf freq"))
          (hi-gain-p (eseq.effects.builtin.filter-core/builtin-fx-param params "hi shelf gain"))
          (lo-freq-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lo shelf freq"))
          (lo-gain-p (eseq.effects.builtin.filter-core/builtin-fx-param params "lo shelf gain"))
          (locut-p (eseq.effects.builtin.filter-core/builtin-fx-param params "in lo cut"))
          (hicut-p (eseq.effects.builtin.filter-core/builtin-fx-param params "in hi cut"))
          (width-p (eseq.effects.builtin.filter-core/builtin-fx-param params "stereo"))
          (chorus-p (eseq.effects.builtin.filter-core/builtin-fx-param params "chorus amount"))
          (rate-p (eseq.effects.builtin.filter-core/builtin-fx-param params "chorus rate"))
          (depth-p (eseq.effects.builtin.filter-core/builtin-fx-param params "mod depth"))
          (gain-p (eseq.effects.builtin.filter-core/builtin-fx-param params "wet gain")))
      (if (and mode-p mix-p size-p bright-p replace-p predelay-p decay-p diffusion-p
               hi-freq-p hi-gain-p lo-freq-p lo-gain-p locut-p hicut-p
               width-p chorus-p rate-p depth-p gain-p)
        (let ((galaxy? (= (round (eseq.effects.param-controls/fx-param-numeric-value mode-p)) 0)))
          (h-stack :gap 0.35 :align :start
            (v-stack :gap 0.15
              (h-stack :gap 0.35 :align :start
                (input-box fx locut-p hicut-p)
                (network-box fx lo-freq-p lo-gain-p hi-freq-p hi-gain-p)
                (chorus-box fx chorus-p rate-p))
              (mode-row fx mode-p galaxy? bright-p replace-p decay-p diffusion-p
                        size-p predelay-p depth-p))
            (output-box fx gain-p width-p mix-p)))
        (eseq.effects.param-grid/fx-param-grid params fx)))))
