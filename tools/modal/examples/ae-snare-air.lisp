;; ae-snare on a real snare drum: the same top as ae-snare.lisp (the stick,
;; two string snares and two mics on a head you place on the drum pad, its
;; materials, the stereo mics and the output high-pass), now the batter of a
;; two-headed shell. The shell's air couples it to a snare-side head
;; (m/cavity: the air spring that splits the two heads' lowest modes, and
;; the shell's air modes coupling the rest), and the wires lie under that
;; head (m/snare-bed), so the hit reaches them through the air, as on a real
;; snare, rather than slapping them directly.
;;
;;   1  stick   the mallet on the batter
;;   2  snare1  a heavy string just under the batter (as in ae-snare)
;;   3  snare2  another, of another material
;;   4  mic1    on the batter, left
;;   5  mic2    on the batter, right
;;      air     the shell's air, 14 x 5.5 inch
;;      reso    the snare-side head, tuned below the batter (the air raises
;;              the pair: the batter's tune is not the pitch it rings at)
;;      wires   eight wires under reso; their contact (Snap) is heard in the
;;              middle, as a mic under the drum
;;
;; The air, reso head, wires and bed are modal-snare-physical's fitted
;; values (snare.lisp), refitted here with tools/modal/bench/snare_fit.py
;; (--map batter=head --map hit=stick1 --map snares=buzz --map mic2=mic3),
;; which hardly moved them.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(def materials (list mat/water mat/spider-silk mat/mylar mat/nylon mat/steel))

(m/patch :name "modal-ae-snare-air"
  :objects (list (m/mallet 'stick :mass 0.0335 :speed 1)
                 (m/membrane 'head :modes 48 :note 44 :shift 0 :tune 0.65 :loss 6 :bright 0.96
                             :made-of materials :thick-mm 3)
                 (m/membrane 'reso :modes 24 :loss 2.93 :bright 0.53 :tune 0.3)
                 (m/cavity 'air 'head 'reso :radius 0.178 :depth 0.14 :modes 8 :loss 34.3)
                 (m/snare-wires 'wires :count 8 :modes 8 :pitch 2105 :loss 2.85 :mass 0.471)
                 (m/string 'snare1 :made-of materials :note 25 :follow false :modes 48 :mass 100
                           :length 0.36 :radius-mm 0.3 :const-loss 15 :freq-loss 2 :mute 0)
                 (m/string 'snare2 :made-of (list mat/spider-silk mat/water mat/mylar mat/nylon mat/steel)
                           :note 25 :follow false :modes 48 :mass 100
                           :length 0.36 :radius-mm 0.3 :const-loss 15 :freq-loss 2 :mute 0))
  :connections (list (m/felt 'stick1 'stick 'head (list 0.51 2.94) :felt 0.0289 :hardness 0.955)
                     (m/strike 'touch1 'head (list 0.345 4.42) 'snare1 0.37 :mic-gap true :offset 0.02 :stiffness 200)
                     (m/strike 'touch2 'head (list 0.453 1.46) 'snare2 0.41 :mic-gap true :offset 0.02 :stiffness 200)
                     (m/snare-bed 'buzz 'reso 'wires :points 2 :iterations 3 :gap 0.0072 :stiffness 195
                                  :drag 0.192 :snap 17.3))
  :listens (list (m/listen 'head (list 0.696 4.93) :left 1 :right 0)
                 (m/listen 'head (list 0.713 5.9) :left 0 :right 1)
                 (m/listen 'reso (list 0.5 2) :gain 0.16 :left 0.7 :right 0.7)
                 (m/listen 'snare1 0.2 :gain 0.1 :left 0 :right 1)
                 (m/listen 'snare2 0.2 :gain 0.1 :left 1 :right 0))
  :out-filter (dict :type :highpass :cutoff 110 :q 0)
  :pad 'head
  :level 2.5)
