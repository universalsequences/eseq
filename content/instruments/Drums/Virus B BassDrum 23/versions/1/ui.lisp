;; Compact performance surface. DSP voicing and presets remain intact.
;; Graphics show named analytic mechanisms, not rendered audio.
(defsynth-ui
  (eseq.effects.drum-surface/panel "VIRUS B BASSDRUM 23"
    (list
      (list "Play" (list "tune" "Tune" 1 nil) (list "sweep" "Sweep" 2 nil))
      (list "Amp" (list "decay" "Decay" 0 nil) (list "release" "Release" 0 nil))
      (list "Timbre" (list "harm" "Harm" 2 nil) (list "bright" "Bright" 2 nil))
      (list "Output" (list "drive" "Drive" 2 nil) (list "level" "Level" 2 nil)))
    (list
      (dict :title "Play" :controls (lambda () (list (list "tune" "Tune" 1 nil) (list "sweep" "Sweep" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/pitch-view (eseq.effects.drum-surface/bind "f_end") (eseq.effects.drum-surface/bind "sweep_a1") (eseq.effects.drum-surface/bind "sweep_a2") (eseq.effects.drum-surface/bind "sweep_r1") (eseq.effects.drum-surface/bind "sweep_r2") (eseq.effects.drum-surface/bind "sweep") (eseq.effects.drum-surface/bind "tune") 0 "sweep")))
      (dict :title "Amp" :controls (lambda () (list (list "attack" "Attack" 0 nil) (list "decay" "Decay" 0 nil) (list "sustain" "Sustain" 2 nil) (list "release" "Release" 0 nil)))
        :visual (lambda () (let ((scope (eseq.effects.custom-ui-runtime/custom-ui-current-scope)))
          (adsr-editor :width 35.2 :height 4.65 :background-color :cyan :curve-color :black :point-color :black
            :attack (eseq.effects.drum-surface/bind "attack") :decay (eseq.effects.drum-surface/bind "decay") :sustain (eseq.effects.drum-surface/bind "sustain") :release (eseq.effects.drum-surface/bind "release")
            :on-change (lambda (env) (eseq.effects.custom-ui-runtime/custom-ui-set-adsr-in-scope scope "attack" "decay" "sustain" "release" env))))))
      (dict :title "Timbre" :controls (lambda () (list (list "harm" "Harm" 2 nil) (list "bright" "Bright" 2 nil) (list "noise" "Noise" 2 nil) (list "hiss" "Hiss" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/pitch-view (eseq.effects.drum-surface/bind "f_end") (eseq.effects.drum-surface/bind "sweep_a1") (eseq.effects.drum-surface/bind "sweep_a2") (eseq.effects.drum-surface/bind "sweep_r1") (eseq.effects.drum-surface/bind "sweep_r2") (eseq.effects.drum-surface/bind "sweep") (eseq.effects.drum-surface/bind "tune") 0 "sweep")))
      (dict :title "Output" :controls (lambda () (list (list "drive" "Drive" 2 nil) (list "level" "Level" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/pitch-view (eseq.effects.drum-surface/bind "f_end") (eseq.effects.drum-surface/bind "sweep_a1") (eseq.effects.drum-surface/bind "sweep_a2") (eseq.effects.drum-surface/bind "sweep_r1") (eseq.effects.drum-surface/bind "sweep_r2") (eseq.effects.drum-surface/bind "sweep") (eseq.effects.drum-surface/bind "tune") 0 "sweep")))
      (dict :title "Bank" :controls (lambda () (list (list "bank" "Bank" 2 nil) (list "bank_env" "Env depth" 2 nil) (list "bank_freq" "Cutoff" 2 nil) (list "bank_res" "Resonance" 2 nil) (list "bank_time" "Time ms" 0 nil) (list "bank_harm" "Harmonic" 2 nil) (list "bank_crunch" "Crush" 2 nil)))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Bank envelope T60 / 0–1 s" (eseq.effects.drum-surface/bind "bank_time") "Amplitude decay setting / 0–1 s" (eseq.effects.drum-surface/bind "decay") "bank_time" "decay")))
      (dict :title "Bank FX" :controls (lambda () (list (list "bank_drive" "Drive" 2 nil) (list "bank_recon" "Smooth" 2 nil) (list "bank_track" "Tracking" 2 '("free" "key"))))
        :visual (lambda () (eseq.effects.drum-surface/envelope "Bank envelope T60 / 0–1 s" (eseq.effects.drum-surface/bind "bank_time") "Release setting / 0–1 s" (eseq.effects.drum-surface/bind "release") "bank_time" "release"))))))
