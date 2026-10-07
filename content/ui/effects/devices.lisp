;; eseq.effects.devices — the eseq.kinds device (and param) a panel dict
;; stands for.
;;
;; The factory panels still lay out from the host's panel dicts (an
;; instrument panel, an effect's `fx` dict, their param dicts), but every
;; value they show comes from the kinds: the device a dict describes is
;; looked up by the dict's own address (its track, chain slot, bus, rack
;; slot), its params by descriptor index. Side-effect free, so any panel
;; module can import it.
;;
;; Every lookup reads by value (a track's `devices`, a device's `params`):
;; a panel re-renders when its device list or descriptor changes, as it did
;; when the host rebuilt the panel dicts.

(module eseq.effects.devices)

(import eseq.kinds :refer (track buses selection))

(export track-at current-track-index instrument-of inst-device fx-device param-device
        param-of with-prm tensor-of base-note-param? param-stored-value)

;; The device of `devices` at chain position `slot` (-1: the instrument), or
;; nil.
(def device-at (devices slot)
  (first (filter (lambda (d) (= d.slot slot)) devices)))

;; Track `i`, or nil (no index, or no such track).
(def track-at (i)
  (if (and (number? i) (>= i 0)) (track i) nil))

;; The current track's position (the legacy commands' track address), or -1.
(def current-track-index ()
  (let ((t selection.track)) (if t t.index -1)))

;; t's instrument device, or nil.
(def instrument-of (t)
  (if t (device-at t.devices -1) nil))

;; The device an instrument panel dict shows: its track's instrument, or the
;; drum rack slot it names (`:rack-slot`).
(def inst-device (inst)
  (let ((d (instrument-of (track-at (get inst :track))))
        (slot (get inst :rack-slot)))
    (if (and d (not (= slot nil))) (device-at d.devices slot) d)))

;; The device an effect panel dict (`fx`) shows: a bus effect, a drum rack
;; slot's effect, a MIDI effect or a chain effect of its track (the current
;; track when the dict names none). Nil unless the device at that address is
;; the dict's effect (`d.type`): a dict outliving a chain edit names nothing.
(def fx-device (fx)
  (let ((d (fx-device-at fx)))
    (if (and d (= d.type (get fx :name))) d nil)))

(def fx-device-at (fx)
  (let ((slot (get fx :slot-idx)))
    (if (get fx :bus-fx)
      (let ((b (nth (buses) (get fx :bus-idx))))
        (if b (device-at b.devices slot) nil))
      (let ((t (if (= (get fx :track-idx) nil) selection.track (track-at (get fx :track-idx)))))
        (if (= t nil)
          nil
          (if (get fx :rack-fx)
            (let ((rack (instrument-of t)))
              (let ((rack-slot (if rack (device-at rack.devices (get fx :rack-slot)) nil)))
                (if rack-slot (device-at rack-slot.devices slot) nil)))
            (if (get fx :midi-fx)
              (device-at t.midi-devices slot)
              (device-at t.devices slot))))))))

;; The device a param dict of an instrument panel belongs to: the drum rack
;; slot it names (`:rack-track`, `:rack-slot`), else the current track's
;; instrument (the instrument commands' target).
(def instrument-param-device (p)
  (let ((rack-track (get p :rack-track)))
    (if (= rack-track nil)
      (instrument-of selection.track)
      (let ((rack (instrument-of (track-at rack-track))))
        (if rack (device-at rack.devices (get p :rack-slot)) nil)))))

;; The device of param dict p: fx's (an effect panel's), else the
;; instrument's (fx false).
(def param-device (fx p)
  (if fx (fx-device fx) (instrument-param-device p)))

;; d's param at descriptor index i, or nil.
(def param-at (d i)
  (if (and d (number? i)) (nth d.params i) nil))

;; The param instance a param dict stands for (`:idx`, its descriptor
;; index), or nil: a dict with no index (the base note control, a sampler's
;; synthetic controls) or a device not published. A dict carrying its param
;; (`:prm`, with-prm) skips the lookup.
(def param-of (fx p)
  (if p
    (or (get p :prm)
        (if (number? (get p :idx)) (param-at (param-device fx p) (get p :idx)) nil))
    nil))

;; p carrying its param (`:prm`), resolved once for every control that reads
;; it; p as it is when it has none.
(def with-prm (fx p)
  (let ((prm (param-of fx p)))
    (if prm (merge p :prm prm) p)))

;; The tensor instance an instrument panel's tensor dict stands for.
(def tensor-of (p)
  (let ((d (instrument-param-device p)))
    (if (and d (number? (get p :idx))) (nth d.tensors (get p :idx)) nil)))

;; The instrument panel's base note control (a dict with no param index).
(def base-note-param? (p)
  (= (get p :control) "base-note"))

;; v (display units) as the legacy effect commands take it: an effect's
;; commands (chain, bus, MIDI or drum rack slot effect) take a % param stored
;; as a ratio (param.percent) in its stored units (display / 100).
;; Instruments and drum rack slot instruments take display units. Callers
;; send nothing for a param not published (prm nil): its units are unknown.
(def param-stored-value (fx prm v)
  (if (and fx prm prm.percent) (/ v 100) v))
