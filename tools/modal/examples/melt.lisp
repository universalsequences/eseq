;; A melt hybrid: one object whose modes morph from a steel string's (a
;; harmonic series) to a rectangular membrane's (inharmonic, drum-like) as
;; morph goes from 0 to 1, struck by a mallet. Both parts have 64 modes.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-melt"
  :objects (list (m/mallet 'mal :mass 0.02 :speed 1.2)
                 (m/melt 'h (m/string 'str :made-of mat/steel :modes 64 :const-loss 0.6 :freq-loss 0.4)
                            (m/rect-membrane 'skin :grid 8 :loss 3)
                         :morph 0.5))
  :connections (list (m/felt 'hit 'mal 'h (list 0.13 (list 0.3 0.4)) :felt 0.02))
  :listens (list (m/listen 'h (list 0.7 (list 0.6 0.7)))))
