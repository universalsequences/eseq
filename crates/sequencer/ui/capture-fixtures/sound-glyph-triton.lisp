;; Sound-glyph visual iteration fixture (sound-glyph spec §6, P2):
;; core/triton plants at defaults plus two heavily-tweaked variants.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/sound-glyph-triton.lisp \
;;     --buffer sound-glyph-gallery --width 1400 --height 800 --out /tmp/sound-glyph-triton.png

(capture-project
  (track :sampler :name "Gallery"))

(def capture-sound-glyphs
  (list
    (dict :key "glyph-demo:triton:default"
      :instrument "core/triton"
      :params (dict))
    (dict :key "glyph-demo:triton:swarm"
      :instrument "core/triton"
      :params (dict :osc2_on 1 :osc2_detune 0.45 :osc2_gain_db 0
                    :cutoff 18000 :resonance 0.9 :drive 0.85
                    :lfo1_rate_hz 18 :lfo1_to_pitch 0.8 :lfo1_to_cutoff 0.9
                    :lfo2_rate_hz 7 :lfo2_to_amp 0.7 :spread 1
                    :feg_int_oct 3 :ams1_amt 0.9))
    (dict :key "glyph-demo:triton:felt"
      :instrument "core/triton"
      :params (dict :osc1_gain_db -20 :osc2_on 0 :cutoff 300
                    :resonance 0.05 :hp_freq 20 :drive 0.02
                    :aeg_attack_ms 3500 :aeg_release_ms 9000
                    :feg_sustain 0.1 :lfo1_rate_hz 0.1 :lfo2_rate_hz 0.1
                    :glide_ms 800 :volume_db -20))))

(def glyph-cell (title key)
  (v-stack :gap 0.2 :align :center
    (sound-glyph :key (str "gallery-" key) :source key
      :width 26 :height 22
      :background-color '(rgba 0.03 0.035 0.04 1))
    (label title :font-size 9 :color :dim)))

(effect-buffer "*sound-glyph-gallery*"
  (h-stack :gap 2 :padding 1
    (glyph-cell "triton / defaults" "glyph-demo:triton:default")
    (glyph-cell "triton / swarm" "glyph-demo:triton:swarm")
    (glyph-cell "triton / felt" "glyph-demo:triton:felt")))
