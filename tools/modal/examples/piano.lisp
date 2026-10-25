;; A grand piano, physically: a felt hammer strikes three unison strings
;; whose bridge rests on a spruce soundboard, everything scaled across the
;; keyboard as a piano's strings and hammers are. Fitted to the 30 Salamander
;; Grand references PM Piano is calibrated from (tools/modal/README.md,
;; "Piano"): the knobs are physical, the numbers are the fit.
;;
;;   strings   speaking length from a concert grand's scale, 2 m in the bass
;;             to 5 cm at the top, and the radius that gives each register
;;             its measured inharmonicity (about 0.4 mm throughout, piano
;;             wire); three per note, detuned a cent or two, so they beat
;;             and, through the bridge they share, decay in two stages: the
;;             in-step motion drains into the board fast, the out-of-step
;;             motion barely moves the bridge and rings on (the aftersound)
;;   hammer    lighter toward the treble; its felt (Stulov) and strike point
;;             (1/8 of the string in the middle, 1/16 at the top) per register
;;   board     one board, but a treble string is some fifty times lighter
;;             than a bass string, so to the string the board is heavier up
;;             the keyboard: its mass follows the note
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-piano"
  :objects (list (m/mallet 'ham :mass (m/by-note 27.5 0.3146 65.4 1.086 130.8 1.034 261.6 0.6437 523.3 0.2579 1046 0.5555 2093 0.2249 4186 0.1735) :speed 1.5)
                 (m/string 'str :made-of mat/steel
                           :length (m/by-note 27.5 2 65.4 1.6 130.8 1.12 261.6 0.64 523.3 0.3298 1046 0.1701 2093 0.09 4186 0.05)
                           :radius-mm (m/by-note 27.5 0.441 65.4 0.4164 130.8 0.4256 261.6 0.4651 523.3 0.4114 1046 0.3855 2093 0.3587 4186 0.3631)
                           :modes 48 :mute 20
                           :const-loss (m/by-note 27.5 0.1318 65.4 0.1913 130.8 0.4259 261.6 0.08291 523.3 0.8821 1046 0.1127 2093 0.05546 4186 0.02992)
                           :freq-loss (m/by-note 27.5 0.006021 65.4 0.008067 130.8 0.01781 261.6 0.1001 523.3 0.038 1046 0.008381 2093 0.004188 4186 0.002143)
                           :unison 3 :detune (m/by-note 27.5 18.56 65.4 3.203 130.8 1.078 261.6 0.7682 523.3 1.282 1046 0.9966 2093 3.213 4186 3.73))
                 (m/plate 'board :made-of mat/spruce :thick-mm 9 :width 1.4 :height 1.1 :grid 10
                          :loss 30 :damp 0.4 :mass (m/by-note 27.5 1809 65.4 244.3 130.8 429.3 261.6 135.3 523.3 87.96 1046 115.4 2093 932.3 4186 1.545e+04)))
  :connections (list (m/felt 'hit 'ham 'str (m/by-note 27.5 0.1218 65.4 0.1338 130.8 0.2154 261.6 0.1632 523.3 0.1603 1046 0.1753 2093 0.05 4186 0.05)
                             :felt (m/by-note 27.5 0.01125 65.4 0.008227 130.8 0.009624 261.6 0.006369 523.3 0.004054 1046 0.003753 2093 0.01082 4186 0.01138) :exponent (m/by-note 27.5 2.887 65.4 3.451 130.8 2.923 261.6 3.604 523.3 3.701 1046 3.495 2093 3.412 4186 4) :hysteresis 0.3 :tau-ms 0.3)
                     (m/spring 'bridge 'str 0.99 'board (list 0.4 0.55) :stiffness (m/by-note 27.5 1.888 65.4 1.282 130.8 0.3776 261.6 1.918 523.3 1.311 1046 2.887 2093 1.66 4186 2.199)))
  :listens (list (m/listen 'board (list 0.6 0.3) :gain 1)
                 (m/listen 'str 0.3 :gain (m/by-note 27.5 0.01913 65.4 0.02435 130.8 0.03481 261.6 0.02083 523.3 0.06876 1046 0.09552 2093 0.365 4186 0.1622))))
