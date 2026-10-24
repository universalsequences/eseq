;; mallet -felt-> string -bridge-> plate: a hammered dulcimer / cimbalom
(import eseq.modal :as m)

(m/patch :name "modal-dulcimer"
  :objects (list (m/mallet 'hammer :mass 0.05 :speed 1)
                 (m/string 'str :material :steel :length 0.5 :radius-mm 0.4 :modes 60)
                 (m/plate 'body :material :spruce :mass 20))
  :connections (list (m/felt 'felt 'hammer 'str 0.12 :felt 0.02 :hardness 0.5)
                     (m/spring 'bridge 'str 0.96 'body (list 0.5 0.4) :stiffness 2))
  :listens (list (m/listen 'str 0.3 :gain 0.6)
                 (m/listen 'body (list 0.3 0.65) :gain 0.7))
  :level 1)
