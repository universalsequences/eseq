; Modal Kick UI — lego-s style, the modal-snare layout minus the WIRES
; column. Two 2-panel columns (BEATER/BEND, HEAD 1/HEAD 2) over one strip
; (SHELL + SHAPE + MIX). Section titles are plain :fg text; knobs use the
; default cyan accent except the two head panels, which get their own colour
; so head 1 and head 2 controls read apart at a glance.
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

(def mdk-strip-w () (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-col-w)) 0.35))
(def mdk-strip (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (mdk-strip-w) (eseq.effects.custom-ui-lego/ui-lego-small-h)
    section :instrument-control-bg body))

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

; bottom strip under the two panel columns: SHELL + SHAPE + MIX
(def mdk-shape-mix ()
  (mdk-strip 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (mdk-title-w "SHELL" 3.4)
      (mdk-num-w-s "shell_pitch" "PITCH" "Hz" 0 (mdk-strip-num-w))
      (mdk-num-w-s "shell_decay" "DECAY" "ms" 0 (mdk-strip-num-w))
      (mdk-num-w-s "shell_level" "LEVEL" false 2 (mdk-strip-num-w))
      (mdk-title-w "SHAPE" 4.0)
      (mdk-num-w-s "drive" "DRIVE" false 2 (mdk-strip-num-w))
      (mdk-num-w-s "tone" "TONE" false 2 (mdk-strip-num-w))
      (mdk-num-w-s "punch" "PUNCH" false 2 (mdk-strip-num-w))
      (mdk-title-w "MIX" 2.6)
      (mdk-num-w-s "level" "LEVEL" false 2 (mdk-strip-num-w)))))

(defsynth-ui
  (v-stack :gap (eseq.effects.custom-ui-lego/ui-lego-gap) :align :start
    (h-stack :gap 0.35 :align :stretch
      (eseq.effects.custom-ui-lego/ui-lego-column-2
        (mdk-beater)
        (mdk-bend))
      (eseq.effects.custom-ui-lego/ui-lego-column-2
        (mdk-head1)
        (mdk-head2)))
    (mdk-shape-mix)))
