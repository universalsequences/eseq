;; A ride cymbal from its geometry: a bronze bell, bow and edge revolved from
;; a wall profile (radius, height, thickness; 20 inch, 3 mm at the bell to
;; 1.2 mm at the edge), clamped at the stand's hole, solved by finite
;; elements the first time this is evaluated (about 20 s; then cached).
;; Struck on the bow by a hard stick, heard above the bow; the panel shows
;; the cymbal in 3-D, moving through its lowest modes (drag to orbit). Its
;; 240 modes reach about 4 kHz: the low, gong-like body of a ride. The
;; measured ride (ride-kit.lisp) carries the sizzling top a solve this size
;; cannot.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-fe-ride" :voicing :one :level 6 :tone 20000
  :objects (list (m/mallet 'stick :mass 0.005 :speed 2)
                 (m/solid 'ride :made-of mat/bronze :modes 240 :loss 0.6 :hf-loss 0.05
                          :shape (dict :revolve
                                       (dict :profile (list (list 0.006 0.046 0.003) (list 0.030 0.045 0.003)
                                                            (list 0.050 0.037 0.0028) (list 0.062 0.024 0.0025)
                                                            (list 0.080 0.020 0.0022) (list 0.120 0.015 0.0019)
                                                            (list 0.180 0.008 0.0016) (list 0.254 0.000 0.0012))
                                             :segments (dict :along 56 :around 128 :through 1)))
                          :block (dict :sphere (dict :center (list 0 0 0.046) :radius 0.008))))
  :connections (list (m/felt 'hit 'stick 'ride (list 0.17 0 0.012) :felt 2 :exponent 1.5))
  :listens (list (m/listen 'ride (list 0.1 0.08 0.016))))
