;; A tissue-box guitar: the school craft project, built from two
;; finite-element solids glued together at named points. Small and coarse on
;; purpose, so it solves in seconds and is quick to edit in the Shape Lab.
;;
;;   box    a cardboard tissue box, hollow, with a round hole in its top.
;;          Its walls are 6 mm, one mesh cell thick; a real box's are about
;;          1 mm. A wall's bending modes rise in proportion to its thickness
;;          (same shapes, scaled pitch), so :tune 0.2 puts them where 1.2 mm
;;          walls would ring. Without it the box clanks at 700 Hz and up
;;   roll   a toilet-paper roll, the neck, glued by one end to the box's end
;;   str    a rubber band from a bridge on the box top, plucked over the
;;          hole: lossy, so it thunks and dies in under half a second
;;
;; The parts and how they are joined:
;;
;;   roll :heel ──'joint (spring)──  box :neck-end     roll sits on the box
;;   str 0.96   ──'bridge (spring)─  box :bridge       band and box drive each
;;                                                     other: the box rings
;;
;; Springs push both ways and keep the energy honest. A mount (m/mount, as
;; the hollow body's nut) only pushes one way, so a mount that closes a loop
;; (band -> box -> roll -> back into the band) feeds energy in and runs
;; away; the band's other end is simply held still here.
;;
;; Coordinates are metres: x along the guitar toward the roll, y across it,
;; z up. The box is centred on the origin, so its top is at z = 0.048. The
;; roll has no :place: the 'joint spring seats it, putting its :heel on the
;; box's :neck-end.
;;
;; Try it:
;;   1. C-c C-c plays it on a draft track (the first run solves both solids,
;;      then they are cached).
;;   2. M-x Shape Lab opens the box. Pick Instrument above the 3-D view to see
;;      the roll seated on it; drag to orbit.
;;   3. Change the box: drag the hole with Move, add a second hole with Hole,
;;      or turn the hole into a slot. Each edit re-meshes at once and
;;      re-solves in the background; the 3-D view swings through the modes.
;;   4. Move :bridge with Point/Move, then Update draft and play again.
;;   5. Switch to the roll's tab and make it longer or thicker-walled. Its
;;      modes come in equal pairs (a tube bends the same way up and across).
;;   6. Turn the box's Tune knob back to 1 to hear the 6 mm box clank.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-fe-tissue-box-guitar"
  :objects (list (m/finger 'fin :depth 0.1)
                 (m/string 'str :made-of mat/rubber :length 0.3 :radius-mm 1 :modes 30
                           :const-loss 6 :freq-loss 5)
                 (m/solid 'box :made-of mat/cardboard :modes 24 :mass 20 :tune 0.2
                          :resolution (dict :element-size 0.006)
                          :shape (dict :difference (list (dict :box (dict :size (list 0.24 0.12 0.096)))
                                                         ;; hollow it out, leaving 6 mm walls
                                                         (dict :box (dict :size (list 0.228 0.108 0.084)))
                                                         ;; the hole in the top
                                                         (dict :cylinder (dict :axis :z :center (list -0.01 0 0.048) :height 0.03 :radius 0.035))))
                          :points (dict :bridge (list -0.075 0 0.048)
                                        :hole-rim (list -0.01 0.042 0.048)
                                        :neck-end (list 0.12 0 0.03)
                                        :side (list 0 -0.06 0)))
                 (m/solid 'roll :made-of mat/cardboard :modes 16 :mass 8
                          :resolution (dict :element-size 0.004)
                          :shape (dict :tube (dict :axis :x :center (list 0.05 0 0) :height 0.1
                                                   :outer-radius 0.022 :inner-radius 0.0205))
                          :points (dict :heel (list 0 0 0.022)
                                        :nut (list 0.1 0 0.022))))
  :connections (list (m/pluck 'pick 'fin 'str 0.8)
                     (m/spring 'bridge 'str 0.96 'box :bridge :stiffness 2)
                     (m/spring 'joint 'roll :heel 'box :neck-end :stiffness 40))
  :listens (list (m/listen 'str 0.2 :gain 0.5)
                 (m/listen 'box :hole-rim :gain 0.8)
                 (m/listen 'roll :nut :gain 0.3))
  :level 1)
