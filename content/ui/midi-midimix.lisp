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
      (let ((buses (filter |b| (not (mixer/group-bus? b)) (mixer/display-buses)))
            (b (nth buses (- index (len items)))))
        (if (= b nil) nil (dict :kind "bus" :bus b))))))

;; The strip's bus: a bus strip's, or a group's own (nil for a loose track,
;; or a group without one).
(def strip-bus (item)
  (let ((g (get item :group)))
    (if g g.bus (get item :bus))))

(def strip-track (item) (get item :track))

;; What strip `index`'s fader, mute and solo move: a loose track, else the
;; strip's bus (nil for none).
(def strip-channel (index)
  (let ((item (strip index)))
    (when item (or (strip-track item) (strip-bus item)))))

(def volume (index value)
  (let ((c (strip-channel index)))
    (when c (set! c.volume value))))

(def master-volume (value)
  (let ((b (mixer/main-bus)))
    (when b (set! b.volume value))))

;; A loose track's send `row` (0 or 1) among its sends to non-group buses:
;; the legacy send edit, as the mixer knob turns it.
(def send (index row value)
  (let ((item (strip index))
        (t (if item (strip-track item) nil)))
    (when t
      (let ((sends (filter |s| (not (mixer/group-bus? s.bus)) t.sends))
            (target (nth sends row)))
        (when target
          (host-command "set-track-bus-send"
            (dict :track t.index :bus target.bus.index :amount value)))))))

(def first-rack-macro (index value)
  (let ((item (strip index))
        (t (if item (strip-track item) nil)))
    (if t (midi/set-rack-macro-value t.index 0 value) nil)))

(def mute (index)
  (let ((c (strip-channel index)))
    (when c (toggle! c.muted))))

(def solo (index)
  (let ((c (strip-channel index)))
    (when c (toggle! c.soloed))))

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
