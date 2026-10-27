;; A cymbal: a free bronze disc (its modes from the Bessel solutions of a free
;; circular plate), struck near the edge by a hard stick and heard on the bow.
;; A flat disc this size rings far lower than a real cymbal, whose dome and
;; curvature stiffen it, so tune lifts it about two and a half octaves;
;; ~110 modes carry it up into the kHz range where a cymbal's sound lives.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-cymbal"
  :objects (list (m/mallet 'stick :mass 0.02 :speed 2)
                 (m/circ-plate 'cym :edge :free :made-of mat/bronze :radius 0.2 :thick-mm 1.5
                               :modes 112 :tune 6 :loss 0.8 :damp 0.1))
  :connections (list (m/felt 'hit 'stick 'cym (list 0.85 0) :felt 0.5 :hardness 0.9))
  :listens (list (m/listen 'cym (list 0.6 0.7))))
