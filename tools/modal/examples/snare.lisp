;; A snare drum from physical parts: a stick thrown at the batter head, the
;; air inside the shell coupling the batter to the snare-side head, and snare
;; wires lying under the snare-side head, rattling against it.
;;
;;   stick   a mass thrown at the batter (felt contact, hardening as it
;;           squashes); hit's x/y knobs move where it lands
;;   batter  the top head, its 32 lowest modes, on the note; Bend (off
;;           here: the benchmark does not reward it, try 0.5-1 by ear)
;;           sharpens a hard hit, which glides down as it rings
;;   reso    the snare-side head, 24 modes
;;   air     the shell's air: the uniform air spring that splits the two
;;           heads' (0,1) modes, and the shell's acoustic modes coupling
;;           their (m,n) modes (m/cavity)
;;   wires   eight steel coils under the reso head, each touching it at two
;;           points (m/snare-wires + m/snare-bed): Gap is the strainer, Drag
;;           the buzz's own loss; the bed's contact force is also heard
;;           (Snap), as a mic beside the snares hears it
;;
;; Benchmarked against content/instruments/Drums/Membrane Snare with
;; tools/modal/bench/snare_bench.py; the knobs here are snare_fit.py's best
;; (38.5 on its current score, which adds a timbre term; lowest peaks 249
;; and 276 Hz against the reference's 236 and 243). The air raises and
;; splits the heads' (0,1) modes, so their tunes are not the pitch the drum
;; rings at. About 18x faster than real time per voice; more modes or
;; touches cost in proportion (see tools/modal/README.md, Drums).
(import eseq.modal :as m)

(m/patch :name "modal-snare-physical"
  :objects (list (m/mallet 'stick :mass 0.047 :speed 1)
                 (m/membrane 'batter :modes 32 :loss 5.4 :bright 6.93 :tune 0.646 :bend 0)
                 (m/membrane 'reso :modes 24 :loss 2.99 :bright 0.53 :tune 0.3)
                 (m/cavity 'air 'batter 'reso :radius 0.178 :depth 0.14 :modes 8 :loss 34.7)
                 (m/snare-wires 'wires :count 8 :modes 8 :pitch 2105 :loss 2.73 :mass 0.471))
  :connections (list (m/felt 'hit 'stick 'batter (list 0.3 0) :felt 0.0254 :hardness 0.97)
                     (m/snare-bed 'snares 'reso 'wires :points 2 :iterations 3 :gap 0.0072 :stiffness 153
                                  :drag 0.208 :snap 17.3))
  :listens (list (m/listen 'batter (list 0.5 0.5))
                 (m/listen 'reso (list 0.5 2) :gain 0.136))
  :level 1)
