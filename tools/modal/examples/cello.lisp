;; bow -> string -bridge-> plate: a bowed string on a wooden body. The bridge
;; sits at the string's very end (0.99): a stiff point further in pins the
;; string a second time, its modes stop being harmonic and the bow can no
;; longer lock into Helmholtz motion (0.96 measured periodicity 0.3-0.5).
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-cello"
  :objects (list (m/string 'str :made-of mat/nylon :length 0.69 :radius-mm 0.7 :modes 60
                                :const-loss 0.5 :freq-loss 0.4 :mute 0)
                 (m/plate 'body :made-of mat/spruce :width 0.75 :height 0.45 :thick-mm 4 :mass 40))
  :connections (list (m/bow 'bow 'str 0.12)
                     (m/spring 'bridge 'str 0.99 'body (list 0.55 0.4) :stiffness 0.3))
  :listens (list (m/listen 'str 0.85 :gain 0.5)
                 (m/listen 'body (list 0.3 0.65) :gain 0.7)))
