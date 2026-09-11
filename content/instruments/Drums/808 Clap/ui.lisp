;; A parameter-driven clap schematic. No audio rendering: the fixed texture
;; illustrates filtered noise; timing and envelopes follow the authored DSP.
(def idclap-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def idclap-section () eseq.vanilla/custom-ui-selected-section)
(def idclap-p (name) (eseq.effects.custom-ui-runtime/custom-ui-current-param name))
(def idclap-bind (name) (eseq.effects.custom-ui-runtime/custom-ui-param-binding (idclap-p name)))
(def idclap-value (name) (reactive-value (idclap-bind name)))
(def idclap-knob (section name title width)
  (eseq.effects.custom-ui-lego/ui-lego-knob-styled-s section name title width 3.45 2.15
    (idclap-c) 2 :linear :widget-knob-track 10 9.5 :center))
(def idclap-label (title width)
  (box :width width :height 0.72 :background-color (idclap-c)
    (label title :width width :height 0.72 :h-align :center :v-align :center
      :font-size 8.2 :color :black :bg :transparent)))
(def idclap-panel (section width body)
  (box :width width :height 4.8 :padding 0.18 :background-color :instrument-group-bg
    :debug-name (str "clap-panel-" section)
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section) body))
(def idclap-num (section name title width decimals)
  (let ((p (idclap-p name))
        (ink (if (eseq.effects.custom-ui-runtime/custom-ui-param-mod-highlighted? p) :white :black)))
    (eseq.effects.custom-ui-runtime/custom-ui-param-mod-wrapper p (str "clap-mod-" name)
      (subtree :key (str "clap-num-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name)
          (eseq.effects.custom-ui-runtime/custom-ui-param-control-key-mode p) "-" name)
        (v-stack :width width :height 1.15 :gap 0.08
          (label title :height 0.5 :v-align :center :font-size 8 :color ink :bg :transparent)
          (number-picker :width width :height 0.55 :noui true :font-size 8.5 :decimals decimals
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :text-align :left
            :text-color (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p)
              (eseq.effects.custom-ui-runtime/custom-ui-param-plock-text-color p) ink)
            :plock-active (if (eseq.effects.custom-ui-runtime/custom-ui-param-plock-active? p) 1 0)
            :plock-color-r (eseq.effects.param-controls/param-plock-color-r)
            :plock-color-g (eseq.effects.param-controls/param-plock-color-g)
            :plock-color-b (eseq.effects.param-controls/param-plock-color-b)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s section p)))))))

;; A named region shares its visible SDF with a larger independent grab area.
(defmacro idclap-handle (name time)
  `(let ((hx (* aspect (- (* 1.88 (sqrt (/ ,time 0.30))) 0.94))))
     (sdf/layer
       (sdf/paint (sdf/translate hx -0.20 (sdf/rect 0.006 0.52)) (rgba 0 0 0 0.35))
       (sdf/region ,name
         (sdf/translate hx -0.74 (sdf/rect 0.047 0.047))
         (material :color (if hit/hover :white :black))
         (sdf/translate hx -0.74 (sdf/rect 0.11 0.13)))
       (sdf/paint (sdf/translate hx -0.74 (sdf/rect 0.028 0.028)) :yellow))))
(defmacro idclap-bracket (start end)
  `(let ((ax (* aspect (- (* 1.88 (sqrt (/ ,start 0.30))) 0.94)))
         (bx (* aspect (- (* 1.88 (sqrt (/ ,end 0.30))) 0.94))))
     (sdf/paint
       (min (sdf/translate (* 0.5 (+ ax bx)) 0.82 (sdf/rect (* 0.5 (- bx ax)) 0.007))
            (sdf/translate ax 0.82 (sdf/rect 0.007 0.06))
            (sdf/translate bx 0.82 (sdf/rect 0.007 0.06)))
       :black)))
(defmacro idclap-seg (time start rate)
  `(if (< ,time ,start) 0 (exp (* ,rate (- ,time ,start)))))
(defmacro idclap-burst (time start)
  `(+ (idclap-seg ,time ,start bdec)
      (* sub (idclap-seg ,time (+ ,start sd) bdec))))

(defwidget eseq-clap-burst-display
  :width 35.2 :height 4.65
  :state (sp1 sp2 sp3 flam bdec l2 l3 l4 sd sub burst fast fastd slow slowd)
  :bindable (sp1 sp2 sp3 flam bdec l2 l3 l4 sd sub burst fast fastd slow slowd)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.94) 1.88) 0 1))
        (t (* 0.30 u u))
        (fl (clamp flam 0.25 4))
        (t2 (* 0.001 sp1 fl))
        (t3 (* 0.001 (+ sp1 sp2) fl))
        (t4 (* 0.001 (+ sp1 sp2 sp3) fl))
        (env (+ (* burst (+ (idclap-burst t 0)
                            (* l2 (idclap-burst t t2))
                            (* l3 (idclap-burst t t3))
                            (* l4 (idclap-burst t t4))))
                (* fast (idclap-seg t t4 fastd))
                (* slow (idclap-seg t t4 slowd))))
        ;; Fixed band-limited-looking detail: stable under p-lock changes.
        (texture (+ 0.25 (* 0.30 (abs (sin (* t 13571))))
                         (* 0.25 (abs (sin (+ 0.6 (* t 23719)))))
                         (* 0.20 (abs (sin (+ 1.3 (* t 7919)))))))
        (amp (* (min 0.66 (* 0.52 env)) texture))
        (wave (- (abs (- y 0.18)) amp)))
    (sdf/layer
      (sdf/paint (sdf/rect width height) :yellow)
      (sdf/paint (sdf/translate 0 0.18 (sdf/rect (* aspect 0.94) 0.007)) (rgba 0 0 0 0.5))
      (sdf/paint (max wave (- (abs x) (* aspect 0.94))) :black)
      (idclap-handle :burst-1 0)
      (idclap-handle :burst-2 t2)
      (idclap-handle :burst-3 t3)
      (idclap-handle :burst-4 t4)
      (idclap-bracket 0 t2)
      (idclap-bracket t2 t3)
      (idclap-bracket t3 t4))))

(def idclap-drag (scope sx sy region)
  (let ((u (max 0 (min 1 (/ (+ sx 0.94) 1.88))))
        (fl (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding
              (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope "flam")))))
    (let ((ms (/ (* 300 u u) (max 0.25 (min 4 fl))))
          (a (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding
              (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope "sp1"))))
          (b (reactive-value (eseq.effects.custom-ui-runtime/custom-ui-param-binding
              (eseq.effects.custom-ui-runtime/custom-ui-param-in-scope scope "sp2")))))
      (let ((name (if (= region :burst-2) "sp1" (if (= region :burst-3) "sp2" (if (= region :burst-4) "sp3" false))))
            (value (if (= region :burst-2) ms (if (= region :burst-3) (- ms a) (- ms a b)))))
        (if name
          (eseq.effects.custom-ui-runtime/custom-ui-set-param-by-name-in-scope scope name (max 4 (min 16 value)))
          false)))))
(def idclap-wave ()
  (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
    (subtree :key (str "clap-wave-" (eseq.effects.custom-ui-runtime/custom-ui-scope-name))
      (eseq-clap-burst-display :debug-name "clap-wave" :width 35.2 :height 4.65
        :sp1 (idclap-bind "sp1") :sp2 (idclap-bind "sp2") :sp3 (idclap-bind "sp3")
        :flam (idclap-bind "flam") :bdec (idclap-bind "bdecay")
        :l2 (idclap-bind "l2") :l3 (idclap-bind "l3") :l4 (idclap-bind "l4")
        :sd (* 0.001 (idclap-value "sub_delay")) :sub (idclap-bind "sub_gain")
        :burst (* (idclap-value "burst_amp") (max 0 (min 4 (idclap-value "snap"))))
        :fast (* (idclap-value "tail_a1") (max 0 (min 4 (idclap-value "body"))))
        :slow (* (idclap-value "tail_a2") (max 0 (min 4 (idclap-value "body"))))
        :fastd (/ (idclap-value "tail_d1") (max 0.1 (min 6 (idclap-value "decay"))))
        :slowd (/ (idclap-value "tail_d2") (max 0.1 (min 6 (idclap-value "decay"))))
        :on-drag (lambda (sx sy region) (idclap-drag scope sx sy region))))))

(def idclap-play ()
  (idclap-panel 0 24
    (v-stack :gap 0.14
      (h-stack :gap 0.4 :height 0.72 :align :center
        (idclap-label "PLAY" 6)
        (label "Note" :height 0.6 :font-size 8 :color :dim :bg :transparent)
        (let ((p (eseq.effects.custom-ui-runtime/custom-ui-current-base-note-param)))
          (number-picker :width 3 :height 0.6 :noui true :decimals 0 :step 1 :font-size 8
            :value (eseq.effects.custom-ui-runtime/custom-ui-param-binding p)
            :min (eseq.effects.custom-ui-runtime/custom-ui-param-control-min p)
            :process-value (eseq.effects.custom-ui-runtime/custom-ui-param-process-value p) :process-clamped (eseq.effects.custom-ui-runtime/custom-ui-param-process-clamped p)
            :max (eseq.effects.custom-ui-runtime/custom-ui-param-control-max p)
            :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback p))))
      (h-stack :gap 0.12
        (idclap-knob 0 "tune" "Tune" 5.7)
        (idclap-knob 0 "flam" "Flam" 5.7)
        (idclap-knob 0 "snap" "Snap" 5.7)
        (idclap-knob 0 "body" "Body" 5.7)
      ))))
(def idclap-shape ()
  (idclap-panel 1 24
    (v-stack :gap 0.14
      (idclap-label "SHAPE" 6)
      (h-stack :gap 0.12
        (idclap-knob 1 "decay" "Decay" 5.7)
        (idclap-knob 1 "bright" "Bright" 5.7)
        (idclap-knob 1 "drive" "Drive" 5.7)
        (idclap-knob 1 "level" "Level" 5.7)
      ))))
(def idclap-filters ()
  (idclap-panel 2 18
    (v-stack :gap 0.14
      (idclap-label "FILTERS" 6)
      (h-stack :gap 0.12
        (idclap-knob 2 "fc1" "FC1" 5.7)
        (idclap-knob 2 "q1" "Q1" 5.7)
        (idclap-knob 2 "fc2" "FC2" 5.7)
      ))))
(def idclap-output ()
  (idclap-panel 3 18
    (v-stack :gap 0.14
      (idclap-label "OUTPUT" 6)
      (h-stack :gap 0.12
        (idclap-knob 3 "out_drive" "Drive" 5.7)
        (idclap-knob 3 "out_gain" "Gain" 5.7)
        (idclap-knob 3 "out_hp" "HPF" 5.7)
      ))))
(def idclap-page-0 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (idclap-num 0 "sp1" "SP1 ms" 8.4 2)
      (idclap-num 0 "sp2" "SP2 ms" 8.4 2)
      (idclap-num 0 "sp3" "SP3 ms" 8.4 2)
      (idclap-num 0 "bdecay" "BDEC" 8.4 0)
    )
    (h-stack :gap 0.35
      (idclap-num 0 "l2" "L2" 8.4 2)
      (idclap-num 0 "l3" "L3" 8.4 2)
      (idclap-num 0 "l4" "L4" 8.4 2)
      (idclap-num 0 "burst_amp" "BAMP" 8.4 2)
    )
  ))
(def idclap-page-1 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (idclap-num 1 "tail_a1" "Fast" 8.4 2)
      (idclap-num 1 "tail_d1" "F.Dec" 8.4 1)
      (idclap-num 1 "tail_a2" "Slow" 8.4 2)
      (idclap-num 1 "tail_d2" "S.Dec" 8.4 1)
    )
    (h-stack :gap 0.35
      (idclap-num 1 "tail_lpf" "TLPF Hz" 8.4 0)
      (idclap-num 1 "sub_delay" "Sub ms" 8.4 2)
      (idclap-num 1 "sub_gain" "Sub gain" 8.4 2)
    )
  ))
(def idclap-page-2 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (idclap-num 2 "q2" "Q2" 8.4 2)
      (idclap-num 2 "g2" "G2" 8.4 2)
      (idclap-num 2 "hp_fc" "HPF Hz" 8.4 0)
    )
  ))
(def idclap-page-3 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (idclap-num 3 "b_hp" "Burst HP" 8.4 2)
      (idclap-num 3 "t_hp" "Tail HP" 8.4 2)
    )
  ))

;; The bank's control envelope in normalized cutoff coordinates, before VCO
;; slew and note tracking. This is an editable control diagram, not audio.
(defwidget eseq-clap-bank-display
  :width 35.2 :height 4.1
  :state (floor depth duration) :bindable (floor depth duration)
  :shader
  (let ((u (clamp (/ (+ (/ x aspect) 0.94) 1.88) 0 1))
        (env (exp (/ (* -6907.7553 u) (max duration 1))))
        (position (clamp (+ floor (* depth env)) 0 1))
        (curve (- 0.82 (* 1.64 position))))
    (sdf/layer
      (sdf/region :curve (sdf/rect width height) :yellow)
      (sdf/paint (max (- y 0.82) (- curve y)) (rgba 0 0 0 0.14))
      (sdf/paint (- (abs (- y curve)) 0.025) :black))))
(def idclap-bank-wave ()
  (let ((gesture (eseq.effects.drum-surface/parameter-gesture "bank_time" "bank_env")))
    (v-stack :width 35.2 :height 4.65 :gap 0.05
      (label "Cutoff control envelope / 0–1 s" :height 0.5 :font-size 8 :color :black :bg :transparent)
      (eseq-clap-bank-display :debug-name "clap-bank-envelope"
        :floor (idclap-bind "bank_freq") :depth (idclap-bind "bank_env") :duration (idclap-bind "bank_time")
        :on-mouse-down (get gesture :down) :on-drag (get gesture :drag) :on-mouse-up (get gesture :up)))))
(def idclap-page-4 ()
  (v-stack :gap 0.15
    (h-stack :gap 0.35
      (idclap-num 4 "bank" "Mix" 8.4 2)
      (idclap-num 4 "bank_freq" "Cutoff" 8.4 2)
      (idclap-num 4 "bank_res" "Resonance" 8.4 2)
      (idclap-num 4 "bank_env" "Env depth" 8.4 2))
    (h-stack :gap 0.35
      (idclap-num 4 "bank_time" "Time ms" 8.4 0)
      (idclap-num 4 "bank_harm" "Harmonic" 8.4 1)
      (idclap-num 4 "bank_crunch" "Crush" 8.4 2)
      (idclap-num 4 "bank_drive" "Drive" 8.4 2))))
(def idclap-page-5 ()
  (h-stack :gap 0.35
    (idclap-num 5 "bank_recon" "Reconstruct" 8.4 2)
    (v-stack :width 8.4 :height 1.15 :gap 0.08
      (label "Tracking" :height 0.5 :v-align :center :font-size 8 :color :black :bg :transparent)
      (let ((p (idclap-p "bank_track")))
        (dropdown :width 8.4 :height 0.55 :font-size 8.5 :options '("Free" "Key")
          :value-index (idclap-bind "bank_track")
          :on-change (eseq.effects.custom-ui-runtime/custom-ui-param-change-callback-s 5 p))))
    (idclap-num 5 "smoothing" "Slew ms" 8.4 1)))

(def idclap-page-button (section title)
  (button title :width 5.6 :height 0.8 :padding 0 :font-size 8
    :color (if (= (idclap-section) section) (idclap-c) :black)
    :border-color :transparent
    :background-color (if (= (idclap-section) section) :black (idclap-c))
    :on-click (eseq.effects.custom-ui-sections/ui-section-select-callback section)))
(def idclap-display ()
  (box :debug-name "clap-display" :width 35.8 :height 9.8 :padding 0.3 :background-color (idclap-c)
    (v-stack :gap 0.15
      (label (str "808 CLAP / " (nth '("BURSTS" "TAIL" "FILTERS" "OUTPUT" "BANK" "BANK FX") (idclap-section)))
        :width 35.2 :height 0.7 :h-align :center :v-align :center :font-size 8 :color (idclap-c) :bg :black)
      (if (< (idclap-section) 4) (idclap-wave) (idclap-bank-wave))
      (box :width 35.2 :height 2.6
        (if (= (idclap-section) 0) (idclap-page-0)
          (if (= (idclap-section) 1) (idclap-page-1)
            (if (= (idclap-section) 2) (idclap-page-2)
              (if (= (idclap-section) 3) (idclap-page-3)
                (if (= (idclap-section) 4) (idclap-page-4) (idclap-page-5)))))))
      (h-stack :gap 0.3
        (idclap-page-button 0 "Bursts") (idclap-page-button 1 "Tail")
        (idclap-page-button 2 "Filters") (idclap-page-button 3 "Output")
        (idclap-page-button 4 "Bank") (idclap-page-button 5 "Bank FX")))))
(defsynth-ui
  (h-stack :height 9.8 :gap 0.3 :align :start
    (v-stack :gap 0.2 (idclap-play) (idclap-shape))
    (idclap-display)
    (v-stack :gap 0.2 (idclap-filters) (idclap-output))))
