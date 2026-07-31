;; Sound-glyph visual iteration fixture (sound-glyph spec §6, P2):
;; core/operator plants at defaults plus two heavily-tweaked variants.
;; Same instrument → identical topology; the geometry must make the three
;; param settings read as visibly different plants.
;;
;;   metal_seq capture --script crates/sequencer/ui/capture-fixtures/sound-glyph-operator.lisp \
;;     --buffer sound-glyph-gallery --width 1400 --height 800 --out /tmp/sound-glyph-operator.png

(capture-project
  (track :sampler :name "Gallery"))

(def capture-sound-glyphs
  (list
    (dict :key "glyph-demo:operator:default"
      :instrument "core/operator"
      :params (dict))
    (dict :key "glyph-demo:operator:bright"
      :instrument "core/operator"
      :params (dict :opa_level_db 0 :opb_level_db -3 :opc_level_db -3
                    :opd_level_db -6 :filter_freq 18000 :filter_res 0.8
                    :filter_drive 0.9 :fm_drive_db 20 :feedback 0.9
                    :lfo_rate_hz 30 :lfo_amount 1 :lfo_to_filter 1
                    :shaper_drive_db 22 :shaper_wet 1 :fenv_amt 60))
    (dict :key "glyph-demo:operator:one-knob"
      :instrument "core/operator"
      ;; The acid test for real-world edits: exactly one param changed from
      ;; defaults must still visibly move its branch.
      :params (dict :filter_freq 18000))
    (dict :key "glyph-demo:operator:dark"
      :instrument "core/operator"
      :params (dict :opa_level_db -40 :opb_level_db -60 :opc_level_db -55
                    :opd_level_db -50 :filter_freq 150 :filter_res 0.05
                    :opa_attack 6000 :opa_decay 18000 :opa_release 15000
                    :penv_amount -40 :fenv_amt -55 :lfo_rate_hz 0.2
                    :shaper_wet 0 :tone 0.05 :volume_db -30))))

(def glyph-cell (title key)
  (v-stack :gap 0.2 :align :center
    (sound-glyph :key (str "gallery-" key) :source key
      :width 26 :height 22
      :background-color '(rgba 0.03 0.035 0.04 1))
    (label title :font-size 9 :color :dim)))

(effect-buffer "*sound-glyph-gallery*"
  (h-stack :gap 2 :padding 1
    (glyph-cell "operator / defaults" "glyph-demo:operator:default")
    (glyph-cell "operator / bright" "glyph-demo:operator:bright")
    (glyph-cell "operator / one-knob" "glyph-demo:operator:one-knob")
    (glyph-cell "operator / dark" "glyph-demo:operator:dark")))
