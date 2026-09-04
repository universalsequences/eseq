; Membrane Tabla UI — lego-s style, mirrors drums/membrane-snare-rim ui.lisp.
; Column A: STRIKE (mask + finger), HEAD (tune/release/syahi), MIC (small).
; Column B: STROKE (the expression panel: stroke morph + press gliss + damp),
; BODY (shell resonators). Param names match dsp.lisp exactly.

(def mtb-strike ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (eseq.effects.custom-ui-lego/ui-lego-matrix-s 0 "strike_mask" "MASK" 4.8 3.05 (eseq.effects.custom-ui-lego/ui-accent-cyan))
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "STRIKE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "scrape" "SCRP" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bend" "BEND" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "finger_hard" "HARD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 3)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "finger_speed" "SPD" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 3)))))

; syahi is the tuning-paste mass: 0 = bare mylar (inharmonic), 1 = classic
; harmonic tabla, 1.5 = over-loaded (darker, plug-heavy)
(def mtb-head ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "HEAD" 4.4 (eseq.effects.custom-ui-lego/ui-accent-green))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "tone_damp" "DAMP" 3.6 3 false (eseq.effects.custom-ui-lego/ui-accent-orange))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "tune" "TUNE" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 1)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "release" "REL" 3.9 (eseq.effects.custom-ui-lego/ui-accent-green) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "syahi" "SYHI" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)))))

(def mtb-mic ()
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "MIC" (eseq.effects.custom-ui-lego/ui-accent-orange) 0
    (h-stack :gap 0.24 :align :end
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "mic_blend" "MIC" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
      (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "level" "LVL" 3.2 2 false (eseq.effects.custom-ui-lego/ui-accent-orange)))))

; the expression panel: stroke 0 = te (closed, on the syahi), 0.5 = tun
; (open harmonic ring), 1 = na (kinar + finger on the sur nodal line);
; press is the bayan heel-of-hand gliss, damp a flat palm mute
(def mtb-stroke ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "STROKE" 4.4 (eseq.effects.custom-ui-lego/ui-accent-orange))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "gliss_range" "GLIS" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-violet)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "damp" "MUTE" 3.6 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "stroke" "STRK" 3.9 (eseq.effects.custom-ui-lego/ui-accent-orange) 2)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "press" "PRSS" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 2)))))

(def mtb-body ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width 10.6 :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "BODY" 4.4 (eseq.effects.custom-ui-lego/ui-accent-blue))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body1_gain" "1 GN" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-blue)))
        (h-stack :gap 0.18 :align :start
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body2_gain" "2 GN" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-cyan))
          (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "body3_gain" "3 GN" 3.4 2 false (eseq.effects.custom-ui-lego/ui-accent-violet))))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body1_freq" "1 HZ" 3.9 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body2_freq" "2 HZ" 3.9 (eseq.effects.custom-ui-lego/ui-accent-cyan) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 "body3_freq" "3 HZ" 3.9 (eseq.effects.custom-ui-lego/ui-accent-violet) 0)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (mtb-strike)
      (mtb-head)
      (mtb-mic))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (mtb-stroke)
      (mtb-body))))
