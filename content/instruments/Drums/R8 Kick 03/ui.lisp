;; Roland R-8 Kick03.
;;
;; Four columns, each within the 10.8-row instrument panel. PLAY / SHAPE on
;; the left are the departure knobs, all no-ops at their defaults so the
;; instrument boots AS the sample. Next the five inharmonic MEMBRANE modes, one
;; row each (FREQ / LEVEL / DECAY / GLIDE). Then the shared tension GLIDE and
;; the CLICK / AIR layers. On the right the RING bank (eight fixed-pitch shell /
;; beater modes) over the identified saturator.
;;
;; Naming follows id808 / idvb23: the badge on the left of a row carries the
;; noun, the fields carry the plain role. Field widths are set from the widest
;; string the parameter's range can print (~0.6 cells per glyph at font-size
;; 9.5) so no number runs into its neighbour.

(def idr8k-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def idr8k-id-c () (eseq.effects.custom-ui-lego/ui-accent-blue))

;; Panel heights: a row of number fields measures 1.64, and a panel's first
;; row starts 1.0 below its top edge.
(def idr8k-h1 () 3.08)
(def idr8k-h2 () 4.90)
(def idr8k-h3 () 6.72)
(def idr8k-h4 () 8.54)
(def idr8k-knob-h () 3.40)

(def idr8k-n (name title width decimals unit)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title width decimals unit (idr8k-id-c)))
(def idr8k-badge (title)
  (eseq.effects.custom-ui-lego/ui-lego-badge-s 0 title 4.0 :fg))
(def idr8k-gap (width)
  (box :width width :height 1.64))
(def idr8k-knob (name title)
  (eseq.effects.custom-ui-lego/ui-lego-knob-sized-s 0 name title 4.6
    (idr8k-knob-h) (idr8k-knob-h) (idr8k-c) 2))

(def idr8k-panel (width height body)
  (eseq.effects.custom-ui-lego/ui-lego-panel-width-s width height 0 :instrument-group-bg
    (box :width :fill :height :fill :v-align :start body)))

;; ---- column 1: the departure knobs ------------------------------------------

(def idr8k-play-panel ()
  (idr8k-panel 34 5.70
    (h-stack :width :fill :gap 0.15 :align :center
      (v-stack :width 4.6 :gap 0.18 :align :start
        (idr8k-badge "PLAY")
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 4.6 (idr8k-c)))
      (idr8k-knob "tune" "TUNE")
      (idr8k-knob "glide" "GLIDE")
      (idr8k-knob "attack" "ATTACK")
      (idr8k-knob "decay" "DECAY")
      (idr8k-knob "drive" "DRIVE")
      (idr8k-knob "level" "LEVEL"))))

(def idr8k-shape-panel ()
  (idr8k-panel 34 4.90
    (h-stack :width :fill :gap 0.15 :align :center
      (v-stack :width 4.6 :gap 0.18 :align :start
        (idr8k-badge "SHAPE")
        (idr8k-gap 4.6))
      (idr8k-knob "knock" "KNOCK")
      (idr8k-knob "ring" "RING")
      (idr8k-knob "rattle" "RATTLE")
      (idr8k-knob "noise" "CLICK")
      (idr8k-knob "hiss" "AIR"))))

;; ---- column 2: the membrane --------------------------------------------------

;; The five inharmonic membrane modes, one row each: base FREQ (where the mode
;; lands once the glide has settled), LEVEL, DECAY (1/s) and how much of the
;; shared GLIDE the mode follows (1 = the full tension curve), and its initial
;; PHASE in cycles (audible through the saturator as the shape of the humps).
(def idr8k-mode-row (badge f-name a-name d-name g-name p-name)
  (h-stack :gap 0.18 :align :start
    (idr8k-badge badge)
    (idr8k-n f-name "FREQ" 4.6 2 "Hz")
    (idr8k-n a-name "LEVEL" 4.6 4 false)
    (idr8k-n d-name "DECAY" 4.6 1 false)
    (idr8k-n g-name "GLIDE" 4.6 3 false)
    (idr8k-n p-name "PHASE" 4.6 3 false)))

(def idr8k-membrane-panel ()
  (idr8k-panel 29 10.20
    (v-stack :width :fill :gap 0.18 :align :start
      (idr8k-mode-row "MODE 1" "lf1" "la1" "ld1" "lg1" "lp1")
      (idr8k-mode-row "MODE 2" "lf2" "la2" "ld2" "lg2" "lp2")
      (idr8k-mode-row "MODE 3" "lf3" "la3" "ld3" "lg3" "lp3")
      (idr8k-mode-row "MODE 4" "lf4" "la4" "ld4" "lg4" "lp4")
      (idr8k-mode-row "MODE 5" "lf5" "la5" "ld5" "lg5" "lp5"))))

;; ---- column 3: the glide and the layers --------------------------------------

;; One tension glide shared by every membrane mode: a fast SNAP over the first
;; milliseconds and a slower DROP under it, each with its RATE in 1/s. RISE is
;; the attack of both banks.
(def idr8k-glide-panel ()
  (idr8k-panel 24 (idr8k-h2)
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.18 :align :start
        (idr8k-badge "GLIDE")
        (idr8k-n "glide_a1" "SNAP" 4.8 3 false)
        (idr8k-n "glide_r1" "RATE" 4.8 1 false)
        (idr8k-n "attack_time" "RISE" 5.4 5 "s"))
      (h-stack :gap 0.18 :align :start
        (idr8k-gap 4.0)
        (idr8k-n "glide_a2" "DROP" 4.8 3 false)
        (idr8k-n "glide_r2" "RATE" 4.8 1 false)))))

;; CLICK is the lowpassed beater burst; AIR the recording hiss.
(def idr8k-layer-row (badge a-name b-name c-name)
  (h-stack :gap 0.18 :align :start
    (idr8k-badge badge)
    (idr8k-n a-name "CUT" 4.8 0 "Hz")
    (idr8k-n b-name "LEVEL" 4.8 4 false)
    (idr8k-n c-name "DECAY" 5.4 1 false)))

(def idr8k-layers-panel ()
  (idr8k-panel 24 (idr8k-h2)
    (v-stack :width :fill :gap 0.18 :align :start
      (idr8k-layer-row "CLICK" "noise_cutoff" "noise_amp" "noise_decay")
      (idr8k-layer-row "AIR" "hiss_cutoff" "hiss_amp" "hiss_decay"))))

;; ---- column 4: the ring bank and the output ----------------------------------

;; Eight fixed-pitch shell / beater modes: the woody knock of a real drum.
(def idr8k-mf (name title) (idr8k-n name title 4.6 1 "Hz"))
(def idr8k-ma (name title) (idr8k-n name title 4.6 4 false))
(def idr8k-md (name title) (idr8k-n name title 4.6 1 false))

(def idr8k-ring-panel ()
  (idr8k-panel 44 (idr8k-h3)
    (v-stack :width :fill :gap 0.18 :align :start
      (h-stack :gap 0.18 :align :start
        (idr8k-badge "FREQ")
        (idr8k-mf "mf1" "1") (idr8k-mf "mf2" "2") (idr8k-mf "mf3" "3") (idr8k-mf "mf4" "4")
        (idr8k-mf "mf5" "5") (idr8k-mf "mf6" "6") (idr8k-mf "mf7" "7") (idr8k-mf "mf8" "8"))
      (h-stack :gap 0.18 :align :start
        (idr8k-badge "LEVEL")
        (idr8k-ma "ma1" "1") (idr8k-ma "ma2" "2") (idr8k-ma "ma3" "3") (idr8k-ma "ma4" "4")
        (idr8k-ma "ma5" "5") (idr8k-ma "ma6" "6") (idr8k-ma "ma7" "7") (idr8k-ma "ma8" "8"))
      (h-stack :gap 0.18 :align :start
        (idr8k-badge "DECAY")
        (idr8k-md "md1" "1") (idr8k-md "md2" "2") (idr8k-md "md3" "3") (idr8k-md "md4" "4")
        (idr8k-md "md5" "5") (idr8k-md "md6" "6") (idr8k-md "md7" "7") (idr8k-md "md8" "8")))))

;; RATTLE is noise gated by the membrane's negative half-cycles (the buzz on
;; every trough of the real hit); OUT the identified saturator — SAT sets the
;; shape only, GAIN the level — and TRACK, how much the ring bank follows the
;; played note (1 = with the membrane; 0 = fixed shell resonance).
(def idr8k-out-panel ()
  (idr8k-panel 44 (idr8k-h1)
    (h-stack :gap 0.60 :align :start
      (h-stack :gap 0.18 :align :start
        (idr8k-badge "RATTLE")
        (idr8k-n "rattle_hp" "HP" 4.8 0 "Hz")
        (idr8k-n "rattle_amp" "LEVEL" 4.8 4 false)
        (idr8k-n "rattle_decay" "DECAY" 4.8 1 false))
      (h-stack :gap 0.18 :align :start
        (idr8k-badge "OUT")
        (idr8k-n "out_drive" "SAT" 4.8 3 false)
        (idr8k-n "out_gain" "GAIN" 4.8 3 false)
        (idr8k-n "ring_track" "TRACK" 4.8 2 false)))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (v-stack :gap 0.10 :align :start
      (idr8k-play-panel)
      (idr8k-shape-panel))
    (idr8k-membrane-panel)
    (v-stack :gap 0.10 :align :start
      (idr8k-glide-panel)
      (idr8k-layers-panel))
    (v-stack :gap 0.10 :align :start
      (idr8k-ring-panel)
      (idr8k-out-panel))))
