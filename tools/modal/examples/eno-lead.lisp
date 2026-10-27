;; An electric lead guitar that feeds back through its amp. A pick plucks a
;; light steel string; a bridge pickup hears it; the amp (fuzz, tone stack,
;; sagging power stage, speaker) plays it; 2 m of air (and the room's
;; reflections) later the sound pushes the maple body, which carries the
;; string's bridge end: its motion shakes the string (m/mount), closing the
;; loop. A solid body takes almost nothing back from the string, so the
;; mount is one way and the string keeps its sustain; and the pickup, on the
;; body, hears only the string moving against it.
;;
;; Held notes sustain, then bloom into feedback at whichever partial the
;; loop favours, and the amp's clipping holds it there. Releasing the key
;; is the player's hand on the strings: the envelope closes the loop and the
;; mute stops the string. Play it with:
;;   Air > Gain      how much sound the guitar catches (step toward the amp)
;;   Air > Distance  which partial blooms: the fundamental, the octave, the
;;                   twelfth, the double octave (and it bends while it moves)
;;   Air > Angle     off the speaker's axis it hears less treble, so it
;;                   squeals less
;;   Air > Room      the reflections: more spots feed back
;;   AMP             Drive, Bias, Bloom (the fuzz's lopsided grit), the tone
;;                   stack, Power and Sag, Presence
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "modal-eno-lead" :voicing :one
  :objects (list (m/finger 'pick :depth 0.06 :stroke-ms 6)
                 (m/string 'str :made-of mat/steel :length 0.628 :radius-mm 0.33 :modes 60
                           :const-loss 0.1 :freq-loss 0.06 :mute 30)
                 ;; a body and glued neck: its lowest mode near 300 Hz, damped
                 ;; as maple and hardware damp it
                 (m/plate 'body :made-of mat/maple :thick-mm 10 :width 0.33 :height 0.42 :mass 30 :loss 40 :damp 3))
  :connections (list (m/pluck 'plk 'pick 'str 0.82)
                     (m/force 'air 'body (list 0.4 0.6) :gain 0.003
                              :signal (list (m/speaker :distance 2 :angle 60 :room 0.5)
                                            (m/envelope :attack-ms 5 :decay-ms 1 :sustain 1 :release-ms 60)))
                     (m/mount 'bridge 'body (list 0.5 0.45) 'str 1))
  :listens (list (m/pickup 'str 0.92 :height 3 :coil-hz 2600 :coil-q 1.8))
  :amp (m/amp :drive 30 :bias 0.25 :bloom 0.6 :mid 1.3 :power 2.5 :sag 0.4 :presence 0.8 :cab 5000))
