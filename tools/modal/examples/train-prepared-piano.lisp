;; A prepared piano to train from samples (eseq.modal.train): one string's
;; partials with a bolt and a mute on it, a felt hammer's thump, a rattle (a
;; loose screw) buzzing while the string is loud, and the soundboard as a mode
;; template. The decoder learns, per sample, where the bolt and the mute sit and
;; how heavy / deep they are, as well as the string's own partials.
;;
;;   eseq lisp run tools/modal/examples/train-prepared-piano.lisp   (from the repo root)
;;   tools/modal/train/run.py .local/train/prepared-piano-v1
;;
;; It needs a pack called "prepared piano" of single prepared-piano notes, each
;; tagged with its note (Save Region into the pack). The samples set the pitch:
;; the voice plays partials of the note it is triggered with, so a pack of one
;; note per key trains a playable instrument.
(import eseq.modal :as m)
(import eseq.modal.train :as t)

;; The soundboard: a spruce plate's first modes, heard near the bridge.
(def board (m/modal-data 'board :freqs '(92 151 197 274 330 412 489 563 650 742 856 980 1120 1290 1480 1700)
                                :losses '(9 10 11 12 13 15 16 18 20 22 25 28 31 35 40 46)
                                :amps '(1 0.8 0.9 0.6 0.7 0.5 0.55 0.4 0.45 0.35 0.3 0.28 0.22 0.2 0.15 0.12)))

(def piano
  (t/voice "prepared-piano" :layout 'piano
    :parts (list
      (t/strings 'string :partials 24 :bolt true :mute true :bus :body)
      ;; the hammer's felt thump
      (t/strikes 'hammer :count 2)
      (t/shaper 'body)
      (t/clipper 'clip)
      (t/record-lp 'rec)
      (t/timing 'time)
      ;; a noise bank with no wires of its own, for the rattle
      (t/bands 'buzz :envelope false)
      (t/rattle 'screw :on 'buzz :of 'string)
      (t/template 'board :modes (t/modes-of board) :keep 12))))

(t/emit-experiment
  (t/experiment "prepared-piano-v1" :voice piano
    :notes "a prepared piano from samples: bolt + mute + rattle on a string, soundboard template"
    :data (t/data :packs '("prepared piano") :frames 65536)
    :seed (t/seed :analyze :neutral)
    :train (t/recipe (t/refine :steps 600 :lr 0.02 :freeze '())
                     (t/refine :steps 200 :lr 0.005 :wave 0.3 :freeze '())
                     (t/pick :band 0.3 :mr 1)
                     (t/pretrain :steps 3000)
                     (t/path 0.02 :steps 4000))
    :ship (t/ship "Prepared Piano" :folder "Research" :extra-knobs '()))
  ".local/train/prepared-piano-v1")
