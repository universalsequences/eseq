; Membrane Snare MK2 UI — the drums/modal-snare control surface over the
; finite-difference engine. Two 2-panel columns (STRIKE/STROKE, HEAD 1/HEAD 2)
; over one wide strip (RIM + SHAPE + MIX), with WIRES as a full-height column
; on the right in the digidrift filter style.
;
; The point of the layout, versus membrane-snare-rim's: the heads get a panel
; each instead of sharing one crowded HEADS box, and the old BODY panel and
; its six body_n_freq / body_n_gain controls are gone entirely. What replaced
; them is the SHAPE section — LOWCUT, DRIVE, TONE, PUNCH — four controls that
; each move the sound a lot, where the three body resonators barely registered.
;
; Section titles are plain :fg text; knobs use the default cyan accent except
; the two head panels, which get a colour each so head 1 and head 2 read apart
; at a glance. Param names match dsp.lisp; rim_drive is DSP-only.

(def mk2-num-w () 4.6)
(def mk2-strip-num-w () 4.5)
(def mk2-wire-num-w () 5.6)
(def mk2-grid-w () (+ (* 2 (mk2-num-w)) 0.18))
(def mk2-knob-w () 4.55)
(def mk2-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def mk2-head1-c () (eseq.effects.custom-ui-lego/ui-accent-green))
(def mk2-head2-c () (eseq.effects.custom-ui-lego/ui-accent-violet))

(def mk2-title-w (title w)
  (box :width w :height 0.82 :v-align :end
    (label title :font-size 9.2 :width w :height 0.82 :color :fg :bg :transparent)))
(def mk2-title (title) (mk2-title-w title (mk2-num-w)))
(def mk2-num-w-s (name title unit decimals w)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title w decimals unit (mk2-c)))
(def mk2-num (name title unit decimals)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title (mk2-num-w) decimals unit (mk2-c)))
(def mk2-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title (mk2-knob-w) color decimals))
; same height as a micro-num (label 0.78 + gap 0.06 + picker 1.0) so a panel
; with empty grid slots keeps its title at the same vertical spot as a full one
(def mk2-blank () (box :width (mk2-num-w) :height 1.84))

(def mk2-strip-w () (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-col-w)) 0.35))
(def mk2-strip (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (mk2-strip-w) (eseq.effects.custom-ui-lego/ui-lego-small-h)
    section :instrument-control-bg body))

; panel skeleton: title + up to 3 pickers in a 2x2 grid on the left, knobs right
(def mk2-panel (title n1 n2 n3 knobs)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (mk2-grid-w) :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (mk2-title title) n1)
        (h-stack :gap 0.18 :align :start n2 n3))
      (box :width :fill :height 0.1)
      knobs)))
(def mk2-knobs-2 (a b) (h-stack :gap 0.10 :align :start a b))
(def mk2-knobs-3 (a b c) (h-stack :gap 0.10 :align :start a b c))

; stick: hardness / speed set how hard the head is hit, scrape adds noise to
; the contact, bright tilts the mic from displacement toward acceleration
(def mk2-strike ()
  (mk2-panel "STRIKE"
    (mk2-num "scrape" "SCRAPE" false 2)
          (mk2-num "bright" "BRIGHT" false 2)
          (mk2-num "bend" "BEND" false 2)
    (mk2-knobs-2 (mk2-knob "stick_hard" "HARD" 3 (mk2-c))
          (mk2-knob "stick_speed" "SPEED" 3 (mk2-c)))))

; stroke: 0 ghost / 0.5 open / 1 rimshot; press = palm on the head.
;
; The grid slot holds an X/Y pad for the strike position. membrane-snare-rim
; put a painted 6x6 tensor matrix here, which is the raw state of the model
; rather than anything a player does; this is the thing a drummer actually
; varies — where on the head the stick lands — and the DSP builds the mask
; from the two numbers.
(def mk2-stroke ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (h-stack :width (mk2-grid-w) :gap 0.18 :align :center
        (mk2-title "STROKE")
        (eseq.effects.custom-ui-lego/ui-lego-xy-pad-s 0 "strike_x" "strike_y" "HIT" 4.6 3.2 (mk2-c)))
      (box :width :fill :height 0.1)
      (mk2-knobs-2 (mk2-knob "stroke" "STROKE" 2 (mk2-c))
            (mk2-knob "press" "PRESS" 2 (mk2-c))))))

; head 1 = batter (struck) head. SPREAD is the bending-stiffness inharmonicity:
; the fundamental stays put and the partials above it stretch sharp.
(def mk2-head1 ()
  (mk2-panel "HEAD 1"
    (mk2-num "tilt" "TILT" false 2)
          (mk2-blank)
          (mk2-num "head_couple" "COUPLE" false 2)
    (mk2-knobs-3 (mk2-knob "tune" "TUNE" 1 (mk2-head1-c))
          (mk2-knob "release" "DECAY" 0 (mk2-head1-c))
          (mk2-knob "stretch" "SPREAD" 2 (mk2-head1-c)))))

; head 2 = resonant (bottom) head; ratio is its pitch relative to head 1
(def mk2-head2 ()
  (mk2-panel "HEAD 2"
    (mk2-num "bottom_mix" "LEVEL" false 2)
          (mk2-blank)
          (mk2-blank)
    (mk2-knobs-2 (mk2-knob "pitch2_ratio" "RATIO" 2 (mk2-head2-c))
          (mk2-knob "release2" "DECAY" 0 (mk2-head2-c)))))

; WIRES: one full-height column spanning both panel rows and the strip, with
; three oversized knobs on a wide pitch so the labels never shrink.
;
; DAMP takes the third knob rather than TENSION. DAMP is the contact
; pseudo-loss — how much energy the head sheds under a riding wire — and it is
; the control that actually decides whether the buzz rides the head or chokes,
; sweeping the late buzz energy over roughly 50 dB. TENSION only moves the
; strainer gap, so it changes how readily the wires engage and then stops
; mattering; it keeps a picker in the bottom row.
(def mk2-wires-w () 18.0)
(def mk2-wires-h ()
  (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-small-h)
     (* 2 (eseq.effects.custom-ui-lego/ui-lego-gap))))
(def mk2-big-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 5.6 3.12 3.12 (mk2-c) decimals))
(def mk2-wires ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (mk2-wires-w) (mk2-wires-h) 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.30 :align :start
        (h-stack :gap 0.22 :align :start
          (mk2-title "WIRES")
          (mk2-num-w-s "wire_decay" "DECAY" "ms" 0 6.0))
        (h-stack :gap 0.10 :align :start
          (mk2-big-knob "snares" "SNARES" 2)
          (mk2-big-knob "wire_pitch" "PITCH" 0)
          (mk2-big-knob "contact_loss" "DAMP" 2))
        (h-stack :gap 0.22 :align :end
          (mk2-num-w-s "rattle" "RATTLE" false 0 (mk2-wire-num-w))
          (mk2-num-w-s "wire_couple" "COUPLE" false 3 (mk2-wire-num-w))
          (mk2-num-w-s "snare_tension" "TENSION" false 2 (mk2-wire-num-w)))))))

; bottom strip under the two panel columns: RIM + SHAPE + MIX.
; LOWCUT leads SHAPE because it runs first in the DSP chain: it takes the low
; end out before DRIVE can turn it into mud.
(def mk2-shape-mix ()
  (mk2-strip 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (mk2-title-w "RIM" 2.6)
      (mk2-num-w-s "rim_pitch" "PITCH" "Hz" 0 (mk2-strip-num-w))
      (mk2-num-w-s "rim_decay" "DECAY" "ms" 0 (mk2-strip-num-w))
      (mk2-num-w-s "rim_level" "LEVEL" false 2 (mk2-strip-num-w))
      (mk2-title-w "SHAPE" 3.6)
      (mk2-num-w-s "lowcut" "LOWCUT" "Hz" 0 (mk2-strip-num-w))
      (mk2-num-w-s "drive" "DRIVE" false 2 (mk2-strip-num-w))
      (mk2-num-w-s "tone" "TONE" false 2 (mk2-strip-num-w))
      (mk2-num-w-s "punch" "PUNCH" false 2 (mk2-strip-num-w))
      (mk2-title-w "MIX" 2.6)
      (mk2-num-w-s "level" "LEVEL" false 2 (mk2-strip-num-w)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (v-stack :gap (eseq.effects.custom-ui-lego/ui-lego-gap) :align :start
      (h-stack :gap 0.35 :align :stretch
        (eseq.effects.custom-ui-lego/ui-lego-column-2
          (mk2-strike)
          (mk2-stroke))
        (eseq.effects.custom-ui-lego/ui-lego-column-2
          (mk2-head1)
          (mk2-head2)))
      (mk2-shape-mix))
    (mk2-wires)))
