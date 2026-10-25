;; An electric lead guitar that feeds back through its amp. A pick plucks a
;; light steel string; a bridge pickup hears it; the amp (fuzz, tone stack,
;; sagging power stage, speaker) plays it; 2 m of air later the sound
;; pushes the maple body, and the body's motion moves the string's end at
;; the bridge, closing the loop. A solid body takes almost nothing back from
;; the string, so that coupling is one way (a force driven by the body's
;; position tap) rather than a spring, and the string keeps its sustain.
;;
;; Held notes sustain, then bloom into feedback at whichever partial the
;; loop favours; the amp's clipping holds it there. Play it with:
;;   Air > Gain      how much sound the guitar catches (step toward the amp)
;;   Air > Distance  which partial blooms: the fundamental, the octave, the
;;                   twelfth (and it bends while it moves)
;;   Air > Angle     off the speaker's axis it hears less treble, so it
;;                   squeals less
;;   AMP             Drive, Bias (asymmetry), Bloom (bias drift), the tone
;;                   stack, Power and Sag
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-eno-lead" :voicing :one
  :objects (list (m/finger 'pick :depth 0.06 :stroke-ms 6)
                 (m/string 'str :made-of mat/steel :length 0.628 :radius-mm 0.33 :modes 60
                           :const-loss 0.1 :freq-loss 0.06 :mute 10)
                 (m/plate 'body :made-of mat/maple :thick-mm 10 :width 0.33 :height 0.42 :mass 30 :damp 1.5))
  :connections (list (m/pluck 'plk 'pick 'str 0.82)
                     (m/force 'air 'body (list 0.4 0.6) :gain 0.008
                              :signal (m/speaker :distance 2 :angle 60))
                     (m/force 'bridge 'str 0.98 :gain 1 :signal (m/tap-position 'body (list 0.5 0.45))))
  :listens (list (m/pickup 'str 0.92 :height 3 :coil-hz 2600 :coil-q 1.8))
  :amp (m/amp :drive 30 :bias 0.25 :bloom 0.5 :mid 1.3 :treble 0.9 :power 2.5 :sag 0.4 :cab 4000))
