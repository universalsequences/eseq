; Modal Snare UI — lego-s style. Two 2-panel columns (STRIKE/STROKE,
; HEAD 1/HEAD 2) over one strip (RIMSHOT + SHAPE + MIX), with WIRES as a
; full-height column on the right (digidrift filter style, oversized knobs).
; The strip is a local wide lego (both column widths + the gap). Section
; titles are plain :fg text; knobs use
; the default cyan accent except the two head panels, which get their own
; colour so head 1 and head 2 controls read apart at a glance.
; Param names match dsp.lisp; tip, wire_drive, rim_drive, dbg are DSP-only.

(def mds-num-w () 4.6)
(def mds-strip-num-w () 5.0)
(def mds-wire-num-w () 5.6)
(def mds-grid-w () (+ (* 2 (mds-num-w)) 0.18))
(def mds-knob-w () 4.55)
(def mds-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def mds-head1-c () (eseq.effects.custom-ui-lego/ui-accent-green))
(def mds-head2-c () (eseq.effects.custom-ui-lego/ui-accent-violet))

(def mds-title-w (title w)
  (box :width w :height 0.82 :v-align :end
    (label title :font-size 9.2 :width w :height 0.82 :color :fg :bg :transparent)))
(def mds-title (title) (mds-title-w title (mds-num-w)))
(def mds-num-w-s (name title unit decimals w)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title w decimals unit (mds-c)))
(def mds-num (name title unit decimals)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title (mds-num-w) decimals unit (mds-c)))
(def mds-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title (mds-knob-w) color decimals))
; same height as a micro-num (label 0.78 + gap 0.06 + picker 1.0) so a panel with
; empty grid slots keeps its title at the same vertical spot as a full one
(def mds-blank () (box :width (mds-num-w) :height 1.84))

(def mds-strip-w () (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-col-w)) 0.35))
(def mds-strip (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (mds-strip-w) (eseq.effects.custom-ui-lego/ui-lego-small-h)
    section :instrument-control-bg body))

; panel skeleton: title + up to 3 pickers in a 2x2 grid on the left, knobs on the right
(def mds-panel (title n1 n2 n3 knobs)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (mds-grid-w) :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (mds-title title) n1)
        (h-stack :gap 0.18 :align :start n2 n3))
      (box :width :fill :height 0.1)
      knobs)))
(def mds-knobs-2 (a b) (h-stack :gap 0.10 :align :start a b))
(def mds-knobs-3 (a b c) (h-stack :gap 0.10 :align :start a b c))

; stick: hardness / speed set how hard the head is hit, scrape adds noise to
; the contact, bright tilts the mic toward the upper partials
(def mds-strike ()
  (mds-panel "STRIKE"
    (mds-num "scrape" "SCRAPE" false 2)
          (mds-num "bright" "BRIGHT" false 2)
          (mds-num "bend" "BEND" false 2)
    (mds-knobs-2 (mds-knob "stick_hard" "HARD" 3 (mds-c))
          (mds-knob "stick_speed" "SPEED" 3 (mds-c)))))

; stroke: 0 ghost / 0.5 open / 1 rimshot; press = palm on the head
(def mds-stroke ()
  (mds-panel "STROKE"
    (mds-blank) (mds-blank) (mds-blank)
    (mds-knobs-2 (mds-knob "stroke" "STROKE" 2 (mds-c))
          (mds-knob "press" "PRESS" 2 (mds-c)))))

; head 1 = batter (struck) head
(def mds-head1 ()
  (mds-panel "HEAD 1"
    (mds-num "tilt" "TILT" false 2)
          (mds-blank)
          (mds-num "head_couple" "COUPLE" false 2)
    (mds-knobs-3 (mds-knob "tune" "TUNE" 1 (mds-head1-c))
          (mds-knob "release" "DECAY" 0 (mds-head1-c))
          (mds-knob "stretch" "SPREAD" 2 (mds-head1-c)))))

; head 2 = resonant (bottom) head; ratio is its pitch relative to head 1
(def mds-head2 ()
  (mds-panel "HEAD 2"
    (mds-num "bottom_mix" "LEVEL" false 2)
          (mds-blank)
          (mds-blank)
    (mds-knobs-2 (mds-knob "pitch2_ratio" "RATIO" 2 (mds-head2-c))
          (mds-knob "release2" "DECAY" 0 (mds-head2-c)))))

; WIRES: one full-height column (digidrift filter style) spanning both panel
; rows and the strip; SNARES / PITCH / TENSION are regular-size knobs on a
; wider pitch so the labels never shrink.
(def mds-wires-w () 18.0)
(def mds-wires-h ()
  (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-small-h)
     (* 2 (eseq.effects.custom-ui-lego/ui-lego-gap))))
(def mds-big-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 5.6 3.12 3.12 (mds-c) decimals))
(def mds-wires ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (mds-wires-w) (mds-wires-h) 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.30 :align :start
        (h-stack :gap 0.22 :align :start
          (mds-title "WIRES")
          (mds-num-w-s "wire_decay" "DECAY" "ms" 0 6.0))
        (h-stack :gap 0.10 :align :start
          (mds-big-knob "snares" "SNARES" 2)
          (mds-big-knob "wire_pitch" "PITCH" 0)
          (mds-big-knob "snare_tension" "TENSION" 2))
        (h-stack :gap 0.22 :align :end
          (mds-num-w-s "rattle" "RATTLE" false 1 (mds-wire-num-w))
          (mds-num-w-s "wire_kick" "BOUNCE" false 2 (mds-wire-num-w))
          (mds-num-w-s "contact_loss" "DAMP" false 3 (mds-wire-num-w)))))))

; bottom strip under the two panel columns: RIMSHOT + SHAPE + MIX
(def mds-shape-mix ()
  (mds-strip 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (mds-title-w "RIM" 2.6)
      (mds-num-w-s "rim_pitch" "PITCH" "Hz" 0 (mds-strip-num-w))
      (mds-num-w-s "rim_decay" "DECAY" "ms" 0 (mds-strip-num-w))
      (mds-num-w-s "rim_level" "LEVEL" false 2 (mds-strip-num-w))
      (mds-title-w "SHAPE" 4.0)
      (mds-num-w-s "drive" "DRIVE" false 2 (mds-strip-num-w))
      (mds-num-w-s "tone" "TONE" false 2 (mds-strip-num-w))
      (mds-num-w-s "punch" "PUNCH" false 2 (mds-strip-num-w))
      (mds-title-w "MIX" 2.6)
      (mds-num-w-s "level" "LEVEL" false 2 (mds-strip-num-w)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (v-stack :gap (eseq.effects.custom-ui-lego/ui-lego-gap) :align :start
      (h-stack :gap 0.35 :align :stretch
        (eseq.effects.custom-ui-lego/ui-lego-column-2
          (mds-strike)
          (mds-stroke))
        (eseq.effects.custom-ui-lego/ui-lego-column-2
          (mds-head1)
          (mds-head2)))
      (mds-shape-mix))
    (mds-wires)))
