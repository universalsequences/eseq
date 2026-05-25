;; Custom MIDI/audio effect parameter lookup helpers.
(def midi-fx-ui-param (fx name)
  (nth (filter |p| (= (get p :name) name) (get fx :params)) 0))

(def midi-fx-ui-param-control (name)
  (let ((p (midi-fx-ui-param midi-fx-ui-current-fx name)))
    (if p
      (fx-param-row p midi-fx-ui-current-fx
        (str "custom-midi-fx-ui-" midi-fx-ui-current-name
             "-slot-" (get midi-fx-ui-current-fx :slot-idx) "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))

(def audio-fx-ui-param (fx name)
  (nth (filter |p| (= (get p :name) name) (get fx :params)) 0))

(def audio-fx-ui-param-control (name)
  (let ((p (audio-fx-ui-param audio-fx-ui-current-fx name)))
    (if p
      (fx-param-row p audio-fx-ui-current-fx
        (str "custom-audio-fx-ui-" (custom-ui-scope-name) "-" name))
      (label (str "missing: " name) :font-size 10 :color :red :bg :transparent))))
