;; bow -> string -bridge-> body: a bowed string on a wooden body.
;;
;; Three things keep the bow locked into Helmholtz motion (the sawtooth that
;; makes a cello a cello) over the whole range:
;;   - the bridge sits at the string's very end (0.999). A stiff point
;;     further in pins the string a second time, its modes stop being
;;     harmonic and the bow cannot lock (0.99 halved the harmonic-to-noise
;;     ratio and lost 20 dB of the upper harmonics).
;;   - the string's stiffness stays the same up and down the range, as a
;;     cello's wound strings keep it: the radius follows the note. A string
;;     this stiff at C2 at C4's gauge stretches its partials and the bow
;;     scratches (harmonic-to-noise ratio +10 dB instead of +30).
;;   - the body is a cello's size, damped as a cello's is: its lowest mode
;;     near 150 Hz, Q about 20. A big, lively plate wolfs a whole octave.
;; The mic near the bridge hears the string's slope there, the force a real
;; bridge passes to the top; the body adds its resonances.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-cello"
  :objects (list (m/string 'str :made-of mat/nylon :length 0.69 :radius-mm (m/by-note 65 0.15 523 1.2) :modes 60
                                :const-loss 0.5 :freq-loss 1.2 :mute 0)
                 (m/plate 'body :made-of mat/spruce :width 0.5 :height 0.35 :thick-mm 5 :mass 40 :loss 30 :damp 5))
  :connections (list (m/bow 'bow 'str 0.12 :pressure 0.3)
                     (m/spring 'bridge 'str 0.999 'body (list 0.55 0.4) :stiffness 0.3))
  :listens (list (m/listen 'str 0.97 :gain 0.5)
                 (m/listen 'body (list 0.3 0.65) :gain 0.3)))
