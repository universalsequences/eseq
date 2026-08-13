(def vox-main-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-medium-wide-s "SPECTRAL VOX" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.32 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "vowel" "vwl" 4.8 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "talk" "tlk" 4.8 (eseq.effects.custom-ui-lego/ui-accent-cyan) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "voice" "vox" 4.8 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "formant" "fmt" 4.8 (eseq.effects.custom-ui-lego/ui-accent-blue) 2)
      (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "resonance" "res" 4.8 (eseq.effects.custom-ui-lego/ui-accent-green) 2))))

(def vox-out-block ()
  (eseq.effects.custom-ui-lego/ui-control-block-small-wide-s "THROAT/OUT" (eseq.effects.custom-ui-lego/ui-accent-violet) 1
    (h-stack :gap 0.30 :align :start
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "body" "bdy" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "breath" "brt" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "mix" "mix" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-num-s 1 "output" "out" 5.2 2 false (eseq.effects.custom-ui-lego/ui-accent-green)))))

(def vox-route-param ()
  (or
    (eseq.effects.custom-effect-ui/audio-fx-ui-param audio-fx-ui-current-fx "sidechain")
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
            :on-change (lambda (v) (eseq.effects.param-controls/param-set-option fx p v))
            :width 10.4 :height 0.92 :font-size 8.8)
          (label "missing route" :font-size 8.8 :width 10.4 :height 0.92
            :color :dim :bg :transparent))))))

(def vox-route-block ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-wide-s "VOICE IN" (eseq.effects.custom-ui-lego/ui-accent-cyan) 0
    (h-stack :gap 0.32 :align :center
      (vox-route-selector)
      (label "vocoder modulator" :font-size 9.0 :color (eseq.effects.custom-ui-lego/ui-accent-orange) :bg :transparent))))

(defeffect-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column-wide
      (vox-main-block)
      (vox-out-block)
      (vox-route-block))))
