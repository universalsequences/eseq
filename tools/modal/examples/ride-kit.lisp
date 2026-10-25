;; PM Ride Kit, rebuilt from data: the ride identified from a record
;; (tools/pm-ride), as 688 measured plate modes and twelve measured strokes,
;; one per key from C, in every octave (each octave transposes the cymbal).
;; Every hit lands on the same ringing plate (:voicing :one), injecting its
;; stroke's complex residue on top of the ring, so close modes beat as they
;; did on the record. The data's wash carries the ~1500 weaker modes as band
;; noise. Regenerate the data with tools/pm-ride/export_modes.py.
(import eseq.modal :as m)

(def ride (modal-read-data "data/ride.modes.json"))

(m/patch :name "modal-ride-kit" :voicing :one :level 2 :tone 20000
  :objects (list (m/modal-data 'ride :data ride :follow :octave))
  :connections (list (m/strokes 'stick 'ride))
  :listens (list (m/listen 'ride 0 :hear :position)))
