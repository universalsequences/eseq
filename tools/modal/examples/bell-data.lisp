;; A bell from measured data: an IRCAM resonance model (data/bell.txt: each
;; mode's frequency, level and bandwidth) struck by a felt mallet. The data is
;; tuned to a 440 Hz nominal, so :root 440 makes it follow the played note.
(import eseq.modal :as m)

(m/patch :name "modal-bell-data"
  :objects (list (m/mallet 'mal :mass 0.05 :speed 1.5)
                 (m/modal-data 'bell :data (modal-read-data "data/bell.txt") :root 440))
  :connections (list (m/felt 'hit 'mal 'bell 0 :felt 0.02 :hardness 0.7))
  :listens (list (m/listen 'bell 0)))
