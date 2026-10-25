;; A bronze bell solved by finite elements: a solid of revolution (radius,
;; height, wall thickness down its profile) meshed and solved for its modes the
;; first time this is evaluated (then cached), struck at the lip by a hard
;; clapper. Its pitch is its own; root 524 (its first mode, a C5) makes it
;; follow the played note instead.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-fe-bell"
  :objects (list (m/mallet 'clapper :mass 0.05 :speed 2)
                 (m/solid 'bell :made-of mat/bronze :modes 40 :root 524 :loss 1 :hf-loss 0.2 :mass 4
                          :shape (dict :revolve
                                       (dict :profile (list (list 0.000 0.250 0.012) (list 0.040 0.250 0.012)
                                                            (list 0.070 0.245 0.010) (list 0.085 0.235 0.009)
                                                            (list 0.090 0.210 0.008) (list 0.092 0.170 0.007)
                                                            (list 0.097 0.120 0.008) (list 0.108 0.070 0.010)
                                                            (list 0.125 0.035 0.016) (list 0.140 0.012 0.020)
                                                            (list 0.150 0.000 0.016))
                                             :segments (dict :along 28 :around 52 :through 1)))))
  :connections (list (m/felt 'hit 'clapper 'bell (list 0.148 0 0.004) :felt 0.3 :hardness 0.9))
  :listens (list (m/listen 'bell (list 0.09 0.05 0.2))))
