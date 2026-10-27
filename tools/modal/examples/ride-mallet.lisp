;; Felt mallets on the identified ride: the plate's 688 measured modes
;; (data/ride.modes.json) struck by a physical mallet through a Stulov felt
;; (its contact stiffens on harder hits) at the Ride stroke's spot, instead
;; of the recorded strokes. A soft mallet's long contact leaves the plate's
;; low ring, the swell of a mallet roll; the mallet meets the plate where it
;; is, so a roll's hits land on a moving surface. Voicing 1: every hit on one
;; plate. Harder felt and a lighter mallet brighten it only so far: the
;; measured data has the mic's view of each mode, not its shape across the
;; plate, so the stroke spots are inferred (see export_modes.py).
(import eseq.modal :as m)

(def ride (modal-read-data "data/ride.modes.json"))

(m/patch :name "modal-ride-mallet" :voicing :one :level 3 :tone 20000
  :objects (list (m/modal-data 'ride :data ride :follow :octave :mass 0.2)
                 (m/mallet 'mallet :mass 0.02 :speed 2))
  :connections (list (m/felt 'felt 'mallet 'ride (m/stroke-point ride "Ride") :felt 1 :exponent 2.5))
  :listens (list (m/listen 'ride 0 :hear :position)))
