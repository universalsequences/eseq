;; Sour: acid voice + built-in filterbank. Layout follows Digi Syn: sources on
;; the left, an envelope/matrix screen in the middle, filter + bank on the right.
;; Big knobs are the per-step p-lock targets.
(def sour-accent () :control-on-bg)
(def sour-ink () :control-on-fg)
;; Screen sections: 0 AMP env, 1 FILTER env, 2 BANK env, 3 MATRIX.
(def sour-section ()
  (let ((section eseq.vanilla/custom-ui-selected-section))
    (if (or (= section 1) (= section 2) (= section 3)) section 0)))
(def sour-knob (name title width height size decimals taper)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s (sour-section) name title
    width height size (sour-accent) decimals taper :widget-knob-track 10.5 9.5 :right))
(def sour-panel (width height body)
  (box :width width :height height :padding 0.12 :corner-radius 2
    :background-color :instrument-group-bg body))
(def sour-label (title width)
  (box :width width :height 0.75 :background-color (sour-accent)
    (label title :width width :height 0.75 :h-align :center :font-size 8 :color (sour-ink) :bg :transparent :v-align :center)))

(def sour-option (name options width ink surface)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "sour-option-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "sour-option-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
        (dropdown :width width :height 0.8 :font-size 8
          :value-index (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
          :value-index-offset (get p :min) :options options
          :text-color ink :chevron-color ink :badge-color :transparent
          :bg-color surface :border-color :transparent
          :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
          :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
          :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
          :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
          :on-change (lambda (v)
            (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope p
              (+ (get p :min) (eseq.effects.param-controls/custom-ui-option-index options v)))))))))

(def sour-readout (name title width decimals step ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white ink)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "sour-num-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "sour-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width width :height 1.1 :gap 0.06
          (label title :v-align :center :height 0.5 :font-size 8.6 :color ink :bg :transparent)
          (number-picker :width width :height 0.50 :noui true :decimals decimals :step step :font-size 8.0
            :unit (eseq.effects.custom-ui-runtime/custom-ui-param-control-unit p)
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color ink :edit-color ink :cursor-color ink
            :plock-style :underline
            :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s (sour-section) p)))))))
(def sour-num (name title width decimals)
  (sour-readout name title width decimals (if (= decimals 0) 1 0.01) (sour-ink)))

;; Single-line readout for dark panels (ink false) or the screen (ink given).
(def sour-inline (name title width decimals ink)
  (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-param name)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p
      (str "sour-inline-mod-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name) "-" name)
      (subtree :key (str "sour-inline-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (h-stack :width width :height 0.8 :gap 0.25 :align :center
          (label title :width (min 5.8 (* width 0.5)) :height 0.8 :font-size 8.6 :color (if ink ink :dim) :bg :transparent :v-align :center)
          (number-picker :width (- width (+ 0.25 (min 5.8 (* width 0.5)))) :height 0.75 :noui true :decimals decimals :font-size 8
            :unit (eseq.effects.custom-ui-runtime/custom-ui-param-control-unit p)
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-color (if ink ink (eseq.effects.custom-ui-runtime/custom-ui-param-knob-text-color p))
            :text-align :left
            :plock-style (if ink :underline :fill)
            :plock-active (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active-prop p)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s (sour-section) p)))))))

;; ---- left: sources ----
(def sour-osc-a ()
  (sour-panel 25 3.5
    (v-stack :gap 0.15
      (h-stack :gap 0.4 :align :center
        (sour-label "OSC A" 4.5)
        (label "saw / pulse into the filter" :height 0.75 :font-size 8 :color :dim :bg :transparent :v-align :center))
      (h-stack :gap 0.35
        (sour-knob "octave" "Octave" 5.6 2.3 3.1 0 "linear")
        (sour-knob "wave" "Saw-Pulse" 5.6 2.3 3.1 2 "linear")
        (sour-knob "pw" "Width" 5.6 2.3 3.1 2 "linear")
        (sour-knob "a_level" "Level" 5.6 2.3 3.1 1 "linear")))))
(def sour-osc-b ()
  (sour-panel 25 3.5
    (v-stack :gap 0.15
      (h-stack :gap 0.4 :align :center
        (sour-label "OSC B" 4.5)
        (label "square, level drives the filter" :height 0.75 :font-size 8 :color :dim :bg :transparent :v-align :center))
      (h-stack :gap 0.35
        (sour-knob "b_oct" "Octave" 5.6 2.3 3.1 0 "linear")
        (sour-knob "unison" "Unison" 5.6 2.3 3.1 2 "linear")
        (sour-knob "detune" "Detune" 5.6 2.3 3.1 0 "linear")
        (sour-knob "b_level" "Level" 5.6 2.3 3.1 1 "linear")))))
(def sour-wobble ()
  (sour-panel 25 2.6
    (h-stack :gap 0.35 :align :center
      (v-stack :gap 0.25
        (sour-label "WOBBLE" 5.4)
        (sour-inline "glide_ms" "Glide" 5.4 0 false))
      (sour-knob "wob_ratio" "Ratio" 5.6 2.3 2.8 3 "log")
      (sour-knob "wob_depth" "Depth" 5.6 2.3 2.8 2 "linear")
      (sour-knob "accent" "Accent" 5.6 2.3 2.8 2 "linear"))))
(def sour-sources ()
  (v-stack :gap 0.1 (sour-osc-a) (sour-osc-b) (sour-wobble)))

;; ---- middle: screen ----
(def sour-env-plot (section a d s r)
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (adsr-editor :width 33.2 :height 3.0 :debug-name "sour-envelope"
      :background-color (sour-accent) :curve-color (sour-ink)
      :point-color (sour-ink) :grid-color (sour-ink)
      :attack (eseq.effects.custom-ui-controls/ui-param-bound-value a 1)
      :decay (eseq.effects.custom-ui-controls/ui-param-bound-value d 300)
      :sustain (eseq.effects.custom-ui-controls/ui-param-bound-value s 0.5)
      :release (eseq.effects.custom-ui-controls/ui-param-bound-value r 100)
      :on-change (lambda (env)
        (do
          (eseq.effects.custom-ui-sections/custom-ui-set-active-adsr scope section (get env :active))
          (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope a d s r env))))))
(def sour-rule () (box :width 33.2 :height 0.03 :background-color (sour-ink)))
(def sour-env-detail (section a d s r stitle extra)
  (v-stack :gap 0.14
    (sour-env-plot section a d s r)
    (h-stack :gap 0.8
      (sour-num a "Attack" 7.6 1)
      (sour-num d "Decay" 7.6 0)
      (sour-num s stitle 7.6 2)
      (sour-num r "Release" 7.6 0))
    (sour-rule)
    extra))
(def sour-src-options ()
  '("off" "wobble" "jitter" "f env" "bank env" "amp env" "accent mem" "glide" "counter" "follower" "chaos" "velocity" "key" "random"))
(def sour-dst-options ()
  '("off" "cutoff" "reso" "hp" "env amt" "bank freqs" "bank modes" "bank mix" "bank fm" "crunch" "pitch" "B level" "width" "amp" "wob depth" "F2 freq"))
(def sour-matrix-row (n)
  (h-stack :gap 0.5 :align :center
    (sour-option (str "m" n "_src") (sour-src-options) 8.0 :fg :instrument-control-bg)
    (label "→" :width 1.2 :height 0.8 :color (sour-ink) :bg :transparent)
    (sour-option (str "m" n "_dst") (sour-dst-options) 10.5 :fg :instrument-control-bg)
    (sour-inline (str "m" n "_amt") "Amt" 9.5 2 (sour-ink))))
(def sour-matrix-detail ()
  (v-stack :gap 0.3
    (sour-matrix-row 1) (sour-matrix-row 2) (sour-matrix-row 3) (sour-matrix-row 4)
    (sour-rule)
    (h-stack :gap 0.8
      (sour-num "jit_rate" "Jitter Hz" 7.6 1)
      (sour-num "jit_smooth" "Jitter smooth" 7.6 2)
      (sour-num "count_len" "Counter len" 7.6 0))))
(def sour-tab (section title)
  (button title :width 4.0 :height 0.75 :debug-name (str "sour-tab-" section) :font-size 8 :padding 0
    :color (if (= (sour-section) section) (sour-accent) (sour-ink))
    :border-color :transparent
    :background-color (if (= (sour-section) section) (sour-ink) (sour-accent))
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))
(def sour-display ()
  (let ((section (sour-section)))
    (box :width 34 :height 9.8 :padding 0.35 :corner-radius 2
      :debug-name "sour-detail-display" :part "instrument/screen" :background-color (sour-accent)
      (v-stack :gap 0.14
        (h-stack :gap 0.15
          (box :width 16.6 :height 0.75 :background-color (sour-ink)
            (label (nth '("SOUR / AMP ENV" "SOUR / FILTER ENV" "SOUR / BANK ENV" "SOUR / MATRIX") section)
              :height 0.75 :font-size 8 :color (sour-accent) :bg :transparent :v-align :center))
          (sour-tab 0 "AMP") (sour-tab 1 "FILTER") (sour-tab 2 "BANK") (sour-tab 3 "MATRIX"))
        (if (= section 3)
          (sour-matrix-detail)
          (if (= section 1)
            (sour-env-detail 1 "env_attack" "decay_ms" "env_sus" "env_release" "Sustain"
              (h-stack :gap 0.8
                (sour-num "env_mod" "Env amount" 7.6 1)
                (sour-num "keytrack" "Keytrack" 7.6 2)
                (sour-num "accent_sweep" "Accent sweep" 7.6 2)
                (sour-num "accent_thresh" "Accent vel" 7.6 2)))
            (if (= section 2)
              (sour-env-detail 2 "bank_attack" "bank_decay" "bank_sus" "bank_rel" "Sustain ±"
                (h-stack :gap 0.55
                  (sour-num "bank_env" "F1 env" 4.9 2)
                  (sour-num "bank2_env" "F2 env" 4.9 2)
                  (sour-num "bank_res" "Res" 4.9 0)
                  (sour-num "bank_par" "Ser-Par" 4.9 2)
                  (sour-num "bank_corr" "Correct" 4.9 2)))
              (sour-env-detail 0 "amp_attack" "amp_decay" "amp_sustain" "amp_release" "Sustain"
                (h-stack :gap 0.8
                  (sour-num "volume_db" "Volume" 7.6 1)
                  (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s (sour-section) 7.6 (sour-ink)))))))))))

;; ---- right: filter + bank ----
(def sour-filter-curve ()
  (let ((cut-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "cutoff"))
        (res-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "reso"))
        (hp-p (eseq.effects.custom-ui-runtime/custom-ui-current-param "hp"))
        (scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (if (and cut-p res-p hp-p)
      (response-curve-editor
        :mode :filter
        :bands (list
          (dict :id 0 :type "lowpass"
            :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding cut-p)
            :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min cut-p)
            :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max cut-p)
            :gain 0 :gain-min -12 :gain-max 12
            :q (eseq.effects.custom-ui-runtime/custom-ui-param-binding res-p)
            :q-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min res-p)
            :q-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max res-p)
            :q-curve-offset 0.5 :q-curve-scale 6.7 :q-curve-power 3.0
            :enabled true :selected true)
          (dict :id 1 :type "highpass"
            :freq (eseq.effects.custom-ui-runtime/custom-ui-param-binding hp-p)
            :freq-min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min hp-p)
            :freq-max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max hp-p)
            :gain 0 :gain-min -12 :gain-max 12
            :q 0.85 :q-min 0 :q-max 1
            :lock-y true
            :enabled true :selected false))
        :freq-min 10 :freq-max 20000 :gain-min -12 :gain-max 12 :q-min 0 :q-max 1
        :background-color :curve-editor-bg :corner-radius 5 :grid-color :curve-editor-grid
        :stroke-color (sour-accent) :stroke-width 3 :point-color (sour-accent)
        :width 29.4 :height 1.9 :debug-name "sour-filter-response"
        :on-action (lambda (event)
          (if (or (= (get event :type) :change-band) (= (get event :type) :commit-band))
            (if (= (get event :id) 1)
              (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope hp-p (get event :freq))
              (do
                (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope cut-p (get event :freq))
                (eseq.effects.custom-ui-runtime/custom-ui-set-param-in-scope scope res-p (get event :q))))
            nil)))
      (label "missing filter params" :font-size 8 :color :red :bg :transparent))))
(def sour-filter-panel ()
  (sour-panel 29.8 5.3
    (v-stack :gap 0.1
      (h-stack :gap 0.5 :align :center
        (sour-label "FILTER" 5.4)
        (label "4-pole LP into resonant HP" :height 0.75 :font-size 8 :color :dim :bg :transparent :v-align :center))
      (h-stack :gap 1.8
        (sour-knob "cutoff" "Cutoff" 8.3 2.15 3.0 0 "log")
        (sour-knob "reso" "Resonance" 8.3 2.15 3.0 2 "linear")
        (sour-knob "hp" "HP" 8.3 2.15 3.0 0 "log"))
      (sour-filter-curve))))
(def sour-harm-options ()
  '("free" "F1÷1" "F1÷1.5" "F1÷2" "F1÷3" "F1÷4" "F1÷5" "F1÷6" "F1÷8" "F1÷9" "F1÷12" "F1÷16"))
(def sour-fm-src-options ()
  '("drive" "osc A" "osc B" "mix" "filt" "wob" "noise"))
(def sour-bank-panel ()
  (sour-panel 29.8 4.4
    (v-stack :gap 0.15
      (h-stack :gap 0.3 :align :center
        (sour-label "BANK" 4.6)
        (sour-option "bank_harm" (sour-harm-options) 5.6 :fg :instrument-control-bg)
        (sour-inline "bank_crunch" "Crunch" 6.6 2 false)
        (sour-inline "bank_fm" "FM" 5.2 2 false)
        (sour-option "bank_fm_src" (sour-fm-src-options) 5.6 :fg :instrument-control-bg))
      (h-stack :gap 0.2
        (sour-knob "bank_drive" "Drive" 4.6 2.3 2.7 1 "linear")
        (sour-knob "bank_freq" "F1 Freq" 4.6 2.3 2.7 0 "log")
        (sour-knob "bank_mode" "F1 LP-HP" 4.6 2.3 2.7 2 "linear")
        (sour-knob "bank2_freq" "F2 Freq" 4.6 2.3 2.7 0 "log")
        (sour-knob "bank2_mode" "F2 LP-HP" 4.6 2.3 2.7 2 "linear")
        (sour-knob "bank_mix" "Mix" 4.6 2.3 2.7 2 "linear")))))

(defsynth-ui
  (h-stack :gap 0.15 :align :start
    (sour-sources)
    (sour-display)
    (v-stack :gap 0.1
      (sour-filter-panel)
      (sour-bank-panel))))
