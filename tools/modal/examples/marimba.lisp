;; A marimba bar: a free bar undercut so its first three modes sit at 1 : 4 : 10
;; (a plain bar rings at 1 : 2.76 : 5.40, a glockenspiel-like clang), struck
;; near its centre by a soft mallet (as players do) and heard near an end,
;; away from the fundamental's nodes at 0.224 and 0.776.
(import eseq.modal :as m)

(m/patch :name "modal-marimba"
  :objects (list (m/mallet 'mal :mass 0.03 :speed 1.2)
                 (m/bar 'bar :undercut 1 :loss 2.5 :hf-loss 1.2 :mute 4))
  :connections (list (m/felt 'hit 'mal 'bar 0.45 :felt 0.01 :hardness 0.4))
  :listens (list (m/listen 'bar 0.08)))
