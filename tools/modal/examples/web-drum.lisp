;; A drum skin stretched in the hub of a spider web, the web anchored to a
;; round frame: the skin rings on the threads as springs, and the threads
;; and the skin share modes, so it sounds neither like a drum nor a web.
;; Solved once when generated (m/web: a tensioned network of 12 spokes and
;; 10 rings, jittered as a real web is, its modes from a dense eigen-solve),
;; then played like any modal object; reshape it (spokes, rings, the hub's
;; size and tension, the threads' tension, irregular, seed) and generate
;; again for another web.
;;
;;   stick  a light mallet on the skin
;;   web    the web with its hub skin (water-heavy beads on the threads)
;;   snare  a string just under the skin, as in ae-snare: low, fixed and
;;          damped hard so it buzzes (lighter: a heavy one resting on a skin
;;          this light clamps it)
;;   mics   one on the skin, one out on the threads, left and right
;;
;; Points on the web are (x y 0), metres from its centre: the node nearest.
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-web-drum"
  :objects (list (m/mallet 'stick :mass 0.02 :speed 1)
                 (m/web 'web :spokes 12 :rings 10 :radius 0.18 :irregular 0.3 :seed 3
                             :spoke-tension 0.05 :ring-tension 0.01 :bead 0.00005
                             :hub 0.07 :hub-tension 400 :hub-density 0.35 :modes 64 :loss 16 :hf-loss 1.5)
                 (m/string 'snare :made-of (list mat/water mat/spider-silk mat/steel) :modes 48 :mass 10
                           :note 25 :follow false :length 0.36 :radius-mm 0.3 :const-loss 15 :freq-loss 2 :mute 0))
  :connections (list (m/felt 'stick1 'stick 'web (list 0.02 0.015 0) :felt 0.01 :hardness 0.6)
                     (m/strike 'touch 'web (list -0.03 -0.02 0) 'snare 0.37 :mic-gap true :offset 0.05 :stiffness 200))
  :listens (list (m/listen 'web (list 0.04 -0.03 0) :left 1 :right 0.3)
                 (m/listen 'web (list -0.1 0.08 0) :gain 2 :left 0.3 :right 1)
                 (m/listen 'snare 0.2 :gain 0.1 :left 0.5 :right 0.5))
  :out-filter (dict :type :highpass :cutoff 80 :q 0)
  :level 10)
