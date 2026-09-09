; Appended to the authored scoped-control helpers by build_ui.py.
(def df-row (section prefix title)
  (df-panel section 19.6 2.35
    (h-stack :gap 0.2 :align :center
      (button title :width 1.7 :height 1.7 :font-size 8 :padding 0
        :color (df-accent) :background-color :transparent
        :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section))
      (df-knob section (str prefix "_ratio") "Ratio")
      (df-knob section (str prefix "_fine") "Fine")
      (df-knob section (str prefix "_level_db") "Level dB"))))

(def df-tab (section title)
  (button title :width 4.55 :height 0.7 :font-size 7.5 :padding 0 :corner-radius 1
    :color (if (= eseq.vanilla/custom-ui-selected-section section) (df-accent) (df-ink))
    :background-color (if (= eseq.vanilla/custom-ui-selected-section section) (df-ink) (df-accent))
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))

(def df-timbre (section prefix)
  (v-stack :gap 0.3
    (adsr-editor :mode :ade :debug-name "df-timbre-contour" :width 32 :height 2.5
      :curve-color (df-ink) :point-color (df-ink) :grid-color (df-ink) :background-color (df-accent)
      :delay-max 5000 :attack-max 5000 :decay-max 12000
      :delay (df-bound (str prefix "_delay_ms") 0)
      :attack (df-bound (str prefix "_attack_ms") 4)
      :decay (df-bound (str prefix "_decay_ms") 400)
      :end (df-bound (str prefix "_end") 0.35)
      :gated (df-bound (str prefix "_gated") 0)
      :hold-on-release (df-bound (str prefix "_hold") 1)
      :on-change (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (lambda (env)
          (eseq.effects.custom-ui-runtime/custom-ui-set-envelope-in-scope scope
            (list (list :delay (str prefix "_delay_ms"))
                  (list :attack (str prefix "_attack_ms"))
                  (list :decay (str prefix "_decay_ms"))
                  (list :end (str prefix "_end"))) env))))
    (h-stack :gap 0.4
      (df-num section (str prefix "_delay_ms") "Delay ms")
      (df-num section (str prefix "_attack_ms") "Attack ms")
      (df-num section (str prefix "_decay_ms") "Decay ms")
      (df-num section (str prefix "_end") "End")
      (df-num section (str prefix "_depth") "FM depth"))
    (h-stack :gap 0.4
      (df-option section (str prefix "_gated") "Contour" '("Triggered" "Gated"))
      (df-option section (str prefix "_reset") "Retrigger" '("Continue" "Reset"))
      (df-option-sized section (str prefix "_hold") "Release" '("Continue" "Hold") 7 1.05 (df-ink) (df-accent)))
    (if (= prefix "a")
      (df-num section "a_keyscale" "Keyscale")
      (h-stack :gap 0.4 (df-readout section "b1_keyscale" "B1 keyscale" false 7 1.05 0.43 2 0.01 (df-ink)) (df-readout section "b2_keyscale" "B2 keyscale" false 7 1.05 0.43 2 0.01 (df-ink))))))

(def df-adsr (section prefix)
  (v-stack :gap 0.4
    (adsr-editor :width 32 :height 2.5 :debug-name "df-adsr"
      :curve-color (df-ink) :point-color (df-ink) :grid-color (df-ink) :background-color (df-accent)
      :attack (df-bound (str prefix "_attack_ms") 4)
      :decay (df-bound (str prefix "_decay_ms") 400)
      :sustain (df-bound (str prefix "_sustain") 0.7)
      :release (df-bound (str prefix "_release_ms") 400)
      :attack-max 5000 :decay-max 12000 :release-max 12000
      :on-change (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
        (lambda (env)
          (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope
            (str prefix "_attack_ms") (str prefix "_decay_ms")
            (str prefix "_sustain") (str prefix "_release_ms") env))))
    (h-stack :gap 0.4
      (df-num section (str prefix "_attack_ms") "Attack ms")
      (df-num section (str prefix "_decay_ms") "Decay ms")
      (df-num section (str prefix "_sustain") "Sustain")
      (df-num section (str prefix "_release_ms") "Release ms"))
    (if (= prefix "filter")
      (h-stack :gap 0.4
        (df-num section "filter_depth" "Env oct")
        (df-num section "keytrack" "Keytrack")
        (df-num section "highpass" "Highpass Hz"))
      (h-stack :gap 0.4
        (df-num section "velocity_amount" "Velocity")
        (df-num section "pan" "Pan")))))

(def df-algorithm-button (index)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param "algorithm"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "df-algorithm-" index)
      (v-stack :gap 0.05
        (label (str index) :width 7.8 :height 0.55 :font-size 7.5 :v-align :center :color (df-ink) :bg :transparent)
        (df-routing :mode index :selected (= (reactive-value (df-bound "algorithm" 2)) index)
        :width 7.8 :height 1.65 :debug-name (str "df-algorithm-" index)
        :on-click (lambda (x y r)
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p index)))))))

(def df-routing-page ()
  (v-stack :gap 0.4
    (h-stack :gap 0.3 (df-algorithm-button 1) (df-algorithm-button 2) (df-algorithm-button 3) (df-algorithm-button 4))
    (h-stack :gap 0.3 (df-algorithm-button 5) (df-algorithm-button 6) (df-algorithm-button 7) (df-algorithm-button 8))
    (h-stack :gap 0.6
      (df-readout 0 "algorithm" "Algorithm" false 5.1 1.05 0.43 0 1 (df-ink))
      (df-num 0 "feedback" "Feedback")
      (df-num 0 "mix_xy" "X / Y")
      (df-num 0 "harmonics" "Harmonics"))))

(def df-spectrum-page ()
  (v-stack :gap 0.4
    (df-spectrum :width 32 :height 4.7 :harm (eseq.effects.param-controls/param-effective-value
        (eseq.effects.custom-ui-runtime/custom-ui-current-param "harmonics")) :debug-name "df-harmonic-spectrum")
    (h-stack :gap 0.5
      (df-num 3 "harmonics" "Harmonics")
      (df-option-sized 4 "phase_reset" "Phase reset" '("Free" "All" "C" "A+B" "A+B2") 8 1.05 (df-ink) (df-accent)))))

(def df-detail ()
  (let ((s eseq.vanilla/custom-ui-selected-section))
    (box :width 34 :height 9.7 :padding 0.35 :corner-radius 2 :background-color (df-accent) :debug-name "df-detail"
      (v-stack :gap 0.35
        (h-stack :gap 0.1 (df-tab 0 "Route") (df-tab 1 "A env") (df-tab 2 "B env") (df-tab 3 "Harm") (df-tab 4 "Phase") (df-tab 5 "Filter") (df-tab 6 "Amp"))
        (if (= s 1) (df-timbre 1 "a")
          (if (= s 2) (df-timbre 2 "b")
            (if (= s 3) (df-spectrum-page)
              (if (= s 4)
                (h-stack :gap 0.4
                  (df-option-sized 4 "phase_reset" "Phase reset" '("Free" "All" "C" "A+B" "A+B2") 8 1.05 (df-ink) (df-accent))
                  (df-num 4 "pan" "Pan") (df-num 4 "velocity_amount" "Velocity"))
                (if (= s 5) (df-adsr 5 "filter")
                  (if (= s 6) (df-adsr 6 "amp") (df-routing-page)))))))))))

(defsynth-ui
  (h-stack :gap 0.2 :align :start
    (v-stack :gap 0.1 :debug-name "df-operators"
      (df-row 2 "b2" "B2") (df-row 2 "b1" "B1") (df-row 1 "a" "A") (df-row 4 "c" "C"))
    (df-detail)
    (v-stack :gap 0.15 :debug-name "df-output"
      (df-panel 5 17 3.6
        (v-stack :gap 0.1
          (df-source-option 5 "filter_type" "Filter" '("I 12dB" "II 24dB") 6)
          (h-stack :gap 0.3 (df-log 5 "cutoff" "Cutoff Hz") (df-knob 5 "resonance" "Resonance") (df-log 5 "highpass" "HP Hz"))))
      (df-panel 6 17 2.9
        (h-stack :gap 0.4 :align :center
          (df-knob 6 "amp_attack_ms" "Attack ms") (df-log 6 "amp_release_ms" "Release ms")))
      (df-panel 4 17 2.9
        (h-stack :gap 0.4 :align :center
          (df-knob 4 "source_db" "Source dB") (df-knob 4 "volume_db" "Volume dB"))))))
