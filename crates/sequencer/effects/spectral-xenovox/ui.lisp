(def vox-main-block ()
  (ui-control-block-medium-s "SPECTRAL VOX" (ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (ui-lego-knob-s 0 "vowel" "vwl" 4.8 (ui-accent-orange) 2)
      (ui-lego-knob-s 0 "talk" "tlk" 4.8 (ui-accent-cyan) 2)
      (ui-lego-knob-s 0 "voice" "vox" 4.8 (ui-accent-violet) 2)
      (ui-lego-knob-s 0 "formant" "fmt" 4.8 (ui-accent-blue) 2)
      (ui-lego-knob-s 0 "resonance" "res" 4.8 (ui-accent-green) 2))))

(def vox-out-block ()
  (ui-control-block-small-s "THROAT/OUT" (ui-accent-violet) 1
    (h-stack :gap 0.30 :align :start
      (ui-lego-num-s 1 "body" "bdy" 5.2 2 false (ui-accent-orange))
      (ui-lego-num-s 1 "breath" "brt" 5.2 2 false (ui-accent-cyan))
      (ui-lego-num-s 1 "mix" "mix" 5.2 2 false (ui-accent-blue))
      (ui-lego-num-s 1 "output" "out" 5.2 2 false (ui-accent-green)))))

(def vox-route-param ()
  (or
    (audio-fx-ui-param audio-fx-ui-current-fx "sidechain")
    (nth
      (filter |p| (string-starts-with? (get p :name) "sidechain")
        (get audio-fx-ui-current-fx :params))
      0)))

(def vox-route-selector ()
  (let ((fx audio-fx-ui-current-fx)
        (p (vox-route-param)))
    (box :debug-name "vox-route-selector" :width 10.4 :height 1.65 :padding 0
      (v-stack :width 10.4 :height 1.65 :gap 0.10 :align :start
        (label "source" :font-size 8.2 :width 10.4 :height 0.52 :color :dim :bg :transparent)
        (if p
          (dropdown :value (get p :text-value)
            :options (get p :options)
            :on-change (lambda (v) (param-set-option fx p v))
            :width 10.4 :height 0.92 :font-size 8.8)
          (label "missing route" :font-size 8.8 :width 10.4 :height 0.92
            :color :dim :bg :transparent))))))

(def vox-route-block ()
  (ui-readout-block-small-s "VOICE IN" (ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :center
      (vox-route-selector)
      (label "vocoder modulator" :font-size 9.0 :color (ui-accent-orange) :bg :transparent))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (ui-lego-column
      (vox-main-block)
      (vox-out-block)
      (vox-route-block))))
