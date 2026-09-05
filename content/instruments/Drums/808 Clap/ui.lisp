;; Factory ID808 Clap — the identified R-8 '808Clap' with a p-lock surface.
;; Left column: the departure knobs (all no-ops at their defaults, so the
;; instrument boots up AS the sample). Right column: the identified scalars,
;; grouped as the voice is built — BURSTS, TAIL, FILTERS, OUTPUT.

(def idclap-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def idclap-id-c () (eseq.effects.custom-ui-lego/ui-accent-blue))
(def idclap-num-w () 4.6)

(def idclap-title (title)
  (box :width (idclap-num-w) :height 0.82 :v-align :end
    (label title :font-size 9.2 :width (idclap-num-w) :height 0.82 :color :fg :bg :transparent)))
(def idclap-num (name title unit decimals)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title (idclap-num-w) decimals unit (idclap-id-c)))
(def idclap-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title 4.55 (idclap-c) decimals))
(def idclap-blank () (box :width (idclap-num-w) :height 1.84))

; departures: two panels of four knobs each
(def idclap-play-panel ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (idclap-num-w) :gap 0.18 :align :start
        (idclap-title "PLAY")
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 (idclap-num-w) (idclap-c)))
      (box :width :fill :height 0.1)
      (h-stack :gap 0.10 :align :start
        (idclap-knob "tune" "TUNE" 1)
        (idclap-knob "flam" "FLAM" 2)
        (idclap-knob "snap" "SNAP" 2)
        (idclap-knob "body" "BODY" 2)))))

(def idclap-shape-panel ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (idclap-num-w) :gap 0.18 :align :start
        (idclap-title "SHAPE")
        (idclap-blank))
      (box :width :fill :height 0.1)
      (h-stack :gap 0.10 :align :start
        (idclap-knob "decay" "DECAY" 2)
        (idclap-knob "bright" "BRIGHT" 2)
        (idclap-knob "drive" "DRIVE" 2)
        (idclap-knob "level" "LEVEL" 2)))))

; identified scalars: dense 4-column number grids
(def idclap-grid-panel (title row1 row2)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (idclap-title title) row1)
        row2))))

(def idclap-bursts-panel ()
  (idclap-grid-panel "BURSTS"
      (h-stack :gap 0.18 :align :start
        (idclap-num "sp1" "SP1" "ms" 2) (idclap-num "sp2" "SP2" "ms" 2) (idclap-num "sp3" "SP3" "ms" 2))
      (h-stack :gap 0.18 :align :start
        (idclap-num "bdecay" "BDEC" false 0) (idclap-num "l2" "L2" false 2) (idclap-num "l3" "L3" false 2) (idclap-num "l4" "L4" false 2))))

(def idclap-tail-panel ()
  (idclap-grid-panel "TAIL"
      (h-stack :gap 0.18 :align :start
        (idclap-num "tail_a1" "FAST" false 2) (idclap-num "tail_d1" "FDEC" false 1) (idclap-num "burst_amp" "BAMP" false 2))
      (h-stack :gap 0.18 :align :start
        (idclap-num "tail_a2" "SLOW" false 2) (idclap-num "tail_d2" "SDEC" false 1) (idclap-num "tail_lpf" "TLPF" "Hz" 0) (idclap-num "sub_gain" "SUB" false 2))))

(def idclap-filter-panel ()
  (idclap-grid-panel "FILTERS"
      (h-stack :gap 0.18 :align :start
        (idclap-num "fc1" "FC1" "Hz" 0) (idclap-num "q1" "Q1" false 2) (idclap-num "fc2" "FC2" "Hz" 0))
      (h-stack :gap 0.18 :align :start
        (idclap-num "q2" "Q2" false 2) (idclap-num "g2" "G2" false 2) (idclap-num "hp_fc" "HPF" "Hz" 0) (idclap-num "sub_delay" "SDLY" "ms" 2))))

(def idclap-output-panel ()
  (idclap-grid-panel "OUTPUT"
      (h-stack :gap 0.18 :align :start
        (idclap-num "b_hp" "BHP" false 2) (idclap-num "t_hp" "THP" false 2) (idclap-num "out_hp" "OHP" "Hz" 0))
      (h-stack :gap 0.18 :align :start
        (idclap-num "out_drive" "ODRV" false 3) (idclap-num "out_gain" "OGAIN" false 3) (idclap-blank) (idclap-blank))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (idclap-play-panel)
      (idclap-shape-panel))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (idclap-bursts-panel)
      (idclap-tail-panel))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (idclap-filter-panel)
      (idclap-output-panel))))
