;; ui/bank-power.lisp — The bank-scoped power toggle (docs/rig-spec.md §6,
;; §10), one control for everything a bank switches on and off: a kit (the
;; drum rack header, `eseq.drum-rack-v2`) and a project-owned sequencer (its
;; view tab, `eseq.instance-frame`).
;;
;; Both publish the same fields: `parked` (off in the playing scene) and
;; `parked-banks` / `mixed-banks` (the bank instances whose every / some
;; scenes switch it off) — a group's fields for a kit, a project.instances
;; row for an instance. `bank-state` reads them for one bank.
;;
;; State/accessor module: no `effect-buffer`, safe to import anywhere.

(module eseq.bank-power)
(import eseq.kinds :refer (instance-row))

(export bank-power-dot
        bank-mixed-dot
        bank-power-toggle
        bank-state
        bank-power-hint
        kit-bank-state
        instance-bank-state)

;; The device enable dot (the instrument and effect panels' look).
(defwidget bank-power-dot
  :width 1.55 :height 1.0
  :paint-margin 0.1
  :state (active)
  :shader
  (sdf/fill (sdf/circle 0.86)
    (material :color (if (> active 0.5) :device-enabled :device-disabled))))

;; The small dot beside an on toggle while the bank is mixed.
(defwidget bank-mixed-dot
  :width 0.5 :height 0.5
  :paint-margin 0.1
  :shader
  (sdf/fill (sdf/circle 0.8) (material :color :fg)))

(def in-banks? (b bs)
  (reduce |found x| (or found (= x b)) false bs))

;; :on, :off (every scene of bank b switches it off) or :mixed (some do),
;; from the published fields. With no bank (b nil, before the host published
;; one) the playing scene decides.
(def bank-state (parked parked-banks mixed-banks b)
  (if (= b nil)
    (if parked :off :on)
    (if (in-banks? b parked-banks)
      :off
      (if (in-banks? b mixed-banks) :mixed :on))))

;; Kit (drum rack group) g in bank b.
(def kit-bank-state (g b)
  (bank-state g.parked g.parked-banks g.mixed-banks b))

;; Project-owned instance id in bank b (a rack-owned or unknown one: :on).
(def instance-bank-state (id b)
  (let ((row (instance-row id)))
    (if (= row nil)
      :on
      (bank-state (get row :parked) (get row :parked-banks) (get row :mixed-banks) b))))

;; The hint standing in for a tooltip: "Kit on in bank C" with noun "Kit",
;; "On in bank C" with noun nil; mixed reads on, "(some scenes off)" after.
(def bank-power-hint (noun state bank-label)
  (let ((word (if (= state :off) "off" "on")))
    (str (if noun (str noun " " word) (if (= state :off) "Off" "On"))
         " in bank " bank-label
         (if (= state :mixed) " (some scenes off)" ""))))

;; The toggle: the enable dot keyed `key`, on unless `state` is :off, with
;; the mixed dot beside it while :mixed. A click calls (on-toggle); on or
;; mixed means "turn the bank off", off "turn it on" — the caller's rule.
(def bank-power-toggle (key state on-toggle)
  (h-stack :debug-name "bank-power" :gap 0 :align :center
    (box :width 1.55 :height 0.99 :v-align :center :h-align :center :padding 0
      (bank-power-dot
        :key key
        :active (if (= state :off) 0 1)
        :on-click |x y r| (on-toggle)))
    (if (= state :mixed)
      (box :debug-name "bank-power-mixed" :width 0.5 :height 0.99
        :v-align :center :h-align :center :padding 0
        (bank-mixed-dot))
      (box :width 0 :height 0))))
