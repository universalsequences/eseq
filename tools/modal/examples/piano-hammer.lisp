;; A felt hammer on a stiff steel string with Stulov's hysteretic felt: the
;; harder the hit, the stiffer the felt gets (exponent 2.5), so loud notes
;; are brighter than soft ones (modestly here: about twice the energy above
;; 1.5 kHz from soft to loud), as on a piano.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-piano-hammer"
  :objects (list (m/mallet 'ham :mass 0.02 :speed 1.5)
                 (m/string 'str :made-of mat/steel :length 1.2 :radius-mm 0.55 :modes 90 :const-loss 0.4 :freq-loss 0.3 :mute 20)
                 (m/plate 'board :made-of mat/spruce :mass 40 :grid 10))
  :connections (list (m/felt 'hit 'ham 'str 0.12 :felt 5 :exponent 2.5 :hysteresis 0.3 :tau-ms 0.3)
                     (m/spring 'bridge 'str 0.98 'board (list 0.4 0.55) :stiffness 0.4))
  :listens (list (m/listen 'str 0.3 :gain 0.6) (m/listen 'board (list 0.6 0.3) :gain 0.6)))
