;; Factory 909 Open Hat — the identified TR-909 open hat with a p-lock surface.
;; Left column: the departure knobs (all no-ops at their defaults, so the
;; instrument boots up AS the sample). Right columns: the identified scalars,
;; grouped as the voice is built — WASH, ENV (with click and output), and the
;; twelve metal MODES (frequency / decay / gain).

(def idhat-c () (eseq.effects.custom-ui-lego/ui-accent-orange))
(def idhat-id-c () (eseq.effects.custom-ui-lego/ui-accent-blue))
(def idhat-num-w () 4.6)

(def idhat-title (title)
  (box :width (idhat-num-w) :height 0.82 :v-align :end
    (label title :font-size 9.2 :width (idhat-num-w) :height 0.82 :color :fg :bg :transparent)))
(def idhat-num (name title unit decimals)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title (idhat-num-w) decimals unit (idhat-id-c)))
(def idhat-knob (name title decimals)
  (eseq.effects.custom-ui-lego/ui-lego-knob-s 0 name title 4.55 (idhat-c) decimals))
(def idhat-blank () (box :width (idhat-num-w) :height 1.84))
; the mode grids are six wide: narrower numbers
(def idhat-mode-w () 3.62)
(def idhat-mnum (name title unit decimals)
  (eseq.effects.custom-ui-lego/ui-lego-micro-num-s 0 name title (idhat-mode-w) decimals unit (idhat-id-c)))

; departures: two panels of knobs
(def idhat-play-panel ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (idhat-num-w) :gap 0.18 :align :start
        (idhat-title "PLAY")
        (eseq.effects.custom-ui-lego/ui-lego-micro-base-note-s 0 (idhat-num-w) (idhat-c)))
      (box :width :fill :height 0.1)
      (h-stack :gap 0.10 :align :start
        (idhat-knob "tune" "TUNE" 1)
        (idhat-knob "decay" "DECAY" 2)
        (idhat-knob "metal" "METAL" 2)
        (idhat-knob "wash" "WASH" 2)))))

(def idhat-shape-panel ()
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (h-stack :width :fill :height :fill :gap 0.30 :align :center
      (v-stack :width (idhat-num-w) :gap 0.18 :align :start
        (idhat-title "SHAPE")
        (idhat-blank))
      (box :width :fill :height 0.1)
      (h-stack :gap 0.10 :align :start
        (idhat-knob "bright" "BRIGHT" 2)
        (idhat-knob "drive" "DRIVE" 2)
        (idhat-knob "level" "LEVEL" 2)
        (idhat-knob "swish" "SWISH" 2)))))

; identified scalars: dense number grids
(def idhat-grid-panel (title row1 row2)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (idhat-title title) row1)
        row2))))
(def idhat-grid-panel-3 (title row1 row2 row3)
  (eseq.effects.custom-ui-lego/ui-control-panel-dense-s 0
    (box :width :fill :height :fill :v-align :start
      (v-stack :width :fill :gap 0.18 :align :start
        (h-stack :gap 0.18 :align :start (idhat-title title) row1)
        row2
        row3))))

(def idhat-wash-panel ()
  (idhat-grid-panel "WASH"
      (h-stack :gap 0.18 :align :start
        (idhat-num "fc1" "FC1" "Hz" 0) (idhat-num "q1" "Q1" false 2) (idhat-num "fc2" "FC2" "Hz" 0) (idhat-num "q2" "Q2" false 2))
      (h-stack :gap 0.18 :align :start
        (idhat-num "g2" "G2" false 2) (idhat-num "hp_fc" "HPF" "Hz" 0) (idhat-num "g_hp" "GHP" false 2) (idhat-num "wash_amp" "WAMP" false 2) (idhat-blank))))

(def idhat-env-panel ()
  (idhat-grid-panel-3 "ENV"
      (h-stack :gap 0.18 :align :start
        (idhat-num "atk" "ATK" "ms" 2) (idhat-num "hold" "HOLD" "ms" 1) (idhat-num "d_tail" "TDEC" false 1) (idhat-num "d_mode" "MDEC" false 1))
      (h-stack :gap 0.18 :align :start
        (idhat-num "a_fast" "FAST" false 2) (idhat-num "d_fast" "FDEC" false 1) (idhat-num "click_amp" "CLK" false 2) (idhat-num "click_decay" "CDEC" false 0) (idhat-num "click_fc" "CFC" "Hz" 0))
      (h-stack :gap 0.18 :align :start
        (idhat-num "sw_rate" "SWRT" "Hz" 1) (idhat-num "sw_amp" "SWAMP" false 1) (idhat-num "out_hp" "OHP" "Hz" 0) (idhat-num "out_drive" "ODRV" false 3) (idhat-num "out_gain" "OGAIN" false 3))))

(def idhat-modes-a-panel ()
  (idhat-grid-panel-3 "MODES 1-6"
      (h-stack :gap 0.18 :align :start (idhat-mnum "m1f" "F1" "Hz" 0) (idhat-mnum "m2f" "F2" "Hz" 0) (idhat-mnum "m3f" "F3" "Hz" 0) (idhat-mnum "m4f" "F4" "Hz" 0) (idhat-mnum "m5f" "F5" "Hz" 0) (idhat-mnum "m6f" "F6" "Hz" 0))
      (h-stack :gap 0.18 :align :start (idhat-mnum "m1d" "D1" false 1) (idhat-mnum "m2d" "D2" false 1) (idhat-mnum "m3d" "D3" false 1) (idhat-mnum "m4d" "D4" false 1) (idhat-mnum "m5d" "D5" false 1) (idhat-mnum "m6d" "D6" false 1))
      (h-stack :gap 0.18 :align :start (idhat-mnum "m1g" "G1" false 3) (idhat-mnum "m2g" "G2" false 3) (idhat-mnum "m3g" "G3" false 3) (idhat-mnum "m4g" "G4" false 3) (idhat-mnum "m5g" "G5" false 3) (idhat-mnum "m6g" "G6" false 3))))

(def idhat-modes-b-panel ()
  (idhat-grid-panel-3 "MODES 7-12"
      (h-stack :gap 0.18 :align :start (idhat-mnum "m7f" "F7" "Hz" 0) (idhat-mnum "m8f" "F8" "Hz" 0) (idhat-mnum "m9f" "F9" "Hz" 0) (idhat-mnum "m10f" "F10" "Hz" 0) (idhat-mnum "m11f" "F11" "Hz" 0) (idhat-mnum "m12f" "F12" "Hz" 0))
      (h-stack :gap 0.18 :align :start (idhat-mnum "m7d" "D7" false 1) (idhat-mnum "m8d" "D8" false 1) (idhat-mnum "m9d" "D9" false 1) (idhat-mnum "m10d" "D10" false 1) (idhat-mnum "m11d" "D11" false 1) (idhat-mnum "m12d" "D12" false 1))
      (h-stack :gap 0.18 :align :start (idhat-mnum "m7g" "G7" false 3) (idhat-mnum "m8g" "G8" false 3) (idhat-mnum "m9g" "G9" false 3) (idhat-mnum "m10g" "G10" false 3) (idhat-mnum "m11g" "G11" false 3) (idhat-mnum "m12g" "G12" false 3))))

(defsynth-ui
  (h-stack :width :fill :gap 0.35 :align :start
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (idhat-play-panel)
      (idhat-shape-panel))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (idhat-wash-panel)
      (idhat-env-panel))
    (eseq.effects.custom-ui-lego/ui-lego-column-2
      (idhat-modes-a-panel)
      (idhat-modes-b-panel))))
