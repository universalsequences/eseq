; Factory Modal Kick UI — the 808/909 Kick three-column lego-s shape:
; BEATER/BEND/SHELL column, HEAD 1/HEAD 2/SHAPE+MIX column, and the
; TONE + BANK column shared verbatim with Drums/808 Kick and Drums/909
; Kick, so the three factory kicks present one bank surface. Section titles
; are plain :fg text; knobs use the default cyan accent except the two head
; panels, which get their own colour so head 1 and head 2 read apart.
; Param names match dsp.lisp; every DSP param is on the panel.

(def mdk-num-w () 4.6)
(def mdk-strip-num-w () 5.0)
(def mdk-grid-w () (+ (* 2 (mdk-num-w)) 0.18))
(def mdk-knob-w () 4.55)
(def mdk-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def mdk-head1-c () (eseq.effects.custom-ui-lego/ui-accent-green))
(def mdk-head2-c () (eseq.effects.custom-ui-lego/ui-accent-violet))

(def mdk-title-w (title w)
  (box :width w :height 0.82 :v-align :end
    (label title :font-size 9.2 :width w :height 0.82 :color :fg :bg :transparent)))
(def mdk-title (title) (mdk-title-w title (mdk-num-w)))
(def mdk-num-w-s (name title unit decimals w)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title w decimals unit (mdk-c)))
(def mdk-num (name title unit decimals)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title (mdk-num-w) decimals unit (mdk-c)))
(def mdk-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title (mdk-knob-w) color decimals))
; same height as a micro-num so a panel with empty grid slots keeps its title
; at the same vertical spot as a full one
(def mdk-blank () (box :width (mdk-num-w) :height 1.84))

; panel skeleton: title + up to 3 pickers in a 2x2 grid on the left, knobs on the right
(def mdk-panel (title n1 n2 n3 knobs)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (mdk-grid-w) :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (mdk-title title) n1)
        (h-stack :gap 0.18 :align :start n2 n3))
      (box :width :fill :height 0.1)
      knobs)))
(def mdk-knobs-2 (a b) (h-stack :gap 0.10 :align :start a b))
(def mdk-knobs-3 (a b c) (h-stack :gap 0.10 :align :start a b c))

; beater: HARD felt..wood sets the contact time, SPEED the impact; SIZE
; spreads the hit over the modes, CLICK is the contact slap, BRIGHT tilts
; the mic toward the upper partials
(def mdk-beater ()
  (mdk-panel "BEATER"
    (mdk-num "beater_size" "SIZE" false 2)
          (mdk-num "click" "CLICK" false 2)
          (mdk-num "bright" "BRIGHT" false 2)
    (mdk-knobs-2 (mdk-knob "beater_hard" "HARD" 4 (mdk-c))
          (mdk-knob "beater_speed" "SPEED" 3 (mdk-c)))))

; bend: how far the hit pushes the pitch sharp, and how fast it settles
(def mdk-bend ()
  (mdk-panel "BEND"
    (mdk-blank) (mdk-blank) (mdk-blank)
    (mdk-knobs-2 (mdk-knob "bend" "BEND" 2 (mdk-c))
          (mdk-knob "bend_time" "TIME" 0 (mdk-c)))))

; head 1 = batter (struck) head; MUFFLE is the pillow against it
(def mdk-head1 ()
  (mdk-panel "HEAD 1"
    (mdk-num "tilt" "TILT" false 2)
          (mdk-num "head_couple" "COUPLE" false 2)
          (mdk-num "muffle" "MUFFLE" false 2)
    (mdk-knobs-3 (mdk-knob "tune" "TUNE" 1 (mdk-head1-c))
          (mdk-knob "release" "DECAY" 0 (mdk-head1-c))
          (mdk-knob "stretch" "SPREAD" 2 (mdk-head1-c)))))

; head 2 = resonant (front) head; PORT is the hole in it
(def mdk-head2 ()
  (mdk-panel "HEAD 2"
    (mdk-num "bottom_mix" "LEVEL" false 2)
          (mdk-num "port" "PORT" false 2)
          (mdk-blank)
    (mdk-knobs-2 (mdk-knob "pitch2_ratio" "RATIO" 2 (mdk-head2-c))
          (mdk-knob "release2" "DECAY" 0 (mdk-head2-c)))))

; small blocks under the two panel columns (the 808/909 OUTPUT block
; shape): SHELL under BEATER/BEND, SHAPE + MIX under HEAD 1/HEAD 2
(def mdk-block (body)
  (eseq.effects.custom-ui-lego/ui-readout-block-small-s "" (mdk-c) 0
    (h-stack :gap 0.24 :align :end
      (box :width 0.3)
      body)))

(def mdk-shell ()
  (mdk-block
    (h-stack :gap 0.24 :align :end
      (mdk-title-w "SHELL" 3.4)
      (mdk-num-w-s "shell_pitch" "PITCH" "Hz" 0 (mdk-strip-num-w))
      (mdk-num-w-s "shell_decay" "DECAY" "ms" 0 (mdk-strip-num-w))
      (mdk-num-w-s "shell_level" "LEVEL" false 2 (mdk-strip-num-w)))))

(def mdk-shape-mix ()
  (mdk-block
    (h-stack :gap 0.24 :align :end
      (mdk-title-w "SHAPE" 3.4)
      (mdk-num-w-s "drive" "DRIVE" false 2 3.9)
      (mdk-num-w-s "tone" "TONE" false 2 3.9)
      (mdk-num-w-s "punch" "PUNCH" false 2 3.9)
      (mdk-title-w "MIX" 2.2)
      (mdk-num-w-s "level" "LEVEL" false 2 3.9))))

; TONE + BANK column: the 808/909 Kick bank surface, verbatim
(def mdk-track-options ()
  '("free" "key"))

(def mdk-bank-panel ()
  (eseq.effects.custom-ui-lego/ui-readout-panel-medium-s 0
    (v-stack :width :fill :height :fill :gap 0.06 :align :stretch
      (h-stack :width :fill :gap 0.30 :align :start
        (box :width 0.2)
        (v-stack :width 12.2 :gap 0.08 :align :start
          (box :height 0.2)
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "BANK" 4.4 :fg)
            (box :width 1.3)
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_time" "TIME" 6.0 0 "ms" (eseq.effects.custom-ui-lego/ui-accent-blue)))
          (h-stack :gap 0.18 :align :start
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_freq" "FLTR" 6.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
            (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_res" "RES" 6.0 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))))
        (h-stack :gap 0.10 :align :start
          (box :width 0.5)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "bank" "AMT" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)
          (eseq.effects.custom-ui-lego/ui-lego-knob-track-full-s 0 "bank_env" "ENV" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 2 :mixer-strip-bg)))
      (h-stack :gap 0.24 :align :end
        (box :width 0.5)
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-step-s 0 "bank_harm" "HARM" 4.2 1 0.5 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_crunch" "CRUSH" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_drive" "DRIVE" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 "bank_recon" "SMTH" 4.2 2 false (eseq.effects.custom-ui-lego/ui-accent-blue))
        (eseq.effects.custom-ui-lego/ui-lego-micro-option-s 0 "bank_track" "TRK" 4.8 (mdk-track-options) :fg)))))

(def mdk-tone-block ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (box :height :fill :padding 0.5
      (v-stack :width 12 :gap 0.18 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 "TONE" 4.4 :fg)
        ))
      (h-stack :gap 0.10 :align :start
        (eseq.effects.custom-ui-lego/ui-lego-log-knob-full-s 0 "lpf" "LPF" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)
        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s 0 "hpf" "HPF" 4.2 (eseq.effects.custom-ui-lego/ui-accent-blue) 0)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.05 :align :stretch
    (eseq.effects.custom-ui-lego/ui-lego-column
      (mdk-beater)
      (mdk-bend)
      (mdk-shell))
    (eseq.effects.custom-ui-lego/ui-lego-column
      (mdk-head1)
      (mdk-head2)
      (mdk-shape-mix))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (mdk-tone-block)
      (mdk-bank-panel))))
