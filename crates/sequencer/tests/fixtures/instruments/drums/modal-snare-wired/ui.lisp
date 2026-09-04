; Modal Snare Wired UI — modal-snare's layout plus the fork's three knobs:
; VISC (HEAD 1, viscous decay law), SPLIT (HEAD 2, degenerate-pair detune)
; and TONE (WIRES, second wire partial). Otherwise lego-s style. Two 2-panel columns (STRIKE/STROKE,
; HEAD 1/HEAD 2) over one strip (RIMSHOT + SHAPE + MIX), with WIRES as a
; full-height column on the right (digidrift filter style, oversized knobs).
; The strip is a local wide lego (both column widths + the gap). Section
; titles are plain :fg text; knobs use
; the default cyan accent except the two head panels, which get their own
; colour so head 1 and head 2 controls read apart at a glance.
; Param names match dsp.lisp; tip, rim_drive, palm, dbg are DSP-only.

(def msw-num-w () 4.6)
(def msw-strip-num-w () 5.0)
(def msw-wire-num-w () 5.6)
(def msw-grid-w () (+ (* 2 (msw-num-w)) 0.18))
(def msw-knob-w () 4.55)
(def msw-c () (eseq.effects.custom-ui-lego/ui-accent-cyan))
(def msw-head1-c () (eseq.effects.custom-ui-lego/ui-accent-green))
(def msw-head2-c () (eseq.effects.custom-ui-lego/ui-accent-violet))

(def msw-title-w (title w)
  (box :width w :height 0.82 :v-align :end
    (label title :font-size 9.2 :width w :height 0.82 :color :fg :bg :transparent)))
(def msw-title (title) (msw-title-w title (msw-num-w)))
(def msw-num-w-s (name title unit decimals w)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title w decimals unit (msw-c)))
(def msw-num (name title unit decimals)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title (msw-num-w) decimals unit (msw-c)))
(def msw-knob (name title decimals color)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title (msw-knob-w) color decimals))
; same height as a micro-num (label 0.78 + gap 0.06 + picker 1.0) so a panel with
; empty grid slots keeps its title at the same vertical spot as a full one
(def msw-blank () (box :width (msw-num-w) :height 1.84))

(def msw-strip-w () (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-col-w)) 0.35))
(def msw-strip (section body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (msw-strip-w) (eseq.effects.custom-ui-lego/ui-lego-small-h)
    section :instrument-control-bg body))

; panel skeleton: title + up to 3 pickers in a 2x2 grid on the left, knobs on the right
(def msw-panel (title n1 n2 n3 knobs)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (msw-grid-w) :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (msw-title title) n1)
        (h-stack :gap 0.18 :align :start n2 n3))
      (box :width :fill :height 0.1)
      knobs)))
(def msw-knobs-2 (a b) (h-stack :gap 0.10 :align :start a b))
(def msw-knobs-3 (a b c) (h-stack :gap 0.10 :align :start a b c))

; stick: hardness / speed set how hard the head is hit, scrape adds noise to
; the contact, bright tilts the mic toward the upper partials
(def msw-strike ()
  (msw-panel "STRIKE"
    (msw-num "scrape" "SCRAPE" false 2)
          (msw-num "bright" "BRIGHT" false 2)
          (msw-num "bend" "BEND" false 2)
    (msw-knobs-2 (msw-knob "stick_hard" "HARD" 3 (msw-c))
          (msw-knob "stick_speed" "SPEED" 3 (msw-c)))))

; stroke: 0 ghost / 0.5 open / 1 rimshot; press = palm on the head
(def msw-stroke ()
  (msw-panel "STROKE"
    (msw-blank) (msw-blank) (msw-blank)
    (msw-knobs-2 (msw-knob "stroke" "STROKE" 2 (msw-c))
          (msw-knob "press" "PRESS" 2 (msw-c)))))

; head 1 = batter (struck) head
(def msw-head1 ()
  (msw-panel "HEAD 1"
    (msw-num "tilt" "TILT" false 2)
          (msw-num "visc" "VISC" false 2)
          (msw-num "head_couple" "COUPLE" false 2)
    (msw-knobs-3 (msw-knob "tune" "TUNE" 1 (msw-head1-c))
          (msw-knob "release" "DECAY" 0 (msw-head1-c))
          (msw-knob "stretch" "SPREAD" 2 (msw-head1-c)))))

; head 2 = resonant (bottom) head; ratio is its pitch relative to head 1
(def msw-head2 ()
  (msw-panel "HEAD 2"
    (msw-num "bottom_mix" "LEVEL" false 2)
          (msw-num "split" "SPLIT" false 2)
          (msw-blank)
    (msw-knobs-2 (msw-knob "pitch2_ratio" "RATIO" 2 (msw-head2-c))
          (msw-knob "release2" "DECAY" 0 (msw-head2-c)))))

; WIRES: one full-height column (digidrift filter style) spanning both panel
; rows and the strip; SNARES / PITCH / TENSION are regular-size knobs on a
; wider pitch so the labels never shrink.
(def msw-wires-w () 23.4)
(def msw-wires-h ()
  (+ (* 2 (eseq.effects.custom-ui-lego/ui-lego-dense-h)) (eseq.effects.custom-ui-lego/ui-lego-small-h)
     (* 2 (eseq.effects.custom-ui-lego/ui-lego-gap))))
(def msw-big-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 5.6 3.12 3.12 (msw-c) decimals))
(def msw-wires ()
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s (msw-wires-w) (msw-wires-h) 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.30 :align :start
        (h-stack :gap 0.22 :align :start
          (msw-title "WIRES")
          (msw-num-w-s "wire_decay" "DECAY" "ms" 0 6.0))
        (h-stack :gap 0.10 :align :start
          (msw-big-knob "snares" "SNARES" 2)
          (msw-big-knob "wire_pitch" "PITCH" 0)
          (msw-big-knob "snare_tension" "TENSION" 2))
        (h-stack :gap 0.22 :align :end
          (msw-num-w-s "rattle" "RATTLE" false 1 (msw-wire-num-w))
          (msw-num-w-s "wire_kick" "BOUNCE" false 2 (msw-wire-num-w))
          (msw-num-w-s "contact_loss" "DAMP" false 3 (msw-wire-num-w))
          (msw-num-w-s "wire_tone" "TONE" false 2 (msw-wire-num-w)))))))

; bottom strip under the two panel columns: RIMSHOT + SHAPE + MIX
(def msw-shape-mix ()
  (msw-strip 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (msw-title-w "RIM" 2.6)
      (msw-num-w-s "rim_pitch" "PITCH" "Hz" 0 (msw-strip-num-w))
      (msw-num-w-s "rim_decay" "DECAY" "ms" 0 (msw-strip-num-w))
      (msw-num-w-s "rim_level" "LEVEL" false 2 (msw-strip-num-w))
      (msw-title-w "SHAPE" 4.0)
      (msw-num-w-s "drive" "DRIVE" false 2 (msw-strip-num-w))
      (msw-num-w-s "tone" "TONE" false 2 (msw-strip-num-w))
      (msw-num-w-s "punch" "PUNCH" false 2 (msw-strip-num-w))
      (msw-title-w "MIX" 2.6)
      (msw-num-w-s "level" "LEVEL" false 2 (msw-strip-num-w)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (v-stack :gap (eseq.effects.custom-ui-lego/ui-lego-gap) :align :start
      (h-stack :gap 0.35 :align :stretch
        (eseq.effects.custom-ui-lego/ui-lego-column-2
          (msw-strike)
          (msw-stroke))
        (eseq.effects.custom-ui-lego/ui-lego-column-2
          (msw-head1)
          (msw-head2)))
      (msw-shape-mix))
    (msw-wires)))
