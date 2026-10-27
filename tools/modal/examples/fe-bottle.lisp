;; A bronze bell solved by finite elements: a solid of revolution (radius,
;; height, wall thickness down its profile) meshed and solved for its modes the
;; first time this is evaluated (then cached), struck at the lip by a hard
;; clapper. Its pitch is its own; root 524 (its first mode, a C5) makes it
;; follow the played note instead.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-fe-bottle"
  :objects (list (m/mallet 'clapper :mass 0.05 :speed 2)
                 (m/solid 'bell :made-of mat/aluminium :modes 40 :root 524 :loss 1 :hf-loss 0.2 :mass 4
                          :shape (dict :difference (list (dict :cylinder (dict :axis :z :center (list -0.0127 0.0279 -0.0129) :height 0.5 :radius 0.1))
                                                         (dict :cylinder (dict :axis :z :center (list -0.0117 0.0277 0.0069) :height 0.46 :radius 0.0733))
                                                         (dict :cylinder (dict :axis :y :center (list -0.0527 0.0279 0.0353) :height 0.32 :radius 0.022))
                                                         (dict :cylinder (dict :axis :y :center (list 0.0273 0.0279 0.0353) :height 0.32 :radius 0.022))))))
  :connections (list (m/felt 'hit 'clapper 'bell (list 0.148 0 0.004) :felt 0.3 :hardness 0.9))
  :listens (list (m/listen 'bell (list 0.09 0.05 0.2))))
