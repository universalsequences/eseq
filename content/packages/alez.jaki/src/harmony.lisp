;; alez.jaki.harmony — publish a track's chord into a channel for jaki
;; (docs/jaki-row-processes-spec.md §11, bead eseq-1sr5.6).
;;
;;   (import alez.jaki.harmony :refer (harmony))
;;
;;   (harmony "chord" :track 1)          ; track 1 = the second track
;;
;;   (jak "bass" :16
;;     . . - .
;;     -> 0 (note (seq :hit 0 3 7 10)) (snap "chord"))
;;
;; Generator ticks cannot read other tracks (process `read` natives only work
;; inside a process eval), so the bridge is a channel. One form declares the
;; value channel NAME and attaches a conductor process that observes track
;; TRACK: on each of its hits it reads the track's chord and sends it as a
;; 12-bit pitch-class mask, bit n set when pitch class n sounds. The chord is
;; the authored one on the step the track is on (`:pattern`), else what its
;; instrument is actually sounding (`:output`, after MIDI FX). Pitches are
;; relative to the source track's root, as lane-harmony reads them. Until the
;; source plays the mask is 0, which leaves jaki notes alone; between its hits
;; the last chord holds.
;;
;; A conductor, not a timer (`:every`) process: timer processes run with no
;; track reads (runtime/process.rs hands them an empty read snapshot), and a
;; conductor needs no slot in the source track's own process chain. It emits
;; nothing; `:play` names the source only because a conductor must name one.
;;
;; jaki reads it with `(snap "chord")`, a route word or a `then` word inside a
;; row process: the hit's final transpose moves to the nearest pitch class in
;; the mask. Channel values cross to the scheduler once per chunk, so a chord
;; change reaches jaki a chunk later.
;;
;; Mechanics mirror alez.sig: the macro runs on the authoring VM and returns
;; ordinary process syntax; the :run body is shipped to the scheduler VM and
;; touches only builtins and process natives (the mask is inline arithmetic).

(module alez.jaki.harmony)

(export harmony)

(def opt-value (opts key default)
  (if (empty? opts)
    default
    (if (= (first opts) key)
      (nth opts 1)
      (opt-value (rest (rest opts)) key default))))

(defmacro harmony (name &rest opts)
  (let ((track (alez.jaki.harmony/opt-value opts :track 0))
        (proc (gensym (str "__harmony-" name)))
        (chord (gensym "chord"))
        (mask (gensym "mask"))
        (pitch (gensym "pitch"))
        (pc (gensym "pc")))
    `(if (__jaki-declare-value-channels (list (list ,name 0)))
         (do
           (def-process ,proc
             :run (let ((,chord (let ((,pitch (read (track ,track :chord :pattern))))
                                  (if (= ,pitch nil) (read (track ,track :chord :output)) ,pitch))))
                    (if (= ,chord nil)
                      nil
                      (send ,name
                        (reduce (lambda (,mask ,pitch)
                                  (let ((,pc (mod (+ (mod (round ,pitch) 12) 12) 12)))
                                    (if (= 1 (mod (floor (/ ,mask (pow 2 ,pc))) 2))
                                      ,mask
                                      (+ ,mask (pow 2 ,pc)))))
                                0 ,chord)))))
           (processes :observe (list ,track) :play (list ,track) (,proc)))
         false)))
