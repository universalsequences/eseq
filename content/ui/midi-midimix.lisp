;; Akai MIDImix factory preset, MIDI channel 1 (zero-based channel 0).
;; Hardware assignments and mixer policy live here, not in the MIDI driver.
(module eseq.midi-midimix)
(import eseq.midi :as midi)
(import eseq.mixer :as mixer)
(import eseq.transport :as transport)

(export install)

;; These are the mixer's actual top-level render items. Expanded group members
;; and nested racks never consume a hardware strip. Remaining strips follow
;; the visible bus row (including Mix), using the mixer's display ordering.
;; Resolve on every event so topology/project changes cannot leave stale targets.
(def strip (index)
  (let ((items (mixer/render-order)))
    (if (< index (len items))
      (nth items index)
      (let ((buses (filter |bus| (not (mixer/group-bus-id? (nth SEQ.bus-ids bus)))
                     (map mixer/display-bus-index (range 0 (len SEQ.bus-names)))))
            (bus (nth buses (- index (len items)))))
        (if (= bus nil) nil (dict :kind "bus" :bus bus))))))

(def group (item)
  (nth SEQ.groups (get item :gidx)))

(def strip-bus (item)
  (if (= (get item :kind) "bus")
    (get item :bus)
    (mixer/bus-index-by-id (get (group item) :bus-id))))

(def volume (index value)
  (let ((item (strip index)))
    (if (= item nil)
      nil
      (if (= (get item :kind) "loose")
        (seq-set-track-volume (get item :track) value)
        (let ((bus (strip-bus item)))
          (if (>= bus 0) (seq-set-bus-volume bus value) nil))))))

(def master-volume (value)
  (let ((bus (mixer/bus-index-by-id 0)))
    (if (>= bus 0) (seq-set-bus-volume bus value) nil)))

(def send (index row value)
  (let ((item (strip index)))
    (if (not (= (get item :kind) "loose"))
      nil
      (let ((track (get item :track))
            (sends (filter |target| (not (mixer/group-bus-id? (get target :bus-id)))
                     (nth SEQ.track-bus-sends track)))
            (target (nth sends row)))
        (if (= target nil)
          nil
          (host-command "set-track-bus-send"
            (dict :track track :bus (get target :bus-idx) :amount value)))))))

(def first-rack-macro (index value)
  (let ((item (strip index)))
    (if (= (get item :kind) "loose")
      (midi/set-rack-macro-value (get item :track) 0 value)
      nil)))

(def mute (index)
  (let ((item (strip index)))
    (if (= item nil)
      nil
      (if (= (get item :kind) "loose")
        (seq-toggle-track-mute (get item :track))
        (let ((bus (strip-bus item)))
          (if (>= bus 0) (seq-toggle-bus-mute bus) nil))))))

(def solo (index)
  (let ((item (strip index)))
    (if (= item nil)
      nil
      (if (= (get item :kind) "loose")
        (seq-toggle-track-solo (get item :track))
        (let ((bus (strip-bus item)))
          (if (>= bus 0) (seq-toggle-bus-solo bus) nil))))))

(def pressed? (msg)
  (and (= (get msg :kind) :note-on) (> (get msg :value) 0)))

(def source (device source)
  (midi/on-device device (midi/on-channel 0 source)))

;; Re-evaluation replaces the same sources. A different endpoint name can be
;; installed from init.lisp without changing this factory module.
(def install (device)
  (for-each
    (lambda (index)
      (let ((base (nth '(16 20 24 28 46 50 54 58) index)))
        (midi/midi-map (source device (midi/cc (+ base 3)))
          (lambda (value msg) (volume index value)))
        (for-each
          (lambda (row)
            (midi/midi-map (source device (midi/cc (+ base row)))
              (lambda (value msg) (send index row value))))
          (range 0 2))
        (midi/midi-map (source device (midi/cc (+ base 2)))
          (lambda (value msg) (first-rack-macro index value)))
        (midi/midi-map (source device (midi/note (+ 1 (* index 3))))
          (lambda (value msg) (if (pressed? msg) (mute index) nil)))
        ;; Holding SOLO makes the hardware send the adjacent solo note.
        (midi/midi-map (source device (midi/note (+ 2 (* index 3))))
          (lambda (value msg) (if (pressed? msg) (solo index) nil)))
        ;; Record Arm buttons always choose the rate, including before a roll.
        (midi/midi-map (source device (midi/note (+ 3 (* index 3))))
          (lambda (value msg)
            (if (pressed? msg) (seq-set-roll-rate index) nil)))))
    (range 0 8))
  (midi/midi-map (source device (midi/cc 62))
    (lambda (value msg) (master-volume value)))
  (midi/midi-map (source device (midi/note 25))
    (lambda (value msg) (if (pressed? msg) (transport/seq-switch-relative -1) nil)))
  (midi/midi-map (source device (midi/note 26))
    (lambda (value msg) (if (pressed? msg) (transport/seq-switch-relative 1) nil)))
  ;; SOLO supplies real press/release events. SEND ALL only dumps CC values.
  ;; Ownership is scoped to the input port and cleared by the host on loss.
  (midi/midi-map (source device (midi/note 27))
    (lambda (value msg) (seq-midi-sequence-roll msg))))

(install "MIDI Mix")
