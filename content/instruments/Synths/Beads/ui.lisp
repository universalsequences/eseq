;; Beads — the module's wavetable synth mode laid out as three surfaces: the
;; wavetable (bank, TIME and its scatter, with a live bank viewer), the grain
;; (SIZE / SHAPE / PITCH and their per-grain randomization), and the cloud
;; (DENSITY, DRY/WET, REVERB, FREEZE). Every control appears exactly once.

(def bd-knob () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def bd-head () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def bd-text () :fg)

(def bd-surf () :instrument-group-bg)
(def bd-bord () :border-inactive)

(def bd-panel-dense (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s section (eseq.effects.custom-ui-lego/ui-lego-col-w) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (bd-surf) (bd-bord) false body))
(def bd-small-row (body)
  (box :width :fill :height :fill :v-align :center body))

;; Differentiated copy of the DSP's integrated tables, for display only.
(def bd-view-file () "instruments/Synths/Beads/waves/view.json")

(def bd-bank-options ()
  '("Harmonics" "Tri Stack" "Drawbars" "Tri/Saw" "Burst/Formant" "Formants"
    "Digi/Metal" "Braids" "FM 2:1" "FM 1:0.5" "FM 1:3" "FM 1:8"))

;; Beads' four media settings, as they set the wavetable bit depth.
(def bd-quality-options ()
  '("dirty 4b" "clean 6b" "vintage 7b" "digital"))

(def bd-wave-h () (+ (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-dense-h) (eseq.effects.custom-ui-lego/ui-lego-gap)))
(def bd-wave-w () (eseq.effects.custom-ui-lego/ui-lego-wide-col-w))

;; The selected bank's 16 waves with the TIME position highlighted; :wave
;; follows the param's effective (modulated) value.
(def bd-viewer ()
  (let ((pbank (eseq.effects.custom-ui-runtime/custom-ui-current-param "bank"))
      (ptime (eseq.effects.custom-ui-runtime/custom-ui-current-param "time")))
    (if (and pbank ptime)
      (wavetable-viewer
        :file (bd-view-file)
        :waves-per-set 16
        :set (eseq.effects.custom-ui-runtime/custom-ui-param-value pbank)
        :wave (eseq.effects.param-controls/param-effective-value ptime)
        :wave-normalized true
        :wave-color (bd-knob)
        :inactive-color :dim
        :background-color :instrument-group-bg
        :width 16.6
        :height 5.0)
      (label "missing wavetable params" :font-size 8 :color :red :bg :transparent))))

(def bd-wave-block ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 0 (bd-wave-w) (bd-wave-h) (bd-surf) (bd-bord) false
    (v-stack :width :fill :height :fill :gap 0.30 :align :start :padding 0.12
      (h-stack :gap 0.22 :align :end
        (eseq.effects.custom-ui-lego/ui-lego-header-s 0 "BEADS" 4.0 (bd-head))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "bank" "bank" 9.0 (bd-bank-options) (bd-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "quality" "quality" 8.0 (bd-quality-options) (bd-text)))
      (h-stack :width :fill :gap 0.40 :align :center
        (bd-viewer)
        (h-stack :gap 0.30 :align :center
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "time" "time" 4.4 4.2 3.8 (bd-knob) 2)
          (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 "time_amt" "scatter" 4.4 4.2 3.8 (bd-knob) 2))))))

(def bd-voice-block ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-x-s 3 (bd-wave-w) (eseq.effects.custom-ui-lego/ui-lego-small-h) (bd-surf) (bd-bord) false
    (bd-small-row
      (h-stack :gap 0.22 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 3 "VOICE" 3.6 (bd-head))
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 3 5.0 (bd-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "amp_attack" "attack" 5.0 0 "ms" (bd-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "amp_release" "release" 5.0 0 "ms" (bd-text))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 3 "level_db" "level" 5.0 1 "dB" (bd-text))))))

;; Per-grain randomization: positive amounts scatter every grain, negative
;; ones scatter only some of them (the module's attenuverter halves).
(def bd-grain-block ()
  (bd-panel-dense 1
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 1 "GRAIN" 4.0 (bd-head))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "size_amt" "rnd sz" 3.2 2 false (bd-text))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "shape_amt" "rnd sh" 3.2 2 false (bd-text))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 1 "pitch_amt" "rnd pt" 3.2 2 false (bd-text))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "size" "size" 3.7 (bd-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "shape" "shape" 3.7 (bd-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 "detune" "pitch" 3.7 (bd-knob) 1)))))

(def bd-cloud-block ()
  (bd-panel-dense 2
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.2 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-header-s 2 "CLOUD" 4.0 (bd-head))
        (h-stack :gap 0.30 :align :center
          (label "freeze" :font-size 9.0 :width 3.2 :color (bd-text) :bg :transparent)
          (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 2 "freeze" "freeze" 4.6 '("off" "on") (bd-text))))
      (h-stack :gap 0.08 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "density" "density" 3.7 (bd-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "dry_wet" "dry/wet" 3.7 (bd-knob) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 2 "reverb" "reverb" 3.7 (bd-knob) 2)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (v-stack :width (bd-wave-w) :gap (eseq.effects.custom-ui-lego/ui-lego-gap)
      (bd-wave-block)
      (bd-voice-block))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (bd-grain-block)
      (bd-cloud-block))))
