;; The identified ride of ride-kit.lisp with Modalys' spectral controls: its
;; 688 measured partials spread about the lowest (Spread above 1 stretches
;; the cymbal toward a gong, below 1 squeezes it toward a bell) and bent
;; (cents), live, while every stroke keeps its measured residues.
(import eseq.modal :as m)

(def ride (modal-read-data "data/ride.modes.json"))

(m/patch :name "modal-ride-warp" :voicing :one :level 2 :tone 20000
  :objects (list (m/spectral (m/modal-data 'ride :data ride :follow :octave) :diffraction 0.8 :bend -300))
  :connections (list (m/strokes 'stick 'ride))
  :listens (list (m/listen 'ride 0 :hear :position)))
