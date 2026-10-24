;; mallet -felt-> circular membrane (the modal-drum patch, as data)
(import eseq.modal :as m)

(m/patch :name "modal-drum-gen"
  :objects (list (m/mallet 'mallet :mass 0.01 :speed 1)
                 (m/membrane 'head :loss 6 :bright 1))
  :connections (list (m/felt 'felt 'mallet 'head (list 0.3 0)))
  :listens (list (m/listen 'head (list 0.5 0.5)))
  :level 1)
