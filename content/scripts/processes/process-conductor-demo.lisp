;; Conductor attachment demo (end-game Slice 3).
;;
;; Evaluate directly or load from project scratch:
;;   (load "content/scripts/processes/process-conductor-demo.lisp")
;;
;; Prepare four melodic tracks:
;;   tracks 0 and 1: sparse melody/bass patterns to observe
;;   tracks 2 and 3: instruments only; local players turn the conductor's
;;                   suggestion into delayed harmony responses
;;
;; The conductor is one stateful instance, invoked once after all observed
;; tracks at a shared tick have resolved. It sees those same-tick resolved
;; values, emits through the normal target-track MIDI-FX path, and also
;; suggests the resulting harmony field for any decentralized followers.

(eseq.seq-script-picker/seq-register-script-source-tab "Conductor Demo")

(def-process call-response-conductor
  :doc "Observe two sparse melodies and publish their shared harmony suggestion."
  :in ((density :float 0 1 :default 0.85))
  :run (let ((low (read (track 0 :transpose)))
             (high (read (track 1 :transpose))))
         (suggest :harmony
           (pitch-field (list low high (+ low 7) (+ high 4))
                        :root low
                        :weight (in :density)))))

(def-process suggestion-response-player
  :doc "On a sparse caller trigger, turn the previous conductor suggestion into one delayed response phrase."
  :in ((listen :field :default :harmony)
       (target :track :default 2)
       (voice :int 0 3 :default 0)
       (density-threshold :float 0 1 :default 0.33)
       (num-notes :int 1 8 :default 2)
       (play-delay :int 1 16 :default 2)
       (timebase :int 0 12 :default 4))
  :run (let ((field (hear (in :listen))))
         (if field
           (if (>= (field-weight field) (in :density-threshold))
             (let ((pitches (field-pitches field))
                   (spacing (* (in :play-delay)
                               (timebase-beats (in :timebase)))))
               (map (lambda (i)
                      (emit :track (in :target)
                            :after (* (+ i 1) spacing)
                            :note (nth pitches
                                       (mod (+ (in :voice) i) (len pitches)))
                            :vel (- 0.8 (* i 0.08))
                            :duration 0.75))
                    (range 0 (in :num-notes))))
             nil)
           nil)))

(def call-response-conductor-h
  (processes :observe (list 0 1) :play (list 2 3)
    (call-response-conductor
      :density (~slider 0.85))))

;; These players wake only when sparse track 0 fires. They hear the conductor's
;; previous suggestion, so the first call primes the field and the next call
;; produces the audible response. Density admits voices progressively.
(def call-response-players-h
  (processes :track 0
    (suggestion-response-player
      :target 2 :voice 0 :density-threshold 0.33
      :num-notes 2 :play-delay 2 :timebase 4)
    (suggestion-response-player
      :target 3 :voice 1 :density-threshold 0.66
      :num-notes 2 :play-delay 2 :timebase 4)))
